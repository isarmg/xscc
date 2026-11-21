use super::*;
use hbb_common::{allow_err, anyhow};
use base::{
    platform::linux::DISTRO,
    session_protocol::{SCRAP_UBUNTU_HIGHER_REQUIRED, SCRAP_XDP_PORTAL_UNAVAILABLE},
};
use scrap::{
    is_cursor_embedded, set_map_err,
    wayland::pipewire::{fill_displays, try_fix_logical_size},
    Capturer, Display, Frame, TraitCapturer,
};
use std::io;

lazy_static::lazy_static! {
    static ref CAP_DISPLAY_INFO: RwLock<Option<CapDisplayInfo>> = RwLock::new(None);
    static ref LOG_SCRAP_COUNT: Mutex<u32> = Mutex::new(0);
    static ref LAST_STAGE_ERR: Mutex<Option<(String, std::time::Instant)>> = Mutex::new(None);
    static ref ACTIVE_DISPLAY_COUNT: Mutex<usize> = Mutex::new(0);
}

pub fn init() {
    set_map_err(map_err_scrap);
}

pub(super) struct ActiveCapture;

pub(super) fn begin_capture() -> ActiveCapture {
    *ACTIVE_DISPLAY_COUNT.lock().unwrap() += 1;
    ActiveCapture
}

impl Drop for ActiveCapture {
    fn drop(&mut self) {
        let mut count = ACTIVE_DISPLAY_COUNT.lock().unwrap();
        *count -= 1;
        let last = *count == 0;
        drop(count);
        if last {
            clear();
        }
    }
}

fn map_err_scrap(err: String) -> io::Error {
    // to-do: Handle error better, do not restart server
    // Reached by the capture loop, which is what this crude self-heal was for. A no-reply
    // during the portal handshake is tagged below and is reported instead of exiting: at
    // login there is someone waiting to be told, and a portal that is slow to activate is
    // not a reason to take the service down.
    if err.starts_with("Did not receive a reply") {
        log::error!("Fatal pipewire error, {}", &err);
        std::process::exit(-1);
    }

    if let Some(tag) = err.strip_prefix(WAYLAND_STAGE_TAG) {
        log_staged_once(&err);
        return io::Error::new(
            io::ErrorKind::Other,
            staged_message(tag, is_ubuntu_before_21()),
        );
    }

    if DISTRO.name.to_uppercase() == "Ubuntu".to_uppercase() {
        if DISTRO.version_id < "21".to_owned() {
            io::Error::new(io::ErrorKind::Other, SCRAP_UBUNTU_HIGHER_REQUIRED)
        } else {
            try_log(&err);
            io::Error::new(io::ErrorKind::Other, err)
        }
    } else {
        try_log(&err);
        let err_lower = err.to_ascii_lowercase();
        if err_lower.contains("org.freedesktop.portal")
            || err_lower.contains("dbus")
            || err_lower.contains("d-bus")
        {
            // The portal D-Bus interface is unreachable. This typically means
            // xdg-desktop-portal has crashed... for more info, see: Issue #12897
            io::Error::new(io::ErrorKind::Other, SCRAP_XDP_PORTAL_UNAVAILABLE)
        } else if err_lower.contains("pipewire") {
            io::Error::new(io::ErrorKind::Other, WAYLAND_NO_USABLE_SCREEN)
        } else {
            io::Error::new(io::ErrorKind::Other, WAYLAND_UNSUPPORTED)
        }
    }
}

/// `Display::all` and `Capturer::new` reach the peer through `map_err_scrap`, but
/// `fill_displays` opens a portal session of its own and returns its error straight up, so a
/// tag has to be resolved here or it lands in the login dialog verbatim.
fn map_staged_err(err: anyhow::Error) -> anyhow::Error {
    let text = err.to_string();
    match text.strip_prefix(WAYLAND_STAGE_TAG) {
        Some(tag) => {
            log_staged_once(&text);
            anyhow::anyhow!(staged_message(tag, is_ubuntu_before_21()))
        }
        None => err,
    }
}

// The video service retries about once a second, so a wedged portal would otherwise write a
// line a second forever. Repeat the message only when the cause changes, or after long
// enough that a reader would want to see the fault is still there.
const STAGE_ERR_REPEAT: std::time::Duration = std::time::Duration::from_secs(600);

