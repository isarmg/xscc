use base::message_proto::{ControlKey, KeyEvent, KeyboardMode};
use std::collections::HashMap;

pub(super) fn legacy_modifier(key: i32) -> Option<ControlKey> {
    match key {
        n if n == ControlKey::Alt as i32
            || n == ControlKey::Menu as i32
            || n == ControlKey::Option as i32 =>
        {
            Some(ControlKey::Alt)
        }
        n if n == ControlKey::RAlt as i32 => Some(ControlKey::RAlt),
        n if n == ControlKey::Control as i32 => Some(ControlKey::Control),
        n if n == ControlKey::RControl as i32 => Some(ControlKey::RControl),
        n if n == ControlKey::Shift as i32 => Some(ControlKey::Shift),
        n if n == ControlKey::RShift as i32 => Some(ControlKey::RShift),
        n if n == ControlKey::Meta as i32 => Some(ControlKey::Meta),
        n if n == ControlKey::RWin as i32 => Some(ControlKey::RWin),
        _ => None,
    }
}

pub(super) fn native_modifier(key: rdev::Key) -> Option<ControlKey> {
    match key {
        rdev::Key::Alt => Some(ControlKey::Alt),
        rdev::Key::AltGr => Some(ControlKey::RAlt),
        rdev::Key::ControlLeft => Some(ControlKey::Control),
        rdev::Key::ControlRight => Some(ControlKey::RControl),
        rdev::Key::ShiftLeft => Some(ControlKey::Shift),
        rdev::Key::ShiftRight => Some(ControlKey::RShift),
        rdev::Key::MetaLeft => Some(ControlKey::Meta),
        rdev::Key::MetaRight => Some(ControlKey::RWin),
        _ => None,
    }
}

#[cfg(any(windows, target_os = "macos", test))]
pub(super) fn virtual_modifier(code: u32) -> Option<ControlKey> {
    // MapVirtualKey maps the non-sided Windows VKs to the left modifier.
    match code {
        0x10 => Some(ControlKey::Shift),
        0x11 => Some(ControlKey::Control),
        0x12 => Some(ControlKey::Alt),
        _ => native_modifier(rdev::win_key_from_keycode(code)),
    }
}

pub(super) fn modifier(event: &KeyEvent) -> Option<ControlKey> {
    match event.mode.enum_value() {
        Ok(KeyboardMode::Map | KeyboardMode::Translate) if event.has_chr() => {
            let code = event.chr();
            #[cfg(windows)]
            if event.mode.enum_value() == Ok(KeyboardMode::Translate) && code >> 16 != 0 {
                return virtual_modifier(code >> 16);
            }
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            let code = code & 0xffff;
            native_modifier(crate::keyboard::keycode_to_rdev_key(code))
        }
        Ok(KeyboardMode::Map | KeyboardMode::Translate) => None,
        _ if event.has_control_key() => legacy_modifier(event.control_key() as i32),
        _ => None,
    }
}

#[derive(Default)]
pub(super) struct PressedModifiers {
    releases: HashMap<(i32, ControlKey), KeyEvent>,
}

impl PressedModifiers {
    pub(super) fn handle(
        &mut self,
        conn: i32,
        event: &KeyEvent,
        mut inject: impl FnMut(&KeyEvent),
    ) {
        let Some(key) = modifier(event) else {
            inject(event);
            return;
        };
        if event.down {
            self.releases
                .entry((conn, key))
                .or_insert_with(|| KeyEvent {
                    mode: event.mode,
                    union: event.union.clone(),
                    ..Default::default()
                });
            // Preserve repeated downs, including the platform's existing repeat behavior.
            inject(event);
        } else if self.releases.remove(&(conn, key)).is_some()
            && !self.releases.keys().any(|(_, held)| *held == key)
        {
            inject(event);
        }
    }

    pub(super) fn release(&mut self, conn: Option<i32>, mut inject: impl FnMut(&KeyEvent)) {
        let mut released = HashMap::new();
        self.releases.retain(|&(owner, key), event| {
            if conn.is_none_or(|conn| owner == conn) {
                released.entry(key).or_insert_with(|| event.clone());
                false
            } else {
                true
            }
        });
        for (key, event) in released {
            if !self.releases.keys().any(|(_, held)| *held == key) {
                inject(&event);
            }
        }
    }

