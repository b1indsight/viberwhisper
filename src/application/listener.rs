use tracing::info;

use super::{LocalServiceGuard, config_context, load_config, start_local_backend};
use crate::core::config::EnvironmentSecretSource;
use crate::core::recording_session::{RecordingState, SessionEvent};
use crate::platform::PlatformAction;
use crate::runtime_config::{self, ListenerConfig, ProfileSelection};
use crate::{audio, core, postprocess, transcriber};

mod event_loop;

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
pub(super) fn run_with_config(
    mut config: ListenerConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;

    use audio::AudioRecorder;
    use core::orchestrator::SessionOrchestrator;
    use event_loop::{AppEvent, ListenerApplication};
    use postprocess::PostProcessor;
    use transcriber::ApiTranscriber;
    use winit::event_loop::{ControlFlow, EventLoop};

    use crate::platform::{NativePlatform, PlatformInterface};

    println!("ViberWhisper - Voice-to-Text Input");
    println!("===================================");
    println!();

    let local_manager = start_local_backend(&mut config.backend)?;
    let _local_manager = LocalServiceGuard::new(local_manager);

    let post_processor = PostProcessor::new(config.backend.post_process);
    let orchestrator = Arc::new(SessionOrchestrator::new(
        Arc::new(ApiTranscriber::new(config.backend.transcriber)?),
        config.orchestrator,
    ));

    let event_loop = EventLoop::<AppEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let platform_proxy = proxy.clone();
    let platform = NativePlatform::start(&config.hotkeys, move |event| {
        let _ = platform_proxy.send_event(AppEvent::Platform(event));
    })?;

    let audio_proxy = proxy.clone();
    let recorder = AudioRecorder::with_config(&config.audio, move |session_id| {
        let _ = audio_proxy.send_event(AppEvent::AudioChunkAvailable { session_id });
    });

    info!("System tray icon started");

    if let Some(hotkey) = config.hotkeys.hold_label.as_deref() {
        println!("Hold {hotkey} to record, release to transcribe.");
    }
    if let Some(hotkey) = config.hotkeys.toggle_label.as_deref() {
        println!("Press {hotkey} to start recording, press again to stop.");
    }
    println!("Press Ctrl+C to exit.");
    println!();

    let mut application =
        ListenerApplication::new(recorder, orchestrator, platform, post_processor, proxy);
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
