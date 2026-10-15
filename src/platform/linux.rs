use super::{gtk_sudo, CursorData, ResultType};
use desktop::Desktop;
pub use base::platform::linux::*;

#[cfg(feature = "drm")]
pub fn dispatch_wayland_display_probe() {
    use std::ffi::OsStr;

    if std::env::args_os().nth(1).as_deref() == Some(OsStr::new(WAYLAND_DISPLAY_PROBE_ARG)) {
        wayland_display_probe_child_main();
    }
}
use hbb_common::{
    allow_err,
    anyhow::anyhow,
    bail,
    config::Config,
    libc::{c_char, c_void},
    log,
    regex::Regex,
    users::{get_user_by_name, os::unix::UserExt},
};
use base::message_proto::{DisplayInfo, Resolution};
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Child, Command},
    string::String,
    sync::atomic::{AtomicBool, Ordering},
    sync::Arc,
    time::{Duration, Instant},
};
use terminfo::{capability as cap, Database};
use wallpaper;

pub const PA_SAMPLE_RATE: u32 = 48000;
static mut UNMODIFIED: bool = true;

#[derive(Clone, Debug)]
struct ActiveUserLookupCache {
    uid: String,
    username: String,
}

const INVALID_TERM_VALUES: [&str; 3] = ["", "unknown", "dumb"];
const SHELL_PROCESSES: [&str; 4] = ["bash", "zsh", "fish", "sh"];

// Terminal type constants
const TERM_XTERM_256COLOR: &str = "xterm-256color";
const TERM_SCREEN_256COLOR: &str = "screen-256color";
const TERM_XTERM: &str = "xterm";

#[cfg(feature = "drm")]
lazy_static::lazy_static! {
    /// Only for per-frame callers; see `is_login_screen_wayland_cached`.
    /// Own block because `#[cfg]` on one item inside a shared one breaks the macro.
    static ref IS_LOGIN_SCREEN_WAYLAND: bool = is_login_screen_wayland();
}

lazy_static::lazy_static! {
    static ref CACHED_TERM: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    static ref DATABASE_XTERM_256COLOR: Option<Database> = {
        match Database::from_name(TERM_XTERM_256COLOR) {
            Ok(database) => Some(database),
            Err(err) => {
                log::error!("Failed to initialize {} database: {}", TERM_XTERM_256COLOR, err);
                None
            }
        }
    };
    static ref ACTIVE_USER_LOOKUP_CACHE: std::sync::Mutex<Option<ActiveUserLookupCache>> =
        std::sync::Mutex::new(None);
    static ref GNOME_MONITOR_LAYOUT_MODE_CACHE: std::sync::Mutex<
        Option<(Instant, Option<GnomeMonitorLayoutMode>)>,
    > = Default::default();
    // https://github.com/rustdesk/rustdesk/issues/13705
    // Check if `sudo -E` actually preserves environment.
    //
    // This flag is only used by `run_as_user()` (root service -> user session). If the current process is not
    // running as `root`, this check is meaningless (and `sudo -n` may fail), so we return `false` directly.
    //
    // On Ubuntu 25.10, `sudo -E` may still succeed but effectively ignores `-E`. Some versions print a warning
    // to stderr (wording may vary by locale), so we verify behavior instead:
    // - Inject a sentinel environment variable into the `sudo` process
    // - Run `sudo -n -E env` and check whether the sentinel is present in stdout
    static ref SUDO_E_PRESERVES_ENV: bool = {
        if !is_root() {
            log::warn!("Not running as root, SUDO_E_PRESERVES_ENV check skipped");
            false
        } else {
            let key = format!("__RUSTDESK_SUDO_E_TEST_{}", std::process::id());
            let val = "1";
            let expected = format!("{key}={val}");
            Command::new("sudo")
                // -n for non-interactive to avoid password prompt
                .env(&key, val)
                .args(["-n", "-E", "env"])
                .output()
                .map(|o| {
                    o.status.success()
                        && String::from_utf8_lossy(&o.stdout).contains(expected.as_str())
                })
                .unwrap_or(false)
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GnomeMonitorLayoutMode {
    Logical,
    Physical,
}

impl GnomeMonitorLayoutMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Logical => "logical",
            Self::Physical => "physical",
        }
    }
}

fn gnome_monitor_layout_mode_from_value(value: u32) -> Option<GnomeMonitorLayoutMode> {
    // Upstream: https://gitlab.gnome.org/GNOME/mutter/-/blob/main/data/dbus-interfaces/org.gnome.Mutter.DisplayConfig.xml
    // Ubuntu mode 3: https://git.launchpad.net/ubuntu/+source/mutter/tree/debian/patches/x11-Add-support-for-fractional-scaling-using-Randr.patch
    match value {
        1 | 3 => Some(GnomeMonitorLayoutMode::Logical),
        2 => Some(GnomeMonitorLayoutMode::Physical),
        _ => None,
    }
}

pub fn gnome_monitor_layout_mode() -> Option<GnomeMonitorLayoutMode> {
    if let Ok(cache) = GNOME_MONITOR_LAYOUT_MODE_CACHE.lock() {
        if let Some((updated_at, result)) = *cache {
            if updated_at.elapsed() < Duration::from_secs(10) {
                return result;
            }
        }
    }

    let result = (|| {
        let is_gnome_desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .split(':')
            .any(|desktop| {
                desktop.eq_ignore_ascii_case("gnome") || desktop.eq_ignore_ascii_case("unity")
            });
        let is_gnome_session = std::env::var("DESKTOP_SESSION")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !is_gnome_desktop && !is_gnome_session.contains("gnome") {
            return None;
        }
        use dbus::{arg::PropMap, blocking::BlockingSender};

        let conn = match dbus::blocking::Connection::new_session() {
            Ok(conn) => conn,
            Err(err) => {
                log::warn!("Failed to connect to the session bus for GNOME monitor layout: {err}");
                return None;
            }
        };
        let message = match dbus::Message::new_method_call(
            "org.gnome.Mutter.DisplayConfig",
            "/org/gnome/Mutter/DisplayConfig",
            "org.gnome.Mutter.DisplayConfig",
            "GetCurrentState",
        ) {
            Ok(message) => message,
            Err(err) => {
                log::warn!("Failed to create GNOME monitor layout query: {err}");
                return None;
            }
        };
        let reply = match conn.send_with_reply_and_block(message, Duration::from_secs(2)) {
            Ok(reply) => reply,
            Err(err) => {
                log::warn!("Failed to query GNOME monitor layout: {err}");
                return None;
            }
        };
        let mut args = reply.iter_init();
        for _ in 0..3 {
            if !args.next() {
                log::warn!("GNOME monitor layout reply is missing properties");
                return None;
            }
        }
        let properties: PropMap = match args.read() {
            Ok(properties) => properties,
            Err(err) => {
                log::warn!("Failed to read GNOME monitor layout properties: {err}");
                return None;
            }
        };
        let Some(value) = dbus::arg::prop_cast::<u32>(&properties, "layout-mode").copied() else {
            log::warn!("GNOME monitor layout reply has no layout-mode");
            return None;
        };
        let mode = gnome_monitor_layout_mode_from_value(value);
        if mode.is_none() {
            log::warn!("GNOME monitor layout reply has unknown layout-mode {value}");
        }
        mode
    })();
    if let Ok(mut cache) = GNOME_MONITOR_LAYOUT_MODE_CACHE.lock() {
        *cache = Some((Instant::now(), result));
    }
    result
}

#[cfg(test)]
mod gnome_monitor_layout_tests {
    use super::*;

    #[test]
    fn maps_logical_layouts() {
        assert_eq!(
            gnome_monitor_layout_mode_from_value(1),
            Some(GnomeMonitorLayoutMode::Logical)
        );
        assert_eq!(
            gnome_monitor_layout_mode_from_value(3),
            Some(GnomeMonitorLayoutMode::Logical)
        );
    }
}

#[inline]
fn update_active_user_lookup_cache(desktop: &Desktop) {
    if let Ok(mut cache) = ACTIVE_USER_LOOKUP_CACHE.lock() {
        if desktop.uid.is_empty() || desktop.username.is_empty() {
            *cache = None;
        } else {
            *cache = Some(ActiveUserLookupCache {
                uid: desktop.uid.clone(),
                username: desktop.username.clone(),
            });
        }
    }
}

#[inline]
fn get_active_user_id_name_from_cache() -> Option<(String, String)> {
    let cache = ACTIVE_USER_LOOKUP_CACHE.lock().ok()?;
    let entry = cache.as_ref()?;
    Some((entry.uid.clone(), entry.username.clone()))
}

pub fn right_alt_is_level3_shift() -> ResultType<bool> {
    bail!("Global keyboard layout queries are unsupported on Wayland");
}

#[inline]
pub fn is_login_screen_wayland() -> bool {
    let values = get_values_of_seat0_with_gdm_wayland(&[0, 2]);
    is_gdm_user(&values[1]) && get_display_server_of_session(&values[0]) == DISPLAY_SERVER_WAYLAND
}

