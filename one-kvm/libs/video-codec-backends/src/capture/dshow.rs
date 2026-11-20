use super::{CaptureError, CapturePixelFormat, DshowCapability};
use std::{
    cmp::Reverse,
    mem::{size_of, ManuallyDrop},
    ptr::{self, NonNull},
};
use windows::{
    core::{w, Error, Interface, GUID},
    Win32::{
        Foundation::{RPC_E_CHANGED_MODE, S_FALSE},
        Media::{
            DirectShow::{
                IAMStreamConfig, IBaseFilter, ICreateDevEnum, PINDIR_OUTPUT,
                VIDEO_STREAM_CONFIG_CAPS,
            },
            MediaFoundation::{
                CLSID_SystemDeviceEnum, CLSID_VideoInputDeviceCategory, FORMAT_VideoInfo,
                FORMAT_VideoInfo2, AM_MEDIA_TYPE, MEDIASUBTYPE_IYUV, MEDIASUBTYPE_MJPG,
                MEDIASUBTYPE_NV12, MEDIASUBTYPE_RGB24, MEDIASUBTYPE_RGB32, MEDIASUBTYPE_UYVY,
                MEDIASUBTYPE_YUY2, MEDIASUBTYPE_YV12, MEDIASUBTYPE_YVYU, VIDEOINFOHEADER,
                VIDEOINFOHEADER2,
            },
        },
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IEnumMoniker,
                IMoniker, StructuredStorage::IPropertyBag, CLSCTX_INPROC_SERVER,
                COINIT_MULTITHREADED,
            },
            Variant::{VariantClear, VARIANT, VT_BSTR},
        },
    },
};

struct ComApartment(bool);

