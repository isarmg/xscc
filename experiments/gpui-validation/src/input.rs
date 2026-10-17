use base::{
    message_proto::{ControlKey, KeyEvent, KeyboardMode, Message, MouseEvent},
    session_protocol::input,
};
use gpui::Keystroke;

#[cfg(any(all(target_os = "windows", test), feature = "live-session"))]
pub fn key_identity(key: &Keystroke, mapper: &dyn gpui::PlatformKeyboardMapper) -> String {
    mapper
        .map_key_equivalent(key.clone(), false)
        .key()
        .to_owned()
}

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
        "home" => Some(ControlKey::Home),
        "end" => Some(ControlKey::End),
        "pageup" => Some(ControlKey::PageUp),
        "pagedown" => Some(ControlKey::PageDown),
        "insert" => Some(ControlKey::Insert),
        "control" | "ctrl" => Some(ControlKey::Control),
        "alt" => Some(ControlKey::Alt),
        "shift" => Some(ControlKey::Shift),
        "super" | "cmd" | "meta" => Some(ControlKey::Meta),
        "f1" => Some(ControlKey::F1),
        "f2" => Some(ControlKey::F2),
        "f3" => Some(ControlKey::F3),
        "f4" => Some(ControlKey::F4),
        "f5" => Some(ControlKey::F5),
        "f6" => Some(ControlKey::F6),
        "f7" => Some(ControlKey::F7),
        "f8" => Some(ControlKey::F8),
        "f9" => Some(ControlKey::F9),
        "f10" => Some(ControlKey::F10),
        "f11" => Some(ControlKey::F11),
        "f12" => Some(ControlKey::F12),
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
        // The host must hold this explicit modifier, not inject and release it as chord metadata.
        event.modifiers.retain(|modifier| *modifier != key.into());
        event.set_control_key(key);
    } else {
        let text = key.key_char.as_deref().unwrap_or(&key.key);
        let mut chars = text.chars();
        let value = chars.next()?;
        if chars.next().is_some() || value.is_control() {
            return None;
        }
        // Legacy Unicode injects text on both down and up; Chr preserves the edge.
        event.set_chr(u32::from(value));
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
    pointer_with_capture(x, y, viewport, frame, kind, button, false)
}

pub fn pointer_with_capture(
    x: f32,
    y: f32,
    viewport: (f32, f32),
    frame: (u32, u32),
    kind: i32,
    button: i32,
    captured: bool,
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
    if !captured
        && (remote_x < 0.0
            || remote_y < 0.0
            || remote_x >= frame.0 as f32
            || remote_y >= frame.1 as f32)
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
        x: remote_x.clamp(0.0, (frame.0 - 1) as f32) as i32,
        y: remote_y.clamp(0.0, (frame.1 - 1) as f32) as i32,
        ..Default::default()
    });
    Some(message)
}

#[cfg(any(feature = "live-session", test))]
pub fn release_key(pressed: &Message) -> Message {
    let mut message = pressed.clone();
    let event = message.mut_key_event();
    event.down = false;
    event.press = false;
    // Legacy Chr uses these flags to decide whether a physical key needs releasing.
    if !event.has_chr() {
        event.modifiers.clear();
    }
    message
}

#[cfg(feature = "live-session")]
pub fn modifier_keys(modifiers: gpui::Modifiers) -> Vec<protobuf::EnumOrUnknown<ControlKey>> {
    [
        ControlKey::Control,
        ControlKey::Alt,
        ControlKey::Shift,
        ControlKey::Meta,
    ]
    .into_iter()
    .zip([
        modifiers.control,
        modifiers.alt,
        modifiers.shift,
        modifiers.platform,
    ])
    .filter_map(|(key, enabled)| enabled.then_some(key.into()))
    .collect()
}

