use core_foundation::{
    base::{kCFAllocatorDefault, TCFType},
    mach_port::{CFMachPort, CFMachPortInvalidate, CFMachPortRef},
    runloop::{kCFRunLoopCommonModes, CFRunLoop, CFRunLoopSource},
    string::CFString,
    uuid::{CFUUIDCreateString, CFUUIDRef, CFUUID},
};
use core_graphics::{
    event::{
        CGEventField, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
        EventField,
    },
    event_source::CGEventSourceStateID,
    sys::CGEventRef,
};
use dispatch::Queue;
use hbb_common::{libc::pthread_main_np, log};
use scrap::quartz::Display;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ffi::c_void,
    io,
    ptr::null_mut,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

type EventCallback = unsafe extern "C" fn(*mut c_void, u32, CGEventRef, *mut c_void) -> CGEventRef;
type DisplayCallback = extern "C" fn(u32, u32, *mut c_void);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        location: CGEventTapLocation,
        placement: CGEventTapPlacement,
        options: CGEventTapOptions,
        events: u64,
        callback: EventCallback,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventGetIntegerValueField(event: CGEventRef, field: CGEventField) -> i64;
    fn CGDisplayRegisterReconfigurationCallback(
        callback: DisplayCallback,
        user_info: *mut c_void,
    ) -> i32;
    fn CGDisplayRemoveReconfigurationCallback(
        callback: DisplayCallback,
        user_info: *mut c_void,
    ) -> i32;
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> CFUUIDRef;
    fn CGDisplayGammaTableCapacity(display: u32) -> u32;
    fn CGGetDisplayTransferByTable(
        display: u32,
        capacity: u32,
        red: *mut f32,
        green: *mut f32,
        blue: *mut f32,
        count: *mut u32,
    ) -> i32;
    fn CGSetDisplayTransferByTable(
        display: u32,
        count: u32,
        red: *const f32,
        green: *const f32,
        blue: *const f32,
    ) -> i32;
    fn CGDisplayRestoreColorSyncSettings();
}

const BEGIN_CONFIGURATION: u32 = 1 << 0;
const SET_MODE: u32 = 1 << 3;
const ADD_DISPLAY: u32 = 1 << 4;
const REMOVE_DISPLAY: u32 = 1 << 5;
const MONITOR_DURATION: Duration = Duration::from_secs(5);
const MONITOR_INTERVAL: Duration = Duration::from_millis(200);

static ACTIVE: AtomicU64 = AtomicU64::new(0);
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

pub(crate) fn is_active() -> bool {
    ACTIVE.load(Ordering::Acquire) != 0
}

pub(crate) fn set_active(on: bool) -> io::Result<()> {
    if unsafe { pthread_main_np() } != 0 {
        set_on_main(on)
    } else {
        Queue::main().exec_sync(move || set_on_main(on))
    }
}

fn set_on_main(on: bool) -> io::Result<()> {
    SESSION.with(|slot| {
        let mut slot = slot.borrow_mut();
        if on {
            if slot.is_none() {
                *slot = Some(Session::start()?);
            }
        } else if let Some(session) = slot.take() {
            session.stop()?;
        }
        Ok(())
    })
}

struct EventTap {
    port: CFMachPort,
    source: CFRunLoopSource,
}

impl EventTap {
    fn install() -> io::Result<Self> {
        use CGEventType::*;
        let events = [
            KeyDown,
            KeyUp,
            FlagsChanged,
            LeftMouseDown,
            LeftMouseUp,
            RightMouseDown,
            RightMouseUp,
            OtherMouseDown,
            OtherMouseUp,
            LeftMouseDragged,
            RightMouseDragged,
            OtherMouseDragged,
            MouseMoved,
            ScrollWheel,
        ];
        let mask = events
            .into_iter()
            .fold(0, |mask, event| mask | (1_u64 << event as u32));
        let port = unsafe {
            CGEventTapCreate(
                CGEventTapLocation::HID,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                mask,
                filter_event,
                null_mut(),
            )
        };
        if port.is_null() {
            return Err(io::Error::other(
                "Could not create the privacy input event tap",
            ));
        }
        let port = unsafe { CFMachPort::wrap_under_create_rule(port) };
        let source = port
            .create_runloop_source(0)
            .map_err(|_| io::Error::other("Could not create the privacy input run-loop source"))?;
        unsafe {
            CFRunLoop::get_main().add_source(&source, kCFRunLoopCommonModes);
            CGEventTapEnable(port.as_concrete_TypeRef(), true);
        }
        Ok(Self { port, source })
    }

