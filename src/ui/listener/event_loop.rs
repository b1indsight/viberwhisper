use std::collections::VecDeque;
use std::sync::{Arc, mpsc};
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
use crate::dictation::Progress;
use crate::history::HistoryStore;
use crate::input::typer::TextTyper;
use crate::platform::{
    DeliveryTarget, NativePlatform, NativePreview, NativeTarget, PlatformAction, PlatformEvent,
    PreviewAction, PreviewEvent, PreviewRect,
};
use crate::postprocess::PostProcessor;
use crate::prompt_lab::{CaptureTranscription, PromptLabCapture, SttSnapshot};
use crate::session::SessionId;
use crate::ui::preview::{
    FinalizationGate, PendingPreview, PreviewCommand, PreviewTranscription, PreviewUpdate,
    wait_for_confirmation,
};

#[derive(Debug, Clone)]
pub(super) enum AppEvent {
    Platform(PlatformEvent),
    AudioChunkAvailable { session_id: SessionId },
    RecognitionProgress { session_id: SessionId },
    FinalizationFinished { session_id: SessionId },
    Preview(PreviewEvent),
    PreviewUpdate(PreviewUpdate),
}

struct FinalizationTask {
    session_id: SessionId,
    gate: Arc<FinalizationGate>,
    stop: Option<mpsc::Sender<()>>,
}

struct StreamingPreview {
    session_id: SessionId,
    progress: Progress,
    destination: Option<(String, Option<PreviewRect>)>,
    shown: bool,
}

impl StreamingPreview {
    /// Route snapshots to the existing bubble, suppressing only an initially empty one.
    /// The UI owner supplies native window operations; filtering and shown state stay here.
    fn render(
        &mut self,
        text: &str,
        display: impl FnOnce(bool, &str, Option<PreviewRect>) -> anyhow::Result<()>,
    ) {
        let Some((destination, anchor)) = &self.destination else {
            return;
        };
        if text.is_empty() && !self.shown {
            return;
        }
        match display(self.shown, destination, *anchor) {
            Ok(()) => self.shown = true,
            Err(error) => {
                error!(%error, "Could not display streaming recognition");
                // Recognition can still complete into the ordinary final preview.
                self.progress.close();
            }
        }
    }
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
    streaming_preview: Option<StreamingPreview>,
}

