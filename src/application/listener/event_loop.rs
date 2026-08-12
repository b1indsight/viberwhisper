use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use tracing::{debug, error, info, warn};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::WindowId;

use super::{hotkey_session_event, toggle_session_event};
use crate::audio::{AudioRecorder, RecorderStartOutcome, RecorderStopOutcome};
use crate::core::orchestrator::{SessionError, SessionOrchestrator};
use crate::core::recording_session::{
    RecordingSessionMachine, RecordingState, SessionEffect, SessionEvent,
};
use crate::input::hotkey::HotkeyEvent;
use crate::input::tray::{TrayAction, TrayEvent, TrayManager};
use crate::input::typer::TextTyper;
use crate::postprocess::{PostProcessor, PostProcessorSession};
use crate::session::SessionId;

#[derive(Debug, Clone)]
pub(super) enum AppEvent {
    Hotkey(HotkeyEvent),
    Tray(TrayEvent),
    AudioChunkAvailable { session_id: SessionId },
    FinalizationFinished { session_id: SessionId },
}

struct FinalizationTask {
    session_id: SessionId,
    gate: Arc<FinalizationGate>,
}

struct FinalizationGate(AtomicBool);

impl FinalizationGate {
    fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn type_text_if_active(
        &self,
        typer: &dyn TextTyper,
        text: &str,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        if self.is_cancelled() {
            return Ok(false);
        }
        typer.type_text(text)?;
        Ok(true)
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
    tray: TrayManager,
    post_processor: PostProcessor,
    typer: Arc<dyn TextTyper>,
    proxy: EventLoopProxy<AppEvent>,
    finalization: Option<FinalizationTask>,
}

impl ListenerApplication {
    pub(super) fn new(
        recorder: AudioRecorder,
        orchestrator: Arc<SessionOrchestrator>,
        tray: TrayManager,
        post_processor: PostProcessor,
        typer: Arc<dyn TextTyper>,
        proxy: EventLoopProxy<AppEvent>,
    ) -> Self {
        Self {
            machine: RecordingSessionMachine::new(),
            recorder,
            orchestrator,
            tray,
            post_processor,
            typer,
            proxy,
            finalization: None,
        }
    }

    fn handle_app_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::Hotkey(event) => {
                if let Some(event) = hotkey_session_event(event, self.machine.state()) {
                    self.drive_session(event_loop, event);
                }
            }
            AppEvent::Tray(event) => {
                if let Some(action) = self.tray.handle_event(event) {
                    let event = match action {
                        TrayAction::Exit => Some(SessionEvent::ShutdownRequested),
                        TrayAction::ToggleRecording => toggle_session_event(self.machine.state()),
                    };
                    if let Some(event) = event {
                        self.drive_session(event_loop, event);
                    }
                }
            }
            AppEvent::AudioChunkAvailable { session_id } => {
                self.drain_audio_chunks(event_loop, session_id);
            }
            AppEvent::FinalizationFinished { session_id } => {
                let event = finalization_completion_event(&mut self.finalization, session_id);
                self.drive_session(event_loop, event);
            }
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
                        if let Err(error) = self.orchestrator.on_chunk_ready(session_id, chunk) {
                            warn!(session_id = session_id.0, error = %error, "Chunk was rejected");
                        }
                    }
                    SessionEffect::CancelRecorder { session_id } => {
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
                        self.tray.set_recording(recording);
                    }
                    SessionEffect::ReadyToExit => event_loop.exit(),
                }
            }
        }
    }

    fn start_session(&mut self, session_id: SessionId) -> SessionEvent {
        match self.recorder.start_recording(session_id) {
            RecorderStartOutcome::Started { session_id } => {
                match self.orchestrator.start_session(session_id) {
                    Ok(()) => SessionEvent::SessionStarted { session_id },
                    Err(error) => {
                        let active_session_id = match &error {
                            crate::core::orchestrator::SessionStartError::ActiveSession {
                                active,
                                ..
                            } => *active,
                        };
                        let cancel_outcome = self.recorder.cancel_recording(session_id);
                        debug!(
                            session_id = session_id.0,
                            ?cancel_outcome,
                            "Recorder startup rollback handled"
                        );
                        if let Err(abort_error) = self.orchestrator.abort_session(active_session_id)
                        {
                            debug!(
                                session_id = active_session_id.0,
                                error = %abort_error,
                                "Orchestrator startup rollback had no matching session"
                            );
                        }
                        error!(
                            session_id = session_id.0,
                            error = %error,
                            "Failed to start recording session"
                        );
                        SessionEvent::SessionStartFailed {
                            session_id,
                            error: error.to_string(),
                        }
                    }
                }
            }
            RecorderStartOutcome::AlreadyRecording {
                requested_session_id,
                active_session_id,
            } => {
                let cancel_outcome = self.recorder.cancel_recording(active_session_id);
                debug!(
                    session_id = active_session_id.0,
                    ?cancel_outcome,
                    "Orphan recorder cleanup handled"
                );
                if let Err(error) = self.orchestrator.abort_session(active_session_id) {
                    debug!(
                        session_id = active_session_id.0,
                        error = %error,
                        "Orphan orchestrator cleanup had no matching session"
                    );
                }
                let error = format!("recorder session {} is already active", active_session_id.0);
                error!(
                    requested_session_id = requested_session_id.0,
                    active_session_id = active_session_id.0,
                    "Failed to start recording session"
                );
                SessionEvent::SessionStartFailed {
                    session_id: requested_session_id,
                    error,
                }
            }
            RecorderStartOutcome::Failed { session_id, error } => {
                error!(
                    session_id = session_id.0,
                    error, "Failed to start recording session"
                );
                SessionEvent::SessionStartFailed { session_id, error }
            }
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
                    warn!(
                        session_id = session_id.0,
                        warning, "Recorder stopped with a warning"
                    );
                }
                for chunk in chunks {
                    if let Err(error) = self.orchestrator.on_chunk_ready(session_id, chunk) {
                        warn!(session_id = session_id.0, error = %error, "Stop-time chunk was rejected");
                    }
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

        let orchestrator = Arc::clone(&self.orchestrator);
        let mut post_processor = self.post_processor.start_session();
        let typer = Arc::clone(&self.typer);
        let proxy = self.proxy.clone();
        spawn_finalization_worker(
            session_id,
            move || {
                finish_transcription(
                    orchestrator.finish_session(session_id),
                    &mut post_processor,
                    typer.as_ref(),
                    gate.as_ref(),
                );
            },
            move |session_id| {
                let _ = proxy.send_event(AppEvent::FinalizationFinished { session_id });
            },
        );
    }

    fn cancel_finalization(&mut self) {
        if let Some(task) = &self.finalization {
            task.gate.cancel();
        }
    }
}