    pub(super) fn forget(&mut self, key: ControlKey) {
        self.releases.retain(|&(_, held), _| held != key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy(key: ControlKey, down: bool) -> KeyEvent {
        let mut event = KeyEvent {
            down,
            ..Default::default()
        };
        event.set_control_key(key);
        event
    }

    fn mapped(key: rdev::Key, mode: KeyboardMode) -> KeyEvent {
        #[cfg(target_os = "linux")]
        let code = rdev::linux_keycode_from_key(key).unwrap();
        #[cfg(target_os = "macos")]
        let code = rdev::macos_keycode_from_key(key).unwrap() as u32;
        #[cfg(windows)]
        let code = if mode == KeyboardMode::Translate {
            rdev::win_keycode_from_key(key).unwrap() << 16
        } else {
            rdev::win_scancode_from_key(key).unwrap()
        };
        let mut event = KeyEvent {
            down: true,
            mode: mode.into(),
            ..Default::default()
        };
        event.set_chr(code);
        event
    }

    #[test]
    fn mixed_modifier_owners_release_only_the_last_owner() {
        for (control, native) in [
            (ControlKey::Shift, rdev::Key::ShiftLeft),
            (ControlKey::RShift, rdev::Key::ShiftRight),
            (ControlKey::Control, rdev::Key::ControlLeft),
            (ControlKey::RControl, rdev::Key::ControlRight),
            (ControlKey::Alt, rdev::Key::Alt),
            (ControlKey::RAlt, rdev::Key::AltGr),
            (ControlKey::Meta, rdev::Key::MetaLeft),
            (ControlKey::RWin, rdev::Key::MetaRight),
        ] {
            let mut keys = PressedModifiers::default();
            let mut edges = Vec::new();
            let mut inject = |event: &KeyEvent| edges.push(event.down);
            keys.handle(1, &legacy(control, true), &mut inject);
            keys.handle(1, &legacy(control, true), &mut inject);
            let mut map = mapped(native, KeyboardMode::Map);
            keys.handle(2, &map, &mut inject);
            keys.handle(3, &mapped(native, KeyboardMode::Translate), &mut inject);
            keys.release(Some(1), &mut inject);
            map.down = false;
            keys.handle(2, &map, &mut inject);
            keys.release(Some(3), &mut inject);
            assert_eq!(edges, [true, true, true, true, false]);
        }
        for alias in [ControlKey::Alt, ControlKey::Menu, ControlKey::Option] {
            assert_eq!(modifier(&legacy(alias, true)), Some(ControlKey::Alt));
        }
        for (code, key) in [
            (0x10, ControlKey::Shift),
            (0x11, ControlKey::Control),
            (0x12, ControlKey::Alt),
        ] {
            assert_eq!(virtual_modifier(code), Some(key));
        }
    }

    #[test]
    fn queued_revocation_disconnect_and_reconnect_have_balanced_edges() {
        let (tx, rx) = std::sync::mpsc::channel();
        for event in [
            (1, Some(legacy(ControlKey::Shift, true))),
            (1, None),
            (1, None),
            (2, Some(legacy(ControlKey::Shift, true))),
            (1, None),
            (2, Some(legacy(ControlKey::Shift, false))),
        ] {
            tx.send(event).unwrap();
        }
        drop(tx);
        let mut keys = PressedModifiers::default();
        let mut edges = Vec::new();
        for (conn, event) in rx {
            let mut inject = |event: &KeyEvent| edges.push(event.down);
            match event {
                Some(event) => keys.handle(conn, &event, &mut inject),
                None => keys.release(Some(conn), &mut inject),
            }
        }
        assert_eq!(edges, [true, false, true, false]);
    }

    #[test]
    fn timeout_and_forced_reset_do_not_leave_stale_owners_or_repeat_commands() {
        let mut keys = PressedModifiers::default();
        let mut edges = Vec::new();
        let mut inject = |event: &KeyEvent| edges.push(event.down);
        keys.handle(1, &legacy(ControlKey::Shift, true), &mut inject);
        keys.forget(ControlKey::Shift);
        keys.handle(2, &legacy(ControlKey::Shift, true), &mut inject);
        keys.handle(2, &legacy(ControlKey::Shift, false), &mut inject);
        keys.handle(3, &legacy(ControlKey::Shift, true), &mut inject);
        keys.release(None, &mut inject);
        keys.handle(4, &legacy(ControlKey::Shift, true), &mut inject);
        keys.handle(4, &legacy(ControlKey::Shift, false), &mut inject);
        assert_eq!(edges, [true, true, false, true, false, true, false]);
        let mut text = KeyEvent::new();
        text.set_seq("text".into());
        let mut commands = Vec::new();
        for event in [
            text,
            legacy(ControlKey::CtrlAltDel, false),
            legacy(ControlKey::LockScreen, false),
        ] {
            keys.handle(1, &event, |event| commands.push(event.clone()));
        }
        keys.release(None, |event| commands.push(event.clone()));
        assert_eq!(commands.len(), 3);
    }
}
