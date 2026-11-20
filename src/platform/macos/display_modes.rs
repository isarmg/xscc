use core_foundation::{
    array::CFArray,
    base::{CFRetain, TCFType},
    boolean::CFBoolean,
    dictionary::{CFDictionary, CFDictionaryRef},
    string::CFString,
};
use core_graphics::display::{
    kCGDisplayShowDuplicateLowResolutionModes, CGConfigureOption, CGDisplay,
    CGDisplayCopyAllDisplayModes, CGDisplayMode, CGDisplayModeCopyPixelEncoding,
};
use foreign_types::ForeignType;
use std::{ffi::c_void, io, ptr::null};

pub(crate) fn copy_modes(id: u32, include_duplicate: bool) -> io::Result<Vec<CGDisplayMode>> {
    let options = include_duplicate.then(|| unsafe {
        CFDictionary::from_CFType_pairs(&[(
            CFString::wrap_under_get_rule(kCGDisplayShowDuplicateLowResolutionModes),
            CFBoolean::true_value(),
        )])
    });
    let options_ptr: CFDictionaryRef = options
        .as_ref()
        .map_or(null(), TCFType::as_concrete_TypeRef);
    let array = unsafe { CGDisplayCopyAllDisplayModes(id, options_ptr) };
    if array.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "CoreGraphics returned no display modes",
        ));
    }
    let array = unsafe { CFArray::<*const c_void>::wrap_under_create_rule(array) };
    Ok(array
        .iter()
        .filter_map(|mode| {
            let mode = *mode;
            if mode.is_null() {
                return None;
            }
            // The array owns each element. Retain it for the returned Rust owner.
            unsafe {
                CFRetain(mode);
                Some(CGDisplayMode::from_ptr(mode.cast_mut().cast()))
            }
        })
        .collect())
}

fn encoding(mode: &CGDisplayMode) -> Option<String> {
    let value = unsafe { CGDisplayModeCopyPixelEncoding(mode.as_ptr()) };
    if value.is_null() {
        return None;
    }
    Some(
        unsafe { CFString::wrap_under_create_rule(value) }
            .to_string()
            .to_ascii_lowercase(),
    )
}

pub(super) fn resolutions(id: u32) -> io::Result<Vec<(u64, u64)>> {
    let current = current_mode(id)?;
    let current_encoding = encoding(&current);
    let mut sizes = copy_modes(id, true)?
        .into_iter()
        .filter(|mode| {
            mode.refresh_rate() == current.refresh_rate() && encoding(mode) == current_encoding
        })
        .map(|mode| (mode.width(), mode.height()))
        .collect::<Vec<_>>();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes.dedup();
    if sizes.len() > 15 {
        sizes.retain(|&(width, _)| width >= 800);
    }
    Ok(sizes)
}

pub(super) fn current_mode(id: u32) -> io::Result<CGDisplayMode> {
    CGDisplay::new(id).display_mode().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "CoreGraphics returned no current display mode",
        )
    })
}

pub(super) fn set_resolution(id: u32, width: usize, height: usize) -> io::Result<()> {
    let display = CGDisplay::new(id);
    let current = current_mode(id)?;
    let current_encoding = encoding(&current);
    let modes = copy_modes(id, true)?;
    let mut matching = modes.into_iter().filter(|mode| {
        mode.width() == width as u64
            && mode.height() == height as u64
            && mode.refresh_rate() == current.refresh_rate()
            && encoding(mode) == current_encoding
    });
    let mut selected = matching.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "The requested display resolution is unavailable",
        )
    })?;
    if selected.pixel_width() <= selected.width() {
        if let Some(hidpi) = matching.find(|mode| mode.pixel_width() > mode.width()) {
            selected = hidpi;
        }
    }
    let config = display
        .begin_configuration()
        .map_err(|code| cg_error("Begin display configuration", code))?;
    if let Err(code) = display.configure_display_with_display_mode(&config, &selected) {
        let _ = display.cancel_configuration(&config);
        return Err(cg_error("Set display mode", code));
    }
    display
        .complete_configuration(&config, CGConfigureOption::ConfigureForSession)
        .map_err(|code| cg_error("Complete display configuration", code))
}

fn cg_error(operation: &str, code: i32) -> io::Error {
    io::Error::other(format!("{operation} failed (CoreGraphics error {code})"))
}