impl ComApartment {
    fn new() -> windows::core::Result<Self> {
        // The guard balances both S_OK and S_FALSE. A preexisting STA is usable as-is.
        let status = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if status == RPC_E_CHANGED_MODE {
            Ok(Self(false))
        } else {
            status.ok()?;
            Ok(Self(true))
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

struct PropertyVariant(VARIANT);

impl Drop for PropertyVariant {
    fn drop(&mut self) {
        unsafe {
            let _ = VariantClear(&mut self.0);
        }
    }
}

struct MediaType(NonNull<AM_MEDIA_TYPE>);

impl Drop for MediaType {
    fn drop(&mut self) {
        unsafe {
            let media = self.0.as_mut();
            if media.cbFormat != 0 {
                CoTaskMemFree(Some(media.pbFormat.cast()));
            }
            ManuallyDrop::drop(&mut media.pUnk);
            CoTaskMemFree(Some(self.0.as_ptr().cast()));
        }
    }
}

fn com_error(error: Error) -> CaptureError {
    CaptureError {
        code: error.code().0,
        message: error.to_string(),
    }
}

fn output_modes(filter: &IBaseFilter) -> windows::core::Result<Vec<DshowCapability>> {
    let pins = unsafe { filter.EnumPins()? };
    let mut modes = Vec::new();
    loop {
        let mut pin = [None];
        let status = unsafe { pins.Next(&mut pin, None) };
        if status == S_FALSE {
            break;
        }
        status.ok()?;
        let [Some(pin)] = pin else {
            break;
        };
        if unsafe { pin.QueryDirection() }.ok() != Some(PINDIR_OUTPUT) {
            continue;
        }
        if let Ok(config) = pin.cast::<IAMStreamConfig>() {
            append_stream_capabilities(&config, &mut modes);
        }
    }
    modes.sort_unstable_by(|a, b| {
        (a.format as u8, a.width, a.height, Reverse(&a.fps)).cmp(&(
            b.format as u8,
            b.width,
            b.height,
            Reverse(&b.fps),
        ))
    });
    modes.dedup_by(|a, b| {
        a.format == b.format && a.width == b.width && a.height == b.height && a.fps == b.fps
    });
    Ok(modes)
}

fn video_device_monikers() -> windows::core::Result<Option<IEnumMoniker>> {
    unsafe {
        let devices: ICreateDevEnum =
            CoCreateInstance(&CLSID_SystemDeviceEnum, None, CLSCTX_INPROC_SERVER)?;
        let mut monikers = None;
        devices.CreateClassEnumerator(&CLSID_VideoInputDeviceCategory, &mut monikers, 0)?;
        Ok(monikers)
    }
}

fn next_moniker(monikers: &IEnumMoniker) -> windows::core::Result<Option<IMoniker>> {
    let mut item = [None];
    let status = unsafe { monikers.Next(&mut item, None) };
    if status == S_FALSE {
        return Ok(None);
    }
    status.ok()?;
    let [moniker] = item;
    Ok(moniker)
}

fn friendly_name(moniker: &IMoniker) -> Option<String> {
    unsafe {
        let bag: IPropertyBag = moniker.BindToStorage(None, None).ok()?;
        let mut property = PropertyVariant(VARIANT::default());
        bag.Read(w!("FriendlyName"), &mut property.0, None).ok()?;
        let value = &*property.0.Anonymous.Anonymous;
        if value.vt != VT_BSTR {
            return None;
        }
        let name = value.Anonymous.bstrVal.to_string();
        (!name.is_empty()).then_some(name)
    }
}

pub(super) fn list_devices() -> Result<Vec<String>, CaptureError> {
    let _apartment = ComApartment::new().map_err(com_error)?;
    let mut names = Vec::new();
    if let Some(monikers) = video_device_monikers().map_err(com_error)? {
        while let Some(moniker) = next_moniker(&monikers).map_err(com_error)? {
            names.extend(friendly_name(&moniker));
        }
    }
    Ok(names)
}

pub(super) fn device_capabilities(name: &str) -> Result<Vec<DshowCapability>, CaptureError> {
    let _apartment = ComApartment::new().map_err(com_error)?;
    if let Some(monikers) = video_device_monikers().map_err(com_error)? {
        while let Some(moniker) = next_moniker(&monikers).map_err(com_error)? {
            if friendly_name(&moniker).as_deref() == Some(name) {
                let filter: IBaseFilter =
                    unsafe { moniker.BindToObject(None, None) }.map_err(com_error)?;
                return output_modes(&filter).map_err(com_error);
            }
        }
    }
    Err(CaptureError {
        code: -1,
        message: format!("DirectShow device {name:?} was not found"),
    })
}

fn pixel_format(subtype: GUID) -> Option<CapturePixelFormat> {
    use CapturePixelFormat::*;
    [
        (MEDIASUBTYPE_MJPG, Mjpeg),
        (MEDIASUBTYPE_YUY2, Yuyv),
        (MEDIASUBTYPE_YVYU, Yvyu),
        (MEDIASUBTYPE_UYVY, Uyvy),
        (MEDIASUBTYPE_NV12, Nv12),
        (MEDIASUBTYPE_RGB24, Rgb24),
        (MEDIASUBTYPE_RGB32, Bgr24),
        (MEDIASUBTYPE_IYUV, Yuv420),
        (MEDIASUBTYPE_YV12, Yvu420),
    ]
    .into_iter()
    .find_map(|(guid, format)| (guid == subtype).then_some(format))
}

fn stream_mode(media: &AM_MEDIA_TYPE, caps: &VIDEO_STREAM_CONFIG_CAPS) -> Option<DshowCapability> {
    let format = pixel_format(media.subtype)?;
    let len = media.cbFormat as usize;
    let (mut width, mut height, interval) = if !media.pbFormat.is_null() {
        // GetStreamCaps owns the allocation and declares its type and length.
        unsafe {
            if media.formattype == FORMAT_VideoInfo && len >= size_of::<VIDEOINFOHEADER>() {
                let info = ptr::read_unaligned(media.pbFormat.cast::<VIDEOINFOHEADER>());
                (
                    info.bmiHeader.biWidth,
                    info.bmiHeader.biHeight.saturating_abs(),
                    info.AvgTimePerFrame,
                )
            } else if media.formattype == FORMAT_VideoInfo2 && len >= size_of::<VIDEOINFOHEADER2>()
            {
                let info = ptr::read_unaligned(media.pbFormat.cast::<VIDEOINFOHEADER2>());
                (
                    info.bmiHeader.biWidth,
                    info.bmiHeader.biHeight.saturating_abs(),
                    info.AvgTimePerFrame,
                )
            } else {
                (0, 0, 0)
            }
        }
    } else {
        (0, 0, 0)
    };
    if width <= 0 || height <= 0 {
        width = caps.InputSize.cx.max(caps.MinOutputSize.cx);
        height = caps.InputSize.cy.max(caps.MinOutputSize.cy);
        if width <= 0 || height <= 0 {
            width = caps.MaxOutputSize.cx;
            height = caps.MaxOutputSize.cy;
        }
    }
    if width <= 0 || height <= 0 {
        return None;
    }

    let mut fps: Vec<_> = [interval, caps.MinFrameInterval, caps.MaxFrameInterval]
        .into_iter()
        .filter(|interval| *interval > 0)
        .map(|interval| (10_000_000.0 / interval as f64).round() as u32)
        .filter(|fps| *fps != 0)
        .collect();
    fps.sort_unstable_by_key(|fps| Reverse(*fps));
    fps.dedup();
    Some(DshowCapability {
        format,
        width: width as u32,
        height: height as u32,
        fps,
    })
}

fn append_stream_capabilities(config: &IAMStreamConfig, modes: &mut Vec<DshowCapability>) {
    let (mut count, mut size) = (0, 0);
    if unsafe { config.GetNumberOfCapabilities(&mut count, &mut size) }.is_err() {
        return;
    }
    let Ok(bytes) = usize::try_from(size) else {
        return;
    };
    if bytes < size_of::<VIDEO_STREAM_CONFIG_CAPS>() {
        return;
    }

    let mut storage = Vec::new();
    if storage.try_reserve_exact(bytes).is_err() {
        return;
    }
    storage.resize(bytes, 0);
    for index in 0..count {
        let mut raw = ptr::null_mut();
        let result = unsafe { config.GetStreamCaps(index, &mut raw, storage.as_mut_ptr()) };
        let Some(media) = NonNull::new(raw).map(MediaType) else {
            continue;
        };
        if result.is_ok() {
            let caps =
                unsafe { ptr::read_unaligned(storage.as_ptr().cast::<VIDEO_STREAM_CONFIG_CAPS>()) };
            modes.extend(stream_mode(unsafe { media.0.as_ref() }, &caps));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directshow_mode_preserves_top_down_height_and_supported_rates() {
        let mut header = VIDEOINFOHEADER::default();
        header.bmiHeader.biWidth = 1920;
        header.bmiHeader.biHeight = -1080;
        header.AvgTimePerFrame = 333_333;
        let media = AM_MEDIA_TYPE {
            subtype: MEDIASUBTYPE_MJPG,
            formattype: FORMAT_VideoInfo,
            cbFormat: size_of::<VIDEOINFOHEADER>() as u32,
            pbFormat: (&mut header as *mut VIDEOINFOHEADER).cast(),
            ..Default::default()
        };
        let caps = VIDEO_STREAM_CONFIG_CAPS {
            MinFrameInterval: 166_667,
            MaxFrameInterval: 333_333,
            ..Default::default()
        };

        let mode = stream_mode(&media, &caps).unwrap();
        assert_eq!(mode.format, CapturePixelFormat::Mjpeg);
        assert_eq!((mode.width, mode.height), (1920, 1080));
        assert_eq!(mode.fps, [60, 30]);
    }
}
