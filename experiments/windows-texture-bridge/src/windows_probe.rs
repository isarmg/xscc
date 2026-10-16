use anyhow::{Context, Result, bail, ensure};
use std::{
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0},
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
    },
    core::{HRESULT, Interface},
};

const TIMEOUT_MS: u32 = 2_000;
const FRAMES: u8 = 30;

struct Device {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    adapter: DXGI_ADAPTER_DESC1,
}

impl Device {
    fn new(adapter: Option<&IDXGIAdapter1>, warp: bool, debug: bool) -> Result<Self> {
        let adapter: Option<IDXGIAdapter> = adapter.map(Interface::cast).transpose()?;
        let mut device = None;
        let mut context = None;
        let mut flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
        if debug {
            flags |= D3D11_CREATE_DEVICE_DEBUG;
        }
        unsafe {
            D3D11CreateDevice(
                adapter.as_ref(),
                if warp {
                    D3D_DRIVER_TYPE_WARP
                } else {
                    D3D_DRIVER_TYPE_UNKNOWN
                },
                HMODULE::default(),
                flags,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .context("create D3D11 device; --debug requires an already installed debug layer")?;
        }
        let device = device.context("D3D11 returned no device")?;
        let context = context.context("D3D11 returned no context")?;
        let dxgi: IDXGIDevice = device.cast()?;
        let adapter: IDXGIAdapter1 = unsafe { dxgi.GetAdapter()? }.cast()?;
        let adapter = unsafe { adapter.GetDesc1()? };
        Ok(Self {
            device,
            context,
            adapter,
        })
    }

    fn texture(&self, desc: &D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D> {
        let mut texture = None;
        unsafe {
            self.device
                .CreateTexture2D(desc, None, Some(&mut texture))?;
        }
        texture.context("D3D11 returned no texture")
    }
}

// windows-rs maps all nonnegative HRESULTs to Ok, but AcquireSync requires
// exactly S_OK: WAIT_TIMEOUT and WAIT_ABANDONED are nonnegative failures here.
fn acquire_status(mutex: &IDXGIKeyedMutex, key: u64, timeout: u32) -> HRESULT {
    unsafe { (mutex.vtable().AcquireSync)(mutex.as_raw(), key, timeout) }
}

fn acquire(mutex: &IDXGIKeyedMutex, key: u64) -> Result<()> {
    let status = acquire_status(mutex, key, TIMEOUT_MS);
    ensure!(
        status.0 == 0,
        "AcquireSync({key}) failed: {status:?}; resource must not be used"
    );
    Ok(())
}

fn check_readback(
    consumer: &Device,
    staging: &ID3D11Texture2D,
    expected: &[u8],
    width: u32,
    height: u32,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_millis(TIMEOUT_MS.into());
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    loop {
        let result = unsafe {
            consumer.context.Map(
                staging,
                0,
                D3D11_MAP_READ,
                D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,
                Some(&mut mapped),
            )
        };
        match result {
            Ok(()) => break,
            Err(error) if error.code() == DXGI_ERROR_WAS_STILL_DRAWING => {
                ensure!(Instant::now() < deadline, "readback timed out");
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return Err(error).context("map diagnostic staging texture"),
        }
    }
    let result = (|| {
        ensure!(!mapped.pData.is_null(), "Map returned a null pointer");
        ensure!(mapped.RowPitch >= width * 4, "invalid mapped row pitch");
        // Read only through the last pixel, never beyond the last mapped row.
        let length = (mapped.RowPitch as usize)
            .checked_mul(height as usize - 1)
            .and_then(|bytes| bytes.checked_add(width as usize * 4))
            .context("mapped byte count overflow")?;
        let bytes = unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), length) };
        super::verify_rows(
            bytes,
            mapped.RowPitch as usize,
            expected,
            width as usize,
            height as usize,
        )
    })();
    unsafe {
        consumer.context.Unmap(staging, 0);
    }
    result
}

