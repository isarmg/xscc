use crate::{
    profile::{Colour, Profile},
    swapchain::drain,
    timing::Signal,
};
use std::{
    ffi::{OsString, c_void},
    io,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    ptr,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

type Handle = *mut c_void;
type Status = i32;

const SUCCESS: Status = 0;
const INVALID_DEVICE_STATE: Status = 0xc000_0184_u32 as i32;
const DATA_ERROR: Status = 0xc000_003e_u32 as i32;
const BUFFER_TOO_SMALL: Status = 0xc000_0023_u32 as i32;
const UNSUCCESSFUL: Status = 0xc000_0001_u32 as i32;
const IO_PENDING: Status = 0x8000_000a_u32 as i32;

const WAIT_OBJECT_0: u32 = 0;
const WAIT_FRAME: u32 = WAIT_OBJECT_0 + 1;
const WAIT_TIMEOUT: u32 = 258;

static PROFILE: Mutex<Weak<Profile>> = Mutex::new(Weak::new());

#[repr(C)]
#[derive(Default)]
struct Frame {
    acquired_at: i64,
    number: u32,
}

unsafe extern "C" {
    fn xscc_configuration_path(buffer: *mut u16, capacity: u32) -> u32;
    fn xscc_initialize_adapter(device: Handle, count: u32) -> Status;
    fn xscc_prefer_render_adapter(adapter: Handle, name: *const u16);
    fn xscc_arrive_monitor(adapter: Handle, index: u32, edid: *const u8, length: u32) -> Status;
    fn xscc_bind_swapchain(chain: Handle, luid: i64, device: *mut Handle) -> Status;
    fn xscc_release_device(device: Handle);
    fn xscc_acquire_frame(chain: Handle, frame: *mut Frame) -> Status;
    fn xscc_finish_frame(chain: Handle, frame: *const Frame) -> Status;
    fn xscc_delete_swapchain(chain: Handle);
}

unsafe extern "system" {
    fn CreateEventW(
        attributes: Handle,
        manual_reset: i32,
        initial_state: i32,
        name: *const u16,
    ) -> Handle;
    fn SetEvent(event: Handle) -> i32;
    fn CloseHandle(handle: Handle) -> i32;
    fn WaitForMultipleObjects(
        count: u32,
        handles: *const Handle,
        wait_all: i32,
        timeout: u32,
    ) -> u32;
    fn AvSetMmThreadCharacteristicsW(name: *const u16, index: *mut u32) -> Handle;
    fn AvRevertMmThreadCharacteristics(handle: Handle) -> i32;
    fn OutputDebugStringW(message: *const u16);
}

struct Device {
    profile: Arc<Profile>,
    initialized: AtomicBool,
}

struct Monitor {
    profile: Arc<Profile>,
    worker: Mutex<Option<Worker>>,
}

fn active_profile() -> Option<Arc<Profile>> {
    PROFILE.lock().ok()?.upgrade()
}

fn load_profile() -> io::Result<Arc<Profile>> {
    if let Some(profile) = active_profile() {
        return Ok(profile);
    }

    let mut buffer = [0u16; 32_768];
    let length = unsafe { xscc_configuration_path(buffer.as_mut_ptr(), buffer.len() as u32) };
    let directory = match length {
        0 => PathBuf::from(r"C:\VirtualDisplayDriver"),
        length if (length as usize) < buffer.len() => {
            let path = PathBuf::from(OsString::from_wide(&buffer[..length as usize]));
            if !path.is_absolute() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Relative VDDPATH",
                ));
            }
            path
        }
        _ => return Err(io::Error::last_os_error()),
    };
    let profile = Arc::new(Profile::load(&directory)?);
    let mut active = PROFILE
        .lock()
        .map_err(|_| io::Error::other("Virtual display profile lock"))?;
    if let Some(existing) = active.upgrade() {
        return Ok(existing);
    }
    *active = Arc::downgrade(&profile);
    Ok(profile)
}

fn diagnostic(message: &str) {
    let text: Vec<_> = "XSCC virtual display: "
        .encode_utf16()
        .chain(message.encode_utf16())
        .chain([10, 0])
        .collect();
    unsafe { OutputDebugStringW(text.as_ptr()) };
}

#[unsafe(no_mangle)]
unsafe extern "C" fn xscc_create_device_state(out: *mut Handle) -> Status {
    match load_profile() {
        Ok(profile) => {
            let state = Box::new(Device {
                profile,
                initialized: AtomicBool::new(false),
            });
            unsafe { out.write(Box::into_raw(state).cast()) };
            SUCCESS
        }
        Err(error) => {
            diagnostic(&error.to_string());
            DATA_ERROR
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn xscc_destroy_device_state(state: Handle) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<Device>()) });
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn xscc_start_device(state: Handle, device: Handle) -> Status {
    let Some(state) = (unsafe { state.cast::<Device>().as_ref() }) else {
        return INVALID_DEVICE_STATE;
    };
    if state.initialized.swap(true, Ordering::AcqRel) {
        return SUCCESS;
    }
    let status = unsafe { xscc_initialize_adapter(device, state.profile.monitor_count) };
    if status < 0 {
        state.initialized.store(false, Ordering::Release);
    }
    status
}

#[unsafe(no_mangle)]
unsafe extern "C" fn xscc_start_monitors(adapter: Handle) -> Status {
    let Some(profile) = active_profile() else {
        return INVALID_DEVICE_STATE;
    };
    let gpu = profile.render_adapter.as_deref().unwrap_or_default();
    let name: Vec<_> = gpu.encode_utf16().chain([0]).collect();
    unsafe { xscc_prefer_render_adapter(adapter, name.as_ptr()) };

    for index in 0..profile.monitor_count {
        let status = unsafe {
            xscc_arrive_monitor(
                adapter,
                index,
                profile.edid.as_ptr(),
                profile.edid.len() as u32,
            )
        };
        if status < 0 {
            return status;
        }
    }
    SUCCESS
}

