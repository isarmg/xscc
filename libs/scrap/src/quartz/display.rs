use super::ffi::*;

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
#[repr(C)]
pub struct Display(u32);

impl Display {
    pub fn primary() -> Display {
        Display(unsafe { CGMainDisplayID() })
    }

    pub fn online() -> Result<Vec<Display>, CGError> {
        read_online_displays(|ids| {
            let mut count = 0;
            let buffer = if ids.is_empty() {
                std::ptr::null_mut()
            } else {
                ids.as_mut_ptr()
            };
            match unsafe { CGGetOnlineDisplayList(ids.len() as u32, buffer, &mut count) } {
                CGError::Success => Ok(count),
                error => Err(error),
            }
        })
    }

    pub fn id(self) -> u32 {
        self.0
    }

    pub fn width(self) -> usize {
        let w = unsafe { CGDisplayPixelsWide(self.0) };
        let s = self.scale();
        if s > 1.0 {
            ((w as f64) * s).round() as usize
        } else {
            w
        }
    }

    pub fn height(self) -> usize {
        let h = unsafe { CGDisplayPixelsHigh(self.0) };
        let s = self.scale();
        if s > 1.0 {
            ((h as f64) * s).round() as usize
        } else {
            h
        }
    }

    pub fn is_builtin(self) -> bool {
        unsafe { CGDisplayIsBuiltin(self.0) != 0 }
    }

    pub fn is_primary(self) -> bool {
        unsafe { CGDisplayIsMain(self.0) != 0 }
    }

    pub fn is_active(self) -> bool {
        unsafe { CGDisplayIsActive(self.0) != 0 }
    }

    pub fn is_online(self) -> bool {
        unsafe { CGDisplayIsOnline(self.0) != 0 }
    }

    pub fn scale(self) -> f64 {
        if !*super::ENABLE_RETINA.lock().unwrap() {
            return 1.0;
        }
        unsafe {
            let mode = CGDisplayCopyDisplayMode(self.0);
            if mode.is_null() {
                return 1.0;
            }
            let scale = backing_scale(
                CGDisplayModeGetWidth(mode),
                CGDisplayModeGetPixelWidth(mode),
            );
            CGDisplayModeRelease(mode);
            scale
        }
    }

    pub fn bounds(self) -> CGRect {
        unsafe { CGDisplayBounds(self.0) }
    }
}

fn backing_scale(logical_width: usize, pixel_width: usize) -> f64 {
    if logical_width == 0 || pixel_width <= logical_width {
        1.0
    } else {
        pixel_width as f64 / logical_width as f64
    }
}

fn read_online_displays(
    mut query: impl FnMut(&mut [u32]) -> Result<u32, CGError>,
) -> Result<Vec<Display>, CGError> {
    let mut capacity = query(&mut [])?.checked_add(1).ok_or(CGError::RangeCheck)?;
    for _ in 0..3 {
        let mut ids = vec![0; capacity as usize];
        let count = query(&mut ids)?;
        // A full buffer may hide newly attached displays.
        if count < capacity {
            ids.truncate(count as usize);
            return Ok(ids.into_iter().map(Display).collect());
        }
        capacity = capacity.checked_mul(2).ok_or(CGError::RangeCheck)?;
    }
    Err(CGError::CannotComplete)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_displays_added_during_the_count_query() {
        let expected: Vec<_> = (100..119).collect();
        let displays = read_online_displays(|ids| {
            if ids.is_empty() {
                return Ok(16);
            }
            let count = expected.len().min(ids.len());
            ids[..count].copy_from_slice(&expected[..count]);
            Ok(count as u32)
        })
        .unwrap();
        assert_eq!(
            displays.into_iter().map(Display::id).collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn retina_scale_uses_logical_and_backing_dimensions() {
        assert_eq!(backing_scale(1512, 3024), 2.0);
        assert_eq!(backing_scale(1920, 1920), 1.0);
        assert_eq!(backing_scale(1920, 0), 1.0);
        assert_eq!(backing_scale(0, 3024), 1.0);
    }
}
