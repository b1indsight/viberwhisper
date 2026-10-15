use std::collections::VecDeque;
use std::sync::Arc;
use std::thread;

use tracing::{debug, error, info, warn};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::WindowId;

use super::permission::PermissionGuidance;
use super::platform_session_event;
use crate::audio::{AudioRecorder, RecorderStartOutcome, RecorderStopOutcome};
use crate::core::orchestrator::{SessionError, SessionOrchestrator};
use crate::core::recording_session::{
    RecordingSessionMachine, RecordingState, SessionEffect, SessionEvent,
};
use crate::history::HistoryStore;
use crate::input::typer::TextTyper;
use crate::platform::{
    NativePlatform, NativePreview, NativeTarget, PlatformAction, PlatformEvent, PreviewAction,
    PreviewEvent,
};
use crate::postprocess::{PostProcessor, PostProcessorSession};
use crate::prompt_lab::{CaptureTranscription, PromptLabCapture, SttSnapshot};
use crate::session::SessionId;
use crate::ui::preview::{
    FinalizationGate, PendingPreview, PreviewCommand, PreviewUpdate, wait_for_confirmation,
};

#[derive(Debug, Clone)]
pub(super) enum AppEvent {
    Platform(PlatformEvent),
    AudioChunkAvailable { session_id: SessionId },
    FinalizationFinished { session_id: SessionId },
    Preview(PreviewEvent),
    PreviewUpdate(PreviewUpdate),
}

struct FinalizationTask {
    session_id: SessionId,
    gate: Arc<FinalizationGate>,
}

fn finalization_completion_event(
    finalization: &mut Option<FinalizationTask>,
    session_id: SessionId,
) -> SessionEvent {
    if finalization
        .as_ref()
        .is_some_and(|task| task.session_id == session_id)
    {
        *finalization = None;
    }
    SessionEvent::SessionStopped { session_id }
}

fn spawn_finalization_worker(
    session_id: SessionId,
    finish: impl FnOnce() + Send + 'static,
    notify: impl FnOnce(SessionId) + Send + 'static,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        finish();
        notify(session_id);
    })
}

pub(super) struct ListenerApplication {
    machine: RecordingSessionMachine,
    recorder: AudioRecorder,
    orchestrator: Arc<SessionOrchestrator>,
    platform: NativePlatform,
    output: ListenerOutput,
    proxy: EventLoopProxy<AppEvent>,
    finalization: Option<FinalizationTask>,
    permission_guidance: PermissionGuidance,
    preview: NativePreview,
    pending_preview: Option<PendingPreview>,
}

pub(super) enum ListenerOutput {
    Delivery {
        typer: Arc<dyn TextTyper>,
        post_processor: PostProcessor,
        history: Option<HistoryStore>,
    },
    Capture {
        capture: Box<PromptLabCapture>,
        stt: SttSnapshot,
    },
}

impl ListenerOutput {
    pub(super) fn delivery(
        typer: Arc<dyn TextTyper>,
        post_processor: PostProcessor,
        history: Option<HistoryStore>,
    ) -> Self {
        Self::Delivery {
            typer,
            post_processor,
            history,
        }
    }

    pub(super) fn capture(capture: PromptLabCapture, stt: SttSnapshot) -> Self {
        Self::Capture {
            capture: Box::new(capture),
            stt,
        }
    }

    fn start_session(&mut self, session_id: SessionId) -> Result<(), String> {
        match self {
            Self::Delivery { .. } => Ok(()),
            Self::Capture { capture, .. } => capture
                .start_now(session_id)
                .map_err(|error| error.to_string()),
        }
    }

    fn archive_chunk(&mut self, session_id: SessionId, chunk: &crate::audio::WavChunk) {
        if let Self::Capture { capture, .. } = self
            && let Err(error) = capture.push(session_id, chunk.clone())
        {
            error!(session_id = session_id.0, error = %error, "Failed to archive audio chunk");
        }
    }

