use std::marker::PhantomData;
use std::sync::Arc;

use anyhow::Result;

use super::backend::PlatformBackend;
use crate::input::hotkey::{HotkeyConfig, HotkeyEvent, HotkeySource, start_hotkey_listener};
use crate::input::tray::{TrayAction, TrayEvent, TrayManager};
use crate::input::typer::TextTyper;

type HotkeyNotify = Arc<dyn Fn(HotkeyEvent) + Send + Sync>;
type TrayNotify = Arc<dyn Fn(TrayEvent) + Send + Sync>;

/// Native callback payload whose representation is private to the platform runtime.
#[derive(Debug, Clone)]
pub(crate) struct PlatformEvent(PlatformEventKind);

#[derive(Debug, Clone)]
enum PlatformEventKind {
    Hotkey(HotkeyEvent),
    Tray(TrayEvent),
}

impl PlatformEvent {
    fn hotkey(event: HotkeyEvent) -> Self {
        Self(PlatformEventKind::Hotkey(event))
    }

    fn tray(event: TrayEvent) -> Self {
        Self(PlatformEventKind::Tray(event))
    }
}

/// Platform-neutral user intent returned to the listener application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlatformAction {
    HoldPressed,
    HoldReleased,
    ToggleRecording,
    ExitRequested,
}

/// Main-thread platform owner parameterized by the backend selected in `platform.rs`.
///
/// The runtime is created and retained on the winit main thread because it owns native tray
/// state. `start` installs process-lifetime callbacks; only the cloned `TextTyper` handle
/// returned by `text_typer` may cross to a finalization worker.
pub(crate) struct PlatformRuntime<B, D = NativeDrivers>
where
    B: PlatformBackend,
    D: RuntimeDrivers<B>,
{
    tray: D::Tray,
    typer: Arc<dyn TextTyper>,
    _backend: PhantomData<B>,
}

pub(crate) trait PlatformTray {
    fn handle_event(&mut self, event: TrayEvent) -> Option<TrayAction>;
    fn set_recording(&mut self, recording: bool);
    fn set_history(&mut self, entries: Vec<String>);
    fn push_history(&mut self, text: String);
}

impl<P: crate::input::tray::TrayPolicy> PlatformTray for TrayManager<P> {
    fn handle_event(&mut self, event: TrayEvent) -> Option<TrayAction> {
        TrayManager::handle_event(self, event)
    }

    fn set_recording(&mut self, recording: bool) {
        TrayManager::set_recording(self, recording);
    }

    fn set_history(&mut self, entries: Vec<String>) {
        TrayManager::set_history(self, entries);
    }

    fn push_history(&mut self, text: String) {
        TrayManager::push_history(self, text);
    }
}

/// Injectable native-driver seam used to verify runtime wiring without installing global hooks.
pub(crate) trait RuntimeDrivers<B: PlatformBackend>: 'static {
    type Tray: PlatformTray;

    fn start_hotkeys(
        config: &HotkeyConfig,
        filter: super::backend::HotkeyFilter,
        notify: HotkeyNotify,
    );
    fn start_tray(notify: TrayNotify) -> Result<Self::Tray>;
}

pub(crate) struct NativeDrivers;

impl<B: PlatformBackend> RuntimeDrivers<B> for NativeDrivers {
    type Tray = TrayManager<B::Tray>;

    fn start_hotkeys(
        config: &HotkeyConfig,
        filter: super::backend::HotkeyFilter,
        notify: HotkeyNotify,
    ) {
        start_hotkey_listener::<B::Hotkeys, _>(config, filter, move |event| notify(event));
    }

    fn start_tray(notify: TrayNotify) -> Result<Self::Tray> {
        TrayManager::<B::Tray>::new(move |event| notify(event))
    }
}

impl<B, D> PlatformRuntime<B, D>
where
    B: PlatformBackend,
    D: RuntimeDrivers<B>,
{
    pub(crate) fn start(
        hotkeys: &HotkeyConfig,
        notify: impl Fn(PlatformEvent) + Send + Sync + 'static,
    ) -> Result<Self> {
        let notify = Arc::new(notify);
        let (typer, hotkey_filter) = B::text_typer_and_hotkey_filter();

        let hotkey_notify = Arc::clone(&notify);
        D::start_hotkeys(
            hotkeys,
            hotkey_filter,
            Arc::new(move |event| hotkey_notify(PlatformEvent::hotkey(event))),
        );

        let tray_notify = Arc::clone(&notify);
        let tray = D::start_tray(Arc::new(move |event| {
            tray_notify(PlatformEvent::tray(event));
        }))?;

        Ok(Self {
            tray,
            typer,
            _backend: PhantomData,
        })
    }

    pub(crate) fn handle_event(&mut self, event: PlatformEvent) -> Option<PlatformAction> {
        match event.0 {
            PlatformEventKind::Hotkey(event) => action_from_hotkey(event),
            PlatformEventKind::Tray(event) => match self.tray.handle_event(event) {
                Some(TrayAction::CopyHistory(text)) => {
                    if let Err(error) = B::copy_to_clipboard(&text) {
                        tracing::error!(%error, "Failed to copy transcription history");
                    }
                    None
                }
                action => action.map(action_from_tray),
            },
        }
    }

    pub(crate) fn set_recording(&mut self, recording: bool) {
        self.tray.set_recording(recording);
    }

    pub(crate) fn set_history(&mut self, entries: Vec<String>) {
        self.tray.set_history(entries);
    }

    pub(crate) fn push_history(&mut self, text: String) {
        self.tray.push_history(text);
    }

    pub(crate) fn text_typer(&self) -> Arc<dyn TextTyper> {
        Arc::clone(&self.typer)
    }
}

