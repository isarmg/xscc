#[cfg(feature = "flutter")]
use crate::flutter;
#[cfg(target_os = "windows")]
use crate::platform::windows::{get_char_from_vk, get_unicode_from_vk};
use crate::ui_session_interface::{InvokeUiSession, Session};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use crate::common::GrabState;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use hbb_common::log;
use base::message_proto::*;
use hbb_common::SessionID;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use rdev::KeyCode;
use rdev::{Event, EventType, Key};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[cfg(windows)]
static mut IS_ALT_GR: bool = false;

#[allow(dead_code)]
const OS_LOWER_WINDOWS: &str = "windows";
#[allow(dead_code)]
const OS_LOWER_LINUX: &str = "linux";
#[allow(dead_code)]
const OS_LOWER_MACOS: &str = "macos";
#[allow(dead_code)]
const OS_LOWER_ANDROID: &str = "android";

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
static KEYBOARD_HOOKED: AtomicBool = AtomicBool::new(false);

// Track key down state for relative mouse mode exit shortcut.
// macOS: Cmd+G (track G key)
// Windows/Linux: Ctrl+Alt (track whichever modifier was pressed last)
// This prevents the exit from retriggering on OS key-repeat.
#[cfg(all(feature = "flutter", any(target_os = "windows", target_os = "macos", target_os = "linux")))]
static EXIT_SHORTCUT_KEY_DOWN: AtomicBool = AtomicBool::new(false);

// Track whether relative mouse mode is currently active.
// This is set by Flutter via set_relative_mouse_mode_state() and checked
// by the rdev grab loop to determine if exit shortcuts should be processed.
#[cfg(all(feature = "flutter", any(target_os = "windows", target_os = "macos", target_os = "linux")))]
static RELATIVE_MOUSE_MODE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set the relative mouse mode state from Flutter.
/// This is called when entering or exiting relative mouse mode.
#[cfg(all(feature = "flutter", any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub fn set_relative_mouse_mode_state(active: bool) {
    RELATIVE_MOUSE_MODE_ACTIVE.store(active, Ordering::SeqCst);
    // Reset exit shortcut state when mode changes to avoid stale state
    if !active {
        EXIT_SHORTCUT_KEY_DOWN.store(false, Ordering::SeqCst);
    }
}

#[cfg(feature = "flutter")]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
static IS_RDEV_ENABLED: AtomicBool = AtomicBool::new(false);

lazy_static::lazy_static! {
    static ref TO_RELEASE: Arc<Mutex<HashMap<Key, (Event, Option<i32>)>>> = Arc::new(Mutex::new(HashMap::new()));
    static ref MODIFIERS_STATE: Mutex<HashMap<Key, bool>> = {
        let mut m = HashMap::new();
        m.insert(Key::ShiftLeft, false);
        m.insert(Key::ShiftRight, false);
        m.insert(Key::ControlLeft, false);
        m.insert(Key::ControlRight, false);
        m.insert(Key::Alt, false);
        m.insert(Key::AltGr, false);
        m.insert(Key::MetaLeft, false);
        m.insert(Key::MetaRight, false);
        Mutex::new(m)
    };
}

pub mod shortcuts;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod state;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use state::{native_key_state, QueryKey};

pub mod client {
    use super::*;