fn log_staged_once(err: &str) {
    let now = std::time::Instant::now();
    let mut last = LAST_STAGE_ERR.lock().unwrap();
    let repeat = match last.as_ref() {
        Some((seen, at)) => seen != err || now.duration_since(*at) >= STAGE_ERR_REPEAT,
        None => true,
    };
    if repeat {
        log::error!("Wayland portal handshake failed: {}", err);
        *last = Some((err.to_owned(), now));
    }
}

fn try_log(err: &String) {
    let mut lock_count = LOG_SCRAP_COUNT.lock().unwrap();
    if *lock_count >= 1000000 {
        return;
    }
    if *lock_count % 10000 == 0 {
        log::error!("Failed scrap {}", err);
    }
    *lock_count += 1;
}

// Translation keys, so the key itself is the English text: an older peer that has never heard
// of them falls back to displaying the key and still reads as a sentence.
const WAYLAND_DECLINED: &str = "The screen sharing request was declined on the remote device";
const WAYLAND_TIMED_OUT: &str = "The screen sharing request timed out on the remote device";
const WAYLAND_NO_SESSION: &str = "RustDesk cannot reach the desktop session on the remote device, check that a desktop session is running and that RustDesk can use it";
const WAYLAND_UNSUPPORTED: &str = "The desktop portal on the remote device is missing a capability needed for screen sharing or remote control, its backend may not be installed";
const WAYLAND_PIPEWIRE_HANDOVER: &str = "Screen sharing was approved on the remote device, but the PipeWire connection could not be opened";
const WAYLAND_ENDED: &str =
    "The screen sharing request ended without completing on the remote device";
// The remedy the message it replaces used to carry, minus the link: this is the outcome
// rustdesk/rustdesk#8600 is about.
const WAYLAND_NO_USABLE_SCREEN: &str = "RustDesk could not obtain a usable screen from the XDG Desktop Portal, the PipeWire library may be too old";
const WAYLAND_GST_UNAVAILABLE: &str =
    "RustDesk could not load a GStreamer component needed for screen capture ({})";

const WAYLAND_STAGE_TAG: &str = "wl-stage:";

// `translate()` on the peer strips the braces itself, so what goes on the wire is the key
// with the detail still *inside* the placeholder.
fn with_detail(key: &str, detail: &str) -> String {
    key.replace("{}", &format!("{{{}}}", detail))
}

fn is_ubuntu_before_21() -> bool {
    DISTRO.name.to_uppercase() == "Ubuntu".to_uppercase() && DISTRO.version_id < "21".to_owned()
}

