use core_foundation::{
    base::{CFAllocatorRef, CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
    dictionary::{CFDictionary, CFDictionaryRef},
    string::{CFString, CFStringRef},
};
use std::{
    ffi::c_void,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    process, ptr, thread,
};

const AVC: u32 = u32::from_be_bytes(*b"avc1");
const HEVC: u32 = u32::from_be_bytes(*b"hvc1");
const PROBE_WIDTH: i32 = 1280;
const PROBE_HEIGHT: i32 = 720;

type CompressionSessionRef = *mut c_void;
type OutputCallback = unsafe extern "C" fn(*mut c_void, *mut c_void, i32, u32, *mut c_void);

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    static kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: CFStringRef;
    static kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: CFStringRef;
    static kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder: CFStringRef;
    fn VTCopySupportedPropertyDictionaryForEncoder(
        width: i32,
        height: i32,
        codec: u32,
        specification: CFDictionaryRef,
        encoder_id: *mut CFStringRef,
        properties: *mut CFDictionaryRef,
    ) -> i32;
    fn VTCompressionSessionCreate(
        allocator: CFAllocatorRef,
        width: i32,
        height: i32,
        codec: u32,
        specification: CFDictionaryRef,
        image_attributes: CFDictionaryRef,
        data_allocator: CFAllocatorRef,
        callback: Option<OutputCallback>,
        context: *mut c_void,
        session: *mut CompressionSessionRef,
    ) -> i32;
    fn VTCompressionSessionPrepareToEncodeFrames(session: CompressionSessionRef) -> i32;
    fn VTCompressionSessionInvalidate(session: CompressionSessionRef);
    fn VTSessionCopyProperty(
        session: CompressionSessionRef,
        key: CFStringRef,
        allocator: CFAllocatorRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn VTIsHardwareDecodeSupported(codec: u32) -> bool;
}

struct CompressionSession(CompressionSessionRef);

impl Drop for CompressionSession {
    fn drop(&mut self) {
        unsafe {
            VTCompressionSessionInvalidate(self.0);
            drop(CFType::wrap_under_create_rule(self.0.cast()));
        }
    }
}

pub(super) fn codec_support() -> (bool, bool, bool, bool) {
    // H.264 hardware encoding remains disabled for this product path.
    (
        false,
        hevc_encoder(),
        unsafe { VTIsHardwareDecodeSupported(AVC) },
        unsafe { VTIsHardwareDecodeSupported(HEVC) },
    )
}

fn hevc_encoder() -> bool {
    let enable = unsafe {
        CFString::wrap_under_get_rule(
            kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder,
        )
    };
    let require = unsafe {
        CFString::wrap_under_get_rule(
            kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder,
        )
    };
    let enabled = (enable, CFBoolean::true_value());
    let required = (require, CFBoolean::true_value());
    let spec = CFDictionary::from_CFType_pairs(&[enabled.clone(), required]);
    let mut encoder = ptr::null();
    let mut properties = ptr::null();
    let result = unsafe {
        VTCopySupportedPropertyDictionaryForEncoder(
            PROBE_WIDTH,
            PROBE_HEIGHT,
            HEVC,
            spec.as_concrete_TypeRef(),
            &mut encoder,
            &mut properties,
        )
    };
    if !encoder.is_null() {
        drop(unsafe { CFString::wrap_under_create_rule(encoder) });
    }
    if !properties.is_null() {
        drop(unsafe { CFDictionary::<CFString>::wrap_under_create_rule(properties) });
    }
    if result == 0 {
        return true;
    }

    // Older drivers may only confirm acceleration on a prepared encoder.
    let spec = CFDictionary::from_CFType_pairs(&[enabled]);
    let mut native = ptr::null_mut();
    let result = unsafe {
        VTCompressionSessionCreate(
            ptr::null(),
            PROBE_WIDTH,
            PROBE_HEIGHT,
            HEVC,
            spec.as_concrete_TypeRef(),
            ptr::null(),
            ptr::null(),
            None,
            ptr::null_mut(),
            &mut native,
        )
    };
    if native.is_null() {
        return false;
    }
    let session = CompressionSession(native);
    if result != 0 || unsafe { VTCompressionSessionPrepareToEncodeFrames(session.0) } != 0 {
        return false;
    }
    let mut value = ptr::null();
    let result = unsafe {
        VTSessionCopyProperty(
            session.0,
            kVTCompressionPropertyKey_UsingHardwareAcceleratedVideoEncoder,
            ptr::null(),
            &mut value,
        )
    };
    if value.is_null() {
        return false;
    }
    let value = unsafe { CFType::wrap_under_create_rule(value) };
    result == 0 && value.downcast_into::<CFBoolean>().is_some_and(bool::from)
}

pub(super) fn monitor_parent() -> io::Result<()> {
    let parent = unsafe { libc::getppid() };
    let fd = unsafe { libc::kqueue() };
    if fd == -1 {
        return Err(io::Error::last_os_error());
    }
    let queue = unsafe { OwnedFd::from_raw_fd(fd) };
    let watch = libc::kevent {
        ident: parent as _,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: ptr::null_mut(),
    };
    let result = unsafe { libc::kevent(fd, &watch, 1, ptr::null_mut(), 0, ptr::null()) };
    let error = (result == -1).then(io::Error::last_os_error);
    if unsafe { libc::getppid() } != parent {
        process::exit(1);
    }
    if let Some(error) = error {
        return Err(error);
    }
    thread::Builder::new()
        .name("codec-probe-parent".into())
        .spawn(move || {
            let mut event = std::mem::MaybeUninit::uninit();
            loop {
                let result = unsafe {
                    libc::kevent(
                        queue.as_raw_fd(),
                        ptr::null(),
                        0,
                        event.as_mut_ptr(),
                        1,
                        ptr::null(),
                    )
                };
                if result > 0 {
                    process::exit(1);
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    log::warn!("Codec probe parent monitor stopped: {error}");
                    break;
                }
            }
        })?;
    Ok(())
}
