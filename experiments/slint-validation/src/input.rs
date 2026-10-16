use base::{
    message_proto::{ControlKey, KeyEvent, KeyboardMode, Message, MouseEvent},
    session_protocol::input,
};
use slint::platform::Key;

pub fn key(text: &str, down: bool, flags: [bool; 4]) -> Option<Message> {
    let mut event = KeyEvent {
        down,
        mode: KeyboardMode::Legacy.into(),
        modifiers: [
            ControlKey::Control,
            ControlKey::Alt,
            ControlKey::Shift,
            ControlKey::Meta,
        ]
        .into_iter()
        .zip(flags)
        .filter_map(|(key, enabled)| enabled.then_some(key.into()))
        .collect(),
        ..Default::default()
    };
    let special = [
        (Key::Return, ControlKey::Return),
        (Key::Escape, ControlKey::Escape),
        (Key::Backspace, ControlKey::Backspace),
        (Key::Delete, ControlKey::Delete),
        (Key::Tab, ControlKey::Tab),
        (Key::LeftArrow, ControlKey::LeftArrow),
        (Key::RightArrow, ControlKey::RightArrow),
        (Key::UpArrow, ControlKey::UpArrow),
        (Key::DownArrow, ControlKey::DownArrow),
    ];
    if let Some((_, control)) = special
        .into_iter()
        .find(|(key, _)| slint::SharedString::from(*key).as_str() == text)
    {
        event.set_control_key(control);
    } else {
        let mut chars = text.chars();
        let value = chars.next()?;
        if chars.next().is_some()
            || value.is_control()
            || ('\u{e000}'..='\u{f8ff}').contains(&value)
        {
            return None;
        }
        event.set_unicode(u32::from(value));
    }
    let mut message = Message::new();
    message.set_key_event(event);
    Some(message)
}

pub fn pointer(
    x: f32,
    y: f32,
    viewport: (f32, f32),
    frame: (u32, u32),
    kind: i32,
    button: i32,
) -> Option<Message> {
    if ![x, y, viewport.0, viewport.1]
        .into_iter()
        .all(f32::is_finite)
        || viewport.0 <= 0.0
        || viewport.1 <= 0.0
        || frame.0 == 0
        || frame.1 == 0
    {
        return None;
    }
    let scale = (viewport.0 / frame.0 as f32).min(viewport.1 / frame.1 as f32);
    let left = (viewport.0 - frame.0 as f32 * scale) / 2.0;
    let top = (viewport.1 - frame.1 as f32 * scale) / 2.0;
    let remote_x = (x - left) / scale;
    let remote_y = (y - top) / scale;
    if remote_x < 0.0 || remote_y < 0.0 || remote_x >= frame.0 as f32 || remote_y >= frame.1 as f32
    {
        return None;
    }
    let event_type = match kind {
        0 => input::MOUSE_TYPE_MOVE,
        1 => input::MOUSE_TYPE_DOWN,
        2 => input::MOUSE_TYPE_UP,
        _ => return None,
    };
    let button = match button {
        0 => 0,
        1 => input::MOUSE_BUTTON_LEFT,
        2 => input::MOUSE_BUTTON_RIGHT,
        3 => input::MOUSE_BUTTON_WHEEL,
        _ => return None,
    };
    let mut message = Message::new();
    message.set_mouse_event(MouseEvent {
        mask: event_type | (button << 3),
        x: remote_x as i32,
        y: remote_y as i32,
        ..Default::default()
    });
    Some(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protobuf::Message as _;

    #[test]
    fn maps_contained_video_coordinates_and_rejects_borders() {
        let event = pointer(500.0, 400.0, (1000.0, 800.0), (1920, 1080), 1, 1).unwrap();
        let decoded = Message::parse_from_bytes(&event.write_to_bytes().unwrap()).unwrap();
        assert_eq!(
            (decoded.mouse_event().x, decoded.mouse_event().y),
            (960, 540)
        );
        assert_eq!(
            decoded.mouse_event().mask,
            input::MOUSE_TYPE_DOWN | (input::MOUSE_BUTTON_LEFT << 3)
        );
        assert!(pointer(500.0, 1.0, (1000.0, 800.0), (1920, 1080), 1, 1).is_none());
        assert!(pointer(f32::NAN, 1.0, (1000.0, 800.0), (1920, 1080), 1, 1).is_none());
    }

    #[test]
    fn preserves_unicode_and_shortcut_modifiers_on_the_real_wire_format() {
        let event = key("中", true, [true, false, true, false]).unwrap();
        let decoded = Message::parse_from_bytes(&event.write_to_bytes().unwrap()).unwrap();
        assert_eq!(decoded.key_event().unicode(), u32::from('中'));
        assert!(decoded.key_event().down);
        assert_eq!(
            decoded.key_event().modifiers,
            vec![ControlKey::Control.into(), ControlKey::Shift.into()]
        );
        assert!(key("\u{e123}", true, [false; 4]).is_none());
        assert!(key("中文", true, [false; 4]).is_none());
    }
}