    fn cancel_session(&mut self, session_id: SessionId) {
        if let Self::Capture { capture, .. } = self {
            capture.cancel(session_id);
        }
    }
}

impl ListenerApplication {
    pub(super) fn new(
        recorder: AudioRecorder,
        orchestrator: Arc<SessionOrchestrator>,
        platform: NativePlatform,
        output: ListenerOutput,
        proxy: EventLoopProxy<AppEvent>,
    ) -> Self {
        let preview_proxy = proxy.clone();
        Self {
            machine: RecordingSessionMachine::new(),
            recorder,
            orchestrator,
            platform,
            output,
            proxy,
            finalization: None,
            permission_guidance: PermissionGuidance::default(),
            preview: NativePreview::new(Box::new(move |event| {
                let _ = preview_proxy.send_event(AppEvent::Preview(event));
            })),
            pending_preview: None,
        }
    }

    fn handle_app_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Platform(event) => {
                if let Some(action) = self.platform.handle_event(event) {
                    if self.pending_preview.is_some()
                        && matches!(
                            action,
                            PlatformAction::HoldPressed | PlatformAction::ToggleRecording
                        )
                    {
                        if !self
                            .pending_preview
                            .as_ref()
                            .is_some_and(|pending| pending.is_busy())
                        {
                            self.preview.focus();
                        }
                        return;
                    }
                    if let Some(event) = platform_session_event(action, self.machine.state()) {
                        self.drive_session(event_loop, event);
                    }
                }
            }
            AppEvent::AudioChunkAvailable { session_id } => {
                self.drain_audio_chunks(event_loop, session_id);
            }
            AppEvent::FinalizationFinished { session_id } => {
                if self
                    .pending_preview
                    .as_ref()
                    .is_some_and(|p| p.session_id == session_id)
                {
                    self.preview.close();
                    self.pending_preview = None;
                }
                let event = finalization_completion_event(&mut self.finalization, session_id);
                self.drive_session(event_loop, event);
            }
            AppEvent::Preview(event) => self.handle_preview_event(event),
            AppEvent::PreviewUpdate(update) => self.handle_preview_update(update),
        }
    }

    fn handle_preview_update(&mut self, update: PreviewUpdate) {
        match update {
            PreviewUpdate::HistorySaved(text) => self.platform.push_history(text),
            PreviewUpdate::PermissionRequired => self.show_permission_guidance_if_needed(),
            PreviewUpdate::Ready {
                session_id,
                text,
                destination,
                anchor,
                warning,
                commands,
            } => {
                if !self
                    .finalization
                    .as_ref()
                    .is_some_and(|task| task.session_id == session_id && !task.gate.is_cancelled())
                {
                    return;
                }
                if let Err(error) =
                    self.preview
                        .show(session_id, &text, &destination, anchor, warning.as_deref())
                {
                    error!(%error, "Could not open confirmation preview");
                    let _ = commands.send(PreviewCommand::Cancel);
                    return;
                }
                self.pending_preview = Some(PendingPreview::new(session_id, commands));
            }
            PreviewUpdate::DeliveryFailed {
                session_id,
                error,
                retry_allowed,
            } => {
                if let Some(pending) = &mut self.pending_preview
                    && pending.failed(session_id, retry_allowed)
                {
                    self.preview.show_error(&error, retry_allowed);
                }
            }
        }
    }

    fn handle_preview_event(&mut self, event: PreviewEvent) {
        let Some(pending) = &mut self.pending_preview else {
            return;
        };
        // Queued callbacks cannot undo or race an already confirmed transaction.
        match event.action {
            PreviewAction::Confirm(text) if pending.begin(event.session_id) => {
                self.preview.hide();
                if pending
                    .commands
                    .send(PreviewCommand::Confirm(text))
                    .is_err()
                {
                    pending.failed(event.session_id, false);
                    self.preview
                        .show_error("处理线程已退出，可复制文字后取消。", false);
                }
            }
            PreviewAction::Cancel if pending.begin_cancel(event.session_id) => {
                let _ = pending.commands.send(PreviewCommand::Cancel);
                self.preview.close();
                self.pending_preview = None;
            }
            _ => {}
        }
    }

    fn show_permission_guidance_if_needed(&mut self) {
        if matches!(self.output, ListenerOutput::Delivery { .. })
            && self
                .permission_guidance
                .request(crate::platform::text_delivery_permission_available())
            && let Err(error) = crate::platform::show_text_delivery_permission_guidance()
        {
            error!(%error, "Failed to show Accessibility permission guidance");
        }
    }

    fn drain_audio_chunks(&mut self, event_loop: &ActiveEventLoop, session_id: SessionId) {
        if !matches!(
            self.machine.state(),
            RecordingState::Recording { session_id: active } if *active == session_id
        ) {
            return;
        }

        while let Some(chunk) = self.recorder.take_ready_chunk() {
            if self.recorder.streaming_overflowed() {
                self.orchestrator.fail_session(
                    session_id,
                    "Realtime recorder backlog exceeded its capacity",
                );
            }
            if let Some(error) = self.orchestrator.recording_error(session_id) {
                error!(%error, "Stopping failed Realtime recording");
                self.drive_session(event_loop, SessionEvent::StopRequested);
                return;
            }
            self.drive_session(
                event_loop,
                SessionEvent::ChunkReady {
                    session_id: chunk.session_id,
                    chunk: chunk.chunk,
                },
            );
        }
    }

    fn drive_session(&mut self, event_loop: &ActiveEventLoop, initial_event: SessionEvent) {
        if matches!(initial_event, SessionEvent::ShutdownRequested) {
            self.cancel_finalization();
        }

        let mut events = VecDeque::from([initial_event]);
        while let Some(event) = events.pop_front() {
            for effect in self.machine.handle(event) {
                match effect {
                    SessionEffect::StartSession { session_id } => {
                        events.push_back(self.start_session(session_id));
                    }
                    SessionEffect::StopSession { session_id } => {
                        if let Some(event) = self.stop_session(session_id) {
                            events.push_back(event);
                        }
                    }
                    SessionEffect::SubmitChunk { session_id, chunk } => {
                        self.output.archive_chunk(session_id, &chunk);
                        self.orchestrator.on_chunk_ready(session_id, chunk);
                    }
                    SessionEffect::CancelRecorder { session_id } => {
                        self.output.cancel_session(session_id);
                        let outcome = self.recorder.cancel_recording(session_id);
                        debug!(
                            session_id = session_id.0,
                            ?outcome,
                            "Recorder cancellation handled"
                        );
                    }
                    SessionEffect::AbortOrchestrator { session_id } => {
                        if let Err(error) = self.orchestrator.abort_session(session_id) {
                            debug!(session_id = session_id.0, error = %error, "No matching orchestrator session to abort");
                        }
                    }
                    SessionEffect::SetTrayRecording(recording) => {
                        self.platform.set_recording(recording);
                    }
                    SessionEffect::ReadyToExit => event_loop.exit(),
                }
            }
        }
    }

    /// Start resources in order, cleaning up the matching sessions when a step fails.
    fn start_session(&mut self, session_id: SessionId) -> SessionEvent {
        let session_id = match self.recorder.start_recording(session_id) {
            RecorderStartOutcome::Started { session_id } => session_id,
            RecorderStartOutcome::AlreadyRecording {
                requested_session_id,
                active_session_id,
            } => {
                self.output.cancel_session(active_session_id);
                self.cancel_recorder(active_session_id);
                self.abort_orchestrator(active_session_id);
                let error = format!("recorder session {} is already active", active_session_id.0);
                error!(
                    requested_session_id = requested_session_id.0,
                    active_session_id = active_session_id.0,
                    "Failed to start recording session"
                );
                return SessionEvent::SessionStartFailed {
                    session_id: requested_session_id,
                    error,
                };
            }
            RecorderStartOutcome::Failed { session_id, error } => {
                error!(
                    session_id = session_id.0,
                    error, "Failed to start recording session"
                );
                return SessionEvent::SessionStartFailed { session_id, error };
            }
        };

        if let Err(error) = self.orchestrator.start_session(session_id) {
            // The new recorder and the existing orchestrator may belong to different sessions.
            self.cancel_recorder(session_id);
            self.output.cancel_session(error.active);
            self.abort_orchestrator(error.active);
            error!(
                session_id = session_id.0,
                error = %error,
                "Failed to start recording session"
            );
            return SessionEvent::SessionStartFailed {
                session_id,
                error: error.to_string(),
            };
        }

        match self.output.start_session(session_id) {
            Ok(()) => SessionEvent::SessionStarted { session_id },
            Err(error) => {
                self.cancel_recorder(session_id);
                self.abort_orchestrator(session_id);
                error!(
                    session_id = session_id.0,
                    error, "Failed to start prompt-lab capture"
                );
                SessionEvent::SessionStartFailed { session_id, error }
            }
        }
    }

    fn cancel_recorder(&mut self, session_id: SessionId) {
        let cancel_outcome = self.recorder.cancel_recording(session_id);
        debug!(
            session_id = session_id.0,
            ?cancel_outcome,
            "Recorder startup rollback handled"
        );
    }

    fn abort_orchestrator(&self, session_id: SessionId) {
        if let Err(error) = self.orchestrator.abort_session(session_id) {
            debug!(
                session_id = session_id.0,
                error = %error,
                "Orchestrator startup rollback had no matching session"
            );
        }
    }

    /// Stop the recorder immediately and leave the state machine in `Stopping`
    /// while convergence, cleanup, and delivery run away from the native event loop.
    fn stop_session(&mut self, session_id: SessionId) -> Option<SessionEvent> {
        match self.recorder.stop_recording(session_id) {
            RecorderStopOutcome::Stopped {
                session_id,
                chunks,
                warning,
            } => {
                if let Some(warning) = warning.as_deref() {
                    self.orchestrator.fail_session(session_id, warning);
                    warn!(
                        session_id = session_id.0,
                        warning, "Recorder stopped with a warning"
                    );
                }
                for chunk in chunks {
                    self.output.archive_chunk(session_id, &chunk);
                    self.orchestrator.on_chunk_ready(session_id, chunk);
                }
                self.spawn_finalization(session_id);
                None
            }
            RecorderStopOutcome::StillRecording { session_id, error } => {
                error!(session_id = session_id.0, error, "Failed to stop recorder");
                Some(SessionEvent::SessionStopFailed { session_id, error })
            }
            RecorderStopOutcome::NotRecording {
                requested_session_id,
            } => {
                self.spawn_finalization(requested_session_id);
                None
            }
        }
    }

    fn spawn_finalization(&mut self, session_id: SessionId) {
        let gate = Arc::new(FinalizationGate::new());
        self.finalization = Some(FinalizationTask {
            session_id,
            gate: Arc::clone(&gate),
        });

        match &mut self.output {
            ListenerOutput::Delivery {
                typer,
                post_processor,
                history,
            } => {
                let orchestrator = Arc::clone(&self.orchestrator);
                let mut post_processor = post_processor.create_session();
                let typer = Arc::clone(typer);
                let history = history.clone();
                let proxy = self.proxy.clone();
                let preview_proxy = self.proxy.clone();
                spawn_finalization_worker(
                    session_id,
                    move || {
                        if !gate.is_cancelled()
                            && let Some(transcription) = finish_transcription(
                                orchestrator.finish_session(session_id),
                                &mut post_processor,
                            )
                        {
                            wait_for_confirmation(
                                session_id,
                                transcription,
                                typer.as_ref(),
                                history.as_ref(),
                                &gate,
                                NativeTarget::capture,
                                &|update| {
                                    preview_proxy
                                        .send_event(AppEvent::PreviewUpdate(update))
                                        .is_ok()
                                },
                            );
                        }
                    },
                    move |session_id| {
                        let _ = proxy.send_event(AppEvent::FinalizationFinished { session_id });
                    },
                );
            }
            ListenerOutput::Capture { capture, stt } => {
                let capture_session = capture.take(session_id);
                let stt = stt.clone();
                let orchestrator = Arc::clone(&self.orchestrator);
                let proxy = self.proxy.clone();
                spawn_finalization_worker(
                    session_id,
                    move || {
                        let transcription =
                            capture_transcription(orchestrator.finish_session(session_id), stt);
                        match capture_session {
                            Ok(capture_session) => match capture_session.finish(transcription) {
                                Ok(sample) => info!(
                                    sample_id = %sample.id,
                                    audio_path = %sample.audio_path.display(),
                                    sidecar_path = %sample.sidecar_path.display(),
                                    "Prompt-lab sample captured"
                                ),
                                Err(error) => error!(
                                    session_id = session_id.0,
                                    error = %error,
                                    "Failed to finalize prompt-lab capture"
                                ),
                            },
                            Err(error) => error!(
                                session_id = session_id.0,
                                error = %error,
                                "Prompt-lab capture session was unavailable"
                            ),
                        }
                    },
                    move |session_id| {
                        let _ = proxy.send_event(AppEvent::FinalizationFinished { session_id });
                    },
                );
            }
        }
    }

    fn cancel_finalization(&mut self) {
        self.preview.close();
        self.pending_preview = None; // Disconnects the waiting worker on shutdown.
        if let Some(task) = &self.finalization {
            task.gate.cancel();
        }
    }
}

