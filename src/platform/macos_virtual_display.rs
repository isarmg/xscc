use core_foundation::{
    array::{CFArray, CFArrayRef},
    base::TCFType,
    dictionary::CFDictionaryRef,
};
use core_graphics::{
    display::{CGConfigureOption, CGDisplay},
    sys::CGDisplayModeRef,
};
use objc2::{
    msg_send,
    rc::{autoreleasepool, Allocated, Retained},
    runtime::{AnyClass, AnyObject, Sel},
    sel,
};
use objc2_foundation::{
    NSArray, NSOperatingSystemVersion, NSPoint, NSProcessInfo, NSSize, NSString,
};
use std::{
    ffi::{c_void, CStr},
    io,
    num::NonZeroU32,
    ptr,
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const VENDOR_ID: u32 = 0x5853;
const PRODUCT_ID: u32 = 0x4343;
const OBSERVE_TIMEOUT: Duration = Duration::from_secs(2);
const WORKER_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(25);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayCopyAllDisplayModes(display: u32, options: CFDictionaryRef) -> CFArrayRef;
    fn CGDisplayModeGetPixelWidth(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeGetRefreshRate(mode: CGDisplayModeRef) -> f64;
    fn CGDisplaySetDisplayMode(
        display: u32,
        mode: CGDisplayModeRef,
        options: CFDictionaryRef,
    ) -> i32;
}

unsafe extern "C" {
    fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut AnyObject;
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh_units: u32,
}

/// The worker owns the Objective-C display. Dropping the sender signals it to release the display.
pub(crate) struct VirtualDisplay {
    id: u32,
    serial: NonZeroU32,
    commands: Mutex<Option<Sender<SetMode>>>,
}

struct SetMode {
    mode: Mode,
    deadline: Instant,
    result: Sender<io::Result<()>>,
}

pub(crate) fn available() -> bool {
    cfg!(target_arch = "aarch64") && DisplayClasses::probe().is_ok()
}

fn is_active(id: u32) -> bool {
    id != 0 && CGDisplay::new(id).is_active()
}

fn current_mode(id: u32) -> io::Result<Mode> {
    if !is_active(id) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Display is inactive",
        ));
    }
    let mode = CGDisplay::new(id)
        .display_mode()
        .ok_or_else(|| io::Error::other("CoreGraphics did not return a current display mode"))?;
    observed_mode(mode.pixel_width(), mode.pixel_height(), mode.refresh_rate())
}

impl VirtualDisplay {
    pub(crate) fn create(name: &str, serial: NonZeroU32, modes: &[Mode]) -> io::Result<Self> {
        if name.trim().is_empty() || modes.is_empty() || modes.iter().any(|mode| !valid_mode(*mode))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Virtual display requires a name and positive mode dimensions and refresh rates",
            ));
        }

        let classes = DisplayClasses::probe()?;
        let (ready, registered) = mpsc::channel();
        let (commands, requests) = mpsc::channel::<SetMode>();
        let name = name.to_owned();
        let initial_mode = modes[0];
        let modes = modes.to_vec();
        thread::Builder::new()
            .name("macos-virtual-display".into())
            .spawn(move || {
                let created = autoreleasepool(|_| classes.register(&name, serial, &modes));
                let (display, id) = match created {
                    Ok(created) => created,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };

                if ready.send(Ok(id)).is_ok() {
                    for request in requests {
                        if Instant::now() >= request.deadline {
                            continue;
                        }
                        let result = autoreleasepool(|_| apply_mode(id, serial, request.mode));
                        let _ = request.result.send(result);
                    }
                }
                autoreleasepool(|_| drop(display));
            })?;

        let id = wait_for_worker(&registered, "create the virtual display")??;
        let display = Self {
            id,
            serial,
            commands: Mutex::new(Some(commands)),
        };
        display.set_mode(initial_mode)?;
        Ok(display)
    }

    pub(crate) fn id(&self) -> u32 {
        self.id
    }

    pub(crate) fn is_online(&self) -> bool {
        owns_online_display(self.id, self.serial)
    }

    pub(crate) fn close(&self) {
        self.commands.lock().unwrap().take();
    }

    pub(crate) fn is_active(&self) -> bool {
        owns_active_display(self.id, self.serial)
    }

    pub(crate) fn mode(&self) -> io::Result<Mode> {
        if !self.is_active() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Virtual display is inactive",
            ));
        }
        current_mode(self.id)
    }

    pub(crate) fn set_mode(&self, mode: Mode) -> io::Result<()> {
        if !valid_mode(mode) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid display mode",
            ));
        }
        if self.mode()? == mode {
            return Ok(());
        }
        let (result, observed) = mpsc::channel();
        {
            let commands = self.commands.lock().unwrap();
            commands
                .as_ref()
                .ok_or_else(|| io::Error::other("The virtual display has been removed"))?
                .send(SetMode {
                    mode,
                    deadline: Instant::now() + WORKER_TIMEOUT,
                    result,
                })
                .map_err(|_| io::Error::other("The virtual display worker stopped"))?;
        }
        wait_for_worker(&observed, "switch the virtual display mode")?
    }
}