/// Maps a `<stage>:<kind>:<detail>` tag from the portal handshake, see
/// `scrap::wayland::pipewire`, onto what to tell the peer. Everything the capture loop reports
/// carries no tag and keeps the legacy substring heuristics above.
fn staged_message(tag: &str, ubuntu_before_21: bool) -> String {
    let mut parts = tag.splitn(3, ':');
    let stage = parts.next().unwrap_or_default();
    let kind = parts.next().unwrap_or_default();
    let detail = parts.next().unwrap_or_default().trim();

    // An outcome that says something about the machine is what the Ubuntu branch was written
    // for, so that branch keeps it. An outcome that says what a person did is a fact no distro
    // check can improve on.
    let of_the_machine = |msg: &str| {
        if ubuntu_before_21 {
            SCRAP_UBUNTU_HIGHER_REQUIRED.to_owned()
        } else {
            msg.to_owned()
        }
    };

    match (stage, kind) {
        (_, "declined") => WAYLAND_DECLINED.to_owned(),
        (_, "ended") => WAYLAND_ENDED.to_owned(),
        (_, "no-response") => WAYLAND_TIMED_OUT.to_owned(),
        ("streams", _) => of_the_machine(WAYLAND_NO_USABLE_SCREEN),
        ("gst-plugin", _) => of_the_machine(&with_detail(WAYLAND_GST_UNAVAILABLE, detail)),
        // The bus the portal lives on was never reached, so the portal has not been asked
        // anything yet and telling anyone to restart it would be a guess.
        ("session-bus", _) => of_the_machine(WAYLAND_NO_SESSION),
        // The portal answered `Start`, so the request was granted and the only thing left
        // was handing over the PipeWire connection. Whatever went wrong, it is not the
        // portal being unavailable -- it had just answered.
        ("open-pipewire-remote", _) => of_the_machine(WAYLAND_PIPEWIRE_HANDOVER),
        // The portal is there and answering; it just does not implement what was called,
        // which restarting it cannot fix.
        (_, "unsupported") => of_the_machine(WAYLAND_UNSUPPORTED),
        // Everything else is the portal not delivering, which is what this key already says --
        // and unlike a message of our own it carries the `systemctl --user restart` remedy.
        _ => of_the_machine(SCRAP_XDP_PORTAL_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wayland_capturer_has_one_owner_until_returned() {
        let slot = Arc::new(Mutex::new(Some(1)));
        let mut lease = CaptureLease::checkout(&slot).unwrap();
        assert!(CaptureLease::checkout(&slot).is_none());
        *lease.capturer() = 2;
        drop(lease);
        assert_eq!(*CaptureLease::checkout(&slot).unwrap().capturer(), 2);
    }

    #[test]
    fn staged_message_names_the_outcome() {
        let m = |tag| staged_message(tag, false);
        assert_eq!(m("start:declined:"), WAYLAND_DECLINED);
        assert_eq!(m("start:ended:"), WAYLAND_ENDED);
        assert_eq!(m("start:no-response:"), WAYLAND_TIMED_OUT);
        // A restored session shows no picker at all, so a timeout anywhere is a timeout and
        // never a claim about someone not answering.
        assert_eq!(m("create-session:no-response:"), WAYLAND_TIMED_OUT);
        assert_eq!(m("streams:empty:"), WAYLAND_NO_USABLE_SCREEN);
    }

    // The peer resolves a message by replacing its first `{...}` with `{}` and looking that
    // up, so a detail-carrying message has to reduce back to its key exactly.
    #[test]
    fn a_detail_carrying_message_reduces_back_to_its_key() {
        let gst = staged_message("gst-plugin:unavailable:pipewiresrc", false);
        assert_eq!(
            gst,
            "RustDesk could not load a GStreamer component needed for screen capture ({pipewiresrc})"
        );
        let open = gst.find('{').expect("no placeholder");
        let close = gst[open..].find('}').expect("unclosed placeholder") + open;
        assert_eq!(
            format!("{}{{}}{}", &gst[..open], &gst[close + 1..]),
            WAYLAND_GST_UNAVAILABLE
        );
    }
}

struct CaptureLease<T> {
    capturer: Option<T>,
    slot: Arc<Mutex<Option<T>>>,
}

impl<T> CaptureLease<T> {
    fn checkout(slot: &Arc<Mutex<Option<T>>>) -> Option<Self> {
        let capturer = slot.lock().unwrap().take()?;
        Some(Self {
            capturer: Some(capturer),
            slot: slot.clone(),
        })
    }

    fn capturer(&mut self) -> &mut T {
        self.capturer
            .as_mut()
            .expect("invariant: capture lease owns its capturer")
    }
}

impl<T> Drop for CaptureLease<T> {
    fn drop(&mut self) {
        *self.slot.lock().unwrap() = self.capturer.take();
    }
}

impl TraitCapturer for CaptureLease<Capturer> {
    fn frame<'a>(&'a mut self, timeout: std::time::Duration) -> std::io::Result<Frame<'a>> {
        self.capturer().frame(timeout)
    }
}

struct CapDisplayInfo {
    displays: Vec<DisplayInfo>,
    primary: usize,
    capturers: Vec<CapDisplay>,
}

struct CapDisplay {
    rect: ((i32, i32), usize, usize),
    capturer: Arc<Mutex<Option<Capturer>>>,
}

/// Uinput desktop rect from the DRM display list, for a login screen where no compositor can be
/// asked. `(minx, maxx, miny, maxy)`, in delivered-orientation physical pixels (a rotated
/// output counts transposed, matching its frames): no compositor here applied a scale, so
/// unlike `desktop_rect_of` there is no logical size to handle.
#[cfg(feature = "drm")]
fn drm_desktop_rect_for_uinput() -> Option<(i32, i32, i32, i32)> {
    let displays = super::drm_capturer::get_display_infos()?;
    if displays.is_empty() {
        return None;
    }
    let minx = displays.iter().map(|d| d.x).min()?;
    let miny = displays.iter().map(|d| d.y).min()?;
    let maxx = displays.iter().map(|d| d.x + d.width).max()?;
    let maxy = displays.iter().map(|d| d.y + d.height).max()?;
    if maxx <= minx || maxy <= miny {
        return None;
    }
    Some((minx, maxx, miny, maxy))
}