impl ApplicationHandler<AppEvent> for ListenerApplication {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        self.show_permission_guidance_if_needed();
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        self.handle_app_event(event_loop, event);
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.cancel_finalization();
    }
}

fn finish_transcription(
    result: Result<String, SessionError>,
    post_processor: &mut PostProcessorSession,
) -> Option<(String, Option<String>)> {
    let prepared = match result {
        Ok(stt_text) => {
            if stt_text.is_empty() {
                info!("Transcription returned empty text");
                return None;
            }
            post_processor.push_stable_chunk(&stt_text);
            let text = match post_processor.finish() {
                Ok(processed) if !processed.is_empty() => processed,
                Ok(_) => {
                    warn!("Post-processing returned empty text, using original STT text");
                    stt_text
                }
                Err(error) => {
                    warn!(error = %error, "Post-processing failed, using original STT text");
                    stt_text
                }
            };
            Some((text, None))
        }
        Err(SessionError::NoChunks) => {
            warn!("No audio chunks to transcribe");
            None
        }
        Err(SessionError::Routing(error)) => {
            error!(error = %error, "Session routing failed while finalizing");
            None
        }
        Err(SessionError::PartialFailure {
            errors,
            partial_text,
        }) => {
            error!(
                failed_chunks = errors.len(),
                "Partial transcription failure"
            );
            Some((
                partial_text,
                Some("部分识别失败，请核对结果后输入。".into()),
            ))
        }
        Err(SessionError::ConvergenceTimeout {
            pending_count,
            partial_text,
        }) => {
            warn!(pending_count, "Convergence timeout");
            Some((
                partial_text,
                Some("识别等待超时，结果可能不完整，请核对后输入。".into()),
            ))
        }
    };
    prepared.filter(|(text, _)| !text.trim().is_empty())
}