    fn enable(&self) {
        unsafe { CGEventTapEnable(self.port.as_concrete_TypeRef(), true) };
    }
}

impl Drop for EventTap {
    fn drop(&mut self) {
        unsafe {
            CGEventTapEnable(self.port.as_concrete_TypeRef(), false);
            CFRunLoop::get_main().remove_source(&self.source, kCFRunLoopCommonModes);
            CFMachPortInvalidate(self.port.as_concrete_TypeRef());
        }
    }
}

unsafe extern "C" fn filter_event(
    _proxy: *mut c_void,
    kind: u32,
    event: CGEventRef,
    _data: *mut c_void,
) -> CGEventRef {
    if kind == CGEventType::TapDisabledByTimeout as u32
        || kind == CGEventType::TapDisabledByUserInput as u32
    {
        let _ = SESSION.try_with(|slot| {
            if let Ok(slot) = slot.try_borrow() {
                if let Some(session) = &*slot {
                    session.tap.enable();
                }
            }
        });
        return event;
    }
    if event.is_null() {
        return event;
    }
    let tag = unsafe { CGEventGetIntegerValueField(event, EventField::EVENT_SOURCE_USER_DATA) };
    let source = unsafe { CGEventGetIntegerValueField(event, EventField::EVENT_SOURCE_STATE_ID) };
    if tag != enigo::ENIGO_INPUT_EXTRA_VALUE
        && source == CGEventSourceStateID::HIDSystemState as i64
    {
        null_mut()
    } else {
        event
    }
}

struct Session {
    generation: u64,
    tap: EventTap,
    saved: BTreeMap<String, Gamma>,
    monitor_until: Option<Instant>,
}

impl Session {
    fn start() -> io::Result<Self> {
        let mut session = Self {
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            tap: EventTap::install()?,
            saved: BTreeMap::new(),
            monitor_until: None,
        };
        ACTIVE.store(session.generation, Ordering::Release);
        let status =
            unsafe { CGDisplayRegisterReconfigurationCallback(display_changed, null_mut()) };
        if let Err(error) = cg_result("Register privacy display notification", status) {
            ACTIVE.store(0, Ordering::Release);
            return Err(error);
        }
        if let Err(error) = session.blackout_all(false) {
            if let Err(restore) = session.stop() {
                log::error!("Could not restore displays after enabling privacy failed: {restore}");
            }
            return Err(error);
        }
        Ok(session)
    }

    fn stop(self) -> io::Result<()> {
        let removed =
            unsafe { CGDisplayRemoveReconfigurationCallback(display_changed, null_mut()) };
        drop(self.tap);
        let result = restore_gammas(self.saved);
        if result.is_err() {
            unsafe { CGDisplayRestoreColorSyncSettings() };
        }
        ACTIVE.store(0, Ordering::Release);
        result.and(cg_result("Remove privacy display notification", removed))
    }

    fn blackout_all(&mut self, monitor: bool) -> io::Result<()> {
        for display in online_displays()? {
            let id = display.id();
            let uuid = display_uuid(id)?;
            let current = Gamma::read(id)?;
            let should_blackout = !monitor || !current.is_blacked_out();
            self.saved.entry(uuid).or_insert(current);
            if should_blackout {
                Gamma::blackout(id)?;
            }
        }
        Ok(())
    }
}

extern "C" fn display_changed(_display: u32, flags: u32, _data: *mut c_void) {
    if flags & BEGIN_CONFIGURATION != 0 || flags & (SET_MODE | ADD_DISPLAY | REMOVE_DISPLAY) == 0 {
        return;
    }
    let generation = ACTIVE.load(Ordering::Acquire);
    if generation != 0 {
        // Keep WindowServer callbacks outside the main-thread transaction and its cleanup.
        Queue::main().exec_async(move || adjust_blackout(generation));
    }
}

fn adjust_blackout(generation: u64) {
    let result = SESSION.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(session) = slot
            .as_mut()
            .filter(|session| session.generation == generation)
        else {
            return Ok(());
        };
        session.blackout_all(true)?;
        if session
            .monitor_until
            .replace(Instant::now() + MONITOR_DURATION)
            .is_none()
        {
            schedule_monitor(generation);
        }
        Ok(())
    });
    stop_on_failure(result);
}

