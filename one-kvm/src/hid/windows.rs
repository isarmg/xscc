//! Input into the selected monitor of the current interactive Windows desktop.
//! Windows provides one shared keyboard focus and pointer for that desktop.

use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::watch;
use windows_sys::Win32::{
    Foundation::POINT,
    System::{
        RemoteDesktop::ProcessIdToSessionId,
        StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_READOBJECTS},
        Threading::GetCurrentProcessId,
    },
    UI::{
        Input::KeyboardAndMouse::*,
        WindowsAndMessaging::{
            GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
            SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
        },
    },
};

use super::{
    backend::{HidBackend, HidBackendRuntimeSnapshot},
    types::{ConsumerEvent, KeyEventType, KeyboardEvent, MouseButton, MouseEvent, MouseEventType},
    windows_mapping::{absolute_coordinate, scan_code},
    CanonicalKey,
};
use crate::{
    error::{AppError, Result},
    platform::windows_desktop::{monitor_for_path, PhysicalCoordinates},
};

#[derive(Default)]
struct Pressed {
    keys: BTreeSet<u8>,
    buttons: u8,
}

pub struct WindowsBackend {
    monitor: String,
    keyboard: bool,
    mouse: bool,
    initialized: AtomicBool,
    pressed: Mutex<Pressed>,
    notify: watch::Sender<()>,
}

impl WindowsBackend {
    pub fn new(monitor: &str, keyboard: bool, mouse: bool) -> Result<Self> {
        if !keyboard && !mouse {
            return Err(AppError::BadRequest(
                "Enable Windows keyboard or mouse input".into(),
            ));
        }
        monitor_for_path(monitor)?;
        let (notify, _) = watch::channel(());
        Ok(Self {
            monitor: monitor.into(),
            keyboard,
            mouse,
            initialized: AtomicBool::new(false),
            pressed: Mutex::new(Pressed::default()),
            notify,
        })
    }

    fn check_interactive_desktop(&self) -> Result<()> {
        let mut session = 0;
        if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 {
            return Err(input_error("Cannot identify the Windows session"));
        }
        if session == 0 {
            return Err(AppError::Config("Windows desktop input requires a logged-in interactive session; Session 0 services must use external HID hardware".into()));
        }
        let desktop = unsafe { OpenInputDesktop(0, 0, DESKTOP_READOBJECTS) };
        if desktop.is_null() {
            return Err(input_error("The interactive input desktop is unavailable"));
        }
        unsafe { CloseDesktop(desktop) };
        Ok(())
    }

    fn require_active(&self) -> Result<()> {
        if !self.initialized.load(Ordering::Acquire) {
            return Err(AppError::Config(
                "Windows desktop input is not initialized".into(),
            ));
        }
        self.check_interactive_desktop()
    }

    fn set_key(pressed: &mut Pressed, key: CanonicalKey, down: bool) -> Result<()> {
        send_key(key, down)?;
        if down {
            pressed.keys.insert(key.to_hid_usage());
        } else {
            pressed.keys.remove(&key.to_hid_usage());
        }
        Ok(())
    }

    fn release(&self) -> Result<()> {
        let mut pressed = self.pressed.lock();
        let keys = pressed.keys.iter().copied().collect::<Vec<_>>();
        for usage in keys {
            if let Some(key) = CanonicalKey::from_hid_usage(usage) {
                Self::set_key(&mut pressed, key, false)?;
            }
        }
        for button in [
            MouseButton::Left,
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::Back,
            MouseButton::Forward,
        ] {
            if pressed.buttons & button.to_hid_bit() != 0 {
                let (flags, data) = button_input(button, false);
                send_mouse(0, 0, data, flags)?;
                pressed.buttons &= !button.to_hid_bit();
            }
        }
        Ok(())
    }
}

#[async_trait]
impl HidBackend for WindowsBackend {
    async fn init(&self) -> Result<()> {
        self.check_interactive_desktop()?;
        monitor_for_path(&self.monitor)?;
        self.initialized.store(true, Ordering::Release);
        self.notify.send_replace(());
        Ok(())
    }

    async fn send_keyboard(&self, event: KeyboardEvent) -> Result<()> {
        self.require_active()?;
        if !self.keyboard {
            return Err(AppError::BadRequest(
                "Keyboard input is disabled for this instance".into(),
            ));
        }
        let mut pressed = self.pressed.lock();
        let modifiers = event.modifiers.to_hid_byte();
        for index in 0..8 {
            let key =
                CanonicalKey::from_hid_usage(0xe0 + index).expect("invariant: HID modifier range");
            let down = if key == event.key {
                event.event_type == KeyEventType::Down
            } else {
                modifiers & (1 << index) != 0
            };
            if pressed.keys.contains(&key.to_hid_usage()) != down {
                Self::set_key(&mut pressed, key, down)?;
            }
        }
        if !event.key.is_modifier() {
            Self::set_key(
                &mut pressed,
                event.key,
                event.event_type == KeyEventType::Down,
            )?;
        }
        Ok(())
    }

