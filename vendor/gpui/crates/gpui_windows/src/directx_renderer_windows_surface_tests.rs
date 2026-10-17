use super::{
    DirectXAtlas, DirectXGlobalElements, DirectXRenderPipelines, DirectXRenderer,
    DirectXRendererDevices, FontInfo, GlobalParams, set_rasterizer_state, update_buffer,
};
use anyhow::{Context, Result};
use gpui::{
    BorderStyle, Bounds, ContentMask, Corners, Edges, PaintSurface, Quad, ScaledPixels,
    WindowsSurface, point, rgb, size, transparent_black,
};
use std::{
    slice,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            Dxgi::{Common::*, *},
        },
    },
    core::Interface,
};

fn warp_device() -> Result<DirectXRendererDevices> {
    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_WARP,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    let device = device.context("WARP returned no device")?;
    let dxgi: IDXGIDevice = device.cast()?;
    let adapter: IDXGIAdapter1 = unsafe { dxgi.GetAdapter()? }.cast()?;
    let desc = unsafe { adapter.GetDesc1()? };
    anyhow::ensure!(
        desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
        "expected WARP software adapter"
    );
    Ok(DirectXRendererDevices {
        dxgi_factory: unsafe { adapter.GetParent()? },
        adapter,
        device,
        device_context: context.context("WARP returned no context")?,
        dxgi_device: Some(dxgi),
        annotation: None,
    })
}

fn texture(device: &ID3D11Device, desc: &D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D> {
    let mut result = None;
    unsafe { device.CreateTexture2D(desc, None, Some(&mut result))? };
    result.context("no texture")
}

fn publish_pattern(producer: &DirectXRendererDevices) -> Result<WindowsSurface> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: 9,
        Height: 5,
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
    let pixels: Vec<u8> = (0..5)
        .flat_map(|_| {
            (0..9).flat_map(|x| {
                if x < 4 {
                    [0, 0, 255, 255]
                } else {
                    [0, 255, 0, 255]
                }
            })
        })
        .collect();
    let source = texture(&producer.device, &desc)?;
    unsafe {
        producer
            .device_context
            .UpdateSubresource(&source, 0, None, pixels.as_ptr().cast(), 36, 0)
    };
    let shared = texture(
        &producer.device,
        &D3D11_TEXTURE2D_DESC {
            MiscFlags: D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32,
            ..desc
        },
    )?;
    let mutex: IDXGIKeyedMutex = shared.cast()?;
    let status = unsafe { (mutex.vtable().AcquireSync)(mutex.as_raw(), 0, 0) };
    anyhow::ensure!(status.0 == 0, "producer AcquireSync failed: {status:?}");
    unsafe {
        producer.device_context.CopyResource(&shared, &source);
        producer.device_context.Flush();
        mutex.ReleaseSync(1)?;
        // The bridge is independent of source and is never mutated after publication.
        WindowsSurface::from_published_texture(shared)
    }
}

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(x), ScaledPixels(y)),
        size: size(ScaledPixels(width), ScaledPixels(height)),
    }
}

fn read_pixels(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    target: &ID3D11Texture2D,
) -> Result<Vec<u8>> {
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { target.GetDesc(&mut desc) };
    let staging = texture(
        device,
        &D3D11_TEXTURE2D_DESC {
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            ..desc
        },
    )?;
    unsafe {
        context.CopyResource(&staging, target);
        context.Flush();
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    loop {
        match unsafe {
            context.Map(
                &staging,
                0,
                D3D11_MAP_READ,
                D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32,
                Some(&mut mapped),
            )
        } {
            Ok(()) => break,
            Err(error) if error.code() == DXGI_ERROR_WAS_STILL_DRAWING => {
                anyhow::ensure!(Instant::now() < deadline, "WARP readback timed out");
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => return Err(error.into()),
        }
    }
    let result = (|| -> Result<Vec<u8>> {
        let row_bytes = desc.Width as usize * 4;
        anyhow::ensure!(
            !mapped.pData.is_null() && mapped.RowPitch as usize >= row_bytes,
            "invalid mapping"
        );
        let mut pixels = vec![0; row_bytes * desc.Height as usize];
        for y in 0..desc.Height as usize {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    mapped.pData.cast::<u8>().add(y * mapped.RowPitch as usize),
                    pixels.as_mut_ptr().add(y * row_bytes),
                    row_bytes,
                );
            }
        }
        Ok(pixels)
    })();
    unsafe { context.Unmap(&staging, 0) };
    result
}