impl ApplicationHandler<AppEvent> for ListenerApplication {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

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
    typer: &dyn TextTyper,
    gate: &FinalizationGate,
) {
    if gate.is_cancelled() {
        return;
    }

    match result {
        Ok(stt_text) => {
            if stt_text.is_empty() {
                info!("Transcription returned empty text");
                return;
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
            info!(text = %text, "Typing transcribed text");
            if let Err(error) = gate.type_text_if_active(typer, &text) {
                error!(error = %error, "Failed to type text");
            }
        }
        Err(SessionError::NoChunks) => warn!("No audio chunks to transcribe"),
        Err(SessionError::Routing(error)) => {
            error!(error = %error, "Session routing failed while finalizing")
        }
        Err(SessionError::PartialFailure {
            errors,
            partial_text,
        }) => {
            error!(
                failed_chunks = errors.len(),
                "Partial transcription failure"
            );
            if !partial_text.is_empty()
                && let Err(error) = gate.type_text_if_active(typer, &partial_text)
            {
                error!(error = %error, "Failed to type partial text");
            }
        }
        Err(SessionError::ConvergenceTimeout {
            pending_count,
            partial_text,
        }) => {
            warn!(pending_count, "Convergence timeout");
            if !partial_text.is_empty()
                && let Err(error) = gate.type_text_if_active(typer, &partial_text)
            {
                error!(error = %error, "Failed to type partial text");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    use crate::core::recording_session::{RecordingSessionMachine, RecordingState, SessionEvent};
    use crate::input::typer::TextTyper;
    use crate::session::SessionId;

    use super::{
        FinalizationGate, FinalizationTask, finalization_completion_event,
        spawn_finalization_worker,
    };

    struct CountingTyper(AtomicUsize);

    impl TextTyper for CountingTyper {
        fn type_text(&self, _text: &str) -> Result<(), Box<dyn std::error::Error>> {
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

        let delivered = gate.type_text_if_active(&typer, "late result").unwrap();

        assert!(!delivered);
        assert_eq!(typer.0.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn active_finalization_injects_text_once() {
        let gate = FinalizationGate::new();
        let typer = CountingTyper(AtomicUsize::new(0));

        let delivered = gate.type_text_if_active(&typer, "ready result").unwrap();

        assert!(delivered);
        assert_eq!(typer.0.load(Ordering::Relaxed), 1);
    }

    struct BlockingTyper {
        entered: Arc<Barrier>,
        release: Arc<Barrier>,
    }

    impl TextTyper for BlockingTyper {
        fn type_text(&self, _text: &str) -> Result<(), Box<dyn std::error::Error>> {
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
                .type_text_if_active(typer.as_ref(), "ready result")
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
                .type_text_if_active(&late_typer, "late result")
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