/// Require an identified Wayland session; unknown boot-time sessions are not capture-ready.
pub fn is_wayland_session_available() -> bool {
    if let Ok(forced) = std::env::var("RUSTDESK_FORCED_DISPLAY_SERVER") {
        return forced == DISPLAY_SERVER_WAYLAND;
    }
    is_desktop_wayland() || is_login_screen_wayland()
}

/// Memoised `is_login_screen_wayland`, for per-frame callers that must not run `loginctl`.
///
/// Only from the per-session `--server`: it is spawned after the session is identified, so the
/// answer is settled. Anything that can run mid-boot must use the uncached form.
#[cfg(feature = "drm")]
#[inline]
pub fn is_login_screen_wayland_cached() -> bool {
    *IS_LOGIN_SCREEN_WAYLAND
}

#[inline]
fn sleep_millis(millis: u64) {
    std::thread::sleep(Duration::from_millis(millis));
}

// Wayland does not expose global pointer or foreign-window geometry to clients.
pub fn get_cursor_pos() -> Option<(i32, i32)> {
    None
}

pub fn set_cursor_pos(_x: i32, _y: i32) -> bool {
    false
}

pub fn clip_cursor(_rect: Option<(i32, i32, i32, i32)>) -> bool {
    false
}

pub fn reset_input_cache() {}

pub fn get_focused_display(_displays: Vec<DisplayInfo>) -> Option<usize> {
    None
}

pub fn get_cursor() -> ResultType<Option<u64>> {
    #[cfg(feature = "drm")]
    if let Some(id) = crate::server::drm_capturer::drm_cursor_id() {
        if id != scrap::drm_reader::HIDDEN_CURSOR_ID
            || !crate::server::display_service::has_non_drm_backed_display()
        {
            return Ok(Some(id));
        }
    }
    // PipeWire supplies its cursor in the captured stream.
    Ok(None)
}

pub fn get_cursor_data(hcursor: u64) -> ResultType<CursorData> {
    #[cfg(feature = "drm")]
    if let Some(c) = crate::server::drm_capturer::drm_cursor() {
        if c.id != scrap::drm_reader::HIDDEN_CURSOR_ID
            || !crate::server::display_service::has_non_drm_backed_display()
        {
            let mut cd: CursorData = Default::default();
            cd.id = c.id;
            cd.width = c.width;
            cd.height = c.height;
            cd.hotx = c.hotx;
            cd.hoty = c.hoty;
            cd.colors = c.colors.into();
            return Ok(cd);
        }
    }
    bail!("No separate Wayland cursor image is available for {}", hcursor)
}

fn start_uinput_service() {
    use crate::server::uinput::service;
    std::thread::spawn(|| {
        service::start_service_control();
    });
    std::thread::spawn(|| {
        service::start_service_keyboard();
    });
    std::thread::spawn(|| {
        service::start_service_mouse();
    });
}

/// Suggests the best terminal type based on the environment.
///
/// The function prioritizes terminal types in the following order:
/// 1. `screen-256color`: Preferred when running inside `tmux` or `screen` sessions,
///    as these multiplexers often support advanced terminal features.
/// 2. `xterm-256color`: Selected if the terminal supports 256 colors, which is
///    suitable for modern terminal applications.
/// 3. `xterm`: Used as a fallback for basic terminal compatibility.
///
/// Terminals like `linux` and `vt100` are excluded because they lack support for
/// modern features required by many applications.
fn suggest_best_term() -> String {
    if is_running_in_tmux() || is_running_in_screen() {
        return TERM_SCREEN_256COLOR.to_string();
    }
    if term_supports_256_colors(TERM_XTERM_256COLOR) {
        return TERM_XTERM_256COLOR.to_string();
    }
    TERM_XTERM.to_string()
}

fn is_running_in_tmux() -> bool {
    std::env::var("TMUX").is_ok()
}

fn is_running_in_screen() -> bool {
    std::env::var("STY").is_ok()
}

fn supports_256_colors(db: &Database) -> bool {
    db.get::<cap::MaxColors>().map_or(false, |n| n.0 >= 256)
}

fn term_supports_256_colors(term: &str) -> bool {
    match term {
        TERM_XTERM_256COLOR => DATABASE_XTERM_256COLOR
            .as_ref()
            .map_or(false, |db| supports_256_colors(db)),
        _ => Database::from_name(term).map_or(false, |db| supports_256_colors(&db)),
    }
}

fn get_cur_term(uid: &str) -> Option<String> {
    // Check cache first - if TERM_XTERM_256COLOR was found before, reuse it
    if let Ok(cache) = CACHED_TERM.lock() {
        if let Some(ref cached) = *cache {
            if cached == TERM_XTERM_256COLOR {
                return Some(cached.clone());
            }
        }
    }

    if uid.is_empty() {
        return None;
    }

    // Check current process environment
    if let Ok(term) = std::env::var("TERM") {
        if term == TERM_XTERM_256COLOR {
            if let Ok(mut cache) = CACHED_TERM.lock() {
                *cache = Some(term.clone());
            }
            return Some(term);
        }
    }

    // Collect all TERM values from shell processes, looking for TERM_XTERM_256COLOR
    let terms = get_all_term_values(uid);

    // Prefer TERM_XTERM_256COLOR
    if terms.iter().any(|t| t == TERM_XTERM_256COLOR) {
        if let Ok(mut cache) = CACHED_TERM.lock() {
            *cache = Some(TERM_XTERM_256COLOR.to_string());
        }
        return Some(TERM_XTERM_256COLOR.to_string());
    }

    // Return first valid TERM if no TERM_XTERM_256COLOR found
    let fallback = terms.into_iter().next();
    if let Some(ref term) = fallback {
        log::debug!(
            "TERM_XTERM_256COLOR not found, using fallback TERM: {}",
            term
        );
    }
    fallback
}

/// Get all TERM values from shell processes (bash, zsh, fish, sh).
/// Returns a Vec of unique, valid TERM values.
fn get_all_term_values(uid: &str) -> Vec<String> {
    let Ok(uid_num) = uid.parse::<u32>() else {
        return Vec::new();
    };

    // Build regex pattern to match shell processes using only argv[0] (the executable path)
    // Pattern: match process name at start or after '/', followed by space or end
    // e.g., "bash", "/bin/bash", "/usr/bin/zsh"
    let shell_pattern = SHELL_PROCESSES
        .iter()
        .map(|p| format!(r"(^|/){p}(\s|$)"))
        .collect::<Vec<_>>()
        .join("|");
    let Ok(re) = Regex::new(&shell_pattern) else {
        return Vec::new();
    };

    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };

    let mut terms = Vec::new();

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(pid_str) = file_name.to_str() else {
            continue;
        };
        if !pid_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let proc_path = entry.path();

        // Check if process belongs to the specified uid
        if let Ok(meta) = std::fs::metadata(&proc_path) {
            use std::os::unix::fs::MetadataExt;
            if meta.uid() != uid_num {
                continue;
            }
        } else {
            continue;
        }

        // Check cmdline matches process pattern
        // /proc/<pid>/cmdline is a sequence of null-terminated strings; the first
        // one (argv[0]) is the executable path. Match the regex only against that
        // to avoid false positives from arguments (e.g., "python /path/to/bash-script.py").
        let cmdline_path = proc_path.join("cmdline");
        let Ok(cmdline) = std::fs::read(&cmdline_path) else {
            continue;
        };
        let exe_end = cmdline
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(cmdline.len());
        let exe_str = String::from_utf8_lossy(&cmdline[..exe_end]);
        if !re.is_match(&exe_str) {
            continue;
        }

        // Read environ and extract TERM
        let environ_path = proc_path.join("environ");
        let Ok(environ) = std::fs::read(&environ_path) else {
            continue;
        };

        for part in environ.split(|&b| b == 0) {
            if part.is_empty() {
                continue;
            }
            if let Some(eq) = part.iter().position(|&b| b == b'=') {
                let key_bytes = &part[..eq];
                if key_bytes == b"TERM" {
                    let val_bytes = &part[eq + 1..];
                    let term = String::from_utf8_lossy(val_bytes).into_owned();
                    if !INVALID_TERM_VALUES.contains(&term.as_str()) && !terms.contains(&term) {
                        // Early return if we found the preferred term
                        if term == TERM_XTERM_256COLOR {
                            return vec![term];
                        }
                        terms.push(term);
                    }
                    break;
                }
            }
        }
    }

    terms
}

