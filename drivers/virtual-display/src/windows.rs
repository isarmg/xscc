mod driver;
mod graphics;
mod wdk;

use self::wdk::abi;
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

struct Frame {
    acquired_at: u64,
    number: u32,
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

    let directory = configuration_directory()?;
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

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn configuration_directory() -> io::Result<PathBuf> {
    let mut buffer = [0_u16; 32_768];
    let mut bytes = std::mem::size_of_val(&buffer) as u32;
    // The Win32 predefined key is the sign-extended 32-bit constant 0x80000002.
    let key = 0x8000_0002_u32 as i32 as isize as abi::HKEY;
    let registry_path = wide(r"SOFTWARE\MikeTheTech\VirtualDisplayDriver");
    let value = wide("VDDPATH");
    let status = unsafe {
        abi::RegGetValueW(
            key,
            registry_path.as_ptr(),
            value.as_ptr(),
            abi::RRF_RT_REG_SZ,
            ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "Invalid VDDPATH registry value");
    match status as u32 {
        abi::ERROR_FILE_NOT_FOUND | abi::ERROR_PATH_NOT_FOUND => {
            return Ok(PathBuf::from(r"C:\VirtualDisplayDriver"));
        }
        0 => {}
        _ => return Err(io::Error::from_raw_os_error(status)),
    }
    let units = (bytes / 2) as usize;
    if !bytes.is_multiple_of(2)
        || units <= 1
        || units > buffer.len()
        || buffer[units - 1] != 0
        || buffer[..units - 1].contains(&0)
    {
        return Err(invalid());
    }
    let path = PathBuf::from(OsString::from_wide(&buffer[..units - 1]));
    if !path.is_absolute() {
        return Err(invalid());
    }
    Ok(path)
}

impl Device {
    fn load() -> io::Result<Self> {
        Ok(Self {
            profile: load_profile()?,
            initialized: AtomicBool::new(false),
        })
    }

    fn start(&self, device: abi::WDFDEVICE) -> Status {
        if self.initialized.swap(true, Ordering::AcqRel) {
            return SUCCESS;
        }
        let status = driver::initialize_adapter(device, self.profile.monitor_count);
        if status < 0 {
            self.initialized.store(false, Ordering::Release);
        }
        status
    }
}

impl Monitor {
    fn new(profile: Arc<Profile>) -> Self {
        Self {
            profile,
            worker: Mutex::default(),
        }
    }

    fn assign(&self, chain: abi::IDDCX_SWAPCHAIN, ready: Handle, luid: abi::LUID) -> Status {
        // IddCx assigns at most one chain per monitor and unassigns it after this callback.
        self.unassign();
        match Worker::start(chain, ready, luid) {
            Ok(next) => {
                *self
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

    fn unassign(&self) {
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        drop(worker);
    }
}

unsafe fn advertise_modes<T>(
    monitor: Option<&Monitor>,
    target: bool,
    capacity: u32,
    output: *mut T,
    count: *mut u32,
    make_mode: fn(&Signal, &Colour) -> T,
) -> Status {
    let profile = match monitor {
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
        let Some(signal) = mode.signal(target) else {
            return DATA_ERROR;
        };
        unsafe { output.add(index).write(make_mode(&signal, &profile.colour)) };
    }
    SUCCESS
}

struct Worker {
    stop: Handle,
    _device: graphics::Com<abi::IDXGIDevice>,
    thread: Option<JoinHandle<()>>,
}

// The stop event may be signaled by any thread. The owner joins before closing it.
unsafe impl Send for Worker {}

impl Worker {
    fn start(chain: abi::IDDCX_SWAPCHAIN, ready: Handle, luid: abi::LUID) -> Result<Self, Status> {
        let device = graphics::bind_swapchain(chain, luid)?;

        let stop = unsafe { CreateEventW(ptr::null_mut(), 1, 0, ptr::null()) };
        let mut worker = Self {
            stop,
            _device: device,
            thread: None,
        };
        if stop.is_null() {
            return Err(UNSUCCESSFUL);
        }
        let (chain, stop, ready) = (chain as usize, stop as usize, ready as usize);
        worker.thread = Some(
            thread::Builder::new()
                .name("virtual-display-swapchain".into())
                .spawn(move || {
                    run(
                        chain as abi::IDDCX_SWAPCHAIN,
                        stop as Handle,
                        ready as Handle,
                    )
                })
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
    }
}

fn run(chain: abi::IDDCX_SWAPCHAIN, stop: Handle, ready: Handle) {
    let mut task = [0; 13];
    for (unit, letter) in task.iter_mut().zip("Distribution".encode_utf16()) {
        *unit = letter;
    }
    let scheduler = unsafe { AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut 0) };

    drain(
        || unsafe { WaitForMultipleObjects(1, &stop, 0, 0) } != WAIT_TIMEOUT,
        || match graphics::acquire_frame(chain) {
            Err(IO_PENDING) => {
                let events = [stop, ready];
                matches!(
                    unsafe { WaitForMultipleObjects(events.len() as u32, events.as_ptr(), 0, 16) },
                    WAIT_FRAME | WAIT_TIMEOUT
                )
            }
            Ok(frame) => graphics::finish_frame(chain, frame) >= 0,
            _ => false,
        },
    );

    unsafe { driver::delete_object(chain.cast()) };
    if !scheduler.is_null() {
        unsafe { AvRevertMmThreadCharacteristics(scheduler) };
    }
}
