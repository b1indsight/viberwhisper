use tracing::{debug, error, info, warn};

use super::{LocalServiceGuard, config_context, load_config, start_local_backend};
use crate::core::config::EnvironmentSecretSource;
use crate::runtime_config::{self, ListenerConfig, ProfileSelection};
use crate::{audio, core, input, postprocess, transcriber};

pub(super) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (store, document) = load_config()?;
    let (config_dir, home_dir) = config_context(&store)?;
    let config = runtime_config::resolve_listener(
        &document,
        &EnvironmentSecretSource,
        ProfileSelection::Configured,
        &config_dir,
        &home_dir,
    )?;
    run_with_config(config)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordingInput {
    HoldPressed,
    HoldReleased,
    TogglePressed,
    TrayClicked,
}

fn normalize_recording_input(
    input: RecordingInput,
    state: &core::recording_session::RecordingState,
) -> Option<core::recording_session::SessionEvent> {
    use core::recording_session::{RecordingState, SessionEvent};

    match (input, state) {
        (
            RecordingInput::HoldPressed
            | RecordingInput::TogglePressed
            | RecordingInput::TrayClicked,
            RecordingState::Idle,
        ) => Some(SessionEvent::StartRequested),
        (
            RecordingInput::HoldReleased
            | RecordingInput::TogglePressed
            | RecordingInput::TrayClicked,
            RecordingState::Recording { .. },
        ) => Some(SessionEvent::StopRequested),
        _ => None,
    }
}

