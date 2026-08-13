use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use rdev::Key;

use super::backend::{HotkeyFilter, PlatformBackend};
use crate::input::hotkey::HotkeyPolicy;
use crate::input::tray::TrayPolicy;
use crate::input::typer::TextTyper;
use tracing::{debug, info};

mod accessibility;
mod hotkey;
mod pasteboard;

pub(crate) struct MacBackend;
pub(crate) struct MacHotkeys;
pub(crate) struct MacTray;

impl PlatformBackend for MacBackend {
    type Hotkeys = MacHotkeys;
    type Tray = MacTray;

    fn config_dir() -> Option<PathBuf> {
        dirs::config_dir().map(|base| base.join("com.b1indsight.viberwhisper"))
    }

    fn text_typer_and_hotkey_filter() -> (Arc<dyn TextTyper>, HotkeyFilter) {
        let (typer, filter) = MacTyper::new();
        (Arc::new(typer), Box::new(filter))
    }
}

impl HotkeyPolicy for MacHotkeys {
    fn unsupported_reason(key: Key) -> Option<&'static str> {
        match key {
            Key::CapsLock => {
                Some("macOS reports Caps Lock as a state change, not a physical press/release pair")
            }
            Key::ControlRight
            | Key::Delete
            | Key::Insert
            | Key::Home
            | Key::End
            | Key::PageUp
            | Key::PageDown
            | Key::NumLock
            | Key::ScrollLock
            | Key::PrintScreen
            | Key::Pause
            | Key::IntlBackslash
            | Key::KpReturn
            | Key::KpMinus
            | Key::KpPlus
            | Key::KpMultiply
            | Key::KpDivide
            | Key::Kp0
            | Key::Kp1
            | Key::Kp2
            | Key::Kp3
            | Key::Kp4
            | Key::Kp5
            | Key::Kp6
            | Key::Kp7
            | Key::Kp8
            | Key::Kp9
            | Key::KpDelete => Some("the current rdev macOS backend does not emit this named key"),
            _ => None,
        }
    }
}

impl TrayPolicy for MacTray {
    fn idle_icon_is_template() -> bool {
        true
    }

    #[cfg(not(test))]
    fn prepare_application() -> Result<(), Box<dyn std::error::Error>> {
        use objc2::MainThreadMarker;
        use objc2_app_kit::{NSApp, NSApplicationActivationPolicy};

        let mtm = MainThreadMarker::new().ok_or("tray must be created on the main thread")?;
        let _ = NSApp(mtm).setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        Ok(())
    }

    #[cfg(not(test))]
    fn double_click_interval() -> Option<Duration> {
        let seconds = objc2_app_kit::NSEvent::doubleClickInterval();
        seconds
            .is_finite()
            .then(|| Duration::from_secs_f64(seconds))
    }

