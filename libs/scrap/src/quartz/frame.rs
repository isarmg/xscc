use std::{io, ops, ptr, slice};

use super::ffi::*;

pub struct Frame {
    surface: IOSurfaceRef,
    inner: &'static [u8],
    bgra: &'static [u8],
    bgra_stride: usize,
}

impl Frame {
    pub unsafe fn new(surface: IOSurfaceRef) -> io::Result<Frame> {
        if surface.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Missing IOSurface",
            ));
        }
        CFRetain(surface);
        IOSurfaceIncrementUseCount(surface);
        let status = IOSurfaceLock(surface, SURFACE_LOCK_READ_ONLY, ptr::null_mut());
        if status != 0 {
            IOSurfaceDecrementUseCount(surface);
            CFRelease(surface);
            return Err(io::Error::other(format!("IOSurfaceLock failed: {status}")));
        }

        // Only a successfully locked surface reaches Drop, which owns its matching unlock.
        let mut frame = Frame {
            surface,
            inner: &[],
            bgra: &[],
            bgra_stride: 0,
        };
        let base = IOSurfaceGetBaseAddress(surface) as *const u8;
        let length = IOSurfaceGetAllocSize(surface);
        if base.is_null() || length == 0 || length > isize::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid IOSurface allocation",
            ));
        }
        // Retain/use-count prevent CGDisplayStream reuse; both views are exposed only through &self.
        frame.inner = slice::from_raw_parts(base, length);
        Ok(frame)
    }

    #[inline]
    pub fn inner(&self) -> &[u8] {
        self.inner
    }

    pub fn stride(&self) -> usize {
        self.bgra_stride
    }

    pub fn surface_to_bgra(&mut self, width: usize, height: usize) -> io::Result<()> {
        let plane = unsafe { IOSurfaceGetBaseAddressOfPlane(self.surface, 0) } as usize;
        let stride = unsafe { IOSurfaceGetBytesPerRowOfPlane(self.surface, 0) };
        let range = bgra_range(self.inner, plane, stride, width, height)?;
        self.bgra = &self.inner[range];
        self.bgra_stride = stride;
        Ok(())
    }
}

fn bgra_range(
    allocation: &[u8],
    plane: usize,
    stride: usize,
    width: usize,
    height: usize,
) -> io::Result<ops::Range<usize>> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid IOSurface BGRA plane layout",
        )
    };
    let row = width.checked_mul(4).ok_or_else(invalid)?;
    let length = stride.checked_mul(height).ok_or_else(invalid)?;
    let offset = plane
        .checked_sub(allocation.as_ptr() as usize)
        .ok_or_else(invalid)?;
    let end = offset.checked_add(length).ok_or_else(invalid)?;
    if width == 0 || height == 0 || stride < row || end > allocation.len() {
        return Err(invalid());
    }
    Ok(offset..end)
}

impl ops::Deref for Frame {
    type Target = [u8];
    fn deref<'a>(&'a self) -> &'a [u8] {
        self.bgra
    }
}

impl Drop for Frame {
    fn drop(&mut self) {
        unsafe {
            let status = IOSurfaceUnlock(self.surface, SURFACE_LOCK_READ_ONLY, ptr::null_mut());
            if status != 0 {
                hbb_common::throttled_log!(
                    std::time::Duration::from_secs(1),
                    warn,
                    "IOSurfaceUnlock failed: {status}"
                );
            }
            IOSurfaceDecrementUseCount(self.surface);
            CFRelease(self.surface);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrowed_bgra_plane_accepts_padded_stride_inside_allocation() {
        let bytes = [0u8; 40];
        let plane = bytes.as_ptr() as usize + 4;
        assert_eq!(bgra_range(&bytes, plane, 16, 3, 2).unwrap(), 4..36);
    }

    #[test]
    fn borrowed_bgra_plane_rejects_short_rows_overflows_and_outside_memory() {
        let bytes = [0u8; 40];
        let base = bytes.as_ptr() as usize;
        for (plane, stride, width, height) in [
            (base, 8, 3, 2),
            (base, usize::MAX, 1, 2),
            (base, 4, usize::MAX, 1),
            (base - 1, 16, 3, 2),
            (base + 16, 16, 3, 2),
            (base, 0, 0, 1),
        ] {
            assert_eq!(
                bgra_range(&bytes, plane, stride, width, height)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
    }
}
