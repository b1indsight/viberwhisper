use objc2_core_graphics::{CGEventSource, CGEventSourceStateID};
use rdev::{EventType, Key};

fn modifier_keycode(key: Key) -> Option<u16> {
    match key {
        Key::MetaLeft => Some(55),
        Key::ShiftLeft => Some(56),
        Key::Alt => Some(58),
        Key::ControlLeft => Some(59),
        Key::ShiftRight => Some(60),
        Key::AltGr => Some(61),
        Key::ControlRight => Some(62),
        Key::Function => Some(63),
        Key::MetaRight => Some(54),
        _ => None,
    }
}

fn normalize_modifier_event(
    event_type: EventType,
    key_is_pressed: impl FnOnce(u16) -> bool,
) -> EventType {
    let key = match &event_type {
        EventType::KeyPress(key) | EventType::KeyRelease(key) => *key,
        _ => return event_type,
    };
    let Some(keycode) = modifier_keycode(key) else {
        return event_type;
    };

    if key_is_pressed(keycode) {
        EventType::KeyPress(key)
    } else {
        EventType::KeyRelease(key)
    }
}

pub(crate) fn normalize_event(event_type: EventType) -> EventType {
    normalize_modifier_event(event_type, |keycode| {
        CGEventSource::key_state(CGEventSourceStateID::HIDSystemState, keycode)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_modifier_direction_from_physical_key_state() {
        let keycodes = [
            (Key::MetaRight, 54),
            (Key::MetaLeft, 55),
            (Key::ShiftLeft, 56),
            (Key::Alt, 58),
            (Key::ControlLeft, 59),
            (Key::ShiftRight, 60),
            (Key::AltGr, 61),
            (Key::ControlRight, 62),
            (Key::Function, 63),
        ];
        for (key, expected) in keycodes {
            assert_eq!(modifier_keycode(key), Some(expected));
        }
        assert_eq!(modifier_keycode(Key::F8), None);

        assert_eq!(
            normalize_modifier_event(EventType::KeyPress(Key::AltGr), |keycode| {
                assert_eq!(keycode, 61);
                false
            }),
            EventType::KeyRelease(Key::AltGr)
        );
        assert_eq!(
            normalize_modifier_event(EventType::KeyRelease(Key::AltGr), |_| true),
            EventType::KeyPress(Key::AltGr)
        );
        assert_eq!(
            normalize_modifier_event(EventType::KeyPress(Key::F8), |_| {
                panic!("ordinary keys do not require a physical-state query")
            }),
            EventType::KeyPress(Key::F8)
        );
    }
}
