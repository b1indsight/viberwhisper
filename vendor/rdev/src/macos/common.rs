#![allow(clippy::upper_case_acronyms)]
use crate::macos::keyboard::Keyboard;
use crate::rdev::{Button, Event, EventType};
use cocoa::base::id;
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, EventField};
use lazy_static::lazy_static;
use std::convert::TryInto;
use std::os::raw::c_void;
use std::sync::Mutex;
use std::time::SystemTime;

use crate::macos::keycodes::key_from_code;

pub type CFMachPortRef = *const c_void;
pub type CFIndex = u64;
pub type CFAllocatorRef = id;
pub type CFRunLoopSourceRef = id;
pub type CFRunLoopRef = id;
pub type CFRunLoopMode = id;
pub type CGEventTapProxy = id;
pub type CGEventRef = CGEvent;

// https://developer.apple.com/documentation/coregraphics/cgeventtapplacement?language=objc
pub type CGEventTapPlacement = u32;
#[allow(non_upper_case_globals)]
pub const kCGHeadInsertEventTap: u32 = 0;

// https://developer.apple.com/documentation/coregraphics/cgeventtapoptions?language=objc
#[allow(non_upper_case_globals)]
#[repr(u32)]
pub enum CGEventTapOption {
    #[cfg(feature = "unstable_grab")]
    Default = 0,
    ListenOnly = 1,
}

#[cfg(test)]
mod viberwhisper_tests {
    use super::*;
    use crate::rdev::Key;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    #[test]
    fn background_key_conversion_preserves_physical_events_without_character_names() {
        // The global hook runs off the main queue. Deriving a character here used to
        // enter TIS/TSM and trap after input-source changes on macOS 26.
        std::thread::spawn(|| unsafe {
            let mut keyboard = Keyboard::new().unwrap();
            for (code, key) in [(0, Key::KeyA), (100, Key::F8), (101, Key::F9)] {
                for pressed in [true, false] {
                    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).unwrap();
                    // Build a raw event: CGEventCreateKeyboardEvent itself may query TIS.
                    let native = CGEvent::new(source).unwrap();
                    let kind = if pressed {
                        CGEventType::KeyDown
                    } else {
                        CGEventType::KeyUp
                    };
                    native.set_type(kind);
                    native.set_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE, code);
                    let event = convert(kind, &native, &mut keyboard).unwrap();
                    let expected = if pressed {
                        EventType::KeyPress(key)
                    } else {
                        EventType::KeyRelease(key)
                    };
                    assert_eq!(event.event_type, expected);
                    assert_eq!(event.name, None);
                }
            }
            LAST_FLAGS = CGEventFlags::CGEventFlagNull;
            let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState).unwrap();
            let native = CGEvent::new(source).unwrap();
            native.set_type(CGEventType::FlagsChanged);
            native.set_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE, 56);
            for (flags, expected) in [
                (CGEventFlags::CGEventFlagShift, EventType::KeyPress(Key::ShiftLeft)),
                (CGEventFlags::CGEventFlagNull, EventType::KeyRelease(Key::ShiftLeft)),
            ] {
                native.set_flags(flags);
                let event = convert(CGEventType::FlagsChanged, &native, &mut keyboard).unwrap();
                assert_eq!(event.event_type, expected);
                assert_eq!(event.name, None);
            }
        })
        .join()
        .unwrap();
    }
}

pub static mut LAST_FLAGS: CGEventFlags = CGEventFlags::CGEventFlagNull;
lazy_static! {
    pub static ref KEYBOARD_STATE: Mutex<Keyboard> = Mutex::new(Keyboard::new().unwrap());
}

// https://developer.apple.com/documentation/coregraphics/cgeventmask?language=objc
pub type CGEventMask = u64;
#[allow(non_upper_case_globals)]
pub const kCGEventMaskForAllEvents: u64 = (1 << CGEventType::LeftMouseDown as u64)
    + (1 << CGEventType::LeftMouseUp as u64)
    + (1 << CGEventType::RightMouseDown as u64)
    + (1 << CGEventType::RightMouseUp as u64)
    + (1 << CGEventType::MouseMoved as u64)
    + (1 << CGEventType::LeftMouseDragged as u64)
    + (1 << CGEventType::RightMouseDragged as u64)
    + (1 << CGEventType::KeyDown as u64)
    + (1 << CGEventType::KeyUp as u64)
    + (1 << CGEventType::FlagsChanged as u64)
    + (1 << CGEventType::ScrollWheel as u64);

#[cfg(target_os = "macos")]
#[link(name = "Cocoa", kind = "framework")]
extern "C" {
    #[allow(improper_ctypes)]
    pub fn CGEventTapCreate(
        tap: CGEventTapLocation,
        place: CGEventTapPlacement,
        options: CGEventTapOption,
        eventsOfInterest: CGEventMask,
        callback: QCallback,
        user_info: id,
    ) -> CFMachPortRef;
    pub fn CFMachPortCreateRunLoopSource(
        allocator: CFAllocatorRef,
        tap: CFMachPortRef,
        order: CFIndex,
    ) -> CFRunLoopSourceRef;
    pub fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFRunLoopMode);
    pub fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    pub fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    pub fn CFRunLoopRun();

    pub static kCFRunLoopCommonModes: CFRunLoopMode;

}
pub type QCallback = unsafe extern "C" fn(
    proxy: CGEventTapProxy,
    _type: CGEventType,
    cg_event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef;

pub unsafe fn convert(
    _type: CGEventType,
    cg_event: &CGEvent,
    _keyboard_state: &mut Keyboard,
) -> Option<Event> {
    let option_type = match _type {
        CGEventType::LeftMouseDown => Some(EventType::ButtonPress(Button::Left)),
        CGEventType::LeftMouseUp => Some(EventType::ButtonRelease(Button::Left)),
        CGEventType::RightMouseDown => Some(EventType::ButtonPress(Button::Right)),
        CGEventType::RightMouseUp => Some(EventType::ButtonRelease(Button::Right)),
        CGEventType::MouseMoved => {
            let point = cg_event.location();
            Some(EventType::MouseMove {
                x: point.x,
                y: point.y,
            })
        }
        CGEventType::KeyDown => {
            let code = cg_event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            Some(EventType::KeyPress(key_from_code(code.try_into().ok()?)))
        }
        CGEventType::KeyUp => {
            let code = cg_event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            Some(EventType::KeyRelease(key_from_code(code.try_into().ok()?)))
        }
        CGEventType::FlagsChanged => {
            let code = cg_event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            let code = code.try_into().ok()?;
            let flags = cg_event.get_flags();
            if flags < LAST_FLAGS {
                LAST_FLAGS = flags;
                Some(EventType::KeyRelease(key_from_code(code)))
            } else {
                LAST_FLAGS = flags;
                Some(EventType::KeyPress(key_from_code(code)))
            }
        }
        CGEventType::ScrollWheel => {
            let delta_y =
                cg_event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_1);
            let delta_x =
                cg_event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_POINT_DELTA_AXIS_2);
            Some(EventType::Wheel { delta_x, delta_y })
        }
        _ => None,
    };
    if let Some(event_type) = option_type {
        // ViberWhisper only consumes physical keys. Character decoding queries TIS/TSM
        // from this background callback and can trap on macOS 26 after input-source changes.
        let name = None;
        return Some(Event {
            event_type,
            time: SystemTime::now(),
            name,
        });
    }
    None
}