    async fn send_mouse(&self, event: MouseEvent) -> Result<()> {
        self.require_active()?;
        let _coordinates = PhysicalCoordinates::enter()?;
        if !self.mouse {
            return Err(AppError::BadRequest(
                "Mouse input is disabled for this instance".into(),
            ));
        }
        let mut pressed = self.pressed.lock();
        match event.event_type {
            MouseEventType::MoveAbs | MouseEventType::Move => {
                let monitor = monitor_for_path(&self.monitor)?;
                let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
                let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
                let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
                let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
                if width <= 0 || height <= 0 {
                    return Err(AppError::Config(
                        "Windows virtual desktop is unavailable".into(),
                    ));
                }
                let (x, y) = if event.event_type == MouseEventType::MoveAbs {
                    (
                        absolute_coordinate(event.x, monitor.left, monitor.width, left, width),
                        absolute_coordinate(event.y, monitor.top, monitor.height, top, height),
                    )
                } else {
                    let mut position = POINT { x: 0, y: 0 };
                    if unsafe { GetCursorPos(&mut position) } == 0 {
                        return Err(input_error("Cannot query the Windows pointer"));
                    }
                    let x = (i64::from(position.x) + i64::from(event.x)).clamp(
                        i64::from(monitor.left),
                        i64::from(monitor.left) + i64::from(monitor.width) - 1,
                    );
                    let y = (i64::from(position.y) + i64::from(event.y)).clamp(
                        i64::from(monitor.top),
                        i64::from(monitor.top) + i64::from(monitor.height) - 1,
                    );
                    (
                        ((x - i64::from(left)) * 65535 / i64::from((width - 1).max(1)))
                            .clamp(0, 65535) as i32,
                        ((y - i64::from(top)) * 65535 / i64::from((height - 1).max(1)))
                            .clamp(0, 65535) as i32,
                    )
                };
                send_mouse(
                    x,
                    y,
                    0,
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                )?;
            }
            MouseEventType::Scroll => send_mouse(
                0,
                0,
                (i32::from(event.scroll) * 120) as u32,
                MOUSEEVENTF_WHEEL,
            )?,
            MouseEventType::Down | MouseEventType::Up => {
                let button = event
                    .button
                    .ok_or_else(|| AppError::BadRequest("Mouse button is required".into()))?;
                let down = event.event_type == MouseEventType::Down;
                let (flags, data) = button_input(button, down);
                send_mouse(0, 0, data, flags)?;
                if down {
                    pressed.buttons |= button.to_hid_bit();
                } else {
                    pressed.buttons &= !button.to_hid_bit();
                }
            }
        }
        Ok(())
    }

    async fn send_consumer(&self, event: ConsumerEvent) -> Result<()> {
        self.require_active()?;
        if !self.keyboard {
            return Err(AppError::BadRequest(
                "Keyboard input is disabled for this instance".into(),
            ));
        }
        let key = match event.usage {
            0xcd => VK_MEDIA_PLAY_PAUSE,
            0xb7 => VK_MEDIA_STOP,
            0xb5 => VK_MEDIA_NEXT_TRACK,
            0xb6 => VK_MEDIA_PREV_TRACK,
            0xe2 => VK_VOLUME_MUTE,
            0xe9 => VK_VOLUME_UP,
            0xea => VK_VOLUME_DOWN,
            _ => return Err(AppError::BadRequest("Unsupported consumer control".into())),
        };
        let _pressed = self.pressed.lock();
        send_keyboard_input(key, 0, 0)?;
        send_keyboard_input(key, 0, KEYEVENTF_KEYUP)
    }

    async fn reset(&self) -> Result<()> {
        self.release()
    }

    async fn shutdown(&self) -> Result<()> {
        self.release()?;
        self.initialized.store(false, Ordering::Release);
        self.notify.send_replace(());
        Ok(())
    }

    fn runtime_snapshot(&self) -> HidBackendRuntimeSnapshot {
        let initialized = self.initialized.load(Ordering::Acquire);
        HidBackendRuntimeSnapshot {
            initialized,
            online: initialized,
            supports_absolute_mouse: self.mouse,
            device: Some(self.monitor.clone()),
            ..Default::default()
        }
    }

    fn subscribe_runtime(&self) -> watch::Receiver<()> {
        self.notify.subscribe()
    }
}

fn input_error(context: &str) -> AppError {
    AppError::Config(format!(
        "{context}: {} (input is subject to Windows session/UIPI permissions)",
        std::io::Error::last_os_error()
    ))
}

fn send(input: INPUT) -> Result<()> {
    if unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) } != 1 {
        return Err(input_error("Windows rejected input"));
    }
    Ok(())
}

fn send_keyboard_input(vk: u16, scan: u16, flags: u32) -> Result<()> {
    send(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    })
}

fn send_key(key: CanonicalKey, down: bool) -> Result<()> {
    let release = if down { 0 } else { KEYEVENTF_KEYUP };
    if key == CanonicalKey::Pause {
        return send_keyboard_input(VK_PAUSE, 0, release);
    }
    let code =
        scan_code(key).ok_or_else(|| AppError::BadRequest("Unsupported Windows key".into()))?;
    let extended = if code & 0x100 != 0 {
        KEYEVENTF_EXTENDEDKEY
    } else {
        0
    };
    send_keyboard_input(0, code & 0xff, KEYEVENTF_SCANCODE | extended | release)
}

fn send_mouse(dx: i32, dy: i32, data: u32, flags: u32) -> Result<()> {
    send(INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    })
}

fn button_input(button: MouseButton, down: bool) -> (u32, u32) {
    match (button, down) {
        (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (MouseButton::Back, true) => (MOUSEEVENTF_XDOWN, 1),
        (MouseButton::Back, false) => (MOUSEEVENTF_XUP, 1),
        (MouseButton::Forward, true) => (MOUSEEVENTF_XDOWN, 2),
        (MouseButton::Forward, false) => (MOUSEEVENTF_XUP, 2),
    }
}
