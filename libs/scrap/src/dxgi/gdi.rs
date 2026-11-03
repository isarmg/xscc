use std::mem::size_of;
use winapi::{
    shared::windef::{HBITMAP, HDC, HGDIOBJ},
    um::wingdi::{
        BitBlt,
        CreateCompatibleBitmap,
        CreateCompatibleDC,
        CreateDCW,
        DeleteDC,
        DeleteObject,
        GetDIBits,
        SelectObject,
        BITMAPINFO,
        BITMAPINFOHEADER,
        BI_RGB,
        CAPTUREBLT,
        DIB_RGB_COLORS, //CAPTUREBLT,
        HGDI_ERROR,
        RGBQUAD,
        SRCCOPY,
    },
};

const PIXEL_WIDTH: i32 = 4;

pub struct CapturerGDI {
    screen_dc: HDC,
    dc: HDC,
    bmp: HBITMAP,
    original_bitmap: HGDIOBJ,
    width: i32,
    height: i32,
}

impl CapturerGDI {
    pub fn new(name: &[u16], width: i32, height: i32) -> Result<Self, Box<dyn std::error::Error>> {
        /* or Enumerate monitors with EnumDisplayMonitors,
        https://stackoverflow.com/questions/34987695/how-can-i-get-an-hmonitor-handle-from-a-display-device-name
            #[no_mangle]
            pub extern "C" fn callback(m: HMONITOR, dc: HDC, rect: LPRECT, lp: LPARAM) -> BOOL {}
        */
        /*
        shared::windef::HMONITOR,
        winuser::{GetMonitorInfoW, GetSystemMetrics, MONITORINFOEXW},
        let mut mi: MONITORINFOEXW = std::mem::MaybeUninit::uninit().assume_init();
        mi.cbSize = size_of::<MONITORINFOEXW>() as _;
        if GetMonitorInfoW(m, &mut mi as *mut MONITORINFOEXW as _) == 0 {
            return Err(format!("Failed to get monitor information of: {:?}", m).into());
        }
        */
        unsafe {
            if name.is_empty() {
                return Err("Empty display name".into());
            }
            let screen_dc = CreateDCW(&name[0], 0 as _, 0 as _, 0 as _);
            if screen_dc.is_null() {
                return Err("Failed to create dc from monitor name".into());
            }

            // Create a Windows Bitmap, and copy the bits into it
            let dc = CreateCompatibleDC(screen_dc);
            if dc.is_null() {
                DeleteDC(screen_dc);
                return Err("Can't get a Windows display".into());
            }

            let bmp = CreateCompatibleBitmap(screen_dc, width, height);
            if bmp.is_null() {
                DeleteDC(screen_dc);
                DeleteDC(dc);
                return Err("Can't create a Windows buffer".into());
            }

            let original_bitmap = SelectObject(dc, bmp as _);
            if original_bitmap.is_null() || original_bitmap == HGDI_ERROR {
                DeleteDC(screen_dc);
                DeleteDC(dc);
                DeleteObject(bmp as _);
                return Err("Can't select Windows buffer".into());
            }
            Ok(Self {
                screen_dc,
                dc,
                bmp,
                original_bitmap,
                width,
                height,
            })
        }
    }

    pub fn frame(&self, data: &mut Vec<u8>) -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            let selected = SelectObject(self.dc, self.bmp as _);
            if selected.is_null() || selected == HGDI_ERROR {
                return Err("Can't select Windows buffer for capture".into());
            }
            let res = BitBlt(
                self.dc,
                0,
                0,
                self.width,
                self.height,
                self.screen_dc,
                0,
                0,
                SRCCOPY | CAPTUREBLT, // CAPTUREBLT enable layered window but also make cursor blinking
            );
            // GetDIBits requires an unselected bitmap, including after a failed copy.
            let restored = SelectObject(self.dc, self.original_bitmap);
            if restored.is_null() || restored == HGDI_ERROR {
                return Err(if res == 0 {
                    "Failed to copy screen to Windows buffer and restore previous bitmap"
                } else {
                    "Failed to restore previous Windows bitmap"
                }
                .into());
            }
            if res == 0 {
                return Err("Failed to copy screen to Windows buffer".into());
            }