fn action_from_hotkey(event: HotkeyEvent) -> Option<PlatformAction> {
    match event {
        HotkeyEvent::Pressed(HotkeySource::Hold) => Some(PlatformAction::HoldPressed),
        HotkeyEvent::Released(HotkeySource::Hold) => Some(PlatformAction::HoldReleased),
        HotkeyEvent::Pressed(HotkeySource::Toggle) => Some(PlatformAction::ToggleRecording),
        HotkeyEvent::Released(HotkeySource::Toggle) => None,
    }
}

fn action_from_tray(action: TrayAction) -> PlatformAction {
    match action {
        TrayAction::ToggleRecording => PlatformAction::ToggleRecording,
        TrayAction::CopyHistory(_) => unreachable!("history copy is handled by PlatformRuntime"),
        TrayAction::Exit => PlatformAction::ExitRequested,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use rdev::{EventType, Key};
    use tray_icon::menu::{MenuEvent, MenuId};

    use crate::core::config::InputSection;
    use crate::input::hotkey::HotkeyPolicy;
    use crate::input::tray::TrayPolicy;

    static RECORDING: AtomicBool = AtomicBool::new(false);
    static TYPED_TEXT: Mutex<Vec<String>> = Mutex::new(Vec::new());

    struct FakeHotkeys;

    impl HotkeyPolicy for FakeHotkeys {
        fn unsupported_reason(_key: Key) -> Option<&'static str> {
            None
        }
    }

    struct FakeTrayPolicy;

    impl TrayPolicy for FakeTrayPolicy {
        fn idle_icon_is_template() -> bool {
            false
        }
    }

    struct RecordingTyper;

    impl TextTyper for RecordingTyper {
        fn type_text(&self, text: &str) -> Result<()> {
            TYPED_TEXT
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(text.to_string());
            Ok(())
        }
    }

    struct FakeBackend;

    impl PlatformBackend for FakeBackend {
        type Hotkeys = FakeHotkeys;
        type Tray = FakeTrayPolicy;

        fn config_dir() -> Option<PathBuf> {
            None
        }

        fn text_typer_and_hotkey_filter()
        -> (Arc<dyn TextTyper>, super::super::backend::HotkeyFilter) {
            (Arc::new(RecordingTyper), Box::new(Some))
        }

        fn copy_to_clipboard(_text: &str) -> Result<()> {
            Ok(())
        }
    }

    struct FakeTray;

    impl PlatformTray for FakeTray {
        fn handle_event(&mut self, event: TrayEvent) -> Option<TrayAction> {
            match event {
                TrayEvent::Menu(_) => Some(TrayAction::Exit),
                TrayEvent::Icon(_) => Some(TrayAction::ToggleRecording),
            }
        }

        fn set_recording(&mut self, recording: bool) {
            RECORDING.store(recording, Ordering::SeqCst);
        }

        fn set_history(&mut self, _entries: Vec<String>) {}

        fn push_history(&mut self, _text: String) {}
    }

    struct FakeDrivers;

    impl RuntimeDrivers<FakeBackend> for FakeDrivers {
        type Tray = FakeTray;

        fn start_hotkeys(
            _config: &HotkeyConfig,
            filter: super::super::backend::HotkeyFilter,
            notify: HotkeyNotify,
        ) {
            if filter(EventType::KeyPress(Key::F8)).is_some() {
                notify(HotkeyEvent::Pressed(HotkeySource::Hold));
            }
        }

        fn start_tray(notify: TrayNotify) -> Result<Self::Tray> {
            notify(TrayEvent::Menu(MenuEvent {
                id: MenuId::new("exit"),
            }));
            Ok(FakeTray)
        }
    }

    #[test]
    fn runtime_wires_drivers_events_state_and_text_delivery() {
        RECORDING.store(false, Ordering::SeqCst);
        TYPED_TEXT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();

        let hotkeys = HotkeyConfig::validate::<FakeHotkeys>(&InputSection::default()).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let received = Arc::clone(&events);
        let mut runtime =
            PlatformRuntime::<FakeBackend, FakeDrivers>::start(&hotkeys, move |event| {
                received
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(event);
            })
            .unwrap();

        let actions: Vec<_> = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .filter_map(|event| runtime.handle_event(event))
            .collect();
        assert_eq!(
            actions,
            [PlatformAction::HoldPressed, PlatformAction::ExitRequested]
        );

        runtime.set_recording(true);
        assert!(RECORDING.load(Ordering::SeqCst));

        runtime.text_typer().type_text("hello").unwrap();
        assert_eq!(
            *TYPED_TEXT
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            ["hello"]
        );
    }

    #[test]
    fn native_input_maps_to_platform_neutral_actions() {
        assert_eq!(
            action_from_hotkey(HotkeyEvent::Pressed(HotkeySource::Hold)),
            Some(PlatformAction::HoldPressed)
        );
        assert_eq!(
            action_from_hotkey(HotkeyEvent::Released(HotkeySource::Hold)),
            Some(PlatformAction::HoldReleased)
        );
        assert_eq!(
            action_from_hotkey(HotkeyEvent::Pressed(HotkeySource::Toggle)),
            Some(PlatformAction::ToggleRecording)
        );
        assert_eq!(
            action_from_hotkey(HotkeyEvent::Released(HotkeySource::Toggle)),
            None
        );
        assert_eq!(
            action_from_tray(TrayAction::ToggleRecording),
            PlatformAction::ToggleRecording
        );
        assert_eq!(
            action_from_tray(TrayAction::Exit),
            PlatformAction::ExitRequested
        );
    }
}