#[inline]
fn try_start_server_(desktop: Option<&Desktop>) -> ResultType<Option<Child>> {
    match desktop {
        Some(desktop) => {
            let mut envs = vec![];
            if !desktop.display.is_empty() {
                envs.push(("DISPLAY", desktop.display.clone()));
            }
            if !desktop.xauth.is_empty() {
                envs.push(("XAUTHORITY", desktop.xauth.clone()));
            }
            if !desktop.wl_display.is_empty() {
                envs.push(("WAYLAND_DISPLAY", desktop.wl_display.clone()));
            }
            if !desktop.home.is_empty() {
                envs.push(("HOME", desktop.home.clone()));
            }
            if !desktop.dbus.is_empty() {
                envs.push(("DBUS_SESSION_BUS_ADDRESS", desktop.dbus.clone()));
            }
            if let Ok(forced_display_server) =
                std::env::var("RUSTDESK_FORCED_DISPLAY_SERVER")
            {
                if !forced_display_server.is_empty() {
                    envs.push((
                        "RUSTDESK_FORCED_DISPLAY_SERVER",
                        forced_display_server,
                    ));
                }
            }
            envs.push((
                "TERM",
                get_cur_term(&desktop.uid).unwrap_or_else(|| suggest_best_term()),
            ));
            run_as_user(
                vec!["--server"],
                Some((desktop.uid.clone(), desktop.username.clone())),
                envs,
            )
        }
        None => Ok(Some(crate::run_me(vec!["--server"])?)),
    }
}

#[inline]
fn start_server(desktop: Option<&Desktop>, server: &mut Option<Child>) {
    match try_start_server_(desktop) {
        Ok(ps) => *server = ps,
        Err(err) => {
            log::error!("Failed to start server: {}", err);
        }
    }
}

/// Whether a just-spawned `--server` is still running after a short grace period, taking ownership of
/// the corpse (clearing `server`) when it is not. `start_server` reports only whether the SPAWN
/// succeeded, which is not the same question: a child that execs and exits immediately still leaves
/// `Some(child)` behind.
///
/// A child that exits is detected as soon as it does; a healthy one costs the full grace, once per
/// start. A server that dies LATER than this is a different (transient) failure, and the restart
/// throttle in `should_start_server` already bounds that case.
#[cfg(feature = "drm")]
fn server_survived_grace(server: &mut Option<Child>) -> bool {
    const GRACE: Duration = Duration::from_millis(1000);
    const STEP_MS: u64 = 100;
    let Some(ps) = server.as_mut() else {
        return false; // spawn itself failed
    };
    let deadline = Instant::now() + GRACE;
    while Instant::now() < deadline {
        match ps.try_wait() {
            Ok(Some(status)) => {
                log::warn!("--server exited {status} within {GRACE:?} of starting");
                *server = None;
                return false;
            }
            Ok(None) => sleep_millis(STEP_MS),
            // We cannot tell; treat it as alive rather than tearing down a possibly healthy child.
            Err(err) => {
                log::error!("error waiting on the just-started --server: {err}");
                return true;
            }
        }
    }
    true
}

fn stop_server(server: &mut Option<Child>) {
    if let Some(mut ps) = server.take() {
        allow_err!(ps.kill());
        sleep_millis(30);
        match ps.try_wait() {
            Ok(Some(_status)) => {}
            Ok(None) => {
                let _res = ps.wait();
            }
            Err(e) => log::error!("error attempting to wait: {e}"),
        }
    }
}

#[inline]
fn stop_rustdesk_servers() {
    let _ = run_cmds(&format!(
        r##"ps -ef | grep -E '{} +--server' | awk '{{print $2}}' | xargs -r kill -9"##,
        crate::get_app_name().to_lowercase(),
    ));
}

fn should_start_server(
    is_display_changed: bool,
    uid: &mut String,
    desktop: &Desktop,
    cm0: &mut bool,
    last_restart: &mut Instant,
    server: &mut Option<Child>,
) -> bool {
    let cm = get_cm();
    let mut start_new = false;
    let mut should_kill = false;

    if is_display_changed || desktop.uid != *uid && !desktop.uid.is_empty() {
        *uid = desktop.uid.clone();
        should_kill = true;
    }

    if !should_kill
        && !cm
        && ((*cm0 && last_restart.elapsed().as_secs() > 60)
            || last_restart.elapsed().as_secs() > 3600)
    {
        // restart server if new connections all closed, or every one hour,
        // as a workaround to resolve "SpotUdp" (dns resolve)
        // and display discovery failures
        should_kill = true;
        log::info!("restart server");
    }

    if should_kill {
        if let Some(ps) = server.as_mut() {
            allow_err!(ps.kill());
            sleep_millis(30);
            *last_restart = Instant::now();
        }
    }

    if let Some(ps) = server.as_mut() {
        match ps.try_wait() {
            Ok(Some(_)) => {
                *server = None;
                start_new = true;
            }
            _ => {}
        }
    } else {
        start_new = true;
    }
    *cm0 = cm;
    start_new
}

// to-do: stop_server(&mut user_server); may not stop child correctly
// stop_rustdesk_servers() is just a temp solution here.
fn force_stop_server() {
    stop_rustdesk_servers();
    sleep_millis(super::SERVICE_INTERVAL);
}

pub fn start_os_service() {
    check_if_stop_service();
    stop_rustdesk_servers();
    start_uinput_service();

    std::thread::spawn(|| {
        allow_err!(crate::ipc::start(crate::POSTFIX_SERVICE));
    });

    // DRM/KMS capture producer (opt-in `drm` feature): a dedicated thread + runtime that streams
    // scanout frames to the user `--server` over the `_drm` service-scoped channel. Runs here
    // because this process is the root service that already holds CAP_SYS_ADMIN for the in-process
    // (direct-mode) libdrmtap read.
    //
    // Builder, like every other thread this feature starts: `thread::spawn` PANICS if the thread
    // cannot be created (EAGAIN under a thread-count or memory limit), and here that panic would
    // unwind out of `start_os_service` -- taking down the root service itself, for a feature whose
    // failure should only cost DRM capture. Losing the producer leaves the consumer to fall back to
    // PipeWire, which is the same path a host without the feature takes.
    #[cfg(feature = "drm")]
    if let Err(err) = std::thread::Builder::new()
        .name("drm-producer".into())
        .spawn(|| {
            crate::ipc::start_drm();
        })
    {
        log::warn!(
            "failed to spawn the drm capture producer thread: {err}; DRM capture is off for \
             this boot and the consumer falls back to PipeWire"
        );
    }

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    let (mut wl_display, mut dbus): (String, String) = ("".to_owned(), "".to_owned());
    let mut desktop = Desktop::default();
    let mut sid = "".to_owned();
    let mut uid = "".to_owned();
    let mut server: Option<Child> = None;
    let mut user_server: Option<Child> = None;
    if let Err(err) = ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    }) {
        println!("Failed to set Ctrl-C handler: {}", err);
    }

    let mut cm0 = false;
    let mut last_restart = Instant::now();
    while running.load(Ordering::SeqCst) {
        desktop.refresh();
        update_active_user_lookup_cache(&desktop);

        // Duplicate logic here with should_start_server.
        if desktop.is_wayland() && (desktop.username == "root" || desktop.is_login_wayland()) {
            // try kill subprocess "--server"
            stop_server(&mut user_server);
            // try start subprocess "--server"
            // No need to check is_display_changed here.
            if should_start_server(
                false,
                &mut uid,
                &desktop,
                &mut cm0,
                &mut last_restart,
                &mut server,
            ) {
                force_stop_server();
                // Run the login-screen --server as the active seat0 session user (the greeter
                // account) rather than root, so the DRM capture GPU/EGL convert never loads the
                // vendor GPU userspace in a privileged process. is_login_wayland() matches a GDM or
                // SDDM Wayland greeter (is_gdm_user covers both), and desktop.uid is that greeter's
                // uid, so this drops to whichever greeter owns seat0. A greeter is_gdm_user does not
                // recognize (e.g. LightDM) never reaches this branch -- it takes the unprivileged
                // else-branch below already. A genuine root graphical session (username=="root")
                // has no lower uid to drop to, so it stays root. The whole branch is gated on the drm
                // feature, so the drm-off build is upstream's single `start_server(None, ..)` line.
                #[cfg(not(feature = "drm"))]
                start_server(None, &mut server);
                #[cfg(feature = "drm")]
                if desktop.username != "root" && !desktop.uid.is_empty() {
                    start_server(Some(&desktop), &mut server);
                    // If dropping to the greeter uid did not produce a RUNNING server, fall back to a
                    // root --server so the login screen stays remotable instead of looping on a
                    // failing greeter spawn. This pays the GPU-in-root tradeoff only on that failure
                    // path, never in the normal greeter case. Liveness, not just spawn success: a
                    // greeter account that cannot actually run it (a nologin shell, a hardened home,
                    // no writable config dir) leaves a child that exits at once, and the loop above
                    // notices only that the child is gone and respawns it, forever, without ever
                    // reaching this fallback -- so the login screen becomes permanently un-remotable
                    // on a host where it used to work.
                    if !server_survived_grace(&mut server) {
                        log::warn!(
                            "greeter --server did not stay up; falling back to a root --server"
                        );
                        start_server(None, &mut server);
                    }
                } else {
                    start_server(None, &mut server);
                }
            }
        } else if desktop.is_wayland() && !desktop.username.is_empty() {
            // try kill subprocess "--server"
            stop_server(&mut server);

            let is_display_changed = desktop.wl_display != wl_display || desktop.dbus != dbus;
            wl_display = desktop.wl_display.clone();
            dbus = desktop.dbus.clone();

            // try start subprocess "--server"
            if should_start_server(
                is_display_changed,
                &mut uid,
                &desktop,
                &mut cm0,
                &mut last_restart,
                &mut user_server,
            ) {
                force_stop_server();
                start_server(Some(&desktop), &mut user_server);
            }
        } else {
            force_stop_server();
            stop_server(&mut user_server);
            stop_server(&mut server);
        }

        let keeps_session = sid == desktop.sid;
        if keeps_session {
            // for fixing https://github.com/rustdesk/rustdesk/issues/3129 to avoid too much dbus calling,
            sleep_millis(500);
        } else {
            sleep_millis(super::SERVICE_INTERVAL);
        }
        sid = desktop.sid.clone();
    }

    if let Some(ps) = user_server.take().as_mut() {
        allow_err!(ps.kill());
    }
    if let Some(ps) = server.take().as_mut() {
        allow_err!(ps.kill());
    }
    log::info!("Exit");
}

