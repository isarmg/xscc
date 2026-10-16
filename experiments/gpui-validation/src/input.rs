use base::{
    message_proto::{ControlKey, KeyEvent, KeyboardMode, Message, MouseEvent},
    session_protocol::input,
};
use gpui::Keystroke;

pub fn key(key: &Keystroke, down: bool) -> Option<Message> {
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
        .zip([
            key.modifiers.control,
            key.modifiers.alt,
            key.modifiers.shift,
            key.modifiers.platform,
        ])
        .filter_map(|(key, enabled)| enabled.then_some(key.into()))
        .collect(),
        ..Default::default()
    };
    let control = match key.key.as_str() {
        "enter" => Some(ControlKey::Return),
        "escape" => Some(ControlKey::Escape),
        "backspace" => Some(ControlKey::Backspace),
        "delete" => Some(ControlKey::Delete),
        "tab" => Some(ControlKey::Tab),
        "left" => Some(ControlKey::LeftArrow),
        "right" => Some(ControlKey::RightArrow),
        "up" => Some(ControlKey::UpArrow),
        "down" => Some(ControlKey::DownArrow),
        "space" => Some(ControlKey::Space),
        _ => None,
    };
    if let Some(key) = control {
        event.set_control_key(key);
    } else {
        let text = key.key_char.as_deref().unwrap_or(&key.key);
        let mut chars = text.chars();
        let value = chars.next()?;
        if chars.next().is_some() || value.is_control() {
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
    let remote_x = (x - (viewport.0 - frame.0 as f32 * scale) / 2.0) / scale;
    let remote_y = (y - (viewport.1 - frame.1 as f32 * scale) / 2.0) / scale;
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
    fn same_contained_coordinates_and_wire_modifiers_as_slint() {
        let message = pointer(500.0, 400.0, (1000.0, 800.0), (1920, 1080), 1, 1).unwrap();
        let decoded = Message::parse_from_bytes(&message.write_to_bytes().unwrap()).unwrap();
        assert_eq!(
            (decoded.mouse_event().x, decoded.mouse_event().y),
            (960, 540)
        );
        assert!(pointer(500.0, 1.0, (1000.0, 800.0), (1920, 1080), 1, 1).is_none());
        let event = key(
            &Keystroke {
                key: "s".into(),
                modifiers: gpui::Modifiers {
                    control: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            true,
        )
        .unwrap();
        assert_eq!(event.key_event().unicode(), u32::from('s'));
        assert_eq!(
            event.key_event().modifiers,
            vec![ControlKey::Control.into()]
        );
    }
}