    /// Tracks grab ownership and serializes transitions across threads.
    ///
    /// Multiple Flutter isolates (one per session window) call
    /// `change_grab_status(Run/Wait)` concurrently. Without serialization a
    /// stale `Wait` from session A can clobber session B's freshly acquired
    /// grab on Windows or macOS.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[derive(Default)]
    struct GrabOwnerState {
        owner: Option<u128>,
        last_grab: Option<std::time::Instant>,
        deferred_pending: bool,
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    lazy_static::lazy_static! {
        static ref IS_GRAB_STARTED: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
        static ref GRAB_STATE: Arc<Mutex<GrabOwnerState>> = Arc::new(Mutex::new(GrabOwnerState::default()));
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub fn start_grab_loop() {
        let mut lock = IS_GRAB_STARTED.lock().unwrap();
        if *lock {
            return;
        }
        super::start_grab_loop();
        *lock = true;
    }

    #[cfg(target_os = "linux")]
    pub fn change_grab_status(_state: GrabState, _keyboard_mode: &str, _session_id: u128) {}

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub fn change_grab_status(state: GrabState, keyboard_mode: &str, session_id: u128) {
        #[cfg(feature = "flutter")]
        if !IS_RDEV_ENABLED.load(Ordering::SeqCst) {
            return;
        }
        // Serialize transitions so a stale `Wait` from a previous owner cannot
        // clobber a fresh `Run` from a different session window.
        let mut release_after_unlock = None;

        let mut gs = GRAB_STATE.lock().unwrap();
        match state {
            GrabState::Ready => {}
            GrabState::Run => {
                #[cfg(windows)]
                update_grab_get_key_name(keyboard_mode);

                // Idempotent: if this session already owns the grab, just
                // refresh the debounce timer (proves the session is still
                // actively focused) and skip the actual grab call.
                if gs.owner == Some(session_id) {
                    gs.last_grab = Some(std::time::Instant::now());
                    // Reset so the next Wait can spawn a fresh deferred-release
                    // timer with an up-to-date snapshot of last_grab.
                    gs.deferred_pending = false;
                    log::debug!(
                        "[grab] Run(0x{:x}): already owner, refresh debounce",
                        session_id
                    );
                    return;
                }

                log::debug!(
                    "[grab] Run(0x{:x}): prev_owner={}, mode={}",
                    session_id,
                    gs.owner
                        .map_or("none".to_string(), |id| format!("0x{:x}", id)),
                    keyboard_mode,
                );

                #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
                KEYBOARD_HOOKED.store(true, Ordering::SeqCst);

                gs.owner = Some(session_id);
                gs.last_grab = Some(std::time::Instant::now());
                // Invalidate any in-flight deferred release from the previous
                // owner so it cannot suppress a fresh timer for the new owner.
                gs.deferred_pending = false;

            }
            GrabState::Wait => {
                // Drop stale `Wait` events that do not correspond to the
                // current grab owner. This prevents a late PointerExit from
                // session A from releasing session B's freshly acquired grab.
                if gs.owner != Some(session_id) {
                    log::debug!(
                        "[grab] Wait(0x{:x}): ignored, owner={}",
                        session_id,
                        gs.owner
                            .map_or("none".to_string(), |id| format!("0x{:x}", id)),
                    );
                    return;
                }

                log::debug!("[grab] Wait(0x{:x}): releasing grab", session_id);

                #[cfg(windows)]
                rdev::set_get_key_unicode(false);

                #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
                KEYBOARD_HOOKED.store(false, Ordering::SeqCst);

                gs.owner = None;
                gs.last_grab = None;
                gs.deferred_pending = false;
                release_after_unlock = Some(take_remote_keys());

            }
            GrabState::Exit => {}
        }
        drop(gs);

        if let Some(to_release) = release_after_unlock {
            release_remote_keys_for_events(keyboard_mode, to_release);
        }
    }

    pub fn process_event(keyboard_mode: &str, event: &Event, lock_modes: Option<i32>) {
        // Shortcut intercept — must come before any wire encoding.
        // A press that fires a shortcut is consumed, and so are its auto repeats
        // and its release; every other event passes through to the
        // encode/forward path.
        //
        // NOTE: Shortcut matching intentionally happens BEFORE any key swapping
        // (swap_modifier_key) so that shortcuts bind to the physical keys pressed,
        // not the swapped keys. This makes shortcut setup intuitive: users bind
        // shortcuts to the actual keys they press, regardless of swap settings.
        // Key swapping only affects what gets sent to the remote.
        //
        // Gated on `feature = "flutter"` because the dispatch target
        // (`flutter::push_session_event`) is Flutter-only. Sciter builds never
        // call `reload_from_config`, so the cache stays disabled and the
        // matcher would no-op anyway — but we still skip the call entirely so
        // a hand-edited config can't silently swallow keys on a UI that has
        // no way to surface the action.
        //
        // `None` for session_id makes the helper resolve through
        // `flutter::get_cur_session_id()` — the rdev grab loop is process-wide
        // and has no per-event session context to thread.
        #[cfg(feature = "flutter")]
        if crate::keyboard::shortcuts::try_dispatch(
            None,
            event,
            keyboard_mode,
            || get_peer_platform().to_lowercase(),
            send_key_event,
        ) {
            return;
        }

        let keyboard_mode = get_keyboard_mode_enum(keyboard_mode);
        if is_long_press(&event) {
            return;
        }
        let peer = get_peer_platform().to_lowercase();
        for key_event in event_to_key_events(peer, &event, keyboard_mode, lock_modes) {
            send_key_event(&key_event);
        }
    }

    pub fn process_event_with_session<T: InvokeUiSession>(
        keyboard_mode: &str,
        event: &Event,
        lock_modes: Option<i32>,
        session: &Session<T>,
        session_id: SessionID,
    ) {
        // Shortcut intercept — see the long comment in `process_event` above
        // for the consume / feature-gate rationale. The only difference
        // here is that the Flutter FFI path threads an explicit SessionID
        // through, so dispatch targets the exact tab the keystroke originated
        // from — no dependency on the global focus tracker.
        #[cfg(feature = "flutter")]
        if crate::keyboard::shortcuts::try_dispatch(
            Some(&session_id),
            event,
            keyboard_mode,
            || session.peer_platform().to_lowercase(),
            |key_event| session.send_key_event(key_event),
        ) {
            return;
        }
        #[cfg(not(feature = "flutter"))]
        let _ = session_id;

        let keyboard_mode = get_keyboard_mode_enum(keyboard_mode);
        if is_long_press(&event) {
            return;
        }
        let peer = session.peer_platform().to_lowercase();
        for key_event in event_to_key_events(peer, &event, keyboard_mode, lock_modes) {
            session.send_key_event(&key_event);
        }
    }

    pub fn get_modifiers_state(
        alt: bool,
        ctrl: bool,
        shift: bool,
        command: bool,
    ) -> (bool, bool, bool, bool) {
        let modifiers_lock = MODIFIERS_STATE.lock().unwrap();
        let ctrl = *modifiers_lock.get(&Key::ControlLeft).unwrap()
            || *modifiers_lock.get(&Key::ControlRight).unwrap()
            || ctrl;
        let shift = *modifiers_lock.get(&Key::ShiftLeft).unwrap()
            || *modifiers_lock.get(&Key::ShiftRight).unwrap()
            || shift;
        let command = *modifiers_lock.get(&Key::MetaLeft).unwrap()
            || *modifiers_lock.get(&Key::MetaRight).unwrap()
            || command;
        let alt = *modifiers_lock.get(&Key::Alt).unwrap()
            || *modifiers_lock.get(&Key::AltGr).unwrap()
            || alt;

        (alt, ctrl, shift, command)
    }

    pub fn legacy_modifiers(
        key_event: &mut KeyEvent,
        alt: bool,
        ctrl: bool,
        shift: bool,
        command: bool,
    ) {
        if alt
            && !crate::is_control_key(&key_event, &ControlKey::Alt)
            && !crate::is_control_key(&key_event, &ControlKey::RAlt)
        {
            key_event.modifiers.push(ControlKey::Alt.into());
        }
        if shift
            && !crate::is_control_key(&key_event, &ControlKey::Shift)
            && !crate::is_control_key(&key_event, &ControlKey::RShift)
        {
            key_event.modifiers.push(ControlKey::Shift.into());
        }
        if ctrl
            && !crate::is_control_key(&key_event, &ControlKey::Control)
            && !crate::is_control_key(&key_event, &ControlKey::RControl)
        {
            key_event.modifiers.push(ControlKey::Control.into());
        }
        if command
            && !crate::is_control_key(&key_event, &ControlKey::Meta)
            && !crate::is_control_key(&key_event, &ControlKey::RWin)
        {
            key_event.modifiers.push(ControlKey::Meta.into());
        }
    }

    #[cfg(target_os = "android")]
    pub fn map_key_to_control_key(key: &rdev::Key) -> Option<ControlKey> {
        match key {
            Key::Alt => Some(ControlKey::Alt),
            Key::ShiftLeft => Some(ControlKey::Shift),
            Key::ControlLeft => Some(ControlKey::Control),
            Key::MetaLeft => Some(ControlKey::Meta),
            Key::AltGr => Some(ControlKey::RAlt),
            Key::ShiftRight => Some(ControlKey::RShift),
            Key::ControlRight => Some(ControlKey::RControl),
            Key::MetaRight => Some(ControlKey::RWin),
            _ => None,
        }
    }

    pub fn event_lock_screen() -> KeyEvent {
        let mut key_event = KeyEvent::new();
        key_event.set_control_key(ControlKey::LockScreen);
        key_event.down = true;
        key_event.mode = KeyboardMode::Legacy.into();
        key_event
    }

    #[inline]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn lock_screen() {
        send_key_event(&event_lock_screen());
    }

    pub fn event_ctrl_alt_del(peer_platform: &str) -> KeyEvent {
        let mut key_event = KeyEvent::new();
        if peer_platform == "Windows" {
            key_event.set_control_key(ControlKey::CtrlAltDel);
            key_event.down = true;
        } else {
            key_event.set_control_key(ControlKey::Delete);
            legacy_modifiers(&mut key_event, true, true, false, false);
            key_event.press = true;
        }
        key_event.mode = KeyboardMode::Legacy.into();
        key_event
    }