/// Set the uinput absolute-pointer range to the whole logical desktop so the compositor maps
/// injected coordinates 1:1 instead of stretching a single-monitor range across all outputs. The
/// PipeWire path does this inline in `check_init`; the DRM path bypasses check_init so it must do it
/// too, otherwise on a multi-monitor host the injected pointer lands on the wrong output — and the
/// hardware cursor, which lives on whichever CRTC the pointer is over, never appears on the captured
/// CRTC (the "cursor not visible" symptom). Reads the layout from the Wayland outputs, so it is
/// independent of the capture backend.
///
/// This is the DRM path's single copy of what `check_init` does inline for PipeWire, and it does the
/// same three things, for the same reasons:
///
/// - drops the cached Wayland layout first, because it can predate compositor changes made while no
///   session was active (rustdesk#15601), and on the hotplug path it is stale by definition;
/// - bounds the IPC wait, because `uinput::client::set_resolution` reads its reply with no timeout of
///   its own, so a hung uinput socket would otherwise block every video-service start on this branch
///   and wedge the hotplug worker inside `rt.block_on`, leaving `UINPUT_REFRESH_BUSY` latched true so
///   that every later hotplug refresh is silently skipped for the process lifetime;
/// - records the applied rect and snapshots the per-display layout baseline, which is what arms the
///   #15601 drift remap. Without it the remap never activates on the DRM path at all.
///
/// It stays a separate copy rather than being folded into `check_init` because `check_init` ships in
/// every Linux build and this feature must not change the drm-off one by so much as a line.
#[cfg(feature = "drm")]
pub(super) async fn update_uinput_resolution() {
    if !crate::input_service::wayland_use_uinput() {
        return;
    }
    // Compositor first at a login screen too: a greeter runs one, and the hbb_common socket
    // fallback reaches it with no environment variables. The DRM union is the fallback, and it is
    // a real loss to land there on a multi-monitor host: DRM has no origins, so its union rect
    // mis-maps the pointer whenever the compositor arranged the outputs side by side.
    //
    // Off the executor: the compositor query can block for the socket probe deadline, and this
    // runs on current-thread runtimes (session init and the hotplug worker). The layout baseline
    // is computed in the SAME task: a failed lookup is not cached, so asking for the rects
    // afterwards would rerun the whole socket probe synchronously.
    //
    // The generation read before the rect is computed labels the range for the DRM cursor
    // calibration; the raw DRM union below is no compositor layout and is labelled with none.
    let gen = scrap::wayland::display::wayland_snapshot_generation();
    let (rect, layout) = match hbb_common::tokio::task::spawn_blocking(|| {
        scrap::wayland::display::clear_wayland_displays_cache();
        match scrap::wayland::display::get_desktop_rect_for_uinput() {
            // The lookup above just cached the displays, so the rects come from that snapshot.
            Some(rect) => Some((rect, scrap::wayland::display::get_display_rects_for_uinput())),
            // Raw DRM union: there is no compositor layout to baseline. Empty keeps the #15601
            // remap inactive, which is right when the origins are unknown anyway.
            None => drm_desktop_rect_for_uinput().map(|rect| (rect, Vec::new())),
        }
    })
    .await
    {
        Ok(Some(pair)) => pair,
        Ok(None) => {
            log::warn!("Failed to get desktop rect for uinput");
            return;
        }
        Err(err) => {
            log::warn!("The desktop rect probe task failed: {err}");
            return;
        }
    };
    let gen = super::display_service::input_map_label(gen, !layout.is_empty());
    // Re-snapshot the baseline on every call: this runs at session init and after every hotplug, and
    // the baseline is what the client's coordinates are measured against.
    let snapshot_layout = move || {
        super::display_service::set_wayland_layout_baseline(layout.clone());
    };
    // Reprogram the device only when the range actually changes. A display stuck in a rebuild loop
    // calls this about once a second, and reapplying an identical range is an IPC roundtrip plus a
    // uinput device reconfiguration under a user who may be at the console.
    let job = move |rt: &tokio::runtime::Runtime| {
        if super::display_service::wayland_uinput_rect() == Some(rect) {
            super::display_service::note_input_map_ready(gen);
            snapshot_layout();
            return;
        }
        let (minx, maxx, miny, maxy) = rect;
        log::info!("update mouse resolution: ({minx}, {maxx}), ({miny}, {maxy})");
        super::display_service::note_input_map_unknown();
        match rt.block_on(async {
            timeout(
                3_000,
                input_service::update_mouse_resolution(minx, maxx, miny, maxy),
            )
            .await
        }) {
            // Record the rect only after a successful apply, so a transient failure is retried on
            // the next call instead of being remembered as applied.
            Ok(Ok(())) => {
                super::display_service::set_wayland_uinput_rect(rect);
                super::display_service::note_input_map_adopted(gen);
                snapshot_layout();
            }
            Ok(Err(err)) => log::error!("Failed to update mouse resolution: {}", err),
            Err(err) => log::error!("Failed to update mouse resolution: {}", err),
        }
    };
    if super::display_service::run_uinput_apply(job).await.is_err() {
        hbb_common::throttled_log!(
            super::display_service::UINPUT_APPLY_LOG_INTERVAL,
            error,
            "Failed to update mouse resolution: the uinput apply thread is gone"
        );
    }
}

