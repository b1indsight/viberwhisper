use tracing::info;

use super::{LocalServiceGuard, config_context, load_config, start_local_backend};
use crate::core::config::EnvironmentSecretSource;
use crate::core::recording_session::{RecordingState, SessionEvent};
use crate::input::hotkey::{HotkeyEvent, HotkeySource};
use crate::runtime_config::{self, ListenerConfig, ProfileSelection};
use crate::{audio, core, input, postprocess, transcriber};

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

fn hotkey_session_event(event: HotkeyEvent, state: &RecordingState) -> Option<SessionEvent> {
    match event {
        HotkeyEvent::Pressed(HotkeySource::Hold) if matches!(state, RecordingState::Idle) => {
            Some(SessionEvent::StartRequested)
        }
        HotkeyEvent::Released(HotkeySource::Hold)
            if matches!(state, RecordingState::Recording { .. }) =>
        {
            Some(SessionEvent::StopRequested)
        }
        HotkeyEvent::Pressed(HotkeySource::Toggle) => toggle_session_event(state),
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
    use input::hotkey::start_hotkey_listener;
    use input::tray::TrayManager;
    use postprocess::PostProcessor;
    use transcriber::ApiTranscriber;
    use winit::event_loop::{ControlFlow, EventLoop};

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

    #[cfg(target_os = "macos")]
    let typer = Arc::new(crate::platform::macos::MacTyper);
    #[cfg(target_os = "windows")]
    let typer = Arc::new(crate::platform::windows::WindowsTyper);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let typer = Arc::new(input::typer::MockTyper);

    let event_loop = EventLoop::<AppEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let hotkey_proxy = proxy.clone();
    start_hotkey_listener(&config.hotkeys, move |event| {
        let _ = hotkey_proxy.send_event(AppEvent::Hotkey(event));
    });

    let audio_proxy = proxy.clone();
    let recorder = AudioRecorder::with_config(&config.audio, move |session_id| {
        let _ = audio_proxy.send_event(AppEvent::AudioChunkAvailable { session_id });
    });

    let tray_proxy = proxy.clone();
    let tray = TrayManager::new(move |event| {
        let _ = tray_proxy.send_event(AppEvent::Tray(event));
    })?;
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
        ListenerApplication::new(recorder, orchestrator, tray, post_processor, typer, proxy);
    event_loop.run_app(&mut application)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{hotkey_session_event, toggle_session_event};
    use crate::core::recording_session::{RecordingState, SessionEvent};
    use crate::input::hotkey::{HotkeyEvent, HotkeySource};
    use crate::session::SessionId;

    #[test]
    fn hotkey_and_toggle_requests_are_state_aware() {
        let idle = RecordingState::Idle;
        let recording = RecordingState::Recording {
            session_id: SessionId(1),
        };
        let starting = RecordingState::Starting {
            session_id: SessionId(2),
        };

        assert_eq!(
            hotkey_session_event(HotkeyEvent::Pressed(HotkeySource::Hold), &idle),
            Some(SessionEvent::StartRequested)
        );
        assert_eq!(
            hotkey_session_event(HotkeyEvent::Released(HotkeySource::Hold), &recording),
            Some(SessionEvent::StopRequested)
        );
        assert_eq!(
            hotkey_session_event(HotkeyEvent::Pressed(HotkeySource::Toggle), &idle),
            Some(SessionEvent::StartRequested)
        );
        assert_eq!(
            hotkey_session_event(HotkeyEvent::Pressed(HotkeySource::Toggle), &recording),
            Some(SessionEvent::StopRequested)
        );
        assert_eq!(
            toggle_session_event(&idle),
            Some(SessionEvent::StartRequested)
        );
        assert_eq!(
            toggle_session_event(&recording),
            Some(SessionEvent::StopRequested)
        );

        assert_eq!(
            hotkey_session_event(HotkeyEvent::Pressed(HotkeySource::Hold), &recording),
            None
        );
        assert_eq!(
            hotkey_session_event(HotkeyEvent::Released(HotkeySource::Hold), &idle),
            None
        );
        assert_eq!(
            hotkey_session_event(HotkeyEvent::Pressed(HotkeySource::Toggle), &starting),
            None
        );
        assert_eq!(toggle_session_event(&starting), None);
    }
}
