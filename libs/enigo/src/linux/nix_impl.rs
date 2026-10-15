use crate::{Key, KeyboardControllable, MouseButton, MouseControllable};
use std::io::Read;

pub type CustomKeyboard = Box<dyn KeyboardControllable + Send>;
pub type CustomMouce = Box<dyn MouseControllable + Send>;

/// Input dispatch for the installed Wayland portal or uinput devices.
#[derive(Default)]
pub struct Enigo {
    custom_keyboard: Option<CustomKeyboard>,
    custom_mouse: Option<CustomMouce>,
}

impl Enigo {
    /// Set custom keyboard.
    pub fn set_custom_keyboard(&mut self, custom_keyboard: CustomKeyboard) {
        self.custom_keyboard = Some(custom_keyboard)
    }
    /// Set custom mouse.
    pub fn set_custom_mouse(&mut self, custom_mouse: CustomMouce) {
        self.custom_mouse = Some(custom_mouse)
    }
    /// Get custom keyboard.
    pub fn get_custom_keyboard(&mut self) -> &mut Option<CustomKeyboard> {
        &mut self.custom_keyboard
    }
    /// Get custom mouse.
    pub fn get_custom_mouse(&mut self) -> &mut Option<CustomMouce> {
        &mut self.custom_mouse
    }
}

impl MouseControllable for Enigo {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_mut_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn mouse_move_to(&mut self, x: i32, y: i32) {
        if let Some(mouse) = &mut self.custom_mouse {
            mouse.mouse_move_to(x, y)
        } else {
            hbb_common::throttled_log!(
                std::time::Duration::from_secs(5),
                warn,
                "Cannot inject input: Wayland mouse is not initialized"
            );
        }
    }
    fn mouse_move_relative(&mut self, x: i32, y: i32) {
        if let Some(mouse) = &mut self.custom_mouse {
            mouse.mouse_move_relative(x, y)
        } else {
            hbb_common::throttled_log!(
                std::time::Duration::from_secs(5),
                warn,
                "Cannot inject input: Wayland mouse is not initialized"
            );
        }
    }
    fn mouse_down(&mut self, button: MouseButton) -> crate::ResultType {
        if let Some(mouse) = &mut self.custom_mouse {
            mouse.mouse_down(button)
        } else {
            Err("Wayland mouse is not initialized".into())
        }
    }
    fn mouse_up(&mut self, button: MouseButton) {
        if let Some(mouse) = &mut self.custom_mouse {
            mouse.mouse_up(button)
        } else {
            hbb_common::throttled_log!(
                std::time::Duration::from_secs(5),
                warn,
                "Cannot inject input: Wayland mouse is not initialized"
            );
        }
    }
    fn mouse_click(&mut self, button: MouseButton) {
        if let Some(mouse) = &mut self.custom_mouse {
            mouse.mouse_click(button)
        } else {
            hbb_common::throttled_log!(
                std::time::Duration::from_secs(5),
                warn,
                "Cannot inject input: Wayland mouse is not initialized"
            );
        }
    }
    fn mouse_scroll_x(&mut self, length: i32) {
        if let Some(mouse) = &mut self.custom_mouse {
            mouse.mouse_scroll_x(length)
        } else {
            hbb_common::throttled_log!(
                std::time::Duration::from_secs(5),
                warn,
                "Cannot inject input: Wayland mouse is not initialized"
            );
        }
    }
    fn mouse_scroll_y(&mut self, length: i32) {
        if let Some(mouse) = &mut self.custom_mouse {
            mouse.mouse_scroll_y(length)
        } else {
            hbb_common::throttled_log!(
                std::time::Duration::from_secs(5),
                warn,
                "Cannot inject input: Wayland mouse is not initialized"
            );
        }
    }
}

fn get_led_state(key: Key) -> bool {
    let led_file = match key {
        // FIXME: the file may be /sys/class/leds/input2 or input5 ...
        Key::CapsLock => "/sys/class/leds/input1::capslock/brightness",
        Key::NumLock => "/sys/class/leds/input1::numlock/brightness",
        _ => {
            return false;
        }
    };

    let status = if let Ok(mut file) = std::fs::File::open(&led_file) {
        let mut content = String::new();
        file.read_to_string(&mut content).ok();
        let status = content.trim_end().to_string().parse::<i32>().unwrap_or(0);
        status
    } else {
        0
    };
    status == 1
}