fn valid_mode(mode: Mode) -> bool {
    mode.width > 0 && mode.height > 0 && mode.refresh_units > 0
}

fn observed_mode(width: u64, height: u64, refresh: f64) -> io::Result<Mode> {
    let refresh_units = (refresh * 10_000.0).round();
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid CoreGraphics display mode",
        )
    };
    if !(1.0..=f64::from(u32::MAX)).contains(&refresh_units) {
        return Err(invalid());
    }
    let mode = Mode {
        width: width.try_into().map_err(|_| invalid())?,
        height: height.try_into().map_err(|_| invalid())?,
        refresh_units: refresh_units as u32,
    };
    if !valid_mode(mode) {
        return Err(invalid());
    }
    Ok(mode)
}

fn apply_mode(id: u32, serial: NonZeroU32, requested: Mode) -> io::Result<()> {
    if !owns_active_display(id, serial) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Virtual display is inactive",
        ));
    }

    let raw = unsafe { CGDisplayCopyAllDisplayModes(id, ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::other(
            "CoreGraphics did not return available display modes",
        ));
    }
    // The copied array owns every borrowed mode, including during the call that installs it.
    let modes = unsafe { CFArray::<*const c_void>::wrap_under_create_rule(raw) };
    let selected = modes
        .iter()
        .map(|mode| (*mode).cast_mut().cast())
        .find(|&mode| {
            let reported = unsafe {
                observed_mode(
                    CGDisplayModeGetPixelWidth(mode) as u64,
                    CGDisplayModeGetPixelHeight(mode) as u64,
                    CGDisplayModeGetRefreshRate(mode),
                )
            };
            reported.is_ok_and(|mode| mode == requested)
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "CoreGraphics does not advertise the requested virtual display mode",
            )
        })?;
    let status = unsafe { CGDisplaySetDisplayMode(id, selected, ptr::null()) };
    if status != 0 {
        return Err(cg_error("Set virtual display mode", status));
    }
    wait_until("switch the virtual display mode", || {
        if !owns_active_display(id, serial) {
            return Ok(false);
        }
        Ok(current_mode(id)? == requested)
    })
}

fn owns_active_display(id: u32, serial: NonZeroU32) -> bool {
    owns_online_display(id, serial) && is_active(id) && !CGDisplay::new(id).is_in_mirror_set()
}

fn owns_online_display(id: u32, serial: NonZeroU32) -> bool {
    let display = CGDisplay::new(id);
    id != 0
        && display.is_online()
        && display.vendor_number() == VENDOR_ID
        && display.model_number() == PRODUCT_ID
        && display.serial_number() == serial.get()
}

fn cg_error(operation: &str, status: i32) -> io::Error {
    io::Error::other(format!("{operation} failed (CoreGraphics error {status})"))
}

fn wait_for_worker<T>(receiver: &Receiver<T>, operation: &str) -> io::Result<T> {
    receiver
        .recv_timeout(WORKER_TIMEOUT)
        .map_err(|error| match error {
            RecvTimeoutError::Timeout => io::Error::new(
                io::ErrorKind::TimedOut,
                format!("Timed out waiting for the worker to {operation}"),
            ),
            RecvTimeoutError::Disconnected => io::Error::other(format!(
                "Virtual display worker stopped while trying to {operation}"
            )),
        })
}

fn wait_until(operation: &str, mut observe: impl FnMut() -> io::Result<bool>) -> io::Result<()> {
    let started = Instant::now();
    loop {
        if observe()? {
            return Ok(());
        }
        let remaining = OBSERVE_TIMEOUT.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("CoreGraphics did not {operation}"),
            ));
        }
        thread::sleep(remaining.min(POLL_INTERVAL));
    }
}