#[unsafe(no_mangle)]
extern "C" fn xscc_create_monitor_state() -> Handle {
    let Some(profile) = active_profile() else {
        return ptr::null_mut();
    };
    Box::into_raw(Box::new(Monitor {
        profile,
        worker: Mutex::default(),
    }))
    .cast()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn xscc_destroy_monitor_state(state: Handle) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state.cast::<Monitor>()) });
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn xscc_advertise_modes(
    state: Handle,
    target: i32,
    capacity: u32,
    output: Handle,
    count: *mut u32,
    native_write: unsafe extern "C" fn(Handle, u32, *const Signal, *const Colour),
) -> Status {
    let state = unsafe { state.cast::<Monitor>().as_ref() };
    let profile = match state {
        Some(state) => Arc::clone(&state.profile),
        None => match active_profile() {
            Some(profile) => profile,
            None => return INVALID_DEVICE_STATE,
        },
    };
    let required = profile.modes.len() as u32;
    unsafe { count.write(required) };
    if capacity == 0 || output.is_null() {
        return SUCCESS;
    }
    if capacity < required {
        return BUFFER_TOO_SMALL;
    }
    for (index, mode) in profile.modes.iter().enumerate() {
        let Some(signal) = mode.signal(target != 0) else {
            return DATA_ERROR;
        };
        unsafe { native_write(output, index as u32, &signal, &profile.colour) };
    }
    SUCCESS
}

#[unsafe(no_mangle)]
unsafe extern "C" fn xscc_adapter_colour(out: *mut Colour) -> Status {
    let Some(profile) = active_profile() else {
        return INVALID_DEVICE_STATE;
    };
    let Colour {
        format,
        sdr_bits,
        hdr_bits,
    } = profile.colour;
    unsafe {
        out.write(Colour {
            format,
            sdr_bits,
            hdr_bits,
        })
    };
    SUCCESS
}

#[unsafe(no_mangle)]
unsafe extern "C" fn xscc_assign_swapchain(
    state: Handle,
    chain: Handle,
    ready: Handle,
    luid: i64,
) -> Status {
    let Some(monitor) = (unsafe { state.cast::<Monitor>().as_ref() }) else {
        return INVALID_DEVICE_STATE;
    };
    // IddCx assigns at most one chain per monitor and unassigns it after this callback.
    let previous = {
        monitor
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    };
    drop(previous);
    match Worker::start(chain, ready, luid) {
        Ok(next) => {
            *monitor
                .worker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(next);
            SUCCESS
        }
        Err(error) => {
            diagnostic(&format!("cannot assign swapchain: {:#010x}", error as u32));
            error
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn xscc_unassign_swapchain(state: Handle) -> Status {
    let Some(monitor) = (unsafe { state.cast::<Monitor>().as_ref() }) else {
        return INVALID_DEVICE_STATE;
    };
    let worker = {
        monitor
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    };
    drop(worker);
    SUCCESS
}

struct Worker {
    stop: Handle,
    device: Handle,
    thread: Option<JoinHandle<()>>,
}

// The stop event may be signaled by any thread. The owner joins before closing it.
unsafe impl Send for Worker {}

impl Worker {
    fn start(chain: Handle, ready: Handle, luid: i64) -> Result<Self, Status> {
        let mut device = ptr::null_mut();
        let status = unsafe { xscc_bind_swapchain(chain, luid, &mut device) };
        if status < 0 {
            return Err(status);
        }

        let stop = unsafe { CreateEventW(ptr::null_mut(), 1, 0, ptr::null()) };
        let mut worker = Self {
            stop,
            device,
            thread: None,
        };
        if stop.is_null() {
            return Err(UNSUCCESSFUL);
        }
        let (chain, stop, ready) = (chain as usize, stop as usize, ready as usize);
        worker.thread = Some(
            thread::Builder::new()
                .name("virtual-display-swapchain".into())
                .spawn(move || run(chain as Handle, stop as Handle, ready as Handle))
                .map_err(|_| UNSUCCESSFUL)?,
        );
        Ok(worker)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            unsafe { SetEvent(self.stop) };
            if thread.join().is_err() {
                diagnostic("swapchain worker stopped unexpectedly");
            }
        }
        if !self.stop.is_null() {
            unsafe { CloseHandle(self.stop) };
        }
        unsafe { xscc_release_device(self.device) };
    }
}

fn run(chain: Handle, stop: Handle, ready: Handle) {
    let mut task = [0; 13];
    for (unit, letter) in task.iter_mut().zip("Distribution".encode_utf16()) {
        *unit = letter;
    }
    let scheduler = unsafe { AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut 0) };

    drain(
        || unsafe { WaitForMultipleObjects(1, &stop, 0, 0) } != WAIT_TIMEOUT,
        || {
            let mut frame = Frame::default();
            match unsafe { xscc_acquire_frame(chain, &mut frame) } {
                IO_PENDING => {
                    let events = [stop, ready];
                    matches!(
                        unsafe {
                            WaitForMultipleObjects(events.len() as u32, events.as_ptr(), 0, 16)
                        },
                        WAIT_FRAME | WAIT_TIMEOUT
                    )
                }
                SUCCESS => (unsafe { xscc_finish_frame(chain, &frame) }) >= 0,
                _ => false,
            }
        },
    );

    unsafe { xscc_delete_swapchain(chain) };
    if !scheduler.is_null() {
        unsafe { AvRevertMmThreadCharacteristics(scheduler) };
    }
}