impl KeyboardControllable for Enigo {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_mut_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn get_key_state(&mut self, key: Key) -> bool {
        if let Some(keyboard) = &mut self.custom_keyboard {
            keyboard.get_key_state(key)
        } else {
            get_led_state(key)
        }
    }
    fn key_sequence(&mut self, sequence: &str) {
        if let Some(keyboard) = &mut self.custom_keyboard {
            keyboard.key_sequence(sequence)
        } else {
            hbb_common::throttled_log!(
                std::time::Duration::from_secs(5),
                warn,
                "Cannot inject input: Wayland keyboard is not initialized"
            );
        }
    }
    fn key_down(&mut self, key: Key) -> crate::ResultType {
        if let Some(keyboard) = &mut self.custom_keyboard {
            keyboard.key_down(key)
        } else {
            Err("Wayland keyboard is not initialized".into())
        }
    }
    fn key_up(&mut self, key: Key) {
        if let Some(keyboard) = &mut self.custom_keyboard {
            keyboard.key_up(key)
        } else {
            hbb_common::throttled_log!(
                std::time::Duration::from_secs(5),
                warn,
                "Cannot inject input: Wayland keyboard is not initialized"
            );
        }
    }
    fn key_click(&mut self, key: Key) {
        if let Some(keyboard) = &mut self.custom_keyboard {
            keyboard.key_click(key)
        } else {
            hbb_common::throttled_log!(
                std::time::Duration::from_secs(5),
                warn,
                "Cannot inject input: Wayland keyboard is not initialized"
            );
        }
    }
}

#[test]
fn test_custom_mouse_dispatch() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct CountingMouse(Arc<AtomicUsize>);
    impl MouseControllable for CountingMouse {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_mut_any(&mut self) -> &mut dyn std::any::Any {
            self
        }
        fn mouse_move_to(&mut self, _x: i32, _y: i32) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn mouse_move_relative(&mut self, _x: i32, _y: i32) {}
        fn mouse_down(&mut self, _button: MouseButton) -> crate::ResultType {
            Ok(())
        }
        fn mouse_up(&mut self, _button: MouseButton) {}
        fn mouse_click(&mut self, _button: MouseButton) {}
        fn mouse_scroll_x(&mut self, _length: i32) {}
        fn mouse_scroll_y(&mut self, _length: i32) {}
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let mut en = Enigo::new();
    en.set_custom_mouse(Box::new(CountingMouse(calls.clone())));

    en.mouse_move_to(10, 20);
    assert_eq!(
        calls.load(Ordering::Relaxed),
        1,
        "custom mouse was not reached"
    );
}

#[test]
fn test_uninitialized_devices_return_errors() {
    let mut en = Enigo::new();
    assert!(en.key_down(Key::Layout('a')).is_err());
    assert!(en.mouse_down(MouseButton::Left).is_err());
}

#[test]
fn test_custom_keyboard_dispatch_preserves_failure() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct FailingKeyboard(Arc<AtomicUsize>);
    impl KeyboardControllable for FailingKeyboard {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_mut_any(&mut self) -> &mut dyn std::any::Any {
            self
        }
        fn get_key_state(&mut self, _key: Key) -> bool {
            false
        }
        fn key_sequence(&mut self, _sequence: &str) {}
        fn key_down(&mut self, _key: Key) -> crate::ResultType {
            self.0.fetch_add(1, Ordering::Relaxed);
            Err("device unavailable".into())
        }
        fn key_up(&mut self, _key: Key) {}
        fn key_click(&mut self, _key: Key) {}
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let mut en = Enigo::new();
    en.set_custom_keyboard(Box::new(FailingKeyboard(calls.clone())));
    assert_eq!(
        en.key_down(Key::Layout('a')).unwrap_err().to_string(),
        "device unavailable"
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}