#[inline]
/// Returns the cached active `(uid, username)` snapshot when available.
/// Callers that require a fresh seat0 lookup should call `get_values_of_seat0` directly.
pub fn get_active_user_id_name() -> (String, String) {
    if let Some(id_name) = get_active_user_id_name_from_cache() {
        return id_name;
    }
    let vec_id_name = get_values_of_seat0(&[1, 2]);
    (vec_id_name[0].clone(), vec_id_name[1].clone())
}

#[inline]
/// Returns the cached active uid when available.
/// Callers that require a fresh seat0 lookup should call `get_values_of_seat0` directly.
pub fn get_active_userid() -> String {
    if let Some((uid, _)) = get_active_user_id_name_from_cache() {
        return uid;
    }
    get_values_of_seat0(&[1])[0].clone()
}

#[inline]
/// Returns the active uid from a fresh seat0 lookup, bypassing the service-loop cache.
pub fn get_active_userid_fresh() -> String {
    // A Wayland greeter owns seat0 while it is up and the DRM backend serves it, so a uid gate that
    // cannot see it rejects the greeter's own `--server`. `Desktop::refresh` reads it the same way.
    #[cfg(feature = "drm")]
    return get_values_of_seat0_with_gdm_wayland(&[1])[0].clone();
    #[cfg(not(feature = "drm"))]
    get_values_of_seat0(&[1])[0].clone()
}

#[inline]
/// The cached active uid as a number, or `None` when the cache is empty. Unlike `get_active_userid`
/// this NEVER falls back to a blocking `loginctl` seat0 lookup, so it is safe to call on an async
/// runtime thread and on a hot path (e.g. per-frame re-auth): a cache miss returns `None` for the
/// caller to treat as "active session momentarily unknown" rather than stalling on a subprocess.
pub fn get_active_userid_cached() -> Option<u32> {
    get_active_user_id_name_from_cache().and_then(|(uid, _)| uid.parse::<u32>().ok())
}

fn get_cm() -> bool {
    // Runs twice a second in the service loop, so walk /proc rather than forking `ps aux`; that
    // fork is also what the `CMD_PS` audit-message workaround this replaces was for.
    let cm = format!(
        "{} --cm",
        std::env::current_exe()
            .unwrap_or_default()
            .to_string_lossy()
    );
    any_process(None, "cmdline", |cmdline| cmdline.contains(&cm))
}

pub fn is_login_wayland() -> bool {
    let files = ["/etc/gdm3/custom.conf", "/etc/gdm/custom.conf"];
    match (
        Regex::new(r"# *WaylandEnable *= *false"),
        Regex::new(r"WaylandEnable *= *true"),
    ) {
        (Ok(pat1), Ok(pat2)) => {
            for file in files {
                if let Ok(contents) = std::fs::read_to_string(file) {
                    return pat1.is_match(&contents) || pat2.is_match(&contents);
                }
            }
        }
        _ => {}
    }
    false
}

#[inline]
pub fn current_is_wayland() -> bool {
    return is_desktop_wayland() && unsafe { UNMODIFIED };
}

// to-do: test the other display manager
fn _get_display_manager() -> String {
    if let Ok(x) = std::fs::read_to_string("/etc/X11/default-display-manager") {
        if let Some(x) = x.split("/").last() {
            return x.to_owned();
        }
    }
    "gdm3".to_owned()
}

#[inline]
/// Returns the cached active username when available.
/// Callers that require a fresh seat0 lookup should call `get_values_of_seat0` directly.
pub fn get_active_username() -> String {
    // Flatpak runs only inside the logged-in user's session, and Flathub builds
    // cannot reach the host's loginctl.
    if is_flatpak() {
        return crate::username();
    }
    if let Some((_, username)) = get_active_user_id_name_from_cache() {
        return username;
    }
    get_values_of_seat0(&[2])[0].clone()
}

pub fn get_user_home_by_name(username: &str) -> Option<PathBuf> {
    return match get_user_by_name(username) {
        None => None,
        Some(user) => {
            let home = user.home_dir();
            if Path::is_dir(home) {
                Some(PathBuf::from(home))
            } else {
                None
            }
        }
    };
}

pub fn get_active_user_home() -> Option<PathBuf> {
    let username = get_active_username();
    if !username.is_empty() {
        match get_user_home_by_name(&username) {
            None => {
                // fallback to most common default pattern
                let home = PathBuf::from(format!("/home/{}", username));
                if home.exists() {
                    return Some(home);
                }
            }
            Some(home) => {
                return Some(home);
            }
        }
    }
    None
}

pub fn get_env_var(k: &str) -> String {
    match std::env::var(k) {
        Ok(v) => v,
        Err(_e) => "".to_owned(),
    }
}

fn is_flatpak() -> bool {
    std::path::PathBuf::from("/.flatpak-info").exists()
}

pub fn is_prelogin() -> bool {
    if is_flatpak() {
        return false;
    }
    let name = get_active_username();
    if let Ok(res) = run_cmds(&format!("getent passwd {}", name)) {
        return res.contains("/bin/false") || res.contains("/usr/sbin/nologin");
    }
    false
}

// Check "Lock".
// "Switch user" can't be checked, because `get_values_of_seat0(&[0])` does not return the session.
// The logged in session is "online" not "active".
// And the "Switch user" screen is usually Wayland login session, which we do not support.
pub fn is_locked() -> bool {
    if is_prelogin() {
        return false;
    }

    let values = get_values_of_seat0(&[0]);
    // Though the values can't be empty, we still add check here for safety.
    // Because we cannot guarantee whether the internal implementation will change in the future.
    // https://github.com/rustdesk/hbb_common/blob/ebb4d4a48cf7ed6ca62e93f8ed124065c6408536/src/platform/linux.rs#L119
    if values.is_empty() {
        log::debug!("Failed to check is locked, values vector is empty.");
        return false;
    }
    let session = &values[0];
    if session.is_empty() {
        log::debug!("Failed to check is locked, session is empty.");
        return false;
    }
    is_session_locked(session)
}

pub fn is_root() -> bool {
    crate::username() == "root"
}

fn is_opensuse() -> bool {
    if let Ok(res) = run_cmds("cat /etc/os-release | grep opensuse") {
        if !res.is_empty() {
            return true;
        }
    }
    false
}

pub fn run_as_user<I, K, V>(
    arg: Vec<&str>,
    user: Option<(String, String)>,
    envs: I,
) -> ResultType<Option<Child>>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let (uid, username) = match user {
        Some(id_name) => id_name,
        None => get_active_user_id_name(),
    };
    let cmd = std::env::current_exe()?;
    if uid.is_empty() {
        bail!("No valid uid");
    }

    let xdg = &format!("XDG_RUNTIME_DIR=/run/user/{uid}");
    if *SUDO_E_PRESERVES_ENV {
        // Original logic: use sudo -E to preserve environment
        let mut args = vec![xdg, "-u", &username, cmd.to_str().unwrap_or("")];
        args.append(&mut arg.clone());
        // -E is required to preserve env
        args.insert(0, "-E");
        let task = Command::new("sudo").envs(envs).args(args).spawn()?;
        Ok(Some(task))
    } else {
        // Fallback: sudo -u username env VAR=VALUE ... cmd args
        // For systems where sudo -E is not supported (e.g., Ubuntu 25.10+)
        //
        // SECURITY: No shell is involved here (we use execve-style argv).
        // Environment is passed via `env` arguments,
        // so there is no shell injection vector.
        //
        // Only accept portable env var names (POSIX portable character set for shells).
        // Most legitimate env vars follow [A-Za-z_][A-Za-z0-9_]* convention.
        // Variables with dots (e.g., "java.home") are Java system properties, not env vars.
        // Being restrictive here is intentional for security in this sudo context.
        fn is_valid_env_key(key: &str) -> bool {
            let mut it = key.chars();
            match it.next() {
                Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
                _ => return false,
            }
            it.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }

        let mut sudo = Command::new("sudo");
        sudo.arg("-u").arg(&username).arg("--").arg("env").arg(xdg);

        for (k, v) in envs {
            let key = k.as_ref().to_string_lossy();
            if !is_valid_env_key(&key) {
                log::warn!("Skipping environment variable with invalid key: '{}'. Only [A-Za-z_][A-Za-z0-9_]* are allowed in sudo context.", key);
                continue;
            }
            // IMPORTANT: do NOT add shell quotes here; `Command` does not invoke a shell.
            // Passing KEY=VALUE as a single argv element is safe and preserves spaces.
            let mut arg = OsString::from(&*key);
            arg.push("=");
            arg.push(v.as_ref());
            sudo.arg(arg);
        }

        sudo.arg(cmd).args(arg);
        let task = sudo.spawn()?;
        Ok(Some(task))
    }
}