/// Runs the listener using an already resolved workflow configuration.
pub(super) fn run_with_config(
    mut config: ListenerConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    use audio::AudioRecorder;
    use core::orchestrator::SessionOrchestrator;
    use core::recording_session::{RecordingSessionMachine, SessionEvent};
    use input::hotkey::{HotkeyEvent, HotkeyManager, HotkeySource};
    use input::tray::{TrayAction, TrayManager};
    use postprocess::PostProcessor;
    use std::sync::Arc;
    use transcriber::{ApiTranscriber, Transcriber};

    println!("ViberWhisper - Voice-to-Text Input");
    println!("===================================");
    println!();

    let local_manager = start_local_backend(&mut config.backend)?;
    let _local_manager = LocalServiceGuard::new(local_manager);
    let hotkey_manager = HotkeyManager::new(&config.hotkeys);

    let mut recorder = AudioRecorder::with_config(&config.audio);

    // Build transcriber and wrap in Arc<dyn Transcriber> for orchestrator injection.
    let transcriber: Arc<dyn Transcriber> =
        Arc::new(ApiTranscriber::new(config.backend.transcriber)?);

    let post_processor = PostProcessor::new(config.backend.post_process);

    let orchestrator = SessionOrchestrator::new(Arc::clone(&transcriber), config.orchestrator);

    #[cfg(target_os = "macos")]
    let typer = crate::platform::macos::MacTyper;
    #[cfg(target_os = "windows")]
    let typer = crate::platform::windows::WindowsTyper;
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let typer = input::typer::MockTyper;

    let mut tray = TrayManager::new()?;
    info!("System tray icon started");
    let mut session_machine = RecordingSessionMachine::new();

    if let Some(hotkey) = config.hotkeys.hold_label.as_deref() {
        println!("Hold {hotkey} to record, release to transcribe.");
    }
    if let Some(hotkey) = config.hotkeys.toggle_label.as_deref() {
        println!("Press {hotkey} to start recording, press again to stop.");
    }
    println!("Press Ctrl+C to exit.");
    println!();

    let mut counter = 0;
    loop {
        tray.update();

        if let Some(action) = tray.check_action() {
            let event = match action {
                TrayAction::Exit => Some(SessionEvent::ShutdownRequested),
                TrayAction::ToggleRecording => {
                    normalize_recording_input(RecordingInput::TrayClicked, session_machine.state())
                }
            };
            if let Some(event) = event
                && drive_session(
                    &mut session_machine,
                    event,
                    &mut recorder,
                    &orchestrator,
                    &mut tray,
                    &post_processor,
                    &typer,
                )
            {
                break Ok(());
            }
        }

        if let Some(event) = hotkey_manager.check_event() {
            let input = match event {
                HotkeyEvent::Pressed(HotkeySource::Hold) => Some(RecordingInput::HoldPressed),
                HotkeyEvent::Released(HotkeySource::Hold) => Some(RecordingInput::HoldReleased),
                HotkeyEvent::Pressed(HotkeySource::Toggle) => Some(RecordingInput::TogglePressed),
                HotkeyEvent::Released(HotkeySource::Toggle) => None,
            };
            if let Some(event) =
                input.and_then(|input| normalize_recording_input(input, session_machine.state()))
            {
                let _ = drive_session(
                    &mut session_machine,
                    event,
                    &mut recorder,
                    &orchestrator,
                    &mut tray,
                    &post_processor,
                    &typer,
                );
            }
        }

        if let Some(chunk) = recorder.take_ready_chunk() {
            let _ = drive_session(
                &mut session_machine,
                SessionEvent::ChunkReady {
                    session_id: chunk.session_id,
                    chunk: chunk.chunk,
                },
                &mut recorder,
                &orchestrator,
                &mut tray,
                &post_processor,
                &typer,
            );
        }

        counter += 1;
        if counter % 300 == 0 {
            let status = format!("{:?}", session_machine.state());
            debug!(
                status = %status,
                hold_hotkey = %config.hotkeys.hold_label.as_deref().unwrap_or("disabled"),
                toggle_hotkey = %config.hotkeys.toggle_label.as_deref().unwrap_or("disabled"),
                "Heartbeat"
            );
        }

        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Executes state-machine effects and feeds component results back into the same transition queue.
fn drive_session(
    machine: &mut core::recording_session::RecordingSessionMachine,
    initial_event: core::recording_session::SessionEvent,
    recorder: &mut audio::AudioRecorder,
    orchestrator: &core::orchestrator::SessionOrchestrator,
    tray: &mut input::tray::TrayManager,
    post_processor: &postprocess::PostProcessor,
    typer: &dyn input::typer::TextTyper,
) -> bool {
    use audio::{RecorderStartOutcome, RecorderStopOutcome};
    use core::recording_session::{SessionEffect, SessionEvent};
    use std::collections::VecDeque;

    let mut events = VecDeque::from([initial_event]);
    let mut ready_to_exit = false;
    while let Some(event) = events.pop_front() {
        for effect in machine.handle(event) {
            match effect {
                SessionEffect::StartSession { session_id } => {
                    let event = match recorder.start_recording(session_id) {
                        RecorderStartOutcome::Started { session_id } => {
                            match orchestrator.start_session(session_id) {
                                Ok(()) => SessionEvent::SessionStarted { session_id },
                                Err(error) => {
                                    let active_session_id = match &error {
                                        core::orchestrator::SessionStartError::ActiveSession {
                                            active,
                                            ..
                                        } => *active,
                                    };
                                    let cancel_outcome = recorder.cancel_recording(session_id);
                                    debug!(
                                        session_id = session_id.0,
                                        ?cancel_outcome,
                                        "Recorder startup rollback handled"
                                    );
                                    if let Err(abort_error) =
                                        orchestrator.abort_session(active_session_id)
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
                            let cancel_outcome = recorder.cancel_recording(active_session_id);
                            debug!(
                                session_id = active_session_id.0,
                                ?cancel_outcome,
                                "Orphan recorder cleanup handled"
                            );
                            if let Err(error) = orchestrator.abort_session(active_session_id) {
                                debug!(
                                    session_id = active_session_id.0,
                                    error = %error,
                                    "Orphan orchestrator cleanup had no matching session"
                                );
                            }
                            let error = format!(
                                "recorder session {} is already active",
                                active_session_id.0
                            );
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
                    };
                    events.push_back(event);
                }
                SessionEffect::StopSession { session_id } => {
                    let event = match recorder.stop_recording(session_id) {
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
                                if let Err(error) = orchestrator.on_chunk_ready(session_id, chunk) {
                                    warn!(session_id = session_id.0, error = %error, "Stop-time chunk was rejected");
                                }
                            }
                            finish_transcription(
                                orchestrator.finish_session(session_id),
                                post_processor,
                                typer,
                            );
                            SessionEvent::SessionStopped { session_id }
                        }
                        RecorderStopOutcome::StillRecording { session_id, error } => {
                            error!(session_id = session_id.0, error, "Failed to stop recorder");
                            SessionEvent::SessionStopFailed { session_id, error }
                        }
                        RecorderStopOutcome::NotRecording {
                            requested_session_id,
                        } => {
                            finish_transcription(
                                orchestrator.finish_session(requested_session_id),
                                post_processor,
                                typer,
                            );
                            SessionEvent::SessionStopped {
                                session_id: requested_session_id,
                            }
                        }
                    };
                    events.push_back(event);
                }
                SessionEffect::SubmitChunk { session_id, chunk } => {
                    if let Err(error) = orchestrator.on_chunk_ready(session_id, chunk) {
                        warn!(session_id = session_id.0, error = %error, "Chunk was rejected");
                    }
                }
                SessionEffect::CancelRecorder { session_id } => {
                    let outcome = recorder.cancel_recording(session_id);
                    debug!(
                        session_id = session_id.0,
                        ?outcome,
                        "Recorder cancellation handled"
                    );
                }
                SessionEffect::AbortOrchestrator { session_id } => {
                    if let Err(error) = orchestrator.abort_session(session_id) {
                        debug!(session_id = session_id.0, error = %error, "No matching orchestrator session to abort");
                    }
                }
                SessionEffect::SetTrayRecording(recording) => tray.set_recording(recording),
                SessionEffect::ReadyToExit => ready_to_exit = true,
            }
        }
    }
    ready_to_exit
}

fn finish_transcription(
    result: Result<String, core::orchestrator::SessionError>,
    post_processor: &postprocess::PostProcessor,
    typer: &dyn input::typer::TextTyper,
) {
    use core::orchestrator::SessionError;

    match result {
        Ok(stt_text) => {
            if stt_text.is_empty() {
                info!("Transcription returned empty text");
                return;
            }
            let text = {
                let mut session = post_processor.start_session();
                session.push_stable_chunk(&stt_text);
                match session.finish() {
                    Ok(processed) if !processed.is_empty() => processed,
                    Ok(_) => {
                        warn!("Post-processing returned empty text, using original STT text");
                        stt_text
                    }
                    Err(error) => {
                        warn!(error = %error, "Post-processing failed, using original STT text");
                        stt_text
                    }
                }
            };
            info!(text = %text, "Typing transcribed text");
            if let Err(error) = typer.type_text(&text) {
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
                && let Err(error) = typer.type_text(&partial_text)
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
                && let Err(error) = typer.type_text(&partial_text)
            {
                error!(error = %error, "Failed to type partial text");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RecordingInput, normalize_recording_input};
    use crate::core::recording_session::{RecordingState, SessionEvent};
    use crate::session::SessionId;

    #[test]
    fn input_normalization_is_source_free_and_state_aware() {
        let idle = RecordingState::Idle;
        let recording = RecordingState::Recording {
            session_id: SessionId(1),
        };
        let starting = RecordingState::Starting {
            session_id: SessionId(2),
        };

        assert_eq!(
            normalize_recording_input(RecordingInput::HoldPressed, &idle),
            Some(SessionEvent::StartRequested)
        );
        assert_eq!(
            normalize_recording_input(RecordingInput::HoldReleased, &recording),
            Some(SessionEvent::StopRequested)
        );
        assert_eq!(
            normalize_recording_input(RecordingInput::TogglePressed, &idle),
            Some(SessionEvent::StartRequested)
        );
        assert_eq!(
            normalize_recording_input(RecordingInput::TogglePressed, &recording),
            Some(SessionEvent::StopRequested)
        );
        assert_eq!(
            normalize_recording_input(RecordingInput::TrayClicked, &idle),
            Some(SessionEvent::StartRequested)
        );
        assert_eq!(
            normalize_recording_input(RecordingInput::TrayClicked, &recording),
            Some(SessionEvent::StopRequested)
        );

        assert_eq!(
            normalize_recording_input(RecordingInput::HoldPressed, &recording),
            None
        );
        assert_eq!(
            normalize_recording_input(RecordingInput::HoldReleased, &idle),
            None
        );
        assert_eq!(
            normalize_recording_input(RecordingInput::TogglePressed, &starting),
            None
        );
    }
}