#[cfg(test)]
fn deliver_text(
    typer: &dyn TextTyper,
    gate: &FinalizationGate,
    text: &str,
    notify_permission_required: &impl Fn(),
) {
    if let Err(error) = gate.type_text_if_active(typer, text, &|| Ok(())) {
        error!(%error, "Failed to type text");
        if !gate.is_cancelled() && crate::platform::is_text_delivery_permission_error(&error) {
            notify_permission_required();
        }
    }
}

fn capture_transcription(
    result: Result<String, SessionError>,
    stt: SttSnapshot,
) -> CaptureTranscription {
    match result {
        Ok(text) if text.is_empty() => CaptureTranscription::failed("STT returned empty text", stt),
        Ok(text) => CaptureTranscription::success(text, stt),
        Err(error) => {
            let partial_text = match &error {
                SessionError::PartialFailure { partial_text, .. }
                | SessionError::ConvergenceTimeout { partial_text, .. }
                    if !partial_text.is_empty() =>
                {
                    Some(partial_text.clone())
                }
                _ => None,
            };
            let message = error.to_string();
            match partial_text {
                Some(text) => CaptureTranscription::partial(text, message, stt),
                None => CaptureTranscription::failed(message, stt),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, mpsc};
    use std::thread;
    use std::time::Duration;

    use crate::core::recording_session::{RecordingSessionMachine, RecordingState, SessionEvent};
    use crate::input::typer::TextTyper;
    use crate::prompt_lab::SttSnapshot;
    use crate::session::SessionId;
    use crate::transcriber::TranscribeError;

    use super::{
        FinalizationGate, FinalizationTask, capture_transcription, deliver_text,
        finalization_completion_event, spawn_finalization_worker,
    };

    #[test]
    fn unrelated_errors_and_canceled_delivery_do_not_request_permission_guidance() {
        struct FailingTyper(AtomicUsize);
        impl TextTyper for FailingTyper {
            fn type_text(&self, _: &str) -> anyhow::Result<()> {
                self.0.fetch_add(1, Ordering::Relaxed);
                // A matching message is insufficient: only the platform's typed denial counts.
                anyhow::bail!("Accessibility permission is required")
            }
        }
        let typer = FailingTyper(AtomicUsize::new(0));
        let prompts = AtomicUsize::new(0);
        let notify = || {
            prompts.fetch_add(1, Ordering::Relaxed);
        };
        let gate = FinalizationGate::new();
        deliver_text(&typer, &gate, "test", &notify);
        gate.cancel();
        deliver_text(&typer, &gate, "late text", &notify);
        assert_eq!(typer.0.load(Ordering::Relaxed), 1);
        assert_eq!(prompts.load(Ordering::Relaxed), 0);
    }

    fn stt_snapshot() -> SttSnapshot {
        SttSnapshot {
            endpoint: "https://api.example.test/v1/audio/transcriptions".to_string(),
            model: "whisper-test".to_string(),
            language: Some("zh".to_string()),
            temperature: 0.0,
            prompt: Some("候选提示词".to_string()),
        }
    }

    #[test]
    fn capture_transcription_preserves_raw_success_without_post_processing() {
        let result = capture_transcription(Ok(" 原始转写 ".to_string()), stt_snapshot());

        assert_eq!(result.text.as_deref(), Some(" 原始转写 "));
        assert!(result.error.is_none());
    }

    #[test]
    fn capture_transcription_keeps_partial_text_and_failure_details() {
        let result = capture_transcription(
            Err(crate::core::orchestrator::SessionError::PartialFailure {
                errors: vec![(0, TranscribeError::Network("request failed".to_string()))],
                partial_text: "部分结果".to_string(),
            }),
            stt_snapshot(),
        );

        assert_eq!(result.text.as_deref(), Some("部分结果"));
        assert!(
            result
                .error
                .as_deref()
                .unwrap()
                .contains("1 chunk(s) failed")
        );
    }

    #[test]
    fn capture_transcription_records_empty_success_as_failure() {
        let result = capture_transcription(Ok(String::new()), stt_snapshot());

        assert!(result.text.is_none());
        assert_eq!(result.error.as_deref(), Some("STT returned empty text"));
    }

    struct CountingTyper(AtomicUsize);

    impl TextTyper for CountingTyper {
        fn type_text(&self, _text: &str) -> anyhow::Result<()> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    #[test]
    fn shutdown_cancellation_prevents_late_text_injection() {
        // A finalization worker may finish after the user exits; that stale result must
        // never paste into whichever application has focus by then.
        let gate = FinalizationGate::new();
        gate.cancel();
        let typer = CountingTyper(AtomicUsize::new(0));

        let delivered = gate
            .type_text_if_active(&typer, "late result", &|| Ok(()))
            .unwrap();

        assert!(!delivered);
        assert_eq!(typer.0.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn active_finalization_injects_text_once() {
        let gate = FinalizationGate::new();
        let typer = CountingTyper(AtomicUsize::new(0));

        let delivered = gate
            .type_text_if_active(&typer, "ready result", &|| Ok(()))
            .unwrap();

        assert!(delivered);
        assert_eq!(typer.0.load(Ordering::Relaxed), 1);
    }

    struct BlockingTyper {
        entered: Arc<Barrier>,
        release: Arc<Barrier>,
    }

    impl TextTyper for BlockingTyper {
        fn type_text(&self, _text: &str) -> anyhow::Result<()> {
            self.entered.wait();
            self.release.wait();
            Ok(())
        }
    }

    #[test]
    fn shutdown_does_not_wait_for_in_flight_text_delivery() {
        // Platform injection may already be in progress when the user exits. Cancellation
        // must return promptly while preventing any later delivery attempt.
        let gate = Arc::new(FinalizationGate::new());
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let typer = Arc::new(BlockingTyper {
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });

        let delivery_gate = Arc::clone(&gate);
        let delivery = thread::spawn(move || {
            delivery_gate
                .type_text_if_active(typer.as_ref(), "ready result", &|| Ok(()))
                .unwrap()
        });
        entered.wait();

        let cancel_gate = Arc::clone(&gate);
        let (cancelled_tx, cancelled_rx) = mpsc::channel();
        let cancellation = thread::spawn(move || {
            cancel_gate.cancel();
            cancelled_tx.send(()).unwrap();
        });
        let cancellation_completed = cancelled_rx.recv_timeout(Duration::from_secs(1)).is_ok();

        release.wait();
        assert!(delivery.join().unwrap());
        cancellation.join().unwrap();
        assert!(cancellation_completed);

        let late_typer = CountingTyper(AtomicUsize::new(0));
        assert!(
            !gate
                .type_text_if_active(&late_typer, "late result", &|| Ok(()))
                .unwrap()
        );
        assert_eq!(late_typer.0.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn finalization_worker_returns_before_completion_and_delivers_session_id() {
        let session_id = SessionId(11);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();

        let worker = spawn_finalization_worker(
            session_id,
            move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            },
            move |finished_session_id| finished_tx.send(finished_session_id).unwrap(),
        );

        started_rx.recv().unwrap();
        assert_eq!(finished_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        release_tx.send(()).unwrap();
        assert_eq!(
            finished_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            session_id
        );
        worker.join().unwrap();
    }

    #[test]
    fn matching_finalization_completion_reaches_idle_and_stale_completion_is_rejected() {
        let session_id = SessionId(1);
        let stale_session_id = SessionId(2);
        let mut machine = RecordingSessionMachine::new();
        machine.handle(SessionEvent::StartRequested);
        machine.handle(SessionEvent::SessionStarted { session_id });
        machine.handle(SessionEvent::StopRequested);
        assert_eq!(machine.state(), &RecordingState::Stopping { session_id });

        let mut finalization = Some(FinalizationTask {
            session_id,
            gate: Arc::new(FinalizationGate::new()),
        });
        let stale_event = finalization_completion_event(&mut finalization, stale_session_id);
        machine.handle(stale_event);
        assert!(finalization.is_some());
        assert_eq!(machine.state(), &RecordingState::Stopping { session_id });
        let matching_event = finalization_completion_event(&mut finalization, session_id);
        machine.handle(matching_event);
        assert!(finalization.is_none());
        assert_eq!(machine.state(), &RecordingState::Idle);
    }
}