fn schedule_monitor(generation: u64) {
    Queue::main().exec_after(MONITOR_INTERVAL, move || {
        let result = SESSION.with(|slot| {
            let mut slot = slot.borrow_mut();
            let Some(session) = slot
                .as_mut()
                .filter(|session| session.generation == generation)
            else {
                return Ok(());
            };
            session.blackout_all(true)?;
            if session
                .monitor_until
                .is_some_and(|until| Instant::now() < until)
            {
                schedule_monitor(generation);
            } else {
                session.monitor_until = None;
            }
            Ok(())
        });
        stop_on_failure(result);
    });
}

fn stop_on_failure(result: io::Result<()>) {
    if let Err(error) = result {
        log::error!("Privacy blackout could not be maintained: {error}");
        if let Err(error) = set_on_main(false) {
            log::error!("Could not restore displays after privacy ended: {error}");
        }
    }
}

struct Gamma {
    red: Vec<f32>,
    green: Vec<f32>,
    blue: Vec<f32>,
}

impl Gamma {
    fn read(id: u32) -> io::Result<Self> {
        let capacity = Self::capacity(id)?;
        let mut red = vec![0.0; capacity];
        let mut green = vec![0.0; capacity];
        let mut blue = vec![0.0; capacity];
        let mut count = 0;
        let result = unsafe {
            CGGetDisplayTransferByTable(
                id,
                capacity as u32,
                red.as_mut_ptr(),
                green.as_mut_ptr(),
                blue.as_mut_ptr(),
                &mut count,
            )
        };
        cg_result("Read display gamma", result)?;
        let count = count as usize;
        if count == 0 || count > capacity {
            return Err(io::Error::other(format!(
                "Display {id} returned an invalid gamma table size"
            )));
        }
        red.truncate(count);
        green.truncate(count);
        blue.truncate(count);
        Ok(Self { red, green, blue })
    }

    fn capacity(id: u32) -> io::Result<usize> {
        let capacity = unsafe { CGDisplayGammaTableCapacity(id) };
        if capacity == 0 {
            return Err(io::Error::other(format!(
                "Display {id} provides no gamma table"
            )));
        }
        Ok(capacity as usize)
    }

    fn is_blacked_out(&self) -> bool {
        self.red
            .iter()
            .chain(&self.green)
            .chain(&self.blue)
            .all(|&value| value <= 0.01)
    }

    fn blackout(id: u32) -> io::Result<()> {
        let black = vec![0.0; Self::capacity(id)?];
        Self::write(id, &black, &black, &black)
    }

    fn write(id: u32, red: &[f32], green: &[f32], blue: &[f32]) -> io::Result<()> {
        let result = unsafe {
            CGSetDisplayTransferByTable(
                id,
                red.len() as u32,
                red.as_ptr(),
                green.as_ptr(),
                blue.as_ptr(),
            )
        };
        cg_result("Set display gamma", result)
    }
}

fn restore_gammas(saved: BTreeMap<String, Gamma>) -> io::Result<()> {
    if saved.is_empty() {
        return Ok(());
    }
    let mut result = Ok(());
    for display in online_displays()? {
        let id = display.id();
        let restored = display_uuid(id).and_then(|uuid| {
            if let Some(gamma) = saved.get(&uuid) {
                Gamma::write(id, &gamma.red, &gamma.green, &gamma.blue)?;
            }
            Ok(())
        });
        result = result.and(restored);
    }
    result
}

fn display_uuid(id: u32) -> io::Result<String> {
    let uuid = unsafe { CGDisplayCreateUUIDFromDisplayID(id) };
    if uuid.is_null() {
        return Err(io::Error::other(format!("Display {id} has no UUID")));
    }
    let uuid = unsafe { CFUUID::wrap_under_create_rule(uuid) };
    let uuid = unsafe { CFUUIDCreateString(kCFAllocatorDefault, uuid.as_concrete_TypeRef()) };
    if uuid.is_null() {
        return Err(io::Error::other(format!(
            "Could not read display {id} UUID"
        )));
    }
    Ok(unsafe { CFString::wrap_under_create_rule(uuid) }.to_string())
}

fn online_displays() -> io::Result<Vec<Display>> {
    Display::online()
        .map_err(|error| io::Error::other(format!("Read online displays failed ({error:?})")))
}

fn cg_result(operation: &str, code: i32) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{operation} failed (CoreGraphics error {code})"
        )))
    }
}