pub fn get_pa_monitor() -> String {
    get_pa_sources()
        .drain(..)
        .map(|x| x.0)
        .filter(|x| x.contains("monitor"))
        .next()
        .unwrap_or("".to_owned())
}

pub fn get_pa_source_name(desc: &str) -> String {
    get_pa_sources()
        .drain(..)
        .filter(|x| x.1 == desc)
        .map(|x| x.0)
        .next()
        .unwrap_or("".to_owned())
}

pub fn get_pa_sources() -> Vec<(String, String)> {
    use pulsectl::controllers::*;
    let mut out = Vec::new();
    match SourceController::create() {
        Ok(mut handler) => {
            if let Ok(devices) = handler.list_devices() {
                for dev in devices.clone() {
                    out.push((
                        dev.name.unwrap_or("".to_owned()),
                        dev.description.unwrap_or("".to_owned()),
                    ));
                }
            }
        }
        Err(err) => {
            log::error!("Failed to get_pa_sources: {:?}", err);
        }
    }
    out
}

pub fn get_default_pa_source() -> Option<(String, String)> {
    use pulsectl::controllers::*;
    match SourceController::create() {
        Ok(mut handler) => {
            if let Ok(dev) = handler.get_default_device() {
                return Some((
                    dev.name.unwrap_or("".to_owned()),
                    dev.description.unwrap_or("".to_owned()),
                ));
            }
        }
        Err(err) => {
            log::error!("Failed to get_pa_source: {:?}", err);
        }
    }
    None
}

pub fn lock_screen() {
    Command::new("xdg-screensaver").arg("lock").spawn().ok();
}

pub fn toggle_blank_screen(_v: bool) {
    // https://unix.stackexchange.com/questions/17170/disable-keyboard-mouse-input-on-unix-under-x
}

pub fn block_input(_v: bool) -> (bool, String) {
    (true, "".to_owned())
}

pub fn is_installed() -> bool {
    if let Ok(p) = std::env::current_exe() {
        p.to_str().unwrap_or_default().starts_with("/usr")
            || p.to_str().unwrap_or_default().starts_with("/nix/store")
    } else {
        false
    }
}

/// Get multiple environment variables from a process matching the given criteria.
/// This version reads /proc directly instead of spawning shell commands.
///
/// # Arguments
/// * `uid` - User ID to filter processes
/// * `process_pat` - Regex pattern to match process cmdline
/// * `names` - Environment variable names to retrieve. **Must be <= 64 elements** due to
///   the internal bitmask used for tie-breaking.
///
/// # Panics (debug builds)
/// Panics if `names.len() > 64`.
///
/// # Implementation notes
/// - Returns values from a *single* best-matching process_pat (for consistency).
/// - Avoids repeated scanning by parsing `environ` once per process.
fn get_envs<'a>(
    uid: &str,
    process_pat: &str,
    names: &[&'a str],
) -> std::collections::HashMap<&'a str, String> {
    get_envs_where(uid, process_pat, names, false, |count| count == names.len())
}

/// The newest process matching `process_pat`, whatever it happens to carry: the semantics of the
/// `ps -u <uid> -f | grep <pat> | tail -1` pipeline the callers below used before. A variable this
/// process does not have means moving on to the next pattern, never on to an older process that
/// may belong to a session which has since logged out.
fn get_envs_of_newest<'a>(
    uid: &str,
    process_pat: &str,
    names: &[&'a str],
) -> std::collections::HashMap<&'a str, String> {
    get_envs_where(uid, process_pat, names, true, |_| true)
}

/// `get_envs` with the caller's own process order and its own notion of a complete answer, told
/// how many of `names` the process carries: the first process `accept` takes wins outright, and
/// the count-based ranking is only the fallback for when no process is accepted at all.
fn get_envs_where<'a, F>(
    uid: &str,
    process_pat: &str,
    names: &[&'a str],
    newest_first: bool,
    mut accept: F,
) -> std::collections::HashMap<&'a str, String>
where
    F: FnMut(usize) -> bool,
{
    // The tie-breaking logic uses a u64 bitmask, limiting us to 64 variables.
    debug_assert!(
        names.len() <= 64,
        "get_envs: names.len() must be <= 64, got {}",
        names.len()
    );

    let empty: std::collections::HashMap<&'a str, String> =
        names.iter().map(|&n| (n, String::new())).collect();

    let Ok(uid_num) = uid.parse::<u32>() else {
        return empty;
    };
    let Ok(re) = Regex::new(process_pat) else {
        return empty;
    };

    // Used for stable tie-breaking when multiple processes match.
    // Higher bits correspond to earlier entries in `names`.
    let name_indices: std::collections::HashMap<&'a str, usize> =
        names.iter().enumerate().map(|(i, &n)| (n, i)).collect();

    let mut best = empty.clone();
    let mut best_count = 0usize;
    let mut best_mask: u64 = 0;

    // Iterate /proc to find matching processes. `newest_first` is only for `get_envs_of_newest`,
    // whose callers need the last PID-ordered match their `ps ... | tail -1` pipelines took;
    // without it the order is whatever readdir returns, which is what `get_envs` has always used.
    // Neither order identifies the active session -- a user with two live graphical sessions has
    // one of each, and picking by PID guesses. See `Desktop::refresh` for who owns that question.
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return best;
    };
    let mut pids: Vec<u32> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .collect();
    if newest_first {
        pids.sort_unstable_by(|a, b| b.cmp(a));
    }

    for pid in pids {
        let proc_path = std::path::Path::new("/proc").join(pid.to_string());

        // Check if process belongs to the specified uid
        if let Ok(meta) = std::fs::metadata(&proc_path) {
            use std::os::unix::fs::MetadataExt;
            if meta.uid() != uid_num {
                continue;
            }
        } else {
            continue;
        }

        // Check cmdline matches process pattern
        let cmdline_path = proc_path.join("cmdline");
        let Ok(cmdline) = std::fs::read(&cmdline_path) else {
            continue;
        };
        let cmdline_str = String::from_utf8_lossy(&cmdline).replace('\0', " ");
        // The `grep -v 'grep'` of the pipeline this replaces. A user grepping for one of these
        // patterns is otherwise the newest match for it, and answers with whatever environment
        // their shell had -- an X forwarding endpoint over ssh, say.
        if cmdline_str.contains("grep") || !re.is_match(&cmdline_str) {
            continue;
        }

        // Read environ and extract matching variables. A read that fails -- the process exited
        // between these two reads -- is a process carrying none of `names`, not a process to
        // skip: skipping it would hand `newest_first` on to an older PID, where the pipeline
        // this replaces stopped at the single PID its `tail -1` had already picked.
        let environ = std::fs::read(proc_path.join("environ")).unwrap_or_default();

        let mut found = empty.clone();
        let mut found_count = 0usize;
        let mut found_mask: u64 = 0;

        for part in environ.split(|&b| b == 0) {
            if part.is_empty() {
                continue;
            }
            let Some(eq) = part.iter().position(|&b| b == b'=') else {
                continue;
            };
            let key_bytes = &part[..eq];
            let val_bytes = &part[eq + 1..];

            let Ok(key) = std::str::from_utf8(key_bytes) else {
                continue;
            };
            if let Some(slot) = found.get_mut(key) {
                if slot.is_empty() {
                    *slot = String::from_utf8_lossy(val_bytes).into_owned();
                    found_count += 1;

                    if let Some(&idx) = name_indices.get(key) {
                        let total = names.len();
                        if total <= 64 {
                            let bit = 1u64 << (total - 1 - idx);
                            found_mask |= bit;
                        }
                    }
                }
            }
        }

        if accept(found_count) {
            return found;
        }

        if found_count > best_count || (found_count == best_count && found_mask > best_mask) {
            best = found;
            best_count = found_count;
            best_mask = found_mask;
        }
    }

    best
}