    #[inline]
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn ctrl_alt_del() {
        send_key_event(&event_ctrl_alt_del(&get_peer_platform()));
    }
}

#[cfg(windows)]
pub fn update_grab_get_key_name(keyboard_mode: &str) {
    match keyboard_mode {
        "map" => rdev::set_get_key_unicode(false),
        "translate" => rdev::set_get_key_unicode(true),
        "legacy" => rdev::set_get_key_unicode(true),
        _ => {}
    };
}

#[cfg(target_os = "windows")]
static mut IS_0X021D_DOWN: bool = false;

#[cfg(target_os = "macos")]
static mut IS_LEFT_OPTION_DOWN: bool = false;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn get_keyboard_mode() -> String {
    #[cfg(feature = "flutter")]
    if let Some(session) = flutter::get_cur_session() {
        return session.get_keyboard_mode();
    }
    "legacy".to_string()
}

/// Check if exit shortcut for relative mouse mode is active.
/// Exit shortcuts (only exits, not toggles):
/// - macOS: Cmd+G
/// - Windows/Linux: Ctrl+Alt (triggered when both are pressed)
/// Note: This shortcut is only available in Flutter client. Sciter client does not support relative mouse mode.
#[cfg(feature = "flutter")]
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn is_exit_relative_mouse_shortcut(key: Key) -> bool {
    let modifiers = MODIFIERS_STATE.lock().unwrap();

    #[cfg(target_os = "macos")]
    {
        // macOS: Cmd+G to exit
        if key != Key::KeyG {
            return false;
        }
        let meta = *modifiers.get(&Key::MetaLeft).unwrap_or(&false)
            || *modifiers.get(&Key::MetaRight).unwrap_or(&false);
        return meta;
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Windows/Linux: Ctrl+Alt to exit
        // Triggered when Ctrl is pressed while Alt is down, or Alt is pressed while Ctrl is down
        let is_ctrl_key = key == Key::ControlLeft || key == Key::ControlRight;
        let is_alt_key = key == Key::Alt || key == Key::AltGr;

        if !is_ctrl_key && !is_alt_key {
            return false;
        }

        let ctrl = *modifiers.get(&Key::ControlLeft).unwrap_or(&false)
            || *modifiers.get(&Key::ControlRight).unwrap_or(&false);
        let alt = *modifiers.get(&Key::Alt).unwrap_or(&false)
            || *modifiers.get(&Key::AltGr).unwrap_or(&false);

        // When Ctrl is pressed and Alt is already down, or vice versa
        (is_ctrl_key && alt) || (is_alt_key && ctrl)
    }
}

/// Notify Flutter to exit relative mouse mode.
/// Note: This is Flutter-only. Sciter client does not support relative mouse mode.
#[cfg(feature = "flutter")]
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
fn notify_exit_relative_mouse_mode() {
    let session_id = flutter::get_cur_session_id();
    flutter::push_session_event(&session_id, "exit_relative_mouse_mode", vec![]);
}

/// Handle relative mouse mode shortcuts in the rdev grab loop.
/// Returns true if the event should be blocked from being sent to the peer.
#[cfg(feature = "flutter")]
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
#[inline]
fn can_exit_relative_mouse_mode_from_grab_loop() -> bool {
    // Only process exit shortcuts when relative mouse mode is actually active.
    // This prevents blocking Ctrl+Alt (or Cmd+G) when not in relative mouse mode.
    if !RELATIVE_MOUSE_MODE_ACTIVE.load(Ordering::SeqCst) {
        return false;
    }

    let Some(session) = flutter::get_cur_session() else {
        return false;
    };

    // Only for remote desktop sessions.
    if !session.is_default() {
        return false;
    }

    // Must have keyboard permission and not be in view-only mode.
    if !*session.server_keyboard_enabled.read().unwrap() {
        return false;
    }
    let lc = session.lc.read().unwrap();
    if lc.view_only.v {
        return false;
    }

    // Peer must support relative mouse mode.
    crate::common::is_support_relative_mouse_mode_num(lc.version)
}

#[cfg(feature = "flutter")]
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
#[inline]
fn should_block_relative_mouse_shortcut(key: Key, is_press: bool) -> bool {
    if !KEYBOARD_HOOKED.load(Ordering::SeqCst) {
        return false;
    }

    // Determine which key to track for key-up blocking based on platform
    #[cfg(target_os = "macos")]
    let is_tracked_key = key == Key::KeyG;
    #[cfg(not(target_os = "macos"))]
    let is_tracked_key = key == Key::ControlLeft
        || key == Key::ControlRight
        || key == Key::Alt
        || key == Key::AltGr;

    // Block key up if key down was blocked (to avoid orphan key up event on remote).
    // This must be checked before clearing the flag below.
    if is_tracked_key && !is_press && EXIT_SHORTCUT_KEY_DOWN.swap(false, Ordering::SeqCst) {
        return true;
    }

    // Exit relative mouse mode shortcuts:
    // - macOS: Cmd+G
    // - Windows/Linux: Ctrl+Alt
    // Guard it to supported/eligible sessions to avoid blocking the chord unexpectedly.
    if is_exit_relative_mouse_shortcut(key) {
        if !can_exit_relative_mouse_mode_from_grab_loop() {
            return false;
        }
        if is_press {
            // Only trigger exit on transition from "not pressed" to "pressed".
            // This prevents retriggering on OS key-repeat.
            if !EXIT_SHORTCUT_KEY_DOWN.swap(true, Ordering::SeqCst) {
                notify_exit_relative_mouse_mode();
            }
        }
        return true;
    }

    false
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn start_grab_loop() {
    std::env::set_var("KEYBOARD_ONLY", "y");
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    std::thread::spawn(move || {
        let try_handle_keyboard = move |event: Event, key: Key, is_press: bool| -> Option<Event> {
            // fix #2211：CAPS LOCK don't work
            if key == Key::CapsLock || key == Key::NumLock {
                return Some(event);
            }

            let _scan_code = event.position_code;
            let _code = event.platform_code as KeyCode;

            #[cfg(feature = "flutter")]
            if should_block_relative_mouse_shortcut(key, is_press) {
                return None;
            }

            let res = if KEYBOARD_HOOKED.load(Ordering::SeqCst) {
                client::process_event(&get_keyboard_mode(), &event, None);
                if is_press {
                    None
                } else {
                    Some(event)
                }
            } else {
                Some(event)
            };

            #[cfg(target_os = "windows")]
            match _scan_code {
                0x1D | 0x021D => rdev::set_modifier(Key::ControlLeft, is_press),
                0xE01D => rdev::set_modifier(Key::ControlRight, is_press),
                0x2A => rdev::set_modifier(Key::ShiftLeft, is_press),
                0x36 => rdev::set_modifier(Key::ShiftRight, is_press),
                0x38 => rdev::set_modifier(Key::Alt, is_press),
                // Right Alt
                0xE038 => rdev::set_modifier(Key::AltGr, is_press),
                0xE05B => rdev::set_modifier(Key::MetaLeft, is_press),
                0xE05C => rdev::set_modifier(Key::MetaRight, is_press),
                _ => {}
            }

            #[cfg(target_os = "windows")]
            unsafe {
                // AltGr
                if _scan_code == 0x021D {
                    IS_0X021D_DOWN = is_press;
                }
            }

            #[cfg(target_os = "macos")]
            unsafe {
                if _code == rdev::kVK_Option {
                    IS_LEFT_OPTION_DOWN = is_press;
                }
            }

            return res;
        };
        let func = move |event: Event| match event.event_type {
            EventType::KeyPress(key) => try_handle_keyboard(event, key, true),
            EventType::KeyRelease(key) => try_handle_keyboard(event, key, false),
            _ => Some(event),
        };
        #[cfg(target_os = "macos")]
        rdev::set_is_main_thread(false);
        #[cfg(target_os = "windows")]
        rdev::set_event_popup(false);
        if let Err(error) = rdev::grab(func) {
            log::error!("rdev Error: {:?}", error)
        }
    });

    ;
}

// #[allow(dead_code)] is ok here. No need to stop grabbing loop.
#[allow(dead_code)]
#[cfg(any(target_os = "windows", target_os = "macos"))]
fn stop_grab_loop() -> Result<(), rdev::GrabError> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    rdev::exit_grab()?;

    Ok(())
}

pub fn is_long_press(event: &Event) -> bool {
    let keys = MODIFIERS_STATE.lock().unwrap();
    match event.event_type {
        EventType::KeyPress(k) => {
            if let Some(&state) = keys.get(&k) {
                if state == true {
                    return true;
                }
            }
        }
        _ => {}
    };
    return false;
}

fn take_remote_keys() -> HashMap<Key, (Event, Option<i32>)> {
    let mut to_release = TO_RELEASE.lock().unwrap();
    std::mem::take(&mut *to_release)
}

fn release_remote_keys_for_events(keyboard_mode: &str, to_release: HashMap<Key, (Event, Option<i32>)>) {
    for (key, (mut event, lock_modes)) in to_release.into_iter() {
        #[cfg(feature = "flutter")]
        if get_peer_platform() == "Windows" && shortcuts::release_modifier_on_leave(key) {
            continue;
        }
        event.event_type = EventType::KeyRelease(key);
        client::process_event(keyboard_mode, &event, lock_modes);
        // If Alt or AltGr is pressed, we need to send another key stoke to release it.
        // Because the controlled side may hold the alt state, if local window is switched by [Alt + Tab].
        if key == Key::Alt || key == Key::AltGr {
            event.event_type = EventType::KeyPress(key);
            client::process_event(keyboard_mode, &event, lock_modes);
            event.event_type = EventType::KeyRelease(key);
            client::process_event(keyboard_mode, &event, lock_modes);
        }
    }
}

#[allow(dead_code)]
pub fn release_remote_keys(keyboard_mode: &str) {
    // todo!: client quit suddenly, how to release keys?
    release_remote_keys_for_events(keyboard_mode, take_remote_keys());
}

pub fn get_keyboard_mode_enum(keyboard_mode: &str) -> KeyboardMode {
    match keyboard_mode {
        "map" => KeyboardMode::Map,
        "translate" => KeyboardMode::Translate,
        "legacy" => KeyboardMode::Legacy,
        _ => KeyboardMode::Map,
    }
}

#[inline]
pub fn is_modifier(key: &rdev::Key) -> bool {
    matches!(
        key,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::MetaLeft
            | Key::MetaRight
            | Key::Alt
            | Key::AltGr
    )
}

#[inline]
#[allow(dead_code)]
pub fn is_modifier_code(evt: &KeyEvent) -> bool {
    match evt.union {
        Some(key_event::Union::Chr(code)) => {
            let key = rdev::linux_key_from_code(code);
            is_modifier(&key)
        }
        _ => false,
    }
}

#[inline]
pub fn is_numpad_rdev_key(key: &rdev::Key) -> bool {
    matches!(
        key,
        Key::Kp0
            | Key::Kp1
            | Key::Kp2
            | Key::Kp3
            | Key::Kp4
            | Key::Kp5
            | Key::Kp6
            | Key::Kp7
            | Key::Kp8
            | Key::Kp9
            | Key::KpMinus
            | Key::KpMultiply
            | Key::KpDivide
            | Key::KpPlus
            | Key::KpDecimal
    )
}

#[inline]
pub fn is_letter_rdev_key(key: &rdev::Key) -> bool {
    matches!(
        key,
        Key::KeyA
            | Key::KeyB
            | Key::KeyC
            | Key::KeyD
            | Key::KeyE
            | Key::KeyF
            | Key::KeyG
            | Key::KeyH
            | Key::KeyI
            | Key::KeyJ
            | Key::KeyK
            | Key::KeyL
            | Key::KeyM
            | Key::KeyN
            | Key::KeyO
            | Key::KeyP
            | Key::KeyQ
            | Key::KeyR
            | Key::KeyS
            | Key::KeyT
            | Key::KeyU
            | Key::KeyV
            | Key::KeyW
            | Key::KeyX
            | Key::KeyY
            | Key::KeyZ
    )
}

// https://github.com/rustdesk/rustdesk/issues/8599
// We just add these keys as letter keys.
#[inline]
pub fn is_letter_rdev_key_ex(key: &rdev::Key) -> bool {
    matches!(
        key,
        Key::LeftBracket | Key::RightBracket | Key::SemiColon | Key::Quote | Key::Comma | Key::Dot
    )
}

#[inline]
fn is_numpad_key(event: &Event) -> bool {
    matches!(event.event_type, EventType::KeyPress(key) | EventType::KeyRelease(key) if is_numpad_rdev_key(&key))
}

// Check is letter key for lock modes.
// Only letter keys need to check and send Lock key state.
#[inline]
fn is_letter_key_4_lock_modes(event: &Event) -> bool {
    matches!(event.event_type, EventType::KeyPress(key) | EventType::KeyRelease(key) if (is_letter_rdev_key(&key) || is_letter_rdev_key_ex(&key)))
}

fn parse_add_lock_modes_modifiers(
    key_event: &mut KeyEvent,
    lock_modes: i32,
    is_numpad_key: bool,
    is_letter_key: bool,
) {
    const CAPS_LOCK: i32 = 1;
    const NUM_LOCK: i32 = 2;
    // const SCROLL_LOCK: i32 = 3;
    if is_letter_key && (lock_modes & (1 << CAPS_LOCK) != 0) {
        key_event.modifiers.push(ControlKey::CapsLock.into());
    }
    if is_numpad_key && lock_modes & (1 << NUM_LOCK) != 0 {
        key_event.modifiers.push(ControlKey::NumLock.into());
    }
    // if lock_modes & (1 << SCROLL_LOCK) != 0 {
    //     key_event.modifiers.push(ControlKey::ScrollLock.into());
    // }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[derive(Clone, Copy)]
enum ModifierKey {
    Alt, Control, RightControl, Shift, RightShift, Meta,
}

// On Wayland the focused Flutter input stream, not an injection device, owns
// modifier state. Native hook platforms retain their existing read-only query.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn get_key_state(key: ModifierKey) -> bool {
    let query = match key {
        ModifierKey::Alt => QueryKey::Alt,
        ModifierKey::Control => QueryKey::Control,
        ModifierKey::RightControl => QueryKey::RightControl,
        ModifierKey::Shift => QueryKey::Shift,
        ModifierKey::RightShift => QueryKey::RightShift,
        ModifierKey::Meta => QueryKey::Meta,
    };
    if let Some(pressed) = native_key_state(query) {
        return pressed;
    }
    let keys = MODIFIERS_STATE.lock().unwrap();
    let pressed = |key| keys.get(&key).copied().unwrap_or(false);
    match key {
        ModifierKey::Alt => pressed(Key::Alt) || pressed(Key::AltGr),
        ModifierKey::Control => pressed(Key::ControlLeft),
        ModifierKey::RightControl => pressed(Key::ControlRight),
        ModifierKey::Shift => pressed(Key::ShiftLeft),
        ModifierKey::RightShift => pressed(Key::ShiftRight),
        ModifierKey::Meta => pressed(Key::MetaLeft) || pressed(Key::MetaRight),
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn add_lock_modes_modifiers(key_event: &mut KeyEvent, is_numpad_key: bool, is_letter_key: bool) {
    if is_letter_key && native_key_state(QueryKey::CapsLock) == Some(true) {
        key_event.modifiers.push(ControlKey::CapsLock.into());
    }
    if is_numpad_key && native_key_state(QueryKey::NumLock) == Some(true) {
        key_event.modifiers.push(ControlKey::NumLock.into());
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn convert_numpad_keys(key: Key, num_lock: Option<bool>) -> Key {
    // Missing compositor state must not turn a synthetic release into a navigation key.
    if num_lock != Some(false) {
        return key;
    }
    match key {
        Key::Kp0 => Key::Insert,
        Key::KpDecimal => Key::Delete,
        Key::Kp1 => Key::End,
        Key::Kp2 => Key::DownArrow,
        Key::Kp3 => Key::PageDown,
        Key::Kp4 => Key::LeftArrow,
        Key::Kp5 => Key::Clear,
        Key::Kp6 => Key::RightArrow,
        Key::Kp7 => Key::Home,
        Key::Kp8 => Key::UpArrow,
        Key::Kp9 => Key::PageUp,
        _ => key,
    }
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod numpad_state_tests {
    use super::*;

    #[test]
    fn only_known_num_lock_off_maps_to_navigation() {
        assert_eq!(convert_numpad_keys(Key::Kp1, Some(false)), Key::End);
        assert_eq!(convert_numpad_keys(Key::Kp1, Some(true)), Key::Kp1);
        assert_eq!(convert_numpad_keys(Key::Kp1, None), Key::Kp1);
        assert_eq!(convert_numpad_keys(Key::KeyA, Some(false)), Key::KeyA);
    }

    fn keypad_event(down: bool) -> Event {
        Event {
            time: std::time::SystemTime::now(),
            unicode: None,
            platform_code: 0,
            position_code: 0,
            event_type: if down { EventType::KeyPress(Key::Kp1) } else { EventType::KeyRelease(Key::Kp1) },
            usb_hid: 0,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            extra_data: 0,
        }
    }

    #[test]
    fn focus_loss_releases_the_same_num_lock_off_key() {
        let mut pending = HashMap::new();
        let down_modes = track_pending_release(&mut pending, &keypad_event(true), Some(0), true);
        let down_key = convert_numpad_keys(Key::Kp1, state::num_lock(down_modes));
        assert_eq!(down_key, Key::End);
        // Mirror take_remote_keys and release_remote_keys_for_events: the
        // focus-loss callback has no new snapshot, so carry the press's own.
        let taken = std::mem::take(&mut pending);
        let (mut event, modes) = taken.get(&Key::Kp1).unwrap().clone();
        event.event_type = EventType::KeyRelease(Key::Kp1);
        let up_modes = track_pending_release(&mut pending, &event, modes, true);
        assert_eq!(convert_numpad_keys(Key::Kp1, state::num_lock(up_modes)), down_key);
        assert!(pending.is_empty());
    }

    #[test]
    fn other_keys_and_modes_keep_current_lock_state() {
        let mut pending = HashMap::new();
        // Map-mode keypad events must not retain the legacy navigation policy.
        track_pending_release(&mut pending, &keypad_event(true), Some(0), false);
        assert_eq!(track_pending_release(&mut pending, &keypad_event(true), Some(4), false), Some(4));
        // Nor should ordinary letters freeze CapsLock across repeat events.
        let mut letter = keypad_event(true);
        letter.event_type = EventType::KeyPress(Key::KeyA);
        track_pending_release(&mut pending, &letter, Some(0), false);
        assert_eq!(track_pending_release(&mut pending, &letter, Some(2), false), Some(2));
        // Even when pairing NumLock, preserve this event's CapsLock bit.
        assert_eq!(track_pending_release(&mut pending, &keypad_event(true), Some(2), true), Some(2));
        assert_eq!(track_pending_release(&mut pending, &keypad_event(true), Some(4), true), Some(0));
    }

    #[test]
    fn repeats_and_lock_changes_preserve_the_press_mapping() {
        let mut pending = HashMap::new();
        track_pending_release(&mut pending, &keypad_event(true), Some(0), true);
        assert_eq!(track_pending_release(&mut pending, &keypad_event(true), Some(4), true), Some(0));
        let modes = track_pending_release(&mut pending, &keypad_event(false), Some(4), true);
        assert_eq!(convert_numpad_keys(Key::Kp1, state::num_lock(modes)), Key::End);
        assert!(pending.is_empty());
    }
}

fn update_modifiers_state(event: &Event) {
    // for mouse
    let mut keys = MODIFIERS_STATE.lock().unwrap();
    match event.event_type {
        EventType::KeyPress(k) => {
            if keys.contains_key(&k) {
                keys.insert(k, true);
            }
        }
        EventType::KeyRelease(k) => {
            if keys.contains_key(&k) {
                keys.insert(k, false);
            }
        }
        _ => {}
    };
}

fn track_pending_release(
    pending: &mut HashMap<Key, (Event, Option<i32>)>,
    event: &Event,
    lock_modes: Option<i32>,
    preserve_num_lock: bool,
) -> Option<i32> {
    const NUM_LOCK: i32 = 1 << 2;
    let recorded = match event.event_type {
        EventType::KeyPress(key) => {
            let previous = pending.get(&key).and_then(|(_, modes)| *modes);
            let modes = if preserve_num_lock {
                previous.or(lock_modes.map(|modes| modes & NUM_LOCK))
            } else {
                None
            };
            // Preserve the original latest-event behavior for every key.
            pending.insert(key, (event.clone(), modes));
            modes
        }
        EventType::KeyRelease(key) => {
            pending.remove(&key).and_then(|(_, modes)| modes)
        }
        _ => None,
    };
    if preserve_num_lock {
        if let Some(recorded) = recorded {
            // Only the NumLock bit controls the paired navigation mapping.
            // CapsLock and all other state still come from the current event.
            return Some((lock_modes.unwrap_or(0) & !NUM_LOCK) | recorded);
        }
    }
    lock_modes
}

pub fn event_to_key_events(
    mut peer: String,
    event: &Event,
    keyboard_mode: KeyboardMode,
    lock_modes: Option<i32>,
) -> Vec<KeyEvent> {
    peer.retain(|c| !c.is_whitespace());

    update_modifiers_state(event);

    // Only legacy Windows-peer keypad navigation needs a paired NumLock
    // decision. Other keys and modes keep their current-event lock state.
    let preserve_num_lock = keyboard_mode == KeyboardMode::Legacy
        && peer.eq_ignore_ascii_case("windows") && is_numpad_key(event);
    let lock_modes = track_pending_release(
        &mut TO_RELEASE.lock().unwrap(), event, lock_modes, preserve_num_lock,
    );

    let mut key_event = KeyEvent::new();
    key_event.mode = keyboard_mode.into();

    let mut key_events = match keyboard_mode {
        KeyboardMode::Map => map_keyboard_mode(peer.as_str(), event, key_event),
        KeyboardMode::Translate => translate_keyboard_mode(peer.as_str(), event, key_event),
        _ => {
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                legacy_keyboard_mode(event, key_event, state::num_lock(lock_modes))
            }
            #[cfg(any(target_os = "android", target_os = "ios"))]
            {
                Vec::new()
            }
        }
    };

    let is_numpad_key = is_numpad_key(&event);
    if keyboard_mode != KeyboardMode::Translate || is_numpad_key {
        let is_letter_key = is_letter_key_4_lock_modes(&event);
        for key_event in &mut key_events {
            if let Some(lock_modes) = lock_modes {
                parse_add_lock_modes_modifiers(key_event, lock_modes, is_numpad_key, is_letter_key);
            } else {
                #[cfg(not(any(target_os = "android", target_os = "ios")))]
                add_lock_modes_modifiers(key_event, is_numpad_key, is_letter_key);
            }
        }
    }
    key_events
}

pub fn send_key_event(key_event: &KeyEvent) {
    #[cfg(feature = "flutter")]
    if let Some(session) = flutter::get_cur_session() {
        session.send_key_event(key_event);
    }
}

pub fn get_peer_platform() -> String {
    #[cfg(feature = "flutter")]
    if let Some(session) = flutter::get_cur_session() {
        return session.peer_platform();
    }
    "Windows".to_string()
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn legacy_keyboard_mode(
    event: &Event,
    mut key_event: KeyEvent,
    num_lock: Option<bool>,
) -> Vec<KeyEvent> {
    let mut events = Vec::new();
    // legacy mode(0): Generate characters locally, look for keycode on other side.
    let (mut key, down_or_up) = match event.event_type {
        EventType::KeyPress(key) => (key, true),
        EventType::KeyRelease(key) => (key, false),
        _ => {
            return events;
        }
    };

    let peer = get_peer_platform();
    let is_win = peer == "Windows";
    if is_win {
        key = convert_numpad_keys(key, num_lock);
    }

    let alt = get_key_state(ModifierKey::Alt);
    #[cfg(windows)]
    let ctrl = {
        let mut tmp = get_key_state(ModifierKey::Control) || get_key_state(ModifierKey::RightControl);
        unsafe {
            if IS_ALT_GR {
                if alt || key == Key::AltGr {
                    if tmp {
                        tmp = false;
                    }
                } else {
                    IS_ALT_GR = false;
                }
            }
        }
        tmp
    };
    #[cfg(not(windows))]
    let ctrl = get_key_state(ModifierKey::Control) || get_key_state(ModifierKey::RightControl);
    let shift = get_key_state(ModifierKey::Shift) || get_key_state(ModifierKey::RightShift);
    #[cfg(windows)]
    let command = crate::platform::windows::get_win_key_state();
    #[cfg(not(windows))]
    let command = get_key_state(ModifierKey::Meta);
    let control_key = match key {
        Key::Alt => Some(ControlKey::Alt),
        Key::AltGr => Some(ControlKey::RAlt),
        Key::Backspace => Some(ControlKey::Backspace),
        Key::ControlLeft => {
            // when pressing AltGr, an extra VK_LCONTROL with a special
            // scancode with bit 9 set is sent, let's ignore this.
            #[cfg(windows)]
            if (event.position_code >> 8) == 0xE0 {
                unsafe {
                    IS_ALT_GR = true;
                }
                return events;
            }
            Some(ControlKey::Control)
        }
        Key::ControlRight => Some(ControlKey::RControl),
        Key::DownArrow => Some(ControlKey::DownArrow),
        Key::Escape => Some(ControlKey::Escape),
        Key::F1 => Some(ControlKey::F1),
        Key::F10 => Some(ControlKey::F10),
        Key::F11 => Some(ControlKey::F11),
        Key::F12 => Some(ControlKey::F12),
        Key::F2 => Some(ControlKey::F2),
        Key::F3 => Some(ControlKey::F3),
        Key::F4 => Some(ControlKey::F4),
        Key::F5 => Some(ControlKey::F5),
        Key::F6 => Some(ControlKey::F6),
        Key::F7 => Some(ControlKey::F7),
        Key::F8 => Some(ControlKey::F8),
        Key::F9 => Some(ControlKey::F9),
        Key::LeftArrow => Some(ControlKey::LeftArrow),
        Key::MetaLeft => Some(ControlKey::Meta),
        Key::MetaRight => Some(ControlKey::RWin),
        Key::Return => Some(ControlKey::Return),
        Key::RightArrow => Some(ControlKey::RightArrow),
        Key::ShiftLeft => Some(ControlKey::Shift),
        Key::ShiftRight => Some(ControlKey::RShift),
        Key::Space => Some(ControlKey::Space),
        Key::Tab => Some(ControlKey::Tab),
        Key::UpArrow => Some(ControlKey::UpArrow),
        Key::Delete => {
            if is_win && ctrl && alt {
                client::ctrl_alt_del();
                return events;
            }
            Some(ControlKey::Delete)
        }
        Key::Apps => Some(ControlKey::Apps),
        Key::Cancel => Some(ControlKey::Cancel),
        Key::Clear => Some(ControlKey::Clear),
        Key::Kana => Some(ControlKey::Kana),
        Key::Hangul => Some(ControlKey::Hangul),
        Key::Junja => Some(ControlKey::Junja),
        Key::Final => Some(ControlKey::Final),
        Key::Hanja => Some(ControlKey::Hanja),
        Key::Hanji => Some(ControlKey::Hanja),
        Key::Lang2 => Some(ControlKey::Convert),
        Key::Print => Some(ControlKey::Print),
        Key::Select => Some(ControlKey::Select),
        Key::Execute => Some(ControlKey::Execute),
        Key::PrintScreen => Some(ControlKey::Snapshot),
        Key::Help => Some(ControlKey::Help),
        Key::Sleep => Some(ControlKey::Sleep),
        Key::Separator => Some(ControlKey::Separator),
        Key::KpReturn => Some(ControlKey::NumpadEnter),
        Key::Kp0 => Some(ControlKey::Numpad0),
        Key::Kp1 => Some(ControlKey::Numpad1),
        Key::Kp2 => Some(ControlKey::Numpad2),
        Key::Kp3 => Some(ControlKey::Numpad3),
        Key::Kp4 => Some(ControlKey::Numpad4),
        Key::Kp5 => Some(ControlKey::Numpad5),
        Key::Kp6 => Some(ControlKey::Numpad6),
        Key::Kp7 => Some(ControlKey::Numpad7),
        Key::Kp8 => Some(ControlKey::Numpad8),
        Key::Kp9 => Some(ControlKey::Numpad9),
        Key::KpDivide => Some(ControlKey::Divide),
        Key::KpMultiply => Some(ControlKey::Multiply),
        Key::KpDecimal => Some(ControlKey::Decimal),
        Key::KpMinus => Some(ControlKey::Subtract),
        Key::KpPlus => Some(ControlKey::Add),
        Key::CapsLock | Key::NumLock | Key::ScrollLock => {
            return events;
        }
        Key::Home => Some(ControlKey::Home),
        Key::End => Some(ControlKey::End),
        Key::Insert => Some(ControlKey::Insert),
        Key::PageUp => Some(ControlKey::PageUp),
        Key::PageDown => Some(ControlKey::PageDown),
        Key::Pause => Some(ControlKey::Pause),
        _ => None,
    };
    if let Some(k) = control_key {
        key_event.set_control_key(k);
    } else {
        let name = event
            .unicode
            .as_ref()
            .and_then(|unicode| unicode.name.clone());
        let mut chr = match &name {
            Some(ref s) => {
                if s.len() <= 2 {
                    // exclude chinese characters
                    s.chars().next().unwrap_or('\0')
                } else {
                    '\0'
                }
            }
            _ => '\0',
        };
        if chr == '·' {
            // special for Chinese
            chr = '`';
        }
        if chr == '\0' {
            chr = match key {
                Key::Num1 => '1',
                Key::Num2 => '2',
                Key::Num3 => '3',
                Key::Num4 => '4',
                Key::Num5 => '5',
                Key::Num6 => '6',
                Key::Num7 => '7',
                Key::Num8 => '8',
                Key::Num9 => '9',
                Key::Num0 => '0',
                Key::KeyA => 'a',
                Key::KeyB => 'b',
                Key::KeyC => 'c',
                Key::KeyD => 'd',
                Key::KeyE => 'e',
                Key::KeyF => 'f',
                Key::KeyG => 'g',
                Key::KeyH => 'h',
                Key::KeyI => 'i',
                Key::KeyJ => 'j',
                Key::KeyK => 'k',
                Key::KeyL => 'l',
                Key::KeyM => 'm',
                Key::KeyN => 'n',
                Key::KeyO => 'o',
                Key::KeyP => 'p',
                Key::KeyQ => 'q',
                Key::KeyR => 'r',
                Key::KeyS => 's',
                Key::KeyT => 't',
                Key::KeyU => 'u',
                Key::KeyV => 'v',
                Key::KeyW => 'w',
                Key::KeyX => 'x',
                Key::KeyY => 'y',
                Key::KeyZ => 'z',
                Key::Comma => ',',
                Key::Dot => '.',
                Key::SemiColon => ';',
                Key::Quote => '\'',
                Key::LeftBracket => '[',
                Key::RightBracket => ']',
                Key::Slash => '/',
                Key::BackSlash => '\\',
                Key::Minus => '-',
                Key::Equal => '=',
                Key::BackQuote => '`',
                _ => '\0',
            }
        }
        if chr != '\0' {
            if chr == 'l' && is_win && command {
                client::lock_screen();
                return events;
            }
            key_event.set_chr(chr as _);
        } else {
            log::error!("Unknown key {:?}", &event);
            return events;
        }
    }
    let (alt, ctrl, shift, command) = client::get_modifiers_state(alt, ctrl, shift, command);
    client::legacy_modifiers(&mut key_event, alt, ctrl, shift, command);

    if down_or_up == true {
        key_event.down = true;
    }
    events.push(key_event);
    events
}

#[inline]
pub fn map_keyboard_mode(_peer: &str, event: &Event, key_event: KeyEvent) -> Vec<KeyEvent> {
    if let Some(evt) = windows_peer_special_key(_peer, event) {
        return vec![evt];
    }

    _map_keyboard_mode(_peer, event, key_event)
        .map(|e| vec![e])
        .unwrap_or_default()
}

fn windows_peer_special_key(peer: &str, event: &Event) -> Option<KeyEvent> {
    if peer != OS_LOWER_WINDOWS {
        return None;
    }

    let (key, down) = match event.event_type {
        EventType::KeyPress(key) => (key, true),
        EventType::KeyRelease(key) => (key, false),
        _ => return None,
    };

    // Handle only `Pause` for Windows peers for now.
    // Windows has no normal scan code for `Pause`, so send it as a legacy control key.
    #[cfg(target_os = "windows")]
    let is_pause = {
        // The Windows scan code can look like `NumLock`; VK_PAUSE distinguishes it.
        let pause_vk_code = rdev::win_code_from_key(Key::Pause);
        key == Key::Pause || pause_vk_code == Some(event.platform_code as _)
    };
    #[cfg(not(target_os = "windows"))]
    let is_pause = key == Key::Pause;
    if !is_pause {
        return None;
    }

    let mut key_event = KeyEvent::new();
    key_event.mode = KeyboardMode::Legacy.into();
    key_event.down = down;
    key_event.set_control_key(ControlKey::Pause);
    let (alt, ctrl, shift, command) = client::get_modifiers_state(false, false, false, false);
    client::legacy_modifiers(&mut key_event, alt, ctrl, shift, command);
    Some(key_event)
}

fn _map_keyboard_mode(_peer: &str, event: &Event, mut key_event: KeyEvent) -> Option<KeyEvent> {
    match event.event_type {
        EventType::KeyPress(..) => {
            key_event.down = true;
        }
        EventType::KeyRelease(..) => {
            key_event.down = false;
        }
        _ => return None,
    };

    #[cfg(target_os = "windows")]
    let keycode = match _peer {
        OS_LOWER_WINDOWS => {
            // https://github.com/rustdesk/rustdesk/issues/1371
            // Filter scancodes that are greater than 255 and the height word is not 0xE0.
            if event.position_code > 255 && (event.position_code >> 8) != 0xE0 {
                return None;
            }
            event.position_code
        }
        OS_LOWER_MACOS => {
            if hbb_common::config::LocalConfig::get_kb_layout_type() == "ISO" {
                rdev::win_scancode_to_macos_iso_code(event.position_code)?
            } else {
                rdev::win_scancode_to_macos_code(event.position_code)?
            }
        }
        OS_LOWER_ANDROID => rdev::win_scancode_to_android_key_code(event.position_code)?,
        _ => rdev::win_scancode_to_linux_code(event.position_code)?,
    };
    #[cfg(target_os = "macos")]
    let keycode = match _peer {
        OS_LOWER_WINDOWS => rdev::macos_code_to_win_scancode(event.platform_code as _)?,
        OS_LOWER_MACOS => event.platform_code as _,
        OS_LOWER_ANDROID => rdev::macos_code_to_android_key_code(event.platform_code as _)?,
        _ => rdev::macos_code_to_linux_code(event.platform_code as _)?,
    };
    #[cfg(target_os = "linux")]
    let keycode = match _peer {
        OS_LOWER_WINDOWS => rdev::linux_code_to_win_scancode(event.position_code as _)?,
        OS_LOWER_MACOS => {
            if hbb_common::config::LocalConfig::get_kb_layout_type() == "ISO" {
                rdev::linux_code_to_macos_iso_code(event.position_code as _)?
            } else {
                rdev::linux_code_to_macos_code(event.position_code as _)?
            }
        }
        OS_LOWER_ANDROID => rdev::linux_code_to_android_key_code(event.position_code as _)?,
        _ => event.position_code as _,
    };
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let keycode = match _peer {
        OS_LOWER_WINDOWS => rdev::usb_hid_code_to_win_scancode(event.usb_hid as _)?,
        OS_LOWER_LINUX => rdev::usb_hid_code_to_linux_code(event.usb_hid as _)?,
        OS_LOWER_MACOS => {
            if hbb_common::config::LocalConfig::get_kb_layout_type() == "ISO" {
                rdev::usb_hid_code_to_macos_iso_code(event.usb_hid as _)?
            } else {
                rdev::usb_hid_code_to_macos_code(event.usb_hid as _)?
            }
        }
        OS_LOWER_ANDROID => rdev::usb_hid_code_to_android_key_code(event.usb_hid as _)?,
        _ => event.usb_hid as _,
    };
    key_event.set_chr(keycode as _);
    Some(key_event)
}

#[cfg(not(any(target_os = "ios")))]
fn try_fill_unicode(_peer: &str, event: &Event, key_event: &KeyEvent, events: &mut Vec<KeyEvent>) {
    match &event.unicode {
        Some(unicode_info) => {
            if let Some(name) = &unicode_info.name {
                if name.len() > 0 {
                    let mut evt = key_event.clone();
                    evt.set_seq(name.to_string());
                    evt.down = true;
                    events.push(evt);
                }
            }
        }
        None =>
        {
            #[cfg(target_os = "windows")]
            if _peer == OS_LOWER_LINUX {
                if is_hot_key_modifiers_down() && unsafe { !IS_0X021D_DOWN } {
                    if let Some(chr) = get_char_from_vk(event.platform_code as u32) {
                        let mut evt = key_event.clone();
                        evt.set_seq(chr.to_string());
                        evt.down = true;
                        events.push(evt);
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn try_fill_win2win_hotkey(
    peer: &str,
    event: &Event,
    key_event: &KeyEvent,
    events: &mut Vec<KeyEvent>,
) {
    if peer == OS_LOWER_WINDOWS && is_hot_key_modifiers_down() && unsafe { !IS_0X021D_DOWN } {
        let mut down = false;
        let win2win_hotkey = match event.event_type {
            EventType::KeyPress(..) => {
                down = true;
                if let Some(unicode) = get_unicode_from_vk(event.platform_code as u32) {
                    Some((unicode as u32 & 0x0000FFFF) | (event.platform_code << 16))
                } else {
                    None
                }
            }
            EventType::KeyRelease(..) => Some(event.platform_code << 16),
            _ => None,
        };
        if let Some(code) = win2win_hotkey {
            let mut evt = key_event.clone();
            evt.set_win2win_hotkey(code);
            evt.down = down;
            events.push(evt);
        }
    }
}

#[cfg(target_os = "windows")]
fn is_hot_key_modifiers_down() -> bool {
    if rdev::get_modifier(Key::ControlLeft) || rdev::get_modifier(Key::ControlRight) {
        return true;
    }
    if rdev::get_modifier(Key::Alt) || rdev::get_modifier(Key::AltGr) {
        return true;
    }
    if rdev::get_modifier(Key::MetaLeft) || rdev::get_modifier(Key::MetaRight) {
        return true;
    }
    return false;
}

#[inline]
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn is_altgr(event: &Event) -> bool {
    #[cfg(target_os = "linux")]
    if event.platform_code == 0xFE03 {
        true
    } else {
        false
    }

    #[cfg(target_os = "windows")]
    if unsafe { IS_0X021D_DOWN } && event.position_code == 0xE038 {
        true
    } else {
        false
    }
}

#[inline]
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn is_press(event: &Event) -> bool {
    matches!(event.event_type, EventType::KeyPress(_))
}

// https://github.com/rustdesk/rustdesk/wiki/FAQ#keyboard-translation-modes
pub fn translate_keyboard_mode(peer: &str, event: &Event, key_event: KeyEvent) -> Vec<KeyEvent> {
    let mut events: Vec<KeyEvent> = Vec::new();

    if let Some(evt) = windows_peer_special_key(peer, event) {
        events.push(evt);
        return events;
    }

    if let Some(unicode_info) = &event.unicode {
        if unicode_info.is_dead {
            #[cfg(target_os = "macos")]
            if peer != OS_LOWER_MACOS && unsafe { IS_LEFT_OPTION_DOWN } {
                // try clear dead key state
                // rdev::clear_dead_key_state();
            } else {
                return events;
            }
            #[cfg(not(target_os = "macos"))]
            return events;
        }
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    if is_numpad_key(&event) {
        events.append(&mut map_keyboard_mode(peer, event, key_event));
        return events;
    }

    #[cfg(target_os = "macos")]
    // ignore right option key
    if event.platform_code == rdev::kVK_RightOption as u32 {
        return events;
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    if is_altgr(event) {
        return events;
    }

    #[cfg(target_os = "windows")]
    if event.position_code == 0x021D {
        return events;
    }

    #[cfg(target_os = "windows")]
    try_fill_win2win_hotkey(peer, event, &key_event, &mut events);

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    if events.is_empty() && is_press(event) {
        try_fill_unicode(peer, event, &key_event, &mut events);
    }

    // If AltGr is down, no need to send events other than unicode.
    #[cfg(target_os = "windows")]
    unsafe {
        if IS_0X021D_DOWN {
            return events;
        }
    }

    #[cfg(target_os = "macos")]
    if !unsafe { IS_LEFT_OPTION_DOWN } {
        try_fill_unicode(peer, event, &key_event, &mut events);
    }

    if events.is_empty() {
        events.append(&mut map_keyboard_mode(peer, event, key_event));
    }
    events
}

#[cfg(not(any(target_os = "ios")))]
pub fn keycode_to_rdev_key(keycode: u32) -> Key {
    #[cfg(target_os = "windows")]
    return rdev::win_key_from_scancode(keycode);
    #[cfg(any(target_os = "linux"))]
    return rdev::linux_key_from_code(keycode);
    #[cfg(any(target_os = "android"))]
    return rdev::android_key_from_code(keycode);
    #[cfg(target_os = "macos")]
    return rdev::macos_key_from_code(keycode.try_into().unwrap_or_default());
}

#[cfg(feature = "flutter")]
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub mod input_source {
    #[cfg(target_os = "macos")]
    use hbb_common::log;
    use hbb_common::SessionID;

    #[cfg(not(target_os = "linux"))]
    use crate::ui_interface::{get_local_option, set_local_option};

    pub const CONFIG_OPTION_INPUT_SOURCE: &str = "input-source";
    // rdev grab mode
    pub const CONFIG_INPUT_SOURCE_1: &str = "Input source 1";
    pub const CONFIG_INPUT_SOURCE_1_TIP: &str = "input_source_1_tip";
    // flutter grab mode
    pub const CONFIG_INPUT_SOURCE_2: &str = "Input source 2";
    pub const CONFIG_INPUT_SOURCE_2_TIP: &str = "input_source_2_tip";

    pub const CONFIG_INPUT_SOURCE_DEFAULT: &str = CONFIG_INPUT_SOURCE_1;

    #[cfg(target_os = "linux")]
    pub fn init_input_source() {}

    #[cfg(target_os = "linux")]
    pub fn change_input_source(_session_id: SessionID, input_source: String) {
        if input_source != CONFIG_INPUT_SOURCE_2 {
            hbb_common::log::warn!("Wayland supports only the Flutter input source");
        }
    }

    #[cfg(target_os = "linux")]
    pub fn get_cur_session_input_source() -> String {
        CONFIG_INPUT_SOURCE_2.to_string()
    }

    #[cfg(target_os = "linux")]
    pub fn get_supported_input_source() -> Vec<(String, String)> {
        vec![(
            CONFIG_INPUT_SOURCE_2.to_string(),
            CONFIG_INPUT_SOURCE_2_TIP.to_string(),
        )]
    }

    #[cfg(not(target_os = "linux"))]
    pub fn init_input_source() {
        #[cfg(target_os = "macos")]
        if !crate::platform::macos::is_can_input_monitoring(false) {
            log::error!("init_input_source, is_can_input_monitoring() false");
            set_local_option(
                CONFIG_OPTION_INPUT_SOURCE.to_string(),
                CONFIG_INPUT_SOURCE_2.to_string(),
            );
            return;
        }
        let cur_input_source = get_cur_session_input_source();
        if cur_input_source == CONFIG_INPUT_SOURCE_1 {
            super::IS_RDEV_ENABLED.store(true, super::Ordering::SeqCst);
        }
        super::client::start_grab_loop();
    }

    #[cfg(not(target_os = "linux"))]
    pub fn change_input_source(session_id: SessionID, input_source: String) {
        let cur_input_source = get_cur_session_input_source();
        if cur_input_source == input_source {
            return;
        }
        if input_source == CONFIG_INPUT_SOURCE_1 {
            #[cfg(target_os = "macos")]
            if !crate::platform::macos::is_can_input_monitoring(false) {
                log::error!("change_input_source, is_can_input_monitoring() false");
                return;
            }
            // It is ok to start grab loop multiple times.
            super::client::start_grab_loop();
            super::IS_RDEV_ENABLED.store(true, super::Ordering::SeqCst);
            crate::flutter_ffi::session_enter_or_leave(session_id, true);
        } else if input_source == CONFIG_INPUT_SOURCE_2 {
            // No need to stop grab loop.
            crate::flutter_ffi::session_enter_or_leave(session_id, false);
            super::IS_RDEV_ENABLED.store(false, super::Ordering::SeqCst);
        }
        set_local_option(CONFIG_OPTION_INPUT_SOURCE.to_string(), input_source);
    }

    #[inline]
    #[cfg(not(target_os = "linux"))]
    pub fn get_cur_session_input_source() -> String {
        let input_source = get_local_option(CONFIG_OPTION_INPUT_SOURCE.to_string());
        if input_source.is_empty() {
            CONFIG_INPUT_SOURCE_DEFAULT.to_string()
        } else {
            input_source
        }
    }

    #[inline]
    #[cfg(not(target_os = "linux"))]
    pub fn get_supported_input_source() -> Vec<(String, String)> {
        vec![
            (
                CONFIG_INPUT_SOURCE_1.to_string(),
                CONFIG_INPUT_SOURCE_1_TIP.to_string(),
            ),
            (
                CONFIG_INPUT_SOURCE_2.to_string(),
                CONFIG_INPUT_SOURCE_2_TIP.to_string(),
            ),
        ]
    }
}
