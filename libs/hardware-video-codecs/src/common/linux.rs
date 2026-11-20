use std::{ffi::CStr, io, process};

pub(super) fn supported_gpu(encode: bool) -> (bool, bool, bool) {
    let (nvidia_library, entry_point) = if encode {
        (c"libnvidia-encode.so.1", c"NvEncodeAPICreateInstance")
    } else {
        (c"libnvcuvid.so.1", c"cuvidCreateDecoder")
    };
    let nvidia = driver_available(c"libcuda.so.1", Some(c"cuInit"))
        && driver_available(nvidia_library, Some(entry_point));
    let amf = if cfg!(target_pointer_width = "64") {
        c"libamfrt64.so.1"
    } else {
        c"libamfrt32.so.1"
    };
    let intel = [
        c"libvpl.so.2",
        c"libvpl.so",
        c"libmfx.so.1",
        c"libmfx.so",
        c"libmfx-gen.so.1.2",
        c"libmfxhw64.so.1",
    ]
    .into_iter()
    .any(|name| driver_available(name, None));
    (nvidia, driver_available(amf, None), intel)
}

fn driver_available(name: &CStr, symbol: Option<&CStr>) -> bool {
    // These names are fixed driver libraries. FFmpeg validates the chosen codec on open.
    unsafe {
        let handle = libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL);
        if handle.is_null() {
            return false;
        }
        let available = symbol.is_none_or(|symbol| !libc::dlsym(handle, symbol.as_ptr()).is_null());
        libc::dlclose(handle);
        available
    }
}

pub(super) fn monitor_parent() -> io::Result<()> {
    let parent = unsafe { libc::getppid() };
    let result = unsafe {
        libc::prctl(
            libc::PR_SET_PDEATHSIG,
            libc::c_ulong::from(libc::SIGKILL as u32),
            0 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::getppid() } != parent {
        process::exit(1);
    }
    Ok(())
}