/// True when `pred` accepts the `/proc/<pid>/<file>` of any process, NULs turned into spaces,
/// optionally only of processes owned by `uid`.
/// Reads `/proc` directly instead of forking `ps` / `pgrep`, for the service-loop callers below.
fn any_process<F: Fn(&str) -> bool>(uid: Option<u32>, file: &str, pred: F) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(pid_str) = file_name.to_str() else {
            continue;
        };
        if !pid_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let proc_path = entry.path();
        if let Some(uid) = uid {
            use std::os::unix::fs::MetadataExt;
            match std::fs::metadata(&proc_path) {
                Ok(meta) if meta.uid() == uid => {}
                _ => continue,
            }
        }
        let Ok(content) = std::fs::read(proc_path.join(file)) else {
            continue;
        };
        if pred(&String::from_utf8_lossy(&content).replace('\0', " ")) {
            return true;
        }
    }
    false
}

#[inline]
fn get_env_from_pid(name: &str, pid: &str) -> String {
    let cmd = format!("cat /proc/{}/environ 2>/dev/null | tr '\\0' '\\n' | grep '^{}=' | tail -1 | sed 's/{}=//g'", pid, name, name);
    if let Ok(x) = run_cmds(&cmd) {
        x.trim_end().to_string()
    } else {
        "".to_owned()
    }
}

#[link(name = "gtk-3")]
extern "C" {
    fn gtk_main_quit();
}

pub fn quit_gui() {
    unsafe { gtk_main_quit() };
}

/*
pub fn exec_privileged(args: &[&str]) -> ResultType<Child> {
    Ok(Command::new("pkexec").args(args).spawn()?)
}
*/

pub fn check_super_user_permission() -> ResultType<bool> {
    gtk_sudo::run(vec!["echo"])?;
    Ok(true)
}

/*
pub fn elevate(args: Vec<&str>) -> ResultType<bool> {
    let cmd = std::env::current_exe()?;
    match cmd.to_str() {
        Some(cmd) => {
            let mut args_with_exe = vec![cmd];
            args_with_exe.append(&mut args.clone());
            // -E required for opensuse
            if is_opensuse() {
                args_with_exe.insert(0, "-E");
            }
            let res = match exec_privileged(&args_with_exe)?.wait() {
                Ok(status) => {
                    if status.success() {
                        true
                    } else {
                        log::error!(
                            "Failed to wait install process, process status: {:?}",
                            status
                        );
                        false
                    }
                }
                Err(e) => {
                    log::error!("Failed to wait install process, error: {}", e);
                    false
                }
            };
            Ok(res)
        }
        None => {
            hbb_common::bail!("Failed to get current exe as str");
        }
    }
}
*/

type GtkSettingsPtr = *mut c_void;
type GObjectPtr = *mut c_void;
#[link(name = "gtk-3")]
extern "C" {
    // fn gtk_init(argc: *mut c_int, argv: *mut *mut c_char);
    fn gtk_settings_get_default() -> GtkSettingsPtr;
}

#[link(name = "gobject-2.0")]
extern "C" {
    fn g_object_get(object: GObjectPtr, first_property_name: *const c_char, ...);
}

pub fn get_double_click_time() -> u32 {
    // GtkSettings *settings = gtk_settings_get_default ();
    // g_object_get (settings, "gtk-double-click-time", &double_click_time, NULL);
    unsafe {
        let mut double_click_time = 0u32;
        let Ok(property) = std::ffi::CString::new("gtk-double-click-time") else {
            return 0;
        };
        let settings = gtk_settings_get_default();
        g_object_get(
            settings,
            property.as_ptr(),
            &mut double_click_time as *mut u32,
            0 as *const c_void,
        );
        double_click_time
    }
}

// Resolution changes require compositor-specific APIs and are not advertised here.
pub fn resolutions(_name: &str) -> Vec<Resolution> {
    Vec::new()
}

pub fn current_resolution(_name: &str) -> ResultType<Resolution> {
    bail!("Resolution control is unsupported on Wayland");
}

pub fn change_resolution_directly(_name: &str, _width: usize, _height: usize) -> ResultType<()> {
    bail!("Resolution control is unsupported on Wayland");
}

mod desktop {
    use super::*;

    pub const XFCE4_PANEL: &str = "xfce4-panel";
    pub const SDDM_GREETER: &str = "sddm-greeter";

    // xdg-desktop-portal runs on all Wayland desktops (GNOME, KDE, wlroots, etc.)
    const XDG_DESKTOP_PORTAL: &str = "xdg-desktop-portal";

    const ENV_KEY_DISPLAY: &str = "DISPLAY";
    const ENV_KEY_XAUTHORITY: &str = "XAUTHORITY";
    const ENV_KEY_WAYLAND_DISPLAY: &str = "WAYLAND_DISPLAY";
    const ENV_KEY_DBUS_SESSION_BUS_ADDRESS: &str = "DBUS_SESSION_BUS_ADDRESS";

    #[derive(Debug, Clone, Default)]
    pub struct Desktop {
        pub sid: String,
        pub username: String,
        pub uid: String,
        pub protocol: String,
        pub display: String,
        pub xauth: String,
        pub home: String,
        pub dbus: String,
        pub wl_display: String,
    }

    impl Desktop {
        #[inline]
        pub fn is_wayland(&self) -> bool {
            self.protocol == DISPLAY_SERVER_WAYLAND
        }

        #[inline]
        pub fn is_login_wayland(&self) -> bool {
            super::is_gdm_user(&self.username) && self.protocol == DISPLAY_SERVER_WAYLAND
        }

        fn get_display_xauth_wayland(&mut self) {
            for _ in 1..=10 {
                // Prefer Wayland-related variables first when multiple portal processes match.
                let mut envs = get_envs(
                    &self.uid,
                    XDG_DESKTOP_PORTAL,
                    &[
                        ENV_KEY_WAYLAND_DISPLAY,
                        ENV_KEY_DBUS_SESSION_BUS_ADDRESS,
                        ENV_KEY_DISPLAY,
                        ENV_KEY_XAUTHORITY,
                    ],
                );
                self.display = envs.remove(ENV_KEY_DISPLAY).unwrap_or_default();
                self.xauth = envs.remove(ENV_KEY_XAUTHORITY).unwrap_or_default();
                self.wl_display = envs.remove(ENV_KEY_WAYLAND_DISPLAY).unwrap_or_default();
                self.dbus = envs
                    .remove(ENV_KEY_DBUS_SESSION_BUS_ADDRESS)
                    .unwrap_or_default();
                // For pure Wayland sessions, prefer `WAYLAND_DISPLAY`.
                // NOTE: On some systems (e.g. Ubuntu 25.10), `DISPLAY`/`XAUTHORITY` may exist even when XWayland
                // is not running, so do NOT treat them as a success condition here.
                let has_wayland = !self.wl_display.is_empty();
                let has_dbus = !self.dbus.is_empty();
                if has_wayland && has_dbus {
                    return;
                }
                sleep_millis(300);
            }
        }

        fn get_home(&mut self) {
            self.home = "".to_string();

            let cmd = format!(
                "getent passwd '{}' | awk -F':' '{{print $6}}'",
                &self.username
            );
            self.home = run_cmds_trim_newline(&cmd).unwrap_or(format!("/home/{}", &self.username));
        }

        /// Preserves an active seat0 session's cached identity so the service loop only retries
        /// late Wayland display discovery instead of repeating the full seat lookup.
        pub fn refresh(&mut self) {
            if !self.sid.is_empty() && is_active_and_seat0(&self.sid) {
                if self.is_wayland() && !self.is_login_wayland() {
                    self.get_display_xauth_wayland();
                }
                return;
            }

            let seat0_values = get_values_of_seat0_with_gdm_wayland(&[0, 1, 2]);
            if seat0_values[0].is_empty() {
                *self = Self::default();
                return;
            }

            // A new session must not inherit the previous user's compositor environment.
            *self = Self::default();
            self.sid = seat0_values[0].clone();
            self.uid = seat0_values[1].clone();
            self.username = seat0_values[2].clone();
            self.protocol = get_display_server_of_session(&self.sid).into();
            if self.is_login_wayland() {
                self.display = "".to_owned();
                self.xauth = "".to_owned();
                // Resolve HOME even on this path. Upstream returned without it because nothing then
                // consumed a login-Wayland Desktop, but the drm build starts a `--server` as the
                // greeter uid here, and a child with no HOME has nowhere to put its config. The
                // compositor variables (WAYLAND_DISPLAY, DBUS, DISPLAY, XAUTHORITY) are left blank
                // on purpose and are NOT an oversight: the drm capture path talks to the root
                // service over `_drm` and to a render node, never to the compositor or the portal,
                // which is the entire reason it works at a login screen. `try_start_server_` skips
                // empty entries, so the greeter child simply does not get them.
                //
                // `is_login_wayland` needs `is_gdm_user(username)`, and a current GDM runs its
                // greeter as `gdm-greeter`, which that helper does not match -- measured on the
                // test host, where the greeter server therefore takes the branch below and gets a
                // fully populated environment. This is for the display managers whose greeter user
                // does match.
                #[cfg(feature = "drm")]
                self.get_home();
                return;
            }

            if self.is_wayland() {
                self.get_home();
                self.get_display_xauth_wayland();
            }
        }
    }
}

