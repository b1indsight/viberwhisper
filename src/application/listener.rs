use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};

use crate::core::config::ListenerConfig;
use crate::core::recording_session::{RecordingState, SessionEvent};
use crate::history::{HistoryStore, HistoryTyper};
use crate::platform::PlatformAction;
use crate::prompt_lab::{DatasetStore, PromptLabCapture, SttSnapshot};
use crate::{audio, core, postprocess, transcriber};

mod event_loop;

fn platform_session_event(action: PlatformAction, state: &RecordingState) -> Option<SessionEvent> {
    match action {
        PlatformAction::HoldPressed if matches!(state, RecordingState::Idle) => {
            Some(SessionEvent::StartRequested)
        }
        PlatformAction::HoldReleased if matches!(state, RecordingState::Recording { .. }) => {
            Some(SessionEvent::StopRequested)
        }
        PlatformAction::ToggleRecording => toggle_session_event(state),
        PlatformAction::ExitRequested => Some(SessionEvent::ShutdownRequested),
        _ => None,
    }
}

fn toggle_session_event(state: &RecordingState) -> Option<SessionEvent> {
    match state {
        RecordingState::Idle => Some(SessionEvent::StartRequested),
        RecordingState::Recording { .. } => Some(SessionEvent::StopRequested),
        _ => None,
    }
}

/// Runs the listener using an already resolved workflow configuration.
pub(super) fn run_with_config(config: ListenerConfig) -> Result<()> {
    run_with_mode(config, ListenerMode::Delivery)
}

pub(super) fn run_capture(config: ListenerConfig, store: DatasetStore) -> Result<()> {
    run_with_mode(config, ListenerMode::Capture(Arc::new(store)))
}

enum ListenerMode {
    Delivery,
    Capture(Arc<DatasetStore>),
}

fn run_with_mode(config: ListenerConfig, mode: ListenerMode) -> Result<()> {
    use std::sync::Arc;

    use audio::AudioRecorder;
    use core::orchestrator::SessionOrchestrator;
    use event_loop::{AppEvent, ListenerApplication, ListenerOutput};
    use postprocess::PostProcessor;
    use transcriber::ApiTranscriber;
    use winit::event_loop::{ControlFlow, EventLoop};

    use crate::platform::NativePlatform;

    info!("ViberWhisper voice-to-text listener starting");

    let capture_stt = match &mode {
        ListenerMode::Delivery => None,
        ListenerMode::Capture(_) => Some(SttSnapshot::from(config.backend.transcriber.metadata())),
    };
    let orchestrator = Arc::new(SessionOrchestrator::new(
        Arc::new(ApiTranscriber::new(config.backend.transcriber)?),
        config.orchestrator,
    ));

    let event_loop = EventLoop::<AppEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let platform_proxy = proxy.clone();
    let mut platform = NativePlatform::start(&config.hotkeys, move |event| {
        let _ = platform_proxy.send_event(AppEvent::Platform(event));
    })?;
    let output = match mode {
        ListenerMode::Delivery => {
            let history_store = HistoryStore::discover()
                .inspect_err(|error| warn!(%error, "Transcription history is unavailable"))
                .ok();
            let recent_history = history_store
                .as_ref()
                .and_then(|store| {
                    store
                        .load_recent()
                        .inspect_err(
                            |error| warn!(%error, "Ignoring unusable transcription history"),
                        )
                        .ok()
                })
                .unwrap_or_default();
            platform.set_history(recent_history);
            let typer = match history_store {
                Some(store) => {
                    let history_proxy = proxy.clone();
                    Arc::new(HistoryTyper::new(
                        store,
                        platform.text_typer(),
                        move |text| {
                            let _ = history_proxy.send_event(AppEvent::HistorySaved(text));
                        },
                    )) as Arc<dyn crate::input::typer::TextTyper>
                }
                None => platform.text_typer(),
            };
            ListenerOutput::delivery(typer, PostProcessor::new(config.backend.post_process))
        }
        ListenerMode::Capture(store) => ListenerOutput::capture(
            PromptLabCapture::new(store),
            capture_stt.expect("capture mode snapshots STT metadata"),
        ),
    };

    let audio_proxy = proxy.clone();
    let recorder = AudioRecorder::with_config(&config.audio, move |session_id| {
        let _ = audio_proxy.send_event(AppEvent::AudioChunkAvailable { session_id });
    });

    info!("System tray icon started");

    if matches!(output, ListenerOutput::Capture { .. }) {
        info!(
            "Prompt-lab capture mode enabled; audio and raw STT results will be saved without typing text"
        );
    }

    if let Some(hotkey) = config.hotkeys.hold_label.as_deref() {
        info!(mode = "hold", hotkey, "Recording hotkey enabled");
    }
    if let Some(hotkey) = config.hotkeys.toggle_label.as_deref() {
        info!(mode = "toggle", hotkey, "Recording hotkey enabled");
    }
    info!("Listener ready; press Ctrl+C to exit");

    let mut application = ListenerApplication::new(recorder, orchestrator, platform, output, proxy);
    event_loop.run_app(&mut application)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::platform_session_event;
    use crate::core::recording_session::{RecordingState, SessionEvent};
    use crate::platform::PlatformAction;
    use crate::session::SessionId;

    #[test]
    fn platform_actions_are_state_aware() {
        let idle = RecordingState::Idle;
        let recording = RecordingState::Recording {
            session_id: SessionId(1),
        };
        let starting = RecordingState::Starting {
            session_id: SessionId(2),
        };

        assert_eq!(
            platform_session_event(PlatformAction::HoldPressed, &idle),
            Some(SessionEvent::StartRequested)
        );
        assert_eq!(
            platform_session_event(PlatformAction::HoldReleased, &recording),
            Some(SessionEvent::StopRequested)
        );
        assert_eq!(
            platform_session_event(PlatformAction::ToggleRecording, &idle),
            Some(SessionEvent::StartRequested)
        );
        assert_eq!(
            platform_session_event(PlatformAction::ToggleRecording, &recording),
            Some(SessionEvent::StopRequested)
        );
        assert_eq!(
            platform_session_event(PlatformAction::ExitRequested, &idle),
            Some(SessionEvent::ShutdownRequested)
        );
        assert_eq!(
            platform_session_event(PlatformAction::HoldPressed, &recording),
            None
        );
        assert_eq!(
            platform_session_event(PlatformAction::HoldReleased, &idle),
            None
        );
        assert_eq!(
            platform_session_event(PlatformAction::ToggleRecording, &starting),
            None
        );
    }
}
