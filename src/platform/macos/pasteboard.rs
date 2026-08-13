use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_core_graphics::{
    CGEvent, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
};
use objc2_foundation::NSString;
use rdev::EventType;

use super::{PasteError, PasteWriter};

const SYNTHETIC_EVENT_FILTER_GRACE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativeEventSpec {
    virtual_key: u16,
    pressed: bool,
    command_flag: bool,
}

const PASTE_EVENT_SPECS: [NativeEventSpec; 4] = [
    NativeEventSpec {
        virtual_key: 55,
        pressed: true,
        command_flag: true,
    },
    NativeEventSpec {
        virtual_key: 9,
        pressed: true,
        command_flag: true,
    },
    NativeEventSpec {
        virtual_key: 9,
        pressed: false,
        command_flag: true,
    },
    NativeEventSpec {
        virtual_key: 55,
        pressed: false,
        command_flag: false,
    },
];

pub(super) fn replace_with_text(pasteboard: &NSPasteboard, text: &str) -> Result<(), String> {
    pasteboard.clearContents();
    let text = NSString::from_str(text);
    // SAFETY: AppKit exports this process-lifetime immutable type identifier.
    let string_type = unsafe { NSPasteboardTypeString };
    if pasteboard.setString_forType(&text, string_type) {
        Ok(())
    } else {
        Err("AppKit rejected the transcription text".to_string())
    }
}

fn post_paste() -> Result<(), String> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .ok_or_else(|| "could not create CoreGraphics event source".to_string())?;

    let mut events = Vec::with_capacity(PASTE_EVENT_SPECS.len());
    for spec in PASTE_EVENT_SPECS {
        let event = CGEvent::new_keyboard_event(Some(&source), spec.virtual_key, spec.pressed)
            .ok_or_else(|| {
                format!(
                    "could not create CoreGraphics keyboard event for key {}",
                    spec.virtual_key
                )
            })?;
        let flags = if spec.command_flag {
            CGEventFlags::MaskCommand
        } else {
            CGEventFlags::empty()
        };
        CGEvent::set_flags(Some(&event), flags);
        events.push(event);
    }

    for event in &events {
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
    }
    Ok(())
}

/// Keeps ViberWhisper's listener filter closed while synthetic Cmd+V events can arrive.
/// The operating system and focused application still receive every event.
struct HotkeySuppression<'a> {
    active: &'a AtomicBool,
}

impl HotkeySuppression<'_> {
    fn begin(active: &AtomicBool) -> HotkeySuppression<'_> {
        assert!(
            active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            "paste hotkey suppression must not overlap"
        );
        HotkeySuppression { active }
    }
}

impl Drop for HotkeySuppression<'_> {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

fn paste_transaction(
    pasteboard: &NSPasteboard,
    writer: &NativePasteWriter,
    text: &str,
) -> Result<(), PasteError> {
    replace_with_text(pasteboard, text).map_err(PasteError::Write)?;

    let _suppression = writer.suppress_hotkeys();
    post_paste().map_err(PasteError::Delivery)?;
    // CoreGraphics posting is asynchronous. Keep the listener callback filtered briefly so it
    // can observe the generated sequence before normal hotkey handling resumes.
    std::thread::sleep(SYNTHETIC_EVENT_FILTER_GRACE);
    Ok(())
}

pub(super) struct NativePasteWriter {
    suppress_hotkeys: Arc<AtomicBool>,
}

impl NativePasteWriter {
    /// Creates the writer and the one callback that shares its private suppression flag.
    /// The callback must be installed in the process-lifetime hotkey listener before `paste` runs.
    pub(super) fn new() -> (
        Self,
        impl Fn(EventType) -> Option<EventType> + Send + 'static,
    ) {
        let suppress_hotkeys = Arc::new(AtomicBool::new(false));
        let listener_suppression = Arc::clone(&suppress_hotkeys);
        let filter = move |event_type| {
            if listener_suppression.load(Ordering::Acquire) {
                None
            } else {
                Some(super::hotkey::normalize_event(event_type))
            }
        };
        (Self { suppress_hotkeys }, filter)
    }

    fn suppress_hotkeys(&self) -> HotkeySuppression<'_> {
        HotkeySuppression::begin(&self.suppress_hotkeys)
    }
}

impl PasteWriter for NativePasteWriter {
    fn paste(&self, text: &str) -> Result<(), PasteError> {
        let pasteboard = NSPasteboard::generalPasteboard();
        paste_transaction(&pasteboard, self, text)
    }
}

#[cfg(test)]
mod tests {
    use rdev::Key;

    use super::*;

    #[test]
    fn native_paste_sequence_is_command_v() {
        assert_eq!(
            PASTE_EVENT_SPECS,
            [
                NativeEventSpec {
                    virtual_key: 55,
                    pressed: true,
                    command_flag: true,
                },
                NativeEventSpec {
                    virtual_key: 9,
                    pressed: true,
                    command_flag: true,
                },
                NativeEventSpec {
                    virtual_key: 9,
                    pressed: false,
                    command_flag: true,
                },
                NativeEventSpec {
                    virtual_key: 55,
                    pressed: false,
                    command_flag: false,
                },
            ]
        );
    }

    #[test]
    fn writer_filter_only_drops_events_during_a_paste_scope() {
        let (writer, filter) = NativePasteWriter::new();
        let event = EventType::KeyPress(Key::KeyV);

        assert_eq!(filter(event), Some(event));
        {
            let _suppression = writer.suppress_hotkeys();
            assert_eq!(filter(event), None);
        }
        assert_eq!(filter(event), Some(event));
    }

    #[test]
    fn native_named_pasteboard_keeps_replacement_text() {
        objc2::rc::autoreleasepool(|_| {
            let pasteboard = NSPasteboard::pasteboardWithUniqueName();
            // SAFETY: AppKit exports this process-lifetime immutable type identifier.
            let string_type = unsafe { NSPasteboardTypeString };
            let original = NSString::from_str("original clipboard value");
            pasteboard.clearContents();
            assert!(pasteboard.setString_forType(&original, string_type));

            replace_with_text(&pasteboard, "native 中文 😀").unwrap();

            assert_eq!(
                pasteboard.stringForType(string_type).unwrap().to_string(),
                "native 中文 😀"
            );
            pasteboard.clearContents();
        });
    }
}