#[test]
fn windows_surface_warp_draw_pixels() -> Result<()> {
    let producer = warp_device()?;
    let devices = warp_device()?;
    anyhow::ensure!(
        producer.device != devices.device,
        "producer and consumer must be distinct devices"
    );
    let source_luid = unsafe { producer.adapter.GetDesc1()? }.AdapterLuid;
    let target_luid = unsafe { devices.adapter.GetDesc1()? }.AdapterLuid;
    anyhow::ensure!(
        source_luid.HighPart == target_luid.HighPart && source_luid.LowPart == target_luid.LowPart,
        "WARP adapter mismatch"
    );
    let published = publish_pattern(&producer)?;
    drop(producer);
    let surfaces = [
        PaintSurface {
            order: 0,
            bounds: bounds(2., 2., 18., 10.),
            content_mask: ContentMask {
                bounds: bounds(4., 4., 12., 6.),
            },
            opacity: 0.5,
            windows_surface: published.clone(),
        },
        PaintSurface {
            order: 0,
            bounds: bounds(22., 2., 8., 10.),
            content_mask: ContentMask {
                bounds: bounds(0., 0., 32., 24.),
            },
            opacity: 1.,
            windows_surface: published.clone(),
        },
    ];
    drop(published);
    let target = texture(
        &devices.device,
        &D3D11_TEXTURE2D_DESC {
            Width: 32,
            Height: 24,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..Default::default()
        },
    )?;
    let mut target_view = None;
    unsafe {
        devices
            .device
            .CreateRenderTargetView(&target, None, Some(&mut target_view))?
    };
    let target_view = target_view.context("no target view")?;
    let mut renderer = DirectXRenderer {
        hwnd: HWND::default(),
        atlas: Arc::new(DirectXAtlas::new(&devices.device, &devices.device_context)),
        globals: DirectXGlobalElements::new(&devices.device)?,
        pipelines: DirectXRenderPipelines::new(&devices.device)?,
        devices: Some(devices.clone()),
        resources: None,
        direct_composition: None,
        font_info: &FontInfo {
            gamma_ratios: [1.; 4],
            grayscale_enhanced_contrast: 0.,
            subpixel_enhanced_contrast: 0.,
            is_bgr: false,
        },
        width: 32,
        height: 24,
        skip_draws: false,
    };
    let context = &devices.device_context;
    set_rasterizer_state(&devices.device, context)?;
    update_buffer(
        context,
        renderer
            .globals
            .global_params_buffer
            .as_ref()
            .context("missing globals")?,
        &[GlobalParams {
            viewport_size: [32., 24.],
            ..Default::default()
        }],
    )?;
    unsafe {
        context.OMSetRenderTargets(Some(&[Some(target_view.clone())]), None);
        context.RSSetViewports(Some(&[D3D11_VIEWPORT {
            Width: 32.,
            Height: 24.,
            MaxDepth: 1.,
            ..Default::default()
        }]));
        context.VSSetConstantBuffers(
            0,
            Some(slice::from_ref(&renderer.globals.global_params_buffer)),
        );
        context.VSSetConstantBuffers(
            1,
            Some(slice::from_ref(&renderer.globals.batch_params_buffer)),
        );
        context.PSSetConstantBuffers(
            0,
            Some(slice::from_ref(&renderer.globals.global_params_buffer)),
        );
    }
    let foreground = Quad {
        order: 1,
        border_style: BorderStyle::Solid,
        bounds: bounds(6., 5., 4., 4.),
        content_mask: ContentMask {
            bounds: bounds(0., 0., 32., 24.),
        },
        background: rgb(0x00ff00).into(),
        border_color: transparent_black(),
        corner_radii: Corners::default(),
        border_widths: Edges::default(),
    };
    renderer
        .pipelines
        .quad_pipeline
        .update_buffer(&devices.device, context, &[foreground])?;
    for _ in 0..3 {
        unsafe { context.ClearRenderTargetView(&target_view, &[0., 0., 1., 1.]) };
        // Deliberate call order checks shader composition, not Scene batching or presentation.
        renderer.draw_surfaces(&surfaces)?;
        renderer.draw_quads(0, 1)?;
        let pixels = read_pixels(&devices.device, context, &target)?;
        for (x, y, expected) in [
            (0, 0, [255, 0, 0, 255]),
            (3, 5, [255, 0, 0, 255]),
            (17, 5, [255, 0, 0, 255]),
            (5, 3, [255, 0, 0, 255]),
            (5, 10, [255, 0, 0, 255]),
            (5, 5, [128, 0, 128, 255]),
            (14, 5, [128, 128, 0, 255]),
            (7, 6, [0, 255, 0, 255]),
            (24, 6, [0, 0, 255, 255]),
            (28, 6, [0, 255, 0, 255]),
        ] {
            let offset = (y * 32 + x) * 4;
            for channel in 0..4 {
                assert!(
                    pixels[offset + channel].abs_diff(expected[channel]) <= 1,
                    "pixel ({x},{y}) channel {channel}: actual {:?}, expected {expected:?}",
                    &pixels[offset..offset + 4]
                );
            }
        }
    }
    unsafe { devices.device.GetDeviceRemovedReason()? };
    Ok(())
}