#[tokio::main(flavor = "current_thread")]
pub(super) async fn ensure_inited() -> ResultType<()> {
    // DRM/KMS capture (opt-in): the root service owns the reader and the capturer self-inits over
    // IPC, so there is no PipeWire recorder to initialize here. But we still must set the uinput
    // desktop rect (check_init does this on the PipeWire path, and the DRM path skips check_init).
    #[cfg(feature = "drm")]
    if super::drm_capturer::is_available_cached() {
        update_uinput_resolution().await;
        return Ok(());
    }
    check_init().await
}

pub(super) fn is_inited() -> Option<Message> {
    {
        #[cfg(feature = "drm")]
        if super::drm_capturer::is_available_cached() {
            return None;
        }
        // try_read: an init waiting for the portal's picker holds the write lock, and this runs on
        // every desktop login. Busy (that init, or clear()) answers not inited yet.
        if !matches!(CAP_DISPLAY_INFO.try_read(), Ok(info) if info.is_some()) {
            let mut msg_out = Message::new();
            let res = MessageBox {
                msgtype: "nook-nocancel-hasclose".to_owned(),
                title: "Wayland".to_owned(),
                text: "Please Select the screen to be shared(Operate on the peer side).".to_owned(),
                link: "".to_owned(),
                ..Default::default()
            };
            msg_out.set_message_box(res);
            Some(msg_out)
        } else {
            None
        }
    }
}

pub(super) async fn check_init() -> ResultType<()> {
    if !crate::platform::linux::is_wayland_session_available() {
        bail!("Screen sharing requires an active Wayland session");
    }
    {
        // try_read for the same reason as is_inited; when busy, the same look waits off the worker.
        let looked = CAP_DISPLAY_INFO.try_read().map(|info| info.is_none()).ok();
        let empty = match looked {
            Some(empty) => empty,
            None => hbb_common::tokio::task::spawn_blocking(|| {
                CAP_DISPLAY_INFO.read().unwrap().is_none()
            })
            .await
            .map_err(|err| anyhow::anyhow!("Wayland display state task failed: {err}"))?,
        };
        if empty {
            if crate::input_service::wayland_use_uinput() {
                // The cached layout may predate compositor changes made while no session
                // was active, https://github.com/rustdesk/rustdesk/issues/15601
                // Read before the rect: an apply that races a layout move is recorded under the
                // older generation, which only streams built before the move measure under.
                #[cfg(feature = "drm")]
                let gen = scrap::wayland::display::wayland_snapshot_generation();
                scrap::wayland::display::clear_wayland_displays_cache();
                if let Some((minx, maxx, miny, maxy)) =
                    scrap::wayland::display::get_desktop_rect_for_uinput()
                {
                    log::info!(
                        "update mouse resolution: ({}, {}), ({}, {})",
                        minx,
                        maxx,
                        miny,
                        maxy
                    );
                    // A display that falls back to PipeWire in a DRM session runs this apply
                    // too: it goes through the uinput apply thread, in turn with the DRM paths.
                    #[cfg(feature = "drm")]
                    {
                        let job = move |rt: &tokio::runtime::Runtime| {
                            super::display_service::note_input_map_unknown();
                            match rt.block_on(async {
                                timeout(
                                    3_000,
                                    input_service::update_mouse_resolution(minx, maxx, miny, maxy),
                                )
                                .await
                            }) {
                                Ok(Ok(())) => {
                                    super::display_service::set_wayland_uinput_rect((
                                        minx, maxx, miny, maxy,
                                    ));
                                    super::display_service::note_input_map_adopted(gen);
                                    // Snapshot the per-display layout the client's coordinates
                                    // will be based on, so the mouse path can correct them if
                                    // the compositor moves a monitor mid-session.
                                    super::display_service::set_wayland_layout_baseline(
                                        scrap::wayland::display::get_display_rects_for_uinput(),
                                    );
                                }
                                Ok(Err(err)) => {
                                    log::error!("Failed to update mouse resolution: {}", err)
                                }
                                Err(err) => log::error!("Failed to update mouse resolution: {}", err),
                            }
                        };
                        if super::display_service::run_uinput_apply(job).await.is_err() {
                            hbb_common::throttled_log!(
                                super::display_service::UINPUT_APPLY_LOG_INTERVAL,
                                error,
                                "Failed to update mouse resolution: the uinput apply thread is gone"
                            );
                        }
                    }
                    // Bound the IPC wait like the periodic refresh does, so a hung
                    // response can't stall session init.
                    #[cfg(not(feature = "drm"))]
                    match timeout(
                        3_000,
                        input_service::update_mouse_resolution(minx, maxx, miny, maxy),
                    )
                    .await
                    {
                        Ok(Ok(())) => {
                            super::display_service::set_wayland_uinput_rect((
                                minx, maxx, miny, maxy,
                            ));
                            // Snapshot the per-display layout the client's coordinates
                            // will be based on, so the mouse path can correct them if
                            // the compositor moves a monitor mid-session.
                            super::display_service::set_wayland_layout_baseline(
                                scrap::wayland::display::get_display_rects_for_uinput(),
                            );
                        }
                        Ok(Err(err)) => log::error!("Failed to update mouse resolution: {}", err),
                        Err(err) => log::error!("Failed to update mouse resolution: {}", err),
                    }
                } else {
                    log::warn!("Failed to get desktop rect for uinput");
                }
            }

            // The portal can wait minutes for the user to answer its picker, under the write lock:
            // it serializes inits and orders them with clear(). Wait on a blocking thread, so no
            // tokio worker is parked meanwhile.
            hbb_common::tokio::task::spawn_blocking(init_displays)
                .await
                .map_err(|err| anyhow::anyhow!("Wayland display init task failed: {err}"))??;
        }
    }
    Ok(())
}