struct NativeDisplay {
    _display: Retained<AnyObject>,
    _descriptor: Retained<AnyObject>,
    _settings: Retained<AnyObject>,
}

struct DisplayClasses {
    descriptor: &'static AnyClass,
    display: &'static AnyClass,
    settings: &'static AnyClass,
    mode: &'static AnyClass,
}

impl DisplayClasses {
    fn probe() -> io::Result<Self> {
        let minimum = NSOperatingSystemVersion {
            majorVersion: 27,
            minorVersion: 0,
            patchVersion: 0,
        };
        let supported = autoreleasepool(|_| {
            NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(minimum)
        });
        if !supported {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Virtual displays require macOS 27 or newer",
            ));
        }

        // Private selector and ABI references:
        // https://github.com/clintcan/macrdp/blob/main/src/virtual_display/private_api.rs
        // https://chromium.googlesource.com/chromium/src/+/HEAD/ui/display/mac/test/virtual_display_util_mac.mm
        Ok(Self {
            descriptor: required_class(
                c"CGVirtualDisplayDescriptor",
                &[
                    sel!(init),
                    sel!(setName:),
                    sel!(setVendorID:),
                    sel!(setProductID:),
                    sel!(setSerialNum:),
                    sel!(setMaxPixelsWide:),
                    sel!(setMaxPixelsHigh:),
                    sel!(setSizeInMillimeters:),
                    sel!(setRedPrimary:),
                    sel!(setGreenPrimary:),
                    sel!(setBluePrimary:),
                    sel!(setWhitePoint:),
                    sel!(setQueue:),
                ],
            )?,
            display: required_class(
                c"CGVirtualDisplay",
                &[
                    sel!(initWithDescriptor:),
                    sel!(applySettings:),
                    sel!(displayID),
                ],
            )?,
            settings: required_class(
                c"CGVirtualDisplaySettings",
                &[sel!(init), sel!(setModes:), sel!(setHiDPI:)],
            )?,
            mode: required_class(
                c"CGVirtualDisplayMode",
                &[sel!(initWithWidth:height:refreshRate:)],
            )?,
        })
    }

    fn register(
        &self,
        name: &str,
        serial: NonZeroU32,
        modes: &[Mode],
    ) -> io::Result<(NativeDisplay, u32)> {
        let previous = CGDisplay::active_displays()
            .map_err(|status| cg_error("Read active displays", status))?;
        let descriptor = self.descriptor(name, serial, modes)?;
        let settings = self.settings(modes)?;

        // The probe checked that the private selectors are available before sending messages.
        let display: Option<Retained<AnyObject>> = unsafe {
            let object: Allocated<AnyObject> = msg_send![self.display, alloc];
            msg_send![object, initWithDescriptor: &*descriptor]
        };
        let display = display
            .ok_or_else(|| io::Error::other("CoreGraphics rejected the display descriptor"))?;
        let applied: bool = unsafe { msg_send![&*display, applySettings: &*settings] };
        if !applied {
            return Err(io::Error::other(
                "CoreGraphics rejected the virtual display modes",
            ));
        }
        let id: u32 = unsafe { msg_send![&*display, displayID] };
        if id == 0 || previous.contains(&id) {
            return Err(io::Error::other(
                "CoreGraphics did not allocate a new display ID",
            ));
        }

        wait_until("attach the virtual display", || {
            Ok(owns_online_display(id, serial))
        })?;
        detach_virtual_mirror(id)?;
        wait_until("activate the virtual display", || {
            let active = CGDisplay::active_displays()
                .map_err(|status| cg_error("Read active displays", status))?;
            Ok(active.contains(&id)
                && owns_online_display(id, serial)
                && !CGDisplay::new(id).is_in_mirror_set())
        })?;

        Ok((
            NativeDisplay {
                _display: display,
                _descriptor: descriptor,
                _settings: settings,
            },
            id,
        ))
    }

    fn descriptor(
        &self,
        name: &str,
        serial: NonZeroU32,
        modes: &[Mode],
    ) -> io::Result<Retained<AnyObject>> {
        let width = modes
            .iter()
            .map(|mode| mode.width)
            .max()
            .unwrap_or_default();
        let height = modes
            .iter()
            .map(|mode| mode.height)
            .max()
            .unwrap_or_default();
        let physical = NSSize::new(
            f64::from(width) * 25.4 / 96.0,
            f64::from(height) * 25.4 / 96.0,
        );
        let name = NSString::from_str(name);
        // The global dispatch queue has process lifetime. On macOS it is an Objective-C object.
        let queue = unsafe { dispatch_get_global_queue(0, 0).as_ref() }
            .ok_or_else(|| io::Error::other("System dispatch queue is unavailable"))?;
        let descriptor = new_object(self.descriptor, "display descriptor")?;
        unsafe {
            let _: () = msg_send![&*descriptor, setName: &*name];
            let _: () = msg_send![&*descriptor, setVendorID: VENDOR_ID];
            let _: () = msg_send![&*descriptor, setProductID: PRODUCT_ID];
            let _: () = msg_send![&*descriptor, setSerialNum: serial.get()];
            if self
                .descriptor
                .instance_method(sel!(setSerialNumber:))
                .is_some()
            {
                let _: () = msg_send![&*descriptor, setSerialNumber: serial.get()];
            }
            let _: () = msg_send![&*descriptor, setMaxPixelsWide: width];
            let _: () = msg_send![&*descriptor, setMaxPixelsHigh: height];
            let _: () = msg_send![&*descriptor, setSizeInMillimeters: physical];
            let _: () = msg_send![&*descriptor, setRedPrimary: NSPoint::new(0.64, 0.33)];
            let _: () = msg_send![&*descriptor, setGreenPrimary: NSPoint::new(0.30, 0.60)];
            let _: () = msg_send![&*descriptor, setBluePrimary: NSPoint::new(0.15, 0.06)];
            let _: () = msg_send![&*descriptor, setWhitePoint: NSPoint::new(0.3127, 0.329)];
            let _: () = msg_send![&*descriptor, setQueue: queue];
        }
        Ok(descriptor)
    }

    fn settings(&self, modes: &[Mode]) -> io::Result<Retained<AnyObject>> {
        let modes = modes
            .iter()
            .map(|mode| {
                let result: Option<Retained<AnyObject>> = unsafe {
                    let allocated: Allocated<AnyObject> = msg_send![self.mode, alloc];
                    msg_send![
                        allocated,
                        initWithWidth: mode.width,
                        height: mode.height,
                        refreshRate: f64::from(mode.refresh_units) / 10_000.0,
                    ]
                };
                result
                    .ok_or_else(|| io::Error::other("CoreGraphics rejected a virtual display mode"))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let modes = NSArray::from_retained_slice(&modes);
        let settings = new_object(self.settings, "display settings")?;
        unsafe {
            let _: () = msg_send![&*settings, setHiDPI: 0_u32];
            let _: () = msg_send![&*settings, setModes: &*modes];
        }
        Ok(settings)
    }
}

fn required_class(name: &CStr, selectors: &[Sel]) -> io::Result<&'static AnyClass> {
    AnyClass::get(name)
        .filter(|class| {
            selectors
                .iter()
                .all(|&selector| class.instance_method(selector).is_some())
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "CoreGraphics lacks the virtual display interface {}",
                    name.to_string_lossy()
                ),
            )
        })
}

fn new_object(class: &AnyClass, description: &str) -> io::Result<Retained<AnyObject>> {
    let result: Option<Retained<AnyObject>> = unsafe {
        let allocated: Allocated<AnyObject> = msg_send![class, alloc];
        msg_send![allocated, init]
    };
    result.ok_or_else(|| io::Error::other(format!("CoreGraphics could not allocate {description}")))
}

fn detach_virtual_mirror(id: u32) -> io::Result<()> {
    let display = CGDisplay::new(id);
    if !display.is_in_mirror_set() {
        return Ok(());
    }
    if display.mirrors_display() == 0 {
        return Err(io::Error::other("Virtual display is a mirror source"));
    }
    let config = display
        .begin_configuration()
        .map_err(|status| cg_error("Begin virtual display configuration", status))?;
    if let Err(status) =
        display.configure_display_mirror_of_display(&config, &CGDisplay::null_display())
    {
        let _ = display.cancel_configuration(&config);
        return Err(cg_error("Disconnect virtual display mirroring", status));
    }
    display
        .complete_configuration(&config, CGConfigureOption::ConfigureForSession)
        .map_err(|status| cg_error("Commit virtual display configuration", status))
}
