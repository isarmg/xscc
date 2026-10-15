//! Read-only local keyboard queries. This module must not depend on input injection.

#[derive(Clone, Copy)]
pub(super) enum QueryKey {
    Alt,
    Control,
    RightControl,
    Shift,
    RightShift,
    Meta,
    CapsLock,
    NumLock,
}

/// Prefer the state carried by this Flutter event. Native hooks have no such
/// snapshot; Windows/macOS can query it without creating an injection device.
pub(super) fn num_lock(lock_modes: Option<i32>) -> Option<bool> {
    lock_modes
        .map(|modes| modes & (1 << 2) != 0)
        .or_else(|| native_key_state(QueryKey::NumLock))
}

#[cfg(target_os = "windows")]
pub(super) fn native_key_state(key: QueryKey) -> Option<bool> {
    use winapi::um::winuser::*;
    let code = match key {
        QueryKey::Alt => VK_MENU,
        QueryKey::Control => VK_LCONTROL,
        QueryKey::RightControl => VK_RCONTROL,
        QueryKey::Shift => VK_SHIFT,
        QueryKey::RightShift => VK_RSHIFT,
        QueryKey::Meta => VK_LWIN,
        QueryKey::CapsLock => VK_CAPITAL,
        QueryKey::NumLock => VK_NUMLOCK,
    };
    // Preserve GetKeyState's thread/message-queue semantics used by the hook.
    let value = unsafe { GetKeyState(code) } as u16;
    let mask = if matches!(key, QueryKey::CapsLock | QueryKey::NumLock) {
        1
    } else {
        0x8000
    };
    Some(value & mask != 0)
}

#[cfg(target_os = "macos")]
pub(super) fn native_key_state(key: QueryKey) -> Option<bool> {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;
    }
    let code = match key {
        QueryKey::Alt => 0x3a,
        QueryKey::Control => 0x3b,
        QueryKey::RightControl => 0x3e,
        QueryKey::Shift => 0x38,
        QueryKey::RightShift => 0x3c,
        QueryKey::Meta => 0x37,
        QueryKey::CapsLock => 0x39,
        // Preserve the client's existing numeric-keypad policy on macOS.
        QueryKey::NumLock => return Some(true),
    };
    Some(unsafe { CGEventSourceKeyState(1, code) })
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(super) fn native_key_state(_key: QueryKey) -> Option<bool> {
    // Wayland state comes from the focused input stream. A fixed sysfs LED
    // device is neither a compositor state source nor a modifier query.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_num_lock_is_authoritative() {
        assert_eq!(num_lock(Some(0)), Some(false));
        assert_eq!(num_lock(Some(1 << 2)), Some(true));
        assert_eq!(num_lock(Some(1 << 1)), Some(false));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_wayland_snapshot_is_unknown() {
        assert_eq!(num_lock(None), None);
        assert_eq!(native_key_state(QueryKey::CapsLock), None);
        assert_eq!(native_key_state(QueryKey::Control), None);
    }
}