/// The part of `check_init` under the CAP_DISPLAY_INFO write lock, the wait for the user while the
/// portal's picker is open included. Blocking.
fn init_displays() -> ResultType<()> {
    let mut lock = CAP_DISPLAY_INFO.write().unwrap();
    if lock.is_none() {
        let mut all = Display::all()?;
        if all.is_empty() {
            bail!("PipeWire returned no capturable displays");
        }
        log::debug!("Initializing displays with fill_displays()");
        {
            let temp_mouse_move_handle = input_service::TemporaryMouseMoveHandle::new();
            let move_mouse_to = |x, y| temp_mouse_move_handle.move_mouse_to(x, y);
            fill_displays(move_mouse_to, crate::get_cursor_pos, &mut all)
                .map_err(map_staged_err)?;
        }
        log::debug!("Attempting to fix logical size with try_fix_logical_size()");
        try_fix_logical_size(&mut all);
        let num = all.len();
        let primary = super::display_service::get_primary_2(&all);
        let mut displays = super::display_service::describe_displays(&all);
        for display in displays.iter_mut() {
            display.cursor_embedded = is_cursor_embedded();
        }

        let mut rects: Vec<((i32, i32), usize, usize)> = Vec::new();
        for d in &all {
            rects.push((d.origin(), d.width(), d.height()));
        }

        log::debug!(
            "#displays={}, primary={}, rects: {:?}, cpus={}/{}",
            num,
            primary,
            rects,
            num_cpus::get_physical(),
            num_cpus::get()
        );

        let mut capturers = Vec::with_capacity(num);
        for (rect, display) in rects.into_iter().zip(all) {
            // No `with_context` here: the peer is shown `format!("{}", err)`, which
            // renders only the outermost layer, and the mapped reason is the inner one.
            let capturer = Capturer::new(display)?;
            capturers.push(CapDisplay {
                rect,
                capturer: Arc::new(Mutex::new(Some(capturer))),
            });
        }
        super::display_service::publish_displays(&displays);
        *lock = Some(CapDisplayInfo {
            displays,
            primary,
            capturers,
        });
    }
    Ok(())
}

