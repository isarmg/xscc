use android_activity::input::{
    KeyAction, KeyEvent, Keycode, MetaState, MotionAction, MotionEvent, Source,
};
use gpui::{
    Capslock, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, ModifiersChangedEvent, Pixels,
    PlatformInput, Point, TouchEvent, TouchId, TouchPhase, point, px,
};
use std::collections::HashMap;

#[derive(Default)]
pub(crate) struct InputState {
    touches: HashMap<(i32, i32), TouchEvent>,
    next_touch_id: u64,
    modifiers: Modifiers,
    capslock: Capslock,
}

impl InputState {
    pub(crate) fn motion(&mut self, event: &MotionEvent<'_>, scale: f32) -> Vec<PlatformInput> {
        if !matches!(
            event.source(),
            Source::Touchscreen | Source::Stylus | Source::BluetoothStylus
        ) {
            return Vec::new();
        }
        let device = event.device_id();
        let action = event.action();
        if action == MotionAction::Cancel {
            return self.cancel_device(Some(device));
        }
        let phase = match action {
            MotionAction::Down | MotionAction::PointerDown => TouchPhase::Started,
            MotionAction::Move => TouchPhase::Moved,
            MotionAction::Up | MotionAction::PointerUp => TouchPhase::Ended,
            _ => return Vec::new(),
        };
        let mut events = if action == MotionAction::Down {
            self.cancel_device(Some(device))
        } else {
            Vec::new()
        };
        for pointer in event.pointers() {
            if action != MotionAction::Move && pointer.pointer_index() != event.pointer_index() {
                continue;
            }
            if let Some(touch) = self.touch(
                device,
                pointer.pointer_id(),
                phase,
                logical_position(pointer.x(), pointer.y(), scale),
            ) {
                events.push(PlatformInput::Touch(touch));
            }
        }
        events
    }

    pub(crate) fn cancel_touches(&mut self) -> Vec<PlatformInput> {
        self.cancel_device(None)
    }

    fn cancel_device(&mut self, device: Option<i32>) -> Vec<PlatformInput> {
        let mut events = Vec::new();
        self.touches.retain(|(touch_device, _), touch| {
            if device.is_some_and(|device| device != *touch_device) {
                return true;
            }
            let mut cancelled = touch.clone();
            cancelled.phase = TouchPhase::Cancelled;
            events.push(PlatformInput::Touch(cancelled));
            false
        });
        events
    }

    fn touch(
        &mut self,
        device: i32,
        pointer: i32,
        phase: TouchPhase,
        position: Point<Pixels>,
    ) -> Option<TouchEvent> {
        let key = (device, pointer);
        let mut event = if phase == TouchPhase::Started {
            self.next_touch_id = self.next_touch_id.checked_add(1)?;
            TouchEvent {
                id: TouchId(self.next_touch_id),
                ..Default::default()
            }
        } else {
            self.touches.get(&key)?.clone()
        };
        event.phase = phase;
        event.position = position;
        if phase == TouchPhase::Ended {
            self.touches.remove(&key);
        } else {
            self.touches.insert(key, event.clone());
        }
        Some(event)
    }

    pub(crate) fn key(&mut self, event: &KeyEvent<'_>) -> Vec<PlatformInput> {
        // Back belongs to the activity/navigation lifecycle, rather than a text keybinding.
        if event.key_code() == Keycode::Back {
            return Vec::new();
        }
        let modifiers = modifiers(event.meta_state());
        let capslock = Capslock {
            on: event.meta_state().caps_lock_on(),
        };
        let mut events = Vec::new();
        if modifiers != self.modifiers || capslock != self.capslock {
            self.modifiers = modifiers;
            self.capslock = capslock;
            events.push(PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                modifiers,
                capslock,
            }));
        }
        let Some(key) = key_name(event.key_code()) else {
            return events;
        };
        let keystroke = Keystroke {
            modifiers,
            key,
            // Text input is not supported yet; do not fabricate US-layout characters.
            key_char: None,
        };
        match event.action() {
            KeyAction::Down => events.push(PlatformInput::KeyDown(KeyDownEvent {
                keystroke,
                is_held: event.repeat_count() > 0,
                prefer_character_input: false,
            })),
            KeyAction::Up => events.push(PlatformInput::KeyUp(KeyUpEvent { keystroke })),
            _ => {}
        }
        events
    }
}

fn logical_position(x: f32, y: f32, scale: f32) -> Point<Pixels> {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    point(px(x / scale), px(y / scale))
}

