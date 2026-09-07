use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};

use crate::audio::AudioConfig;
use crate::core::config::{
    ConfigDocument, InputSection, SecretSource, ValidationIssue, ValidationReport, fields,
};
use crate::core::orchestrator::OrchestratorConfig;
use crate::core::recording_session::{RecordingState, SessionEvent};
use crate::history::{HistoryStore, HistoryTyper};
use crate::input::hotkey::HotkeyConfig;
use crate::platform::PlatformAction;
use crate::postprocess::PostProcessConfig;
use crate::prompt_lab::{DatasetStore, PromptLabCapture, SttSnapshot};
use crate::transcriber::TranscriberConfig;
use crate::{audio, core, postprocess, transcriber};

mod event_loop;

/// Settings shared by live delivery and raw STT capture.
#[derive(Debug)]
pub(super) struct RecordingConfig {
    pub(super) hotkeys: HotkeyConfig,
    pub(super) audio: AudioConfig,
    pub(super) orchestrator: OrchestratorConfig,
    pub(super) transcriber: TranscriberConfig,
}

impl RecordingConfig {
    pub(super) fn from_config(
        document: &ConfigDocument,
        secrets: &dyn SecretSource,
    ) -> Result<Self, Vec<ValidationIssue>> {
        let mut issues = Vec::new();
        let hotkeys = ValidationReport::collect(
            document.select(
                (fields::InputHoldHotkey, fields::InputToggleHotkey),
                secrets,
                |(hold_hotkey, toggle_hotkey)| {
                    crate::platform::validate_hotkeys(&InputSection {
                        hold_hotkey,
                        toggle_hotkey,
                    })
                },
            ),
            &mut issues,
        );
        let transcriber = ValidationReport::collect(
            TranscriberConfig::from_config(document, secrets),
            &mut issues,
        );
        match (hotkeys, transcriber) {
            (Some(hotkeys), Some(transcriber)) => Ok(Self {
                hotkeys,
                audio: AudioConfig::from_config(document, secrets),
                orchestrator: document.select(
                    fields::TranscriptionLanguage,
                    secrets,
                    OrchestratorConfig::new,
                ),
                transcriber,
            }),
            _ => Err(issues),
        }
    }
}

/// Live text delivery adds optional cleanup to the recording settings.
#[derive(Debug)]
pub(super) struct ListenerConfig {
    pub(super) recording: RecordingConfig,
    pub(super) post_process: PostProcessConfig,
}

impl ListenerConfig {
    pub(super) fn from_config(
        document: &ConfigDocument,
        secrets: &dyn SecretSource,
    ) -> Result<Self, ValidationReport> {
        let mut issues = Vec::new();
        let recording =
            ValidationReport::collect(RecordingConfig::from_config(document, secrets), &mut issues);
        let post_process = ValidationReport::collect(
            PostProcessConfig::from_config(document, secrets),
            &mut issues,
        );
        match (recording, post_process) {
            (Some(recording), Some(post_process)) => Ok(Self {
                recording,
                post_process,
            }),
            _ => Err(ValidationReport::from(issues)),
        }
    }
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
pub(super) fn run_with_config(config: ListenerConfig) -> Result<()> {
    run_with_mode(
        config.recording,
        ListenerMode::Delivery(config.post_process),
    )
}

pub(super) fn run_capture(config: RecordingConfig, store: DatasetStore) -> Result<()> {
    run_with_mode(config, ListenerMode::Capture(Arc::new(store)))
}

enum ListenerMode {
    Delivery(PostProcessConfig),
    Capture(Arc<DatasetStore>),
}

fn run_with_mode(config: RecordingConfig, mode: ListenerMode) -> Result<()> {
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
        ListenerMode::Delivery(_) => None,
        ListenerMode::Capture(_) => Some(SttSnapshot::from(config.transcriber.metadata())),
    };
    let orchestrator = Arc::new(SessionOrchestrator::new(
        Arc::new(ApiTranscriber::new(config.transcriber)?),
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
        ListenerMode::Delivery(post_process) => {
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
            ListenerOutput::delivery(typer, PostProcessor::new(post_process))
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
    use super::{ListenerConfig, RecordingConfig};
    use crate::core::config::{ConfigDocument, SecretSource};
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

    struct EmptySecrets;

    impl SecretSource for EmptySecrets {
        fn get(&self, _name: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn raw_capture_settings_ignore_invalid_cleanup_configuration() {
        let mut document = ConfigDocument::default();
        document.post_process.enabled = true;
        document.inference.api.post_process.api_url = Some("not a URL".to_string());
        // Dataset capture archives raw STT, so an incomplete cleanup setup must not block it.
        let config = RecordingConfig::from_config(&document, &EmptySecrets).unwrap();
        assert_eq!(config.hotkeys.hold_label.as_deref(), Some("F8"));
        assert!(ListenerConfig::from_config(&document, &EmptySecrets).is_err());
    }

    #[test]
    fn listener_reports_errors_from_all_required_configurations() {
        use crate::core::config::ConfigKey;
        let mut document = ConfigDocument::default();
        document.input.hold_hotkey = "not a hotkey".to_string();
        document.inference.api.transcription.api_url = "not a URL".to_string();
        document.inference.api.transcription.model.clear();
        document.post_process.enabled = true;
        // Setup and config-check should report all fixable settings in one pass.
        let report = ListenerConfig::from_config(&document, &EmptySecrets).unwrap_err();
        for key in [
            ConfigKey::InputHoldHotkey,
            ConfigKey::ApiTranscriptionUrl,
            ConfigKey::ApiTranscriptionModel,
            ConfigKey::ApiPostProcessUrl,
            ConfigKey::ApiPostProcessModel,
        ] {
            assert!(
                report.issues.iter().any(|issue| issue.key == key),
                "missing {key:?}"
            );
        }
    }
}