pub(super) async fn get_displays_and_primary() -> ResultType<(Vec<DisplayInfo>, usize)> {
    #[cfg(feature = "drm")]
    if super::drm_capturer::is_available_cached() {
        // This function runs once per login (update_get_sync_displays_on_login is its only
        // caller), and login is the moment the client is PROMISED a display list -- so refresh
        // that list over a live `_drm` handshake first. The service wakes sleeping displays and
        // answers with the settled truth, which is what makes an unattended box with an idled,
        // DISABLED panel connectable at all: the cached list would either omit the panel (probed
        // while asleep) or advertise a display with no scanout behind it (probed while awake), and
        // either way the wake then firing inside the capture handshake would change the list the
        // client had already been given. Properly async, so the executor is never blocked; on any
        // failure the cache serves as before.
        super::drm_capturer::refresh_displays_for_login().await;
        let snapshot = hbb_common::tokio::task::spawn_blocking(
            super::drm_capturer::get_display_infos_and_primary,
        )
        .await
        .map_err(|err| anyhow::anyhow!("Wayland display probe task failed: {err}"))?;
        if let Some(snapshot) = snapshot {
            return Ok(snapshot);
        }
    }
    check_init().await?;
    // The read can wait for an init another thread started since: off the worker too.
    hbb_common::tokio::task::spawn_blocking(|| -> ResultType<(Vec<DisplayInfo>, usize)> {
        // Keep one read guard so clear/reinitialization cannot split these across cache snapshots.
        let cache = CAP_DISPLAY_INFO.read().unwrap();
        let info = cache
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Failed to get capturer display info"))?;
        Ok((info.displays.clone(), info.primary))
    })
    .await
    .map_err(|err| anyhow::anyhow!("Wayland display info task failed: {err}"))?
}

pub fn clear() {
    let mut cache = CAP_DISPLAY_INFO.write().unwrap();
    if *ACTIVE_DISPLAY_COUNT.lock().unwrap() != 0 {
        return;
    }
    // The DRM path augments its geometry from the compositor's Wayland outputs (logical origin +
    // scale), which scrap caches process-wide. The PipeWire path clears that cache on session close,
    // but the DRM path opens no PipeWire session, so without this it would keep matching DRM outputs
    // against STALE geometry after a monitor hotplug/rotation/scale change. Invalidate it on teardown
    // so the next session re-reads fresh geometry (lazily, on the next enumeration) and self-heals.
    #[cfg(feature = "drm")]
    if super::drm_capturer::is_available_cached() {
        scrap::wayland::display::clear_wayland_displays_cache();
    }
    // NOTE: intentionally do NOT reset the DRM probe cache here. `clear()` runs on every capturer
    // teardown (which happens on each video-service restart), and re-probing `_drm` from the async
    // enumeration path blocks the executor long enough to trip "deadline has elapsed" and spiral
    // into a restart loop. DRM availability is fixed at service start, so the cache stays valid.
    *cache = None;
}

/// The portal session close when the last remote connection goes (`AuthedConnID::drop`). A portal
/// request waiting for the user holds the session slot for up to 3 minutes, and the drop can run
/// on a tokio worker: the close waits on its own thread, and one runs at a time.
pub(super) fn try_close_portal_session() {
    static CLOSER: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
    // No session and no request pending (drm, x11, closed already): nothing to wait for.
    if matches!(scrap::wayland::pipewire::RDP_SESSION_INFO.try_lock(), Ok(info) if info.is_none()) {
        return;
    }
    let mut closer = CLOSER.lock().unwrap();
    // A close still running covers this one.
    if matches!(&*closer, Some(close) if !close.is_finished()) {
        return;
    }
    match std::thread::Builder::new()
        .name("portal-session-close".into())
        .spawn(|| {
            // The drop counted before this thread ran: count again.
            if !AUTHED_CONNS
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.conn_type == AuthConnType::Remote)
            {
                scrap::wayland::pipewire::try_close_session();
            }
        }) {
        Ok(close) => *closer = Some(close),
        // A peer can drop connections at any rate.
        Err(err) => hbb_common::throttled_log!(
            std::time::Duration::from_secs(60),
            error,
            "Failed to spawn the portal session close: {}",
            err
        ),
    }
}

/// Initialize the PipeWire/portal capture path from the plain (sync) video thread, so a DRM display
/// that cannot be captured can fall through to PipeWire for THAT display. `ensure_inited` short-circuits
/// to the DRM branch whenever DRM is globally available, so it never runs `check_init`; this helper
/// drives the same async portal ScreenCast init directly (mirroring `ensure_inited`'s pattern). Needed
/// because `is_available()` is a GLOBAL verdict — it stays true for the still-working DRM outputs — so
/// without a per-display fallback a single failed/demoted DRM display would restart-loop the video
/// service instead of degrading to PipeWire only for itself.
#[cfg(feature = "drm")]
#[tokio::main(flavor = "current_thread")]
async fn ensure_pipewire_inited() -> ResultType<()> {
    check_init().await
}