/// A session-bus idle-inhibit interface, tried in order; the first that answers wins.
/// `org.freedesktop.ScreenSaver` is absent on purpose: that is the one `keepawake` already tried.
struct SessionInhibitTarget {
    dest: &'static str,
    path: &'static str,
    iface: &'static str,
    /// GNOME takes `(app_id, xid, reason, flags)`; PowerManagement takes `(app, reason)`.
    gnome_shape: bool,
    uninhibit: &'static str,
}

/// `org.gnome.SessionManager.Inhibit` flag 8 = idle only; logout/switch-user/suspend would take
/// away actions the person at the machine should keep.
const GNOME_INHIBIT_IDLE: u32 = 8;

const SESSION_INHIBIT_TARGETS: &[SessionInhibitTarget] = &[
    // Measured on a GDM greeter: output held 129.9 s with the inhibit, 30.3 s without.
    SessionInhibitTarget {
        dest: "org.gnome.SessionManager",
        path: "/org/gnome/SessionManager",
        iface: "org.gnome.SessionManager",
        gnome_shape: true,
        uninhibit: "Uninhibit",
    },
    // What powerdevil and xfce4-power-manager implement. NOT tested here; costs one failed call
    // where absent, and the log below names every interface tried.
    SessionInhibitTarget {
        dest: "org.freedesktop.PowerManagement",
        path: "/org/freedesktop/PowerManagement/Inhibit",
        iface: "org.freedesktop.PowerManagement.Inhibit",
        gnome_shape: false,
        uninhibit: "UnInhibit",
    },
];

/// Idle inhibit for the case `keepawake` cannot serve: it inhibits `org.freedesktop.ScreenSaver`,
/// which a GDM greeter bus neither provides nor can activate, so its `create()` fails outright.
/// The connection is kept because the inhibit is bound to it: dropping it releases the inhibit.
struct SessionIdleInhibit {
    conn: dbus::blocking::Connection,
    target: &'static SessionInhibitTarget,
    cookie: u32,
}

impl SessionIdleInhibit {
    fn new(reason: &str) -> Option<Self> {
        let conn = match dbus::blocking::Connection::new_session() {
            Ok(conn) => conn,
            Err(err) => {
                log::info!("wakelock: no session bus for the idle inhibit fallback ({err})");
                return None;
            }
        };
        let app = crate::get_app_name();
        let mut refused = Vec::new();
        for target in SESSION_INHIBIT_TARGETS {
            let res: Result<(u32,), dbus::Error> = {
                let proxy = conn.with_proxy(
                    target.dest,
                    target.path,
                    std::time::Duration::from_secs(3),
                );
                if target.gnome_shape {
                    // Inhibit(s app_id, u xid, s reason, u flags) -> u cookie; xid 0 = no window.
                    proxy.method_call(
                        target.iface,
                        "Inhibit",
                        (app.clone(), 0u32, reason.to_owned(), GNOME_INHIBIT_IDLE),
                    )
                } else {
                    proxy.method_call(target.iface, "Inhibit", (app.clone(), reason.to_owned()))
                }
            };
            match res {
                Ok((cookie,)) => {
                    log::info!(
                        "wakelock: holding a {} idle inhibit (cookie {cookie})",
                        target.dest
                    );
                    return Some(Self {
                        conn,
                        target,
                        cookie,
                    });
                }
                Err(err) => refused.push(format!("{}: {err}", target.dest)),
            }
        }
        // Name every interface tried and why it failed: on an untested desktop this log is what
        // turns "the screen still blanks" into a report naming the missing interface.
        log::info!(
            "wakelock: no session idle inhibitor answered, so the compositor may still blank this \
             screen ({})",
            refused.join("; ")
        );
        None
    }
}

impl Drop for SessionIdleInhibit {
    fn drop(&mut self) {
        let proxy = self.conn.with_proxy(
            self.target.dest,
            self.target.path,
            std::time::Duration::from_secs(3),
        );
        // Best effort: the session manager ties the inhibit to the caller's bus name, so dropping
        // `conn` below releases it even if this call does not get through.
        let res: Result<(), dbus::Error> =
            proxy.method_call(self.target.iface, self.target.uninhibit, (self.cookie,));
        if let Err(err) = res {
            log::debug!("wakelock: releasing the idle inhibit by closing the bus instead ({err})");
        }
    }
}

pub struct WakeLock(Option<keepawake::AwakeHandle>, Option<SessionIdleInhibit>);

impl WakeLock {
    pub fn new(display: bool, idle: bool, sleep: bool) -> Self {
        match keepawake::Builder::new()
            .display(display)
            .idle(idle)
            .sleep(sleep)
            .create()
        {
            Ok(handle) => WakeLock(Some(handle), None),
            Err(err) => {
                // Not `.ok()`: a discarded error is how a login screen ran with no inhibitor at
                // all and nobody noticed.
                log::info!("wakelock: keepawake could not take the inhibit ({err})");
                // keepawake asks for the ScreenSaver inhibit first and abandons the whole request
                // if it fails, losing the logind idle/sleep inhibits that stop the HOST suspending
                // mid-session. Re-ask without the display part: those are on the system bus.
                let system = if idle || sleep {
                    match keepawake::Builder::new()
                        .display(false)
                        .idle(idle)
                        .sleep(sleep)
                        .create()
                    {
                        Ok(handle) => Some(handle),
                        Err(err) => {
                            log::info!(
                                "wakelock: the logind idle/sleep inhibit did not come back \
                                 either ({err})"
                            );
                            None
                        }
                    }
                } else {
                    None
                };
                let session = if display {
                    SessionIdleInhibit::new("incoming session")
                } else {
                    None
                };
                WakeLock(system, session)
            }
        }
    }
}

fn has_cmd(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .status()
        .map(|x| x.success())
        .unwrap_or_default()
}

pub fn run_cmds_privileged(cmds: &str) -> bool {
    crate::platform::gtk_sudo::run(vec![cmds]).is_ok()
}

/// Spawn the current executable after a delay.
///
/// # Security
/// The executable path is safely quoted using `shell_quote()` to prevent
/// command injection vulnerabilities. The `secs` parameter is a u32, so it
/// cannot contain malicious input.
///
/// # Arguments
/// * `secs` - Number of seconds to wait before spawning
pub fn run_me_with(secs: u32) {
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(e) => {
            log::error!("Failed to get current exe: {}", e);
            return;
        }
    };

    // SECURITY: Use shell_quote to safely escape the executable path,
    // preventing command injection even if the path contains special characters.
    let exe_quoted = shell_quote(&exe.to_string_lossy());

    // Spawn a background process that sleeps and then executes.
    // The child process is automatically orphaned when parent exits,
    // and will be adopted by init (PID 1).
    Command::new(CMD_SH.as_str())
        .arg("-c")
        .arg(&format!("sleep {secs}; exec {exe_quoted}"))
        .spawn()
        .ok();
}

fn switch_service(stop: bool) -> String {
    // SECURITY: Use trusted home directory lookup via getpwuid instead of $HOME env var
    // to prevent confused-deputy attacks where an attacker manipulates environment variables.
    let home = get_home_dir_trusted()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    Config::set_option("stop-service".into(), if stop { "Y" } else { "" }.into());
    if !home.is_empty() && home != "/root" && !Config::get().is_empty() {
        let app_name_lower = crate::get_app_name().to_lowercase();
        let app_name0 = crate::get_app_name();
        let config_subdir = format!(".config/{}", app_name_lower);

        // SECURITY: Quote all paths to prevent shell injection from paths containing
        // spaces, semicolons, or other special characters.
        let src1 = shell_quote(&format!("{}/{}/{}.toml", home, config_subdir, app_name0));
        let src2 = shell_quote(&format!("{}/{}/{}2.toml", home, config_subdir, app_name0));
        let dst = shell_quote(&format!("/root/{}/", config_subdir));

        format!("cp -f {} {}; cp -f {} {};", src1, dst, src2, dst)
    } else {
        "".to_owned()
    }
}

pub fn uninstall_service(show_new_window: bool, _: bool) -> bool {
    if !has_cmd("systemctl") {
        // Failed when installed + flutter run + started by `show_new_window`.
        return false;
    }
    log::info!("Uninstalling service...");
    let cp = switch_service(true);
    let app_name = crate::get_app_name().to_lowercase();
    // systemctl kill rustdesk --tray, execute cp first
    if !run_cmds_privileged(&format!(
        "{cp} systemctl disable {app_name}; systemctl stop {app_name};"
    )) {
        Config::set_option("stop-service".into(), "".into());
        return true;
    }
    // systemctl stop will kill child processes, below may not be executed.
    if show_new_window {
        run_me_with(2);
    }
    std::process::exit(0);
}