#[cfg(any(feature = "live-session", test))]
pub fn wheel(x: i32, y: i32, trackpad: bool, modifiers: gpui::Modifiers) -> Option<Message> {
    if x == 0 && y == 0 {
        return None;
    }
    let mut message = Message::new();
    message.set_mouse_event(MouseEvent {
        mask: if trackpad {
            input::MOUSE_TYPE_TRACKPAD
        } else {
            input::MOUSE_TYPE_WHEEL
        },
        x: x.clamp(-120, 120),
        y: y.clamp(-120, 120),
        modifiers: [
            ControlKey::Control,
            ControlKey::Alt,
            ControlKey::Shift,
            ControlKey::Meta,
        ]
        .into_iter()
        .zip([
            modifiers.control,
            modifiers.alt,
            modifiers.shift,
            modifiers.platform,
        ])
        .filter_map(|(key, enabled)| enabled.then_some(key.into()))
        .collect(),
        ..Default::default()
    });
    Some(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protobuf::Message as _;
    #[test]
    fn printable_keys_use_edges_and_release_the_original_character() {
        let pressed = key(
            &Keystroke {
                key: "a".into(),
                key_char: Some("A".into()),
                modifiers: gpui::Modifiers {
                    shift: true,
                    ..Default::default()
                },
            },
            true,
        )
        .unwrap();
        assert!(pressed.key_event().has_chr());
        assert_eq!(pressed.key_event().chr(), u32::from('A'));
        assert!(pressed.key_event().down);
        let released = release_key(&pressed);
        assert_eq!(released.key_event().chr(), u32::from('A'));
        assert!(!released.key_event().down);
        assert_eq!(
            released.key_event().modifiers,
            vec![ControlKey::Shift.into()]
        );
    }

    #[test]
    fn hotkey_release_keeps_press_modifiers_after_control_is_released_or_focus_is_lost() {
        let pressed = key(
            &Keystroke {
                key: "c".into(),
                modifiers: gpui::Modifiers {
                    control: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            true,
        )
        .unwrap();
        let released = release_key(&pressed);
        assert_eq!(released.key_event().chr(), u32::from('c'));
        assert_eq!(
            released.key_event().modifiers,
            vec![ControlKey::Control.into()]
        );
        assert!(!released.key_event().down);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn shifted_key_identity_survives_releasing_shift_first() {
        let platform = gpui::current_platform(true);
        let mapper = platform.keyboard_mapper();
        let shifted = mapper.map_key_equivalent(
            Keystroke {
                key: "1".into(),
                modifiers: gpui::Modifiers {
                    shift: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            false,
        );
        let release = Keystroke {
            key: shifted.key().into(),
            ..Default::default()
        };
        assert_ne!(shifted.inner().key, release.key);
        for (control, alt) in [(false, false), (true, false), (true, true)] {
            let press = Keystroke {
                modifiers: gpui::Modifiers {
                    control,
                    alt,
                    ..shifted.inner().modifiers
                },
                ..shifted.inner().clone()
            };
            assert_eq!(
                key_identity(&press, mapper.as_ref()),
                key_identity(&release, mapper.as_ref())
            );
        }
    }

    #[test]
    fn explicit_modifiers_stay_down_and_use_focus_loss_release() {
        for (name, control) in [
            ("control", ControlKey::Control),
            ("alt", ControlKey::Alt),
            ("shift", ControlKey::Shift),
            ("super", ControlKey::Meta),
        ] {
            let pressed = key(
                &Keystroke {
                    key: name.into(),
                    modifiers: gpui::Modifiers {
                        control: true,
                        alt: true,
                        shift: true,
                        platform: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                true,
            )
            .unwrap();
            assert_eq!(pressed.key_event().control_key(), control);
            assert!(pressed.key_event().down);
            assert!(!pressed.key_event().modifiers.contains(&control.into()));
            assert_eq!(pressed.key_event().modifiers.len(), 3);
            let released = release_key(&pressed);
            assert_eq!(released.key_event().control_key(), control);
            assert!(!released.key_event().down);
            assert!(released.key_event().modifiers.is_empty());
        }
    }

    #[test]
    fn captured_drag_clamps_to_frame_edges_but_outside_clicks_are_ignored() {
        assert!(pointer(500.0, 1.0, (1000.0, 800.0), (1920, 1080), 1, 1).is_none());
        let dragged =
            pointer_with_capture(-40.0, 900.0, (1000.0, 800.0), (1920, 1080), 0, 0, true).unwrap();
        assert_eq!(
            (dragged.mouse_event().x, dragged.mouse_event().y),
            (0, 1079)
        );
        let released =
            pointer_with_capture(1100.0, 1.0, (1000.0, 800.0), (1920, 1080), 2, 1, true).unwrap();
        assert_eq!(
            (released.mouse_event().x, released.mouse_event().y),
            (1919, 0)
        );
        assert_eq!(
            released.mouse_event().mask,
            input::MOUSE_TYPE_UP | (input::MOUSE_BUTTON_LEFT << 3)
        );
    }

    #[test]
    fn function_keys_and_wheel_use_native_protocol() {
        let event = key(
            &Keystroke {
                key: "f12".into(),
                ..Default::default()
            },
            true,
        )
        .unwrap();
        assert_eq!(event.key_event().control_key(), ControlKey::F12);
        let event = wheel(
            0,
            1,
            false,
            gpui::Modifiers {
                shift: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(event.mouse_event().mask, input::MOUSE_TYPE_WHEEL);
        assert_eq!(event.mouse_event().y, 1);
        assert_eq!(
            event.mouse_event().modifiers,
            vec![ControlKey::Shift.into()]
        );
        assert!(wheel(0, 0, false, Default::default()).is_none());
    }

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
        assert_eq!(event.key_event().chr(), u32::from('s'));
        assert_eq!(
            event.key_event().modifiers,
            vec![ControlKey::Control.into()]
        );
    }
}
