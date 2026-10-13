//! Native confirmation windows live on the UI thread. Destinations are captured and
//! retained by the finalization worker; only commands and display data cross threads.

use anyhow::Result;

use crate::session::SessionId;

#[cfg(target_os = "macos")]
pub(crate) use super::macos::preview::{NativePreview, NativeTarget};
#[cfg(target_os = "windows")]
pub(crate) use super::windows::preview::{NativePreview, NativeTarget};

#[derive(Debug, Clone)]
pub(crate) enum PreviewAction {
    Confirm(String),
    Cancel,
}

#[derive(Debug, Clone)]
pub(crate) struct PreviewEvent {
    pub session_id: SessionId,
    pub action: PreviewAction,
}

pub(crate) type PreviewNotify = Box<dyn Fn(PreviewEvent)>;

/// A native destination stays on the worker that captured it. Validation must also
/// run immediately before injection, after any platform focus-settling delay.
pub(crate) trait DeliveryTarget {
    fn name(&self) -> String;
    fn restore(&self) -> Result<()>;
    fn validate(&self) -> Result<()>;
}

/// Both native windows consume confirmation key-down and repeat events, but only
/// emit an action after its matching release. Losing focus disarms the key.
#[derive(Default)]
pub(super) struct ConfirmationKey(Option<PreviewKey>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreviewKey {
    Confirm,
    Cancel,
}

impl ConfirmationKey {
    pub(super) fn press(&mut self, action: PreviewKey) {
        self.0 = Some(action);
    }
    pub(super) fn release(&mut self, action: PreviewKey) -> Option<PreviewKey> {
        if self.0 == Some(action) {
            self.0.take()
        } else {
            None
        }
    }
    pub(super) fn reset(&mut self) {
        self.0 = None;
    }
    #[cfg(target_os = "windows")]
    pub(super) fn is_pressed(&self, key: PreviewKey) -> bool {
        self.0 == Some(key)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod fallback {
    use super::*;
    pub(crate) struct NativeTarget;
    impl NativeTarget {
        pub(crate) fn capture() -> Result<Self> {
            anyhow::bail!("Desktop preview is unsupported on this platform")
        }
    }
    impl DeliveryTarget for NativeTarget {
        fn name(&self) -> String {
            "Unsupported platform".into()
        }
        fn restore(&self) -> Result<()> {
            anyhow::bail!("Desktop preview is unsupported on this platform")
        }
        fn validate(&self) -> Result<()> {
            self.restore()
        }
    }
    pub(crate) struct NativePreview;
    impl NativePreview {
        pub(crate) fn new(_: PreviewNotify) -> Self {
            Self
        }
        pub(crate) fn show(
            &mut self,
            _: SessionId,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> Result<()> {
            anyhow::bail!("Desktop preview is unsupported on this platform")
        }
        pub(crate) fn show_error(&mut self, _: &str, _: bool) {}
        pub(crate) fn focus(&self) {}
        pub(crate) fn hide(&self) {}
        pub(crate) fn close(&mut self) {}
        #[cfg(feature = "integration")]
        pub(crate) fn smoke_confirmation(
            &self,
            _: &str,
            _: &dyn Fn() -> Result<()>,
        ) -> Result<bool> {
            anyhow::bail!("Unsupported platform")
        }
    }
}
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) use fallback::{NativePreview, NativeTarget};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmation_waits_for_release_and_drops_focus_lost_keys() {
        // Held Enter must never hide the preview while its repeats can still reach
        // a chat application; an Enter pressed in another window is not confirmation.
        let mut key = ConfirmationKey::default();
        assert_eq!(key.release(PreviewKey::Confirm), None);
        for _ in 0..5 {
            key.press(PreviewKey::Confirm);
        }
        assert_eq!(key.release(PreviewKey::Confirm), Some(PreviewKey::Confirm));
        assert_eq!(key.release(PreviewKey::Confirm), None);
        key.press(PreviewKey::Confirm);
        key.reset();
        assert_eq!(key.release(PreviewKey::Confirm), None);
    }
}