pub(super) enum ListenerOutput {
    Delivery {
        typer: Arc<dyn TextTyper>,
        post_processor: Arc<PostProcessor>,
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
            post_processor: Arc::new(post_processor),
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
            streaming_preview: None,
        }
    }

    fn handle_app_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Platform(event) => {
                if let Some(action) = self.platform.handle_event(event) {
                    if (self.pending_preview.is_some()
                        || self.streaming_preview.is_some()
                            && matches!(self.machine.state(), RecordingState::Stopping { .. }))
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
                    .streaming_preview
                    .as_ref()
                    .is_some_and(|stream| stream.session_id == session_id)
                {
                    self.streaming_preview.take().unwrap().progress.close();
                    self.preview.close();
                }
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
            AppEvent::RecognitionProgress { session_id } => self.render_progress(session_id),
            AppEvent::Preview(event) => self.handle_preview_event(event_loop, event),
            AppEvent::PreviewUpdate(update) => self.handle_preview_update(update),
        }
    }

    fn handle_preview_update(&mut self, update: PreviewUpdate) {
        match update {
            PreviewUpdate::HistorySaved(text) => self.platform.push_history(text),
            PreviewUpdate::PermissionRequired => self.show_permission_guidance_if_needed(),
            PreviewUpdate::Streaming {
                session_id,
                destination,
                anchor,
            } => {
                if let Some(stream) = &mut self.streaming_preview
                    && stream.session_id == session_id
                {
                    stream.destination = Some((destination, anchor));
                    self.render_progress(session_id);
                }
            }
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
                let shown = self.streaming_preview.take().is_some_and(|stream| {
                    stream.progress.close();
                    stream.session_id == session_id && stream.shown
                });
                let display = if shown {
                    self.preview.update_text(&text, true, warning.as_deref())
                } else {
                    self.preview
                        .show(session_id, &text, &destination, anchor, warning.as_deref())
                };
                if let Err(error) = display {
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

    fn render_progress(&mut self, session_id: SessionId) {
        if !self
            .finalization
            .as_ref()
            .is_some_and(|task| task.session_id == session_id && !task.gate.is_cancelled())
        {
            return;
        }
        let Some(stream) = self
            .streaming_preview
            .as_mut()
            .filter(|stream| stream.session_id == session_id)
        else {
            return;
        };
        let text = stream.progress.latest();
        stream.render(&text, |shown, destination, anchor| {
            if shown {
                self.preview.update_text(&text, false, None)
            } else {
                self.preview.show_streaming(
                    session_id,
                    &text,
                    destination,
                    anchor,
                    matches!(self.machine.state(), RecordingState::Stopping { .. }),
                )
            }
        });
    }

    fn handle_preview_event(&mut self, event_loop: &ActiveEventLoop, event: PreviewEvent) {
        if matches!(event.action, PreviewAction::Cancel)
            && self
                .streaming_preview
                .as_ref()
                .is_some_and(|stream| stream.session_id == event.session_id)
        {
            self.cancel_finalization();
            self.drive_session(
                event_loop,
                SessionEvent::CancelRequested {
                    session_id: event.session_id,
                },
            );
            return;
        }
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

        let progress = if self.orchestrator.is_realtime()
            && matches!(self.output, ListenerOutput::Delivery { .. })
        {
            let proxy = self.proxy.clone();
            Some(Progress::new(move || {
                let _ = proxy.send_event(AppEvent::RecognitionProgress { session_id });
            }))
        } else {
            None
        };
        let started = match &progress {
            Some(progress) => self
                .orchestrator
                .start_session_with_progress(session_id, Some(progress.clone())),
            None => self.orchestrator.start_session(session_id),
        };
        if let Err(error) = started {
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
            Ok(()) => {
                if let Some(progress) = progress {
                    self.streaming_preview = Some(StreamingPreview {
                        session_id,
                        progress,
                        destination: None,
                        shown: false,
                    });
                    let (stop, receiver) = mpsc::channel();
                    let gate = Arc::new(FinalizationGate::new());
                    self.finalization = Some(FinalizationTask {
                        session_id,
                        gate: gate.clone(),
                        stop: Some(stop),
                    });
                    self.spawn_delivery_worker(session_id, gate, Some(receiver));
                }
                SessionEvent::SessionStarted { session_id }
            }
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
        if let Some(task) = &mut self.finalization
            && task.session_id == session_id
            && let Some(stop) = task.stop.take()
        {
            let _ = stop.send(());
            if self
                .streaming_preview
                .as_ref()
                .is_some_and(|stream| stream.shown)
            {
                self.preview.focus();
            }
            return;
        }
        let gate = Arc::new(FinalizationGate::new());
        self.finalization = Some(FinalizationTask {
            session_id,
            gate: Arc::clone(&gate),
            stop: None,
        });

        match &mut self.output {
            ListenerOutput::Delivery { .. } => self.spawn_delivery_worker(session_id, gate, None),
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

    /// Realtime captures its destination before the first bubble and waits for stop
    /// on this same delivery worker. Progress reaches the UI independently while
    /// final recognition blocks here; native target objects never cross threads.
    fn spawn_delivery_worker(
        &mut self,
        session_id: SessionId,
        gate: Arc<FinalizationGate>,
        stop: Option<mpsc::Receiver<()>>,
    ) {
        let ListenerOutput::Delivery {
            typer,
            post_processor,
            history,
        } = &self.output
        else {
            return;
        };
        let orchestrator = Arc::clone(&self.orchestrator);
        let post_processor = Arc::clone(post_processor);
        let typer = Arc::clone(typer);
        let history = history.clone();
        let proxy = self.proxy.clone();
        let preview_proxy = self.proxy.clone();
        spawn_finalization_worker(
            session_id,
            move || {
                let notify = |update| {
                    preview_proxy
                        .send_event(AppEvent::PreviewUpdate(update))
                        .is_ok()
                };
                let captured = if let Some(stop) = stop {
                    let target = NativeTarget::capture();
                    if !notify(PreviewUpdate::Streaming {
                        session_id,
                        destination: target
                            .as_ref()
                            .map_or_else(|_| "无法确定输入目标".into(), DeliveryTarget::name),
                        anchor: target
                            .as_ref()
                            .ok()
                            .and_then(DeliveryTarget::preview_anchor),
                    }) || stop.recv().is_err()
                        || gate.is_cancelled()
                    {
                        if let Ok(target) = &target
                            && let Err(error) = target.restore()
                        {
                            debug!(%error, "Could not restore cancelled streaming destination");
                        }
                        return;
                    }
                    Some(target)
                } else {
                    None
                };
                let transcription = (!gate.is_cancelled())
                    .then(|| {
                        finish_transcription(
                            orchestrator.finish_for_delivery(session_id),
                            &post_processor,
                        )
                    })
                    .flatten();
                if let Some(transcription) = transcription.filter(|_| !gate.is_cancelled()) {
                    wait_for_confirmation(
                        session_id,
                        transcription,
                        typer.as_ref(),
                        history.as_ref(),
                        &gate,
                        || captured.unwrap_or_else(NativeTarget::capture),
                        &notify,
                    );
                } else if let Some(Ok(target)) = captured
                    && let Err(error) = target.restore()
                {
                    debug!(%error, "Could not restore failed streaming destination");
                }
            },
            move |session_id| {
                let _ = proxy.send_event(AppEvent::FinalizationFinished { session_id });
            },
        );
    }

    fn cancel_finalization(&mut self) {
        self.preview.close();
        self.pending_preview = None; // Disconnects the waiting worker on shutdown.
        if let Some(stream) = self.streaming_preview.take() {
            stream.progress.cancel();
        }
        if let Some(task) = &mut self.finalization {
            task.gate.cancel();
            task.stop.take();
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
        self.orchestrator.shutdown();
        if let ListenerOutput::Delivery { post_processor, .. } = &self.output {
            post_processor.shutdown();
        }
    }
}

fn finish_transcription(
    result: Result<crate::dictation::Transcript, SessionError>,
    post_processor: &PostProcessor,
) -> Option<PreviewTranscription> {
    let (result, mut confirmation) = match result {
        Ok(transcript) => (Ok(transcript.text), transcript.confirmation),
        Err(error) => (Err(error), None),
    };
    let prepared = match result {
        Ok(stt_text) => {
            if stt_text.is_empty() {
                info!("Transcription returned empty text");
                return None;
            }
            let text = match post_processor.process_for_delivery(&stt_text) {
                Ok(processed) if !processed.text.is_empty() => {
                    confirmation = processed.confirmation.or(confirmation);
                    processed.text
                }
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
    prepared
        .filter(|(text, _)| !text.trim().is_empty())
        .map(|(text, warning)| PreviewTranscription {
            text,
            warning,
            confirmation,
        })
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
        FinalizationGate, FinalizationTask, StreamingPreview, capture_transcription,
        finalization_completion_event, spawn_finalization_worker,
    };

    #[test]
    fn listener_progress_retracts_and_resumes_without_opening_an_initially_empty_bubble() {
        let mut stream = StreamingPreview {
            session_id: SessionId(1),
            progress: crate::dictation::Progress::new(|| {}),
            destination: Some(("saved editor".into(), None)),
            shown: false,
        };
        let mut visible = None;
        let mut operations = Vec::new();
        // A validated empty aggregate retracts a previously streamed draft while
        // recording continues. Exercise the listener's routing, not NativePreview directly.
        for (snapshot, expected) in [
            ("", None),
            ("暂定正文", Some("暂定正文")),
            ("", Some("")),
            ("后续识别正文", Some("后续识别正文")),
        ] {
            stream.render(snapshot, |shown, destination, anchor| {
                assert_eq!(
                    shown,
                    visible.is_some(),
                    "updates must reuse the visible bubble"
                );
                assert_eq!(destination, "saved editor");
                assert!(anchor.is_none());
                operations.push(shown);
                visible = Some(snapshot.to_owned());
                Ok(())
            });
            assert_eq!(visible.as_deref(), expected);
        }
        assert_eq!(operations, [false, true, true]); // Show once, then two updates.
    }

    fn stt_snapshot() -> SttSnapshot {
        SttSnapshot {
            input_layout: None,
            endpoint: "https://api.example.test/v1/audio/transcriptions".to_string(),
            model: "whisper-test".to_string(),
            language: Some("zh".to_string()),
            temperature: 0.0,
            prompt: Some("候选提示词".to_string()),
        }
    }

    #[test]
    fn http_finalization_keeps_feedback_and_guards_failed_or_untrusted_results() {
        use crate::core::{config::ConfigDocument, listener::ListenerConfig};
        use crate::postprocess::{PostProcessor, test_support::stub};
        use serde_json::json;

        // Exercise the actual desktop finalization adapter: returning a clean
        // string but losing its confirmation handle would silently disable learning.
        let wrong = "使用维伯耳语。";
        let response = |context| {
            json!({
                "text":wrong, "context":context,
                "memory_candidates":[{"kind":"proper_noun","text":"维伯耳语","quote":wrong}]
            })
            .to_string()
        };
        let (endpoint, requests) = stub(vec![
            response(json!({"kind":"new","label":"项目"})),
            response(json!({"kind":"new","label":"项目"})),
            response(json!({"kind":"existing","id":"项目"})),
            response(json!({"kind":"new","label":"其他"})),
            json!({"text":wrong,"context":{"kind":"existing","id":"项目"},"memory_candidates":[]})
                .to_string(),
            "invalid JSON".into(),
            response(json!({"kind":"existing","id":"项目"})),
        ]);
        let mut document = ConfigDocument::default();
        document.dictation.url = "unused invalid Realtime URL".into();
        document.post_process.enabled = true;
        document.post_process.preheat_enabled = false;
        document.inference.api.post_process.api_url = Some(endpoint.to_string());
        document.inference.api.post_process.model = Some("fixture".into());
        let config = ListenerConfig::from_config(&document).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("dictation-memory.json");
        let processor = PostProcessor::new_desktop(
            config.post_process,
            config.memory,
            Some(path.clone()),
            config.language,
        )
        .unwrap();
        let finish =
            || super::finish_transcription(Ok(wrong.to_owned().into()), &processor).unwrap();
        drop(finish()); // Cancel the first preview before confirming any edit.
        let first = finish();
        assert!(!path.exists());
        first
            .confirmation
            .unwrap()
            .confirm("使用ViberWhisper。".into())
            .unwrap();
        let corrected = finish();
        assert_eq!(corrected.text, "使用ViberWhisper。");
        drop(corrected);
        for _ in 0..2 {
            let unchanged = finish();
            assert_eq!(
                unchanged.text, wrong,
                "different context or missing candidate must not trigger replacement"
            );
            drop(unchanged);
        }
        let fallback = finish();
        assert_eq!(fallback.text, wrong);
        assert!(fallback.confirmation.is_none());
        assert_eq!(processor.process_text(wrong).unwrap(), wrong); // CLI/setup stay isolated.
        processor.shutdown();

        let inputs: Vec<serde_json::Value> = (0..7)
            .map(|_| {
                let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
                serde_json::from_str(request.body["messages"][1]["content"].as_str().unwrap())
                    .unwrap()
            })
            .collect();
        assert_eq!(inputs[0]["contexts"], json!([]));
        assert_eq!(
            inputs[1]["contexts"],
            json!([]),
            "cancelled output must not supply context"
        );
        assert_eq!(
            inputs[6]["contexts"],
            json!([]),
            "plain-text entry must not upload personal memory"
        );
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(stored["observations"].as_array().unwrap().len(), 1);
        assert_eq!(stored["observations"][0]["text"], "使用ViberWhisper。");
        assert_eq!(stored["corrections"].as_array().unwrap().len(), 1);
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
    fn cancelled_preview_handoffs_restore_before_finalization_completes() {
        use crate::dictation::Confirmation;
        use crate::history::HistoryStore;
        use crate::platform::DeliveryTarget;
        use crate::ui::preview::{
            PreviewCommand, PreviewTranscription, PreviewUpdate, wait_for_confirmation,
        };

        struct RestoreTarget(Arc<AtomicUsize>);
        impl DeliveryTarget for RestoreTarget {
            fn name(&self) -> String {
                "saved editor".into()
            }
            fn restore(&self) -> anyhow::Result<()> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            fn validate(&self) -> anyhow::Result<()> {
                panic!("an abandoned preview must not validate for injection")
            }
        }

        #[derive(Clone, Copy, Debug)]
        enum Handoff {
            BeforeCapture,
            DuringCapture,
            RejectedReady,
            DisconnectedReady,
            CancelledCommand,
        }
        // Esc can win before Ready, while it is queued, or before its command is
        // consumed. Dropping Ready also disconnects the worker without a command.
        for handoff in [
            Handoff::BeforeCapture,
            Handoff::DuringCapture,
            Handoff::RejectedReady,
            Handoff::DisconnectedReady,
            Handoff::CancelledCommand,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let history = HistoryStore::at(directory.path().join("history.jsonl"), 4096);
            let worker_history = history.clone();
            let restores = Arc::new(AtomicUsize::new(0));
            let restored = Arc::clone(&restores);
            let typer = Arc::new(CountingTyper(AtomicUsize::new(0)));
            let worker_typer = Arc::clone(&typer);
            let (confirmation, feedback) = Confirmation::channel();
            let (completed, completion) = mpsc::channel();
            let worker = spawn_finalization_worker(
                SessionId(11),
                move || {
                    let gate = FinalizationGate::new();
                    let target = RestoreTarget(restores); // Already captured by streaming.
                    if matches!(handoff, Handoff::BeforeCapture) {
                        gate.cancel();
                    }
                    wait_for_confirmation(
                        SessionId(11),
                        PreviewTranscription {
                            text: "candidate".into(),
                            warning: None,
                            confirmation: Some(confirmation),
                        },
                        worker_typer.as_ref(),
                        Some(&worker_history),
                        &gate,
                        || {
                            if matches!(handoff, Handoff::DuringCapture) {
                                gate.cancel();
                            }
                            Ok(target)
                        },
                        &|update| {
                            let PreviewUpdate::Ready { commands, .. } = update else {
                                panic!("an abandoned preview must not save or learn");
                            };
                            if matches!(handoff, Handoff::CancelledCommand) {
                                gate.cancel();
                                commands
                                    .send(PreviewCommand::Confirm("stale".into()))
                                    .unwrap();
                            }
                            !matches!(handoff, Handoff::RejectedReady)
                        },
                    );
                },
                move |session_id| {
                    assert_eq!(restored.load(Ordering::SeqCst), 1, "{handoff:?}");
                    completed.send(session_id).unwrap();
                },
            );
            assert_eq!(
                completion.recv_timeout(Duration::from_secs(1)).unwrap(),
                SessionId(11)
            );
            worker.join().unwrap();
            assert_eq!(typer.0.load(Ordering::Relaxed), 0);
            assert!(history.load_recent().unwrap().is_empty());
            assert!(feedback.blocking_recv().is_err());
        }
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
            stop: None,
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
