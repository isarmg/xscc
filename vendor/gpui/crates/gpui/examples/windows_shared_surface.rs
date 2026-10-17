//! Opt-in synthetic shared-texture integration proof, not a video benchmark.
#[cfg(not(target_os = "windows"))]
fn main() {
    panic!("This integration example requires Windows; no GPU test was performed");
}

#[cfg(target_os = "windows")]
fn main() -> anyhow::Result<()> {
    windows_example::run()
}

#[cfg(target_os = "windows")]
mod windows_example {
    use anyhow::{Context as _, Result, ensure};
    use gpui::{
        App, Bounds, Context, MouseButton, Window, WindowBounds, WindowOptions, WindowsSurface,
        div, prelude::*, px, rgb, size, surface,
    };
    use gpui_platform::application;
    use windows::{
        Win32::{
            Foundation::HMODULE,
            Graphics::{
                Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0},
                Direct3D11::*,
                Dxgi::{Common::*, *},
            },
        },
        core::Interface,
    };

    struct Proof {
        frames: [WindowsSurface; 2],
        alternate: bool,
    }

    impl Render for Proof {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().bg(rgb(0x222222)).p_8().child(
                div()
                    .relative()
                    .w(px(400.))
                    .h(px(210.))
                    .overflow_hidden()
                    .child(
                        surface(self.frames[usize::from(self.alternate)].clone())
                            .absolute()
                            .left(px(-30.))
                            .top(px(-15.))
                            .w(px(460.))
                            .h(px(240.)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(280.))
                            .top(px(140.))
                            .w(px(100.))
                            .h(px(50.))
                            .opacity(0.5)
                            .child(
                                surface(self.frames[usize::from(self.alternate)].clone())
                                    .size_full(),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(30.))
                            .top(px(35.))
                            .size(px(85.))
                            .bg(rgb(if self.alternate { 0xffff00 } else { 0xff00ff }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.alternate = !this.alternate;
                                    cx.notify();
                                }),
                            ),
                    ),
            )
        }
    }

    pub fn run() -> Result<()> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
        let adapter = unsafe { factory.EnumAdapters1(0)? };
        let desc = unsafe { adapter.GetDesc1()? };
        ensure!(
            desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0,
            "this presentation proof requires a physical adapter; no WARP fallback"
        );
        eprintln!(
            "Producer adapter={} LUID={:08x}:{:08x}",
            String::from_utf16_lossy(&desc.Description).trim_end_matches('\0'),
            desc.AdapterLuid.HighPart,
            desc.AdapterLuid.LowPart
        );
        let adapter: IDXGIAdapter = adapter.cast()?;
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        let device = device.context("no producer device")?;
        let context = context.context("no producer context")?;
        let frames = [
            publish_frame(&device, &context, 0)?,
            publish_frame(&device, &context, 1)?,
        ];
        eprintln!("Published two immutable 257x129 BGRA frames with one GPU isolation copy each.");
        eprintln!(
            "Click the colored foreground square repeatedly. It must stay above the changing pattern."
        );
        eprintln!(
            "Resize, cover/uncover, minimize/restore, and move between DPI scales. No automatic PASS is claimed."
        );
        application().run(move |cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(520.), px(350.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|_| Proof {
                        frames,
                        alternate: false,
                    })
                },
            )
            .expect("open integration proof window");
            cx.activate(true);
        });
        Ok(())
    }

    fn publish_frame(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        phase: u8,
    ) -> Result<WindowsSurface> {
        let width = 257;
        let height = 129;
        let pixels: Vec<u8> = (0..height)
            .flat_map(|y| {
                (0..width).flat_map(move |x| {
                    if (x / 16 + y / 16 + u32::from(phase)) % 2 == 0 {
                        [255, 50, 0, 255]
                    } else {
                        [0, 200, 255, 255]
                    }
                })
            })
            .collect();
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
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let mut producer = None;
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut producer))? };
        let producer = producer.context("no producer texture")?;
        unsafe {
            context.UpdateSubresource(&producer, 0, None, pixels.as_ptr().cast(), width * 4, 0)
        };
        let mut bridge = None;
        let shared_desc = D3D11_TEXTURE2D_DESC {
            BindFlags: (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
            MiscFlags: D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32,
            ..desc
        };
        unsafe { device.CreateTexture2D(&shared_desc, None, Some(&mut bridge))? };
        let bridge = bridge.context("no shared texture")?;
        let mutex: IDXGIKeyedMutex = bridge.cast()?;
        let status = unsafe { (mutex.vtable().AcquireSync)(mutex.as_raw(), 0, 0) };
        ensure!(status.0 == 0, "producer AcquireSync failed: {status:?}");
        unsafe {
            context.CopyResource(&bridge, &producer);
            context.Flush();
            mutex.ReleaseSync(1)?;
        }
        // This function retains no mutable aliases; every published texture is unique.
        unsafe { WindowsSurface::from_published_texture(bridge) }
    }
}