pub(super) fn get_capturer_for_display(
    display_idx: usize,
) -> ResultType<super::video_service::CapturerInfo> {
    if !crate::platform::linux::is_wayland_session_available() {
        bail!("Screen sharing requires an active Wayland session");
    }
    // DRM/KMS capture path: build the capturer straight from the service `_drm` stream, bypassing
    // the PipeWire CAP_DISPLAY_INFO machinery entirely. `is_available()` is a GLOBAL verdict, so a
    // per-display DRM failure (an ungrabbable/demoted CRTC, or — after the phase-2 split — a
    // render-node-absent seat or a convert failure on the unprivileged side) must NOT propagate out
    // and restart-loop this per-display video service. Instead fall THROUGH to PipeWire for just this
    // display; the other DRM outputs keep streaming over DRM.
    // Like the login, this gate keeps the probing form on purpose: it runs on the video thread,
    // not an async executor, and it is the capture-build path, so a definitive verdict is worth
    // seconds here. With the login it is what makes a cold cache recoverable -- warm_availability
    // gives up after its attempts, so if EVERY gate were cache-only a --server that started
    // before the root service would never see DRM again for the rest of its life.
    #[cfg(feature = "drm")]
    if super::drm_capturer::is_available() {
        match super::drm_capturer::get_capturer_info(display_idx) {
            Ok(info) => return Ok(info),
            Err(e) => {
                log::warn!(
                    "drm capturer for display {} unavailable ({:#}); falling back to PipeWire",
                    display_idx,
                    e
                );
                ensure_pipewire_inited()?;
            }
        }
    }
    // Resolved BEFORE the read guard below, deliberately. `get_display_infos` runs
    // `augment_with_wayland_geometry`, which is a compositor output roundtrip, and `clear()` takes
    // the WRITE guard on every capturer teardown -- which is exactly what is happening when a DRM
    // display is demoted or flapping, i.e. precisely when this path runs. Holding the read guard
    // across that roundtrip would stall every concurrent teardown for its duration, and the value
    // does not depend on anything inside the guard.
    #[cfg(feature = "drm")]
    let drm_advertised = if super::drm_capturer::is_available_cached() {
        match super::drm_capturer::get_display_infos() {
            Some(list) => Some((list.get(display_idx).cloned(), list.len() == 1)),
            None => Some((None, false)),
        }
    } else {
        None
    };
    let cache = CAP_DISPLAY_INFO.read().unwrap();
    let info = cache
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Failed to get capturer display info"))?;
    let display = info.capturers.get(display_idx).ok_or_else(|| {
        anyhow::anyhow!("Failed to get capturer display info for display {display_idx}")
    })?;
    let rect = display.rect;

    #[cfg(feature = "drm")]
    if let Some((Some(advertised), single_display)) = drm_advertised {
        // Both backends report physical dimensions. A full-workspace portal stream
        // can substitute for a single output, but a transposed stream would still
        // disagree with the client's advertised orientation.
        let single_display = single_display && info.capturers.len() == 1;
        let size_matches =
            advertised.width as usize == rect.1 && advertised.height as usize == rect.2;
        let transposed =
            advertised.width as usize == rect.2 && advertised.height as usize == rect.1;
        let consistent = advertised.x == rect.0 .0
            && advertised.y == rect.0 .1
            && (size_matches || (single_display && !transposed));
        if !consistent {
            super::drm_capturer::mark_fallback_rejected(display_idx);
            bail!(
                "drm display {} demoted with no geometry-consistent PipeWire stream{} (advertised {}x{}+{}+{} vs stream {}x{}+{}+{}); advertised offline",
                display_idx,
                if transposed {
                    " - stream is transposed vs advertised"
                } else {
                    ""
                },
                advertised.width,
                advertised.height,
                advertised.x,
                advertised.y,
                rect.1,
                rect.2,
                rect.0 .0,
                rect.0 .1
            );
        }
    }
    let mut capturer = CaptureLease::checkout(&display.capturer).ok_or_else(|| {
        anyhow::anyhow!("Wayland display {display_idx} is already being captured")
    })?;
    capturer.capturer().reset_frame_history();
    Ok(super::video_service::CapturerInfo {
        origin: rect.0,
        width: rect.1,
        height: rect.2,
        ndisplay: info.capturers.len(),
        current: display_idx,
        privacy_mode_id: 0,
        _capturer_privacy_mode_id: 0,
        capturer: Box::new(capturer),
    })
}

pub fn common_get_error() -> String {
    if DISTRO.name.to_uppercase() == "Ubuntu".to_uppercase() {
        if DISTRO.version_id < "21".to_owned() {
            return "".to_owned();
        }
    } else {
        // to-do: check other distros
    }
    return "".to_owned();
}