fn probe_size(producer: &Device, consumer: &Device, width: u32, height: u32) -> Result<()> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
        ..Default::default()
    };
    let source = producer.texture(&desc)?;
    let shared = producer.texture(&D3D11_TEXTURE2D_DESC {
        MiscFlags: D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32,
        ..desc
    })?;
    let resource: IDXGIResource = shared.cast()?;
    // This legacy shared handle belongs to the resource, not CloseHandle. Both
    // resource references remain alive until all frame work has completed.
    let handle = unsafe { resource.GetSharedHandle()? };
    ensure!(!handle.is_invalid(), "invalid shared resource handle");
    let mut imported: Option<ID3D11Texture2D> = None;
    unsafe {
        consumer.device.OpenSharedResource(handle, &mut imported)?;
    }
    let imported = imported.context("OpenSharedResource returned no texture")?;
    let mut imported_desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        imported.GetDesc(&mut imported_desc);
    }
    ensure!(
        imported_desc.Width == width
            && imported_desc.Height == height
            && imported_desc.Format == desc.Format
            && imported_desc.MipLevels == 1
            && imported_desc.ArraySize == 1
            && imported_desc.SampleDesc.Count == 1
            && imported_desc.SampleDesc.Quality == 0
            && imported_desc.BindFlags == desc.BindFlags
            && imported_desc.Usage == D3D11_USAGE_DEFAULT
            && imported_desc.CPUAccessFlags == 0
            && imported_desc.MiscFlags == D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32,
        "imported texture descriptor mismatch"
    );
    let producer_mutex: IDXGIKeyedMutex = shared.cast()?;
    let consumer_mutex: IDXGIKeyedMutex = imported.cast()?;
    let staging = consumer.texture(&D3D11_TEXTURE2D_DESC {
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        ..desc
    })?;
    let status = acquire_status(&consumer_mutex, 1, 0);
    ensure!(
        status.0 == 0x102,
        "unreleased key must return WAIT_TIMEOUT, got {status:?}"
    );

    for frame in 0..FRAMES {
        let pixels = super::pattern(width as usize, height as usize, frame);
        acquire(&producer_mutex, 0)?;
        unsafe {
            producer.context.UpdateSubresource(
                &source,
                0,
                None,
                pixels.as_ptr().cast(),
                width * 4,
                0,
            );
            producer.context.CopyResource(&shared, &source);
            producer.context.Flush();
            producer_mutex.ReleaseSync(1)?;
        }
        // Exercise a delayed handoff, without concurrent use of either context.
        thread::sleep(Duration::from_millis(2));
        acquire(&consumer_mutex, 1)?;
        unsafe {
            consumer.context.CopyResource(&staging, &imported);
            consumer.context.Flush();
            consumer_mutex.ReleaseSync(0)?;
        }
        check_readback(consumer, &staging, &pixels, width, height)
            .with_context(|| format!("{width}x{height} frame {frame}"))?;
        unsafe {
            producer.device.GetDeviceRemovedReason()?;
            consumer.device.GetDeviceRemovedReason()?;
        }
    }
    println!(
        "PASS size={width}x{height} frames={FRAMES} exact_bgra=true timeout_negative_test=true"
    );
    Ok(())
}

pub fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let warp = args.iter().any(|s| s == "--warp");
    let hardware = args.iter().any(|s| s == "--hardware");
    let debug = args.iter().any(|s| s == "--debug");
    if warp == hardware
        || args
            .iter()
            .any(|s| !["--warp", "--hardware", "--debug"].contains(&s.as_str()))
    {
        bail!("Usage: xscc-windows-texture-bridge (--warp | --hardware) [--debug]");
    }
    let adapter = if hardware {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
        let adapter = unsafe { factory.EnumAdapters1(0)? };
        let desc = unsafe { adapter.GetDesc1()? };
        ensure!(
            desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0,
            "adapter 0 is software; hardware mode refuses fallback"
        );
        Some(adapter)
    } else {
        None
    };
    let producer = Device::new(adapter.as_ref(), warp, debug)?;
    let consumer = Device::new(adapter.as_ref(), warp, debug)?;
    let a = producer.adapter.AdapterLuid;
    let b = consumer.adapter.AdapterLuid;
    ensure!(
        a.HighPart == b.HighPart && a.LowPart == b.LowPart,
        "producer/consumer adapter LUID mismatch"
    );
    ensure!(
        producer.device.as_raw() != consumer.device.as_raw(),
        "expected two distinct devices"
    );
    let name = String::from_utf16_lossy(&producer.adapter.Description);
    println!(
        "mode={} adapter={} luid={:08x}:{:08x} distinct_devices=true",
        if warp { "WARP" } else { "hardware" },
        name.trim_end_matches('\0'),
        a.HighPart as u32,
        a.LowPart
    );
    for (width, height) in [(64, 48), (257, 129), (64, 48)] {
        probe_size(&producer, &consumer, width, height)?;
    }
    println!(
        "PASS diagnostic only: CPU synthetic upload + GPU bridge copy + staging readback; no GPUI, decoder, presentation, or zero-copy claim"
    );
    Ok(())
}