            let stride = self.width * PIXEL_WIDTH;
            let size: usize = (stride * self.height) as usize;
            data.resize(size, 0);

            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as _,
                    biWidth: self.width as _,
                    // Request top-down rows so no full-frame mirror/rotation is needed.
                    biHeight: -self.height,
                    biPlanes: 1,
                    biBitCount: (8 * PIXEL_WIDTH) as _,
                    biCompression: BI_RGB,
                    biSizeImage: (self.width * self.height * PIXEL_WIDTH) as _,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 0,
                    biClrImportant: 0,
                },
                bmiColors: [RGBQUAD {
                    rgbBlue: 0,
                    rgbGreen: 0,
                    rgbRed: 0,
                    rgbReserved: 0,
                }],
            };

            // copy bits into Vec
            let res = GetDIBits(
                self.dc,
                self.bmp,
                0,
                self.height as _,
                &mut data[0] as *mut u8 as _,
                &mut bmi as _,
                DIB_RGB_COLORS,
            );
            if res == 0 {
                return Err("GetDIBits failed".into());
            }
            Ok(())
        }
    }
}

impl Drop for CapturerGDI {
    fn drop(&mut self) {
        unsafe {
            DeleteDC(self.screen_dc);
            DeleteDC(self.dc);
            DeleteObject(self.bmp as _);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winapi::um::wingdi::{CreateBitmap, SetPixel, CLR_INVALID};

    #[test]
    fn gdi_frames_keep_top_down_bgra_order_when_reusing_the_output() {
        unsafe {
            let (width, height) = (3, 2);
            let source_dc = CreateCompatibleDC(std::ptr::null_mut());
            let source = CreateBitmap(width, height, 1, 32, std::ptr::null());
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            assert!(!source_dc.is_null() && !source.is_null() && !dc.is_null());
            assert!(!SelectObject(source_dc, source as _).is_null());
            let bmp = CreateCompatibleBitmap(source_dc, width, height);
            assert!(!bmp.is_null());
            let original_bitmap = SelectObject(dc, bmp as _);
            assert!(!original_bitmap.is_null() && original_bitmap != HGDI_ERROR);
            let mut capturer = CapturerGDI {
                screen_dc: source_dc,
                dc,
                bmp,
                original_bitmap,
                width,
                height,
            };
            let mut output = Vec::new();
            capturer.screen_dc = std::ptr::null_mut();
            let copied = capturer.frame(&mut output);
            capturer.screen_dc = source_dc;
            assert!(copied.is_err());
            let previous = SelectObject(source_dc, bmp as _);
            assert_eq!(previous, source as _);
            let selected_elsewhere = capturer.frame(&mut output);
            assert_eq!(SelectObject(source_dc, previous), bmp as _);
            assert!(selected_elsewhere.is_err());
            for blue in [30u8, 90] {
                let mut expected = Vec::new();
                for y in 0..height {
                    for x in 0..width {
                        let (red, green) = ((10 + y * 60) as u8, (20 + x * 50) as u8);
                        let color = red as u32 | (green as u32) << 8 | (blue as u32) << 16;
                        assert_ne!(SetPixel(source_dc, x, y, color), CLR_INVALID);
                        expected.push([blue, green, red]);
                    }
                }
                capturer.frame(&mut output).unwrap();
                assert_eq!(output.len(), (width * height * PIXEL_WIDTH) as usize);
                for (actual, expected) in output.chunks_exact(4).zip(expected) {
                    // GDI's fourth byte is reserved, not an alpha-channel contract.
                    assert_eq!(&actual[..3], expected);
                }
                let previous = SelectObject(source_dc, bmp as _);
                assert_eq!(previous, source as _);
                assert_eq!(SelectObject(source_dc, previous), bmp as _);
            }
            drop(capturer);
            assert_ne!(DeleteObject(source as _), 0);
        }
    }
}