pub fn install_service() -> bool {
    let _installing = crate::platform::InstallingService::new();
    if !has_cmd("systemctl") {
        return false;
    }
    log::info!("Installing service...");
    let cp = switch_service(false);
    let app_name = crate::get_app_name().to_lowercase();
    if !run_cmds_privileged(&format!(
        "{cp} systemctl enable {app_name}; systemctl start {app_name};"
    )) {
        Config::set_option("stop-service".into(), "Y".into());
    }
    true
}

fn check_if_stop_service() {
    if Config::get_option("stop-service".into()) == "Y" {
        let app_name = crate::get_app_name().to_lowercase();
        allow_err!(run_cmds(&format!(
            "systemctl disable {app_name}; systemctl stop {app_name}"
        )));
    }
}

pub fn check_autostart_config() -> ResultType<()> {
    // SECURITY: Use trusted home directory lookup via getpwuid instead of $HOME env var
    // to prevent confused-deputy attacks where an attacker manipulates environment variables.
    let home = match get_home_dir_trusted() {
        Some(p) => p.to_string_lossy().to_string(),
        None => {
            log::warn!("Failed to get trusted home directory for autostart config check");
            return Ok(());
        }
    };
    let app_name = crate::get_app_name().to_lowercase();
    let path = format!("{home}/.config/autostart");
    let file = format!("{path}/{app_name}.desktop");
    // https://github.com/rustdesk/rustdesk/issues/4863
    std::fs::remove_file(&file).ok();
    /*
        std::fs::create_dir_all(&path).ok();
        if !Path::new(&file).exists() {
            // write text to the desktop file
            let mut file = std::fs::File::create(&file)?;
            file.write_all(
                format!(
                    "
    [Desktop Entry]
    Type=Application
    Exec={app_name} --tray
    NoDisplay=false
            "
                )
                .as_bytes(),
            )?;
        }
        */
    Ok(())
}

pub struct WallPaperRemover {
    old_path: String,
    old_path_dark: Option<String>, // ubuntu 22.04 light/dark theme have different uri
}

impl WallPaperRemover {
    pub fn new() -> ResultType<Self> {
        let start = std::time::Instant::now();
        let old_path = wallpaper::get().map_err(|e| anyhow!(e.to_string()))?;
        let old_path_dark = wallpaper::get_dark().ok();
        if old_path.is_empty() && old_path_dark.clone().unwrap_or_default().is_empty() {
            bail!("already solid color");
        }
        wallpaper::set_from_path("").map_err(|e| anyhow!(e.to_string()))?;
        wallpaper::set_dark_from_path("").ok();
        log::info!(
            "created wallpaper remover,  old_path: {:?}, old_path_dark: {:?}, elapsed: {:?}",
            old_path,
            old_path_dark,
            start.elapsed(),
        );
        Ok(Self {
            old_path,
            old_path_dark,
        })
    }

    pub fn support() -> bool {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
        if wallpaper::gnome::is_compliant(&desktop) || desktop.as_str() == "XFCE" {
            return wallpaper::get().is_ok();
        }
        false
    }
}

impl Drop for WallPaperRemover {
    fn drop(&mut self) {
        allow_err!(wallpaper::set_from_path(&self.old_path).map_err(|e| anyhow!(e.to_string())));
        if let Some(old_path_dark) = &self.old_path_dark {
            allow_err!(wallpaper::set_dark_from_path(old_path_dark.as_str())
                .map_err(|e| anyhow!(e.to_string())));
        }
    }
}

#[inline]
pub fn is_selinux_enforcing() -> bool {
    match run_cmds("getenforce") {
        Ok(output) => output.trim() == "Enforcing",
        Err(_) => match run_cmds("sestatus") {
            Ok(output) => {
                for line in output.lines() {
                    if line.contains("Current mode:") {
                        return line.contains("enforcing");
                    }
                }
                false
            }
            Err(_) => false,
        },
    }
}

/// Get the app ID for shortcuts inhibitor permission.
/// Returns different ID based on whether running in Flatpak or native.
/// The ID must match the installed .desktop filename, as GNOME Shell's
/// inhibitShortcutsDialog uses `Shell.WindowTracker.get_window_app(window).get_id()`.
fn get_shortcuts_inhibitor_app_id() -> String {
    if is_flatpak() {
        // In Flatpak, FLATPAK_ID is set automatically by the runtime to the app ID
        // (e.g., "com.rustdesk.RustDesk"). This is the most reliable source.
        // Fall back to constructing from app name if not available.
        match std::env::var("FLATPAK_ID") {
            Ok(id) if !id.is_empty() => format!("{}.desktop", id),
            _ => {
                let app_name = crate::get_app_name();
                format!("com.{}.{}.desktop", app_name.to_lowercase(), app_name)
            }
        }
    } else {
        format!("{}.desktop", crate::get_app_name().to_lowercase())
    }
}

const PERMISSION_STORE_DEST: &str = "org.freedesktop.impl.portal.PermissionStore";
const PERMISSION_STORE_PATH: &str = "/org/freedesktop/impl/portal/PermissionStore";
const PERMISSION_STORE_IFACE: &str = "org.freedesktop.impl.portal.PermissionStore";

/// Clear GNOME shortcuts inhibitor permission via D-Bus.
/// This allows the permission dialog to be shown again.
pub fn clear_gnome_shortcuts_inhibitor_permission() -> ResultType<()> {
    let app_id = get_shortcuts_inhibitor_app_id();
    log::info!(
        "Clearing shortcuts inhibitor permission for app_id: {}, is_flatpak: {}",
        app_id,
        is_flatpak()
    );

    let conn = dbus::blocking::Connection::new_session()?;
    let proxy = conn.with_proxy(
        PERMISSION_STORE_DEST,
        PERMISSION_STORE_PATH,
        std::time::Duration::from_secs(3),
    );

    // DeletePermission(s table, s id, s app) -> ()
    let result: Result<(), dbus::Error> = proxy.method_call(
        PERMISSION_STORE_IFACE,
        "DeletePermission",
        ("gnome", "shortcuts-inhibitor", app_id.as_str()),
    );

    match result {
        Ok(()) => {
            log::info!("Successfully cleared GNOME shortcuts inhibitor permission");
            Ok(())
        }
        Err(e) => {
            let err_name = e.name().unwrap_or("");
            // If the permission doesn't exist, that's also fine
            if err_name == "org.freedesktop.portal.Error.NotFound"
                || err_name == "org.freedesktop.DBus.Error.UnknownObject"
                || err_name == "org.freedesktop.DBus.Error.ServiceUnknown"
            {
                log::info!(
                    "GNOME shortcuts inhibitor permission was not set ({})",
                    err_name
                );
                Ok(())
            } else {
                bail!("Failed to clear permission: {}", e)
            }
        }
    }
}

/// Check if GNOME shortcuts inhibitor permission exists.
pub fn has_gnome_shortcuts_inhibitor_permission() -> bool {
    let app_id = get_shortcuts_inhibitor_app_id();

    let conn = match dbus::blocking::Connection::new_session() {
        Ok(c) => c,
        Err(e) => {
            log::debug!("Failed to connect to session bus: {}", e);
            return false;
        }
    };
    let proxy = conn.with_proxy(
        PERMISSION_STORE_DEST,
        PERMISSION_STORE_PATH,
        std::time::Duration::from_secs(3),
    );

    // Lookup(s table, s id) -> (a{sas} permissions, v data)
    // We only need the permissions dict; check if app_id is a key.
    let result: Result<
        (
            std::collections::HashMap<String, Vec<String>>,
            dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,
        ),
        dbus::Error,
    > = proxy.method_call(
        PERMISSION_STORE_IFACE,
        "Lookup",
        ("gnome", "shortcuts-inhibitor"),
    );

    match result {
        Ok((permissions, _)) => {
            let found = permissions.contains_key(&app_id);
            log::debug!(
                "Shortcuts inhibitor permission lookup: app_id={}, found={}, keys={:?}",
                app_id,
                found,
                permissions.keys().collect::<Vec<_>>()
            );
            found
        }
        Err(e) => {
            log::debug!("Failed to query shortcuts inhibitor permission: {}", e);
            false
        }
    }
}

#[cfg(test)]
mod wayland_only_tests {
    use super::*;

    #[test]
    fn unsupported_global_pointer_control_reports_failure() {
        assert!(!set_cursor_pos(0, 0));
        assert!(!clip_cursor(Some((0, 0, 100, 100))));
        assert!(right_alt_is_level3_shift().is_err());
        assert!(resolutions("display").is_empty());
        assert!(change_resolution_directly("display", 1920, 1080).is_err());
    }
}