    #[cfg(not(test))]
    fn set_icon(
        tray_icon: &tray_icon::TrayIcon,
        icon: tray_icon::Icon,
        is_template: bool,
    ) -> tray_icon::Result<()> {
        tray_icon.set_icon_with_as_template(Some(icon), is_template)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessibilityInsert {
    Inserted,
    Unsupported(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AccessibilityError {
    PermissionDenied,
    NoFocusedElement,
    SecureControl,
    Native { operation: &'static str, code: i32 },
    UnexpectedType { operation: &'static str },
}

impl std::fmt::Display for AccessibilityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => formatter.write_str("Accessibility permission is required"),
            Self::NoFocusedElement => formatter.write_str("no focused Accessibility element"),
            Self::SecureControl => {
                formatter.write_str("refusing to inject text into a secure control")
            }
            Self::Native { operation, code } => {
                write!(
                    formatter,
                    "Accessibility {operation} failed with error {code}"
                )
            }
            Self::UnexpectedType { operation } => {
                write!(
                    formatter,
                    "Accessibility {operation} returned an unexpected value type"
                )
            }
        }
    }
}

impl std::error::Error for AccessibilityError {}

trait AccessibilityWriter {
    fn insert_selected_text(&self, text: &str) -> Result<AccessibilityInsert, AccessibilityError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PasteError {
    Write(String),
    Delivery(String),
}

impl std::fmt::Display for PasteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Write(message) => write!(formatter, "pasteboard write failed: {message}"),
            Self::Delivery(message) => write!(formatter, "paste delivery failed: {message}"),
        }
    }
}

impl std::error::Error for PasteError {}

trait PasteWriter {
    fn paste(&self, text: &str) -> Result<(), PasteError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InjectionOutcome {
    Noop,
    Accessibility,
    Paste,
}

#[derive(Debug)]
enum MacInjectionError {
    Accessibility(AccessibilityError),
    Paste(PasteError),
}

impl std::fmt::Display for MacInjectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Accessibility(error) => error.fmt(formatter),
            Self::Paste(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for MacInjectionError {}

fn route_injection(
    text: &str,
    accessibility: &impl AccessibilityWriter,
    paste: &impl PasteWriter,
) -> Result<InjectionOutcome, MacInjectionError> {
    if text.is_empty() {
        return Ok(InjectionOutcome::Noop);
    }

    match accessibility.insert_selected_text(text) {
        Ok(AccessibilityInsert::Inserted) => Ok(InjectionOutcome::Accessibility),
        Ok(AccessibilityInsert::Unsupported(reason)) => {
            debug!(
                reason,
                "Accessibility selected-text insertion is unsupported; using native paste fallback"
            );
            paste
                .paste(text)
                .map(|()| InjectionOutcome::Paste)
                .map_err(MacInjectionError::Paste)
        }
        Err(error) => Err(MacInjectionError::Accessibility(error)),
    }
}

/// Native macOS text delivery using focused AX selection first and a clipboard-replacing paste
/// fallback only when the focused control does not support selected-text assignment.
struct MacTyper {
    paste: pasteboard::NativePasteWriter,
    delivery: Mutex<()>,
}

impl MacTyper {
    pub(crate) fn new() -> (
        Self,
        impl Fn(rdev::EventType) -> Option<rdev::EventType> + Send + 'static,
    ) {
        let (paste, hotkey_filter) = pasteboard::NativePasteWriter::new();
        (
            Self {
                paste,
                delivery: Mutex::new(()),
            },
            hotkey_filter,
        )
    }
}

impl TextTyper for MacTyper {
    fn type_text(&self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        if text.is_empty() {
            return Ok(());
        }

        // Serializing the focus delay with delivery prevents a concurrent finalizer from shifting
        // the focus-settling window immediately before another transaction.
        let _delivery = self
            .delivery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::thread::sleep(Duration::from_millis(100));

        let outcome = objc2::rc::autoreleasepool(|_| {
            route_injection(text, &accessibility::NativeAccessibility, &self.paste)
        })?;
        match outcome {
            InjectionOutcome::Noop => {}
            InjectionOutcome::Accessibility => {
                info!(
                    text_bytes = text.len(),
                    "text inserted through Accessibility"
                );
            }
            InjectionOutcome::Paste => {
                info!(
                    text_bytes = text.len(),
                    "text delivered through native paste fallback; clipboard contains transcription"
                );
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    #[test]
    fn macos_policy_owns_native_hotkey_and_icon_rules() {
        assert!(MacTray::idle_icon_is_template());
        for key in [
            Key::CapsLock,
            Key::ControlRight,
            Key::Delete,
            Key::Insert,
            Key::Home,
            Key::End,
            Key::PageUp,
            Key::PageDown,
            Key::NumLock,
            Key::ScrollLock,
            Key::PrintScreen,
            Key::Pause,
            Key::IntlBackslash,
            Key::KpReturn,
            Key::KpMinus,
            Key::KpPlus,
            Key::KpMultiply,
            Key::KpDivide,
            Key::Kp0,
            Key::Kp1,
            Key::Kp2,
            Key::Kp3,
            Key::Kp4,
            Key::Kp5,
            Key::Kp6,
            Key::Kp7,
            Key::Kp8,
            Key::Kp9,
            Key::KpDelete,
        ] {
            assert!(MacHotkeys::unsupported_reason(key).is_some(), "{key:?}");
        }
        assert_eq!(MacHotkeys::unsupported_reason(Key::F8), None);
    }

    struct FakeAccessibility {
        result: RefCell<Option<Result<AccessibilityInsert, AccessibilityError>>>,
        texts: RefCell<Vec<String>>,
    }

    impl FakeAccessibility {
        fn returning(result: Result<AccessibilityInsert, AccessibilityError>) -> Self {
            Self {
                result: RefCell::new(Some(result)),
                texts: RefCell::new(Vec::new()),
            }
        }
    }

    impl AccessibilityWriter for FakeAccessibility {
        fn insert_selected_text(
            &self,
            text: &str,
        ) -> Result<AccessibilityInsert, AccessibilityError> {
            self.texts.borrow_mut().push(text.to_string());
            self.result
                .borrow_mut()
                .take()
                .expect("fake Accessibility result used once")
        }
    }

    #[derive(Default)]
    struct FakePaste {
        calls: Cell<usize>,
        texts: RefCell<Vec<String>>,
    }

    impl PasteWriter for FakePaste {
        fn paste(&self, text: &str) -> Result<(), PasteError> {
            self.calls.set(self.calls.get() + 1);
            self.texts.borrow_mut().push(text.to_string());
            Ok(())
        }
    }

    #[test]
    fn direct_accessibility_insert_never_uses_paste_fallback() {
        let accessibility = FakeAccessibility::returning(Ok(AccessibilityInsert::Inserted));
        let paste = FakePaste::default();

        let outcome = route_injection("hello", &accessibility, &paste).unwrap();

        assert_eq!(outcome, InjectionOutcome::Accessibility);
        assert_eq!(paste.calls.get(), 0);
    }

    #[test]
    fn unsupported_selected_text_uses_paste_once_with_exact_text() {
        let accessibility = FakeAccessibility::returning(Ok(AccessibilityInsert::Unsupported(
            "selected text is not settable",
        )));
        let paste = FakePaste::default();
        let text = "line 1\n\"quoted\" \\ slash 中文 😀";

        let outcome = route_injection(text, &accessibility, &paste).unwrap();

        assert_eq!(outcome, InjectionOutcome::Paste);
        assert_eq!(paste.calls.get(), 1);
        assert_eq!(paste.texts.borrow().as_slice(), [text]);
    }

    #[test]
    fn secure_and_hard_accessibility_errors_never_fall_back() {
        let cases = [
            AccessibilityError::SecureControl,
            AccessibilityError::PermissionDenied,
            AccessibilityError::NoFocusedElement,
            AccessibilityError::Native {
                operation: "set selected text",
                code: -25204,
            },
        ];

        for error in cases {
            let accessibility = FakeAccessibility::returning(Err(error));
            let paste = FakePaste::default();

            assert!(matches!(
                route_injection("secret", &accessibility, &paste),
                Err(MacInjectionError::Accessibility(_))
            ));
            assert_eq!(paste.calls.get(), 0);
        }
    }

    #[test]
    fn empty_text_is_a_complete_no_op() {
        let accessibility = FakeAccessibility::returning(Ok(AccessibilityInsert::Inserted));
        let paste = FakePaste::default();

        assert_eq!(
            route_injection("", &accessibility, &paste).unwrap(),
            InjectionOutcome::Noop
        );
        assert!(accessibility.texts.borrow().is_empty());
        assert_eq!(paste.calls.get(), 0);
    }
}