fn modifiers(state: MetaState) -> Modifiers {
    Modifiers {
        control: state.ctrl_on(),
        alt: state.alt_on(),
        shift: state.shift_on(),
        platform: state.meta_on(),
        function: state.function_on(),
    }
}

fn key_name(key: Keycode) -> Option<String> {
    let code = u32::from(key);
    if (u32::from(Keycode::A)..=u32::from(Keycode::Z)).contains(&code) {
        return Some(char::from(b'a' + (code - u32::from(Keycode::A)) as u8).to_string());
    }
    if (u32::from(Keycode::Keycode0)..=u32::from(Keycode::Keycode9)).contains(&code) {
        return Some(char::from(b'0' + (code - u32::from(Keycode::Keycode0)) as u8).to_string());
    }
    if (u32::from(Keycode::F1)..=u32::from(Keycode::F12)).contains(&code) {
        return Some(format!("f{}", code - u32::from(Keycode::F1) + 1));
    }
    Some(
        match key {
            Keycode::Enter | Keycode::NumpadEnter | Keycode::DpadCenter => "enter",
            Keycode::Escape => "escape",
            Keycode::Del => "backspace",
            Keycode::ForwardDel => "delete",
            Keycode::Tab => "tab",
            Keycode::Space => "space",
            Keycode::DpadLeft => "left",
            Keycode::DpadRight => "right",
            Keycode::DpadUp => "up",
            Keycode::DpadDown => "down",
            Keycode::MoveHome => "home",
            Keycode::MoveEnd => "end",
            Keycode::PageUp => "pageup",
            Keycode::PageDown => "pagedown",
            Keycode::Insert => "insert",
            Keycode::Comma => ",",
            Keycode::Period => ".",
            Keycode::Grave => "`",
            Keycode::Minus => "-",
            Keycode::Equals => "=",
            Keycode::LeftBracket => "[",
            Keycode::RightBracket => "]",
            Keycode::Backslash => "\\",
            Keycode::Semicolon => ";",
            Keycode::Apostrophe => "'",
            Keycode::Slash => "/",
            _ => return None,
        }
        .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_ids_survive_reordering_and_are_not_reused() {
        let mut state = InputState::default();
        let position = logical_position(40., 80., 2.);
        let first = state.touch(1, 7, TouchPhase::Started, position).unwrap();
        let second = state.touch(1, 3, TouchPhase::Started, position).unwrap();
        assert_ne!(first.id, second.id);
        assert_eq!(
            state.touch(1, 3, TouchPhase::Moved, position).unwrap().id,
            second.id
        );
        assert_eq!(
            state.touch(1, 7, TouchPhase::Ended, position).unwrap().id,
            first.id
        );
        assert_ne!(
            state.touch(1, 7, TouchPhase::Started, position).unwrap().id,
            first.id
        );
        assert_eq!(position, point(px(20.), px(40.)));
    }

    #[test]
    fn cancellation_preserves_other_devices_and_clears_active_contacts() {
        let mut state = InputState::default();
        let position = logical_position(10., 20., f32::NAN);
        let first = state.touch(1, 0, TouchPhase::Started, position).unwrap();
        let second = state.touch(2, 0, TouchPhase::Started, position).unwrap();
        let cancelled = state.cancel_device(Some(1));
        assert!(
            matches!(&cancelled[..], [PlatformInput::Touch(event)] if event.id == first.id && event.phase == TouchPhase::Cancelled)
        );
        assert!(state.touch(1, 0, TouchPhase::Moved, position).is_none());
        assert_eq!(
            state.touch(2, 0, TouchPhase::Moved, position).unwrap().id,
            second.id
        );
        assert_eq!(state.cancel_touches().len(), 1);
        assert!(state.cancel_touches().is_empty());
        assert_eq!(position, point(px(10.), px(20.)));
    }

    #[test]
    fn hardware_navigation_and_binding_keys_use_gpui_names() {
        assert_eq!(key_name(Keycode::A).as_deref(), Some("a"));
        assert_eq!(key_name(Keycode::Keycode9).as_deref(), Some("9"));
        assert_eq!(key_name(Keycode::F12).as_deref(), Some("f12"));
        assert_eq!(key_name(Keycode::Del).as_deref(), Some("backspace"));
        assert_eq!(key_name(Keycode::MoveHome).as_deref(), Some("home"));
        assert!(key_name(Keycode::Back).is_none());
    }
}
