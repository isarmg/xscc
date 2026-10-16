#![cfg_attr(
    all(target_os = "windows", feature = "live-session", not(debug_assertions)),
    windows_subsystem = "windows"
)]

#[cfg(all(target_os = "windows", feature = "live-session"))]
mod client;
#[cfg(any(all(target_os = "windows", feature = "live-session"), test))]
mod host_address;
mod input;
mod options;
#[cfg(all(target_os = "windows", feature = "live-session"))]
mod presentation;
#[cfg(all(target_os = "windows", feature = "live-session"))]
mod remote_video;
#[cfg(all(target_os = "windows", feature = "live-session"))]
mod text_field;
#[cfg(any(all(target_os = "windows", feature = "live-session"), test))]
mod theme;
use options::Options;
#[path = "../../shared/management.rs"]
mod management;
#[path = "../../shared/video.rs"]
mod video;

use gpui::{
    App, AppContext, Bounds, Context, FocusHandle, ImageSource, IntoElement, KeyDownEvent,
    KeyUpEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels,
    Point, Render, RenderImage, TitlebarOptions, Window, WindowBounds, WindowOptions, canvas, div,
    img, prelude::*, px, rgb, size,
};
use protobuf::Message as _;
use std::{
    cell::Cell,
    fs::OpenOptions,
    io::Write,
    rc::Rc,
    sync::{Arc, Mutex},
    time::Instant,
};
use xscc_native_management::manager::NativeManager;

const REVISION: &str = "28d0a3547f16b9609b301b48ab363bfca649c864";

#[derive(Default)]
struct Measurements {
    submitted: u64,
    render_calls: u64,
    inputs: u64,
    key_down: u64,
    key_up: u64,
    pointer_events: u64,
    source_to_submit_ms: Vec<f64>,
    error: Option<String>,
}
impl Measurements {
    fn record(&mut self, message: &base::message_proto::Message) {
        match message
            .write_to_bytes()
            .and_then(|bytes| base::message_proto::Message::parse_from_bytes(&bytes))
        {
            Ok(_) => self.inputs += 1,
            Err(error) => self.error = Some(format!("input serialization: {error}")),
        }
    }
}
struct Probe {
    focus: FocusHandle,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    extent: (u32, u32),
    mailbox: Arc<Mutex<video::Mailbox>>,
    stats: Arc<Mutex<Measurements>>,
    frame: Option<Arc<RenderImage>>,
    manager: Option<Arc<NativeManager>>,
    diagnostic: bool,
}
impl Probe {
    fn pointer(&mut self, position: Point<Pixels>, kind: i32, button: i32) {
        let bounds = self.bounds.get();
        if let Some(message) = input::pointer(
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
            (bounds.size.width.into(), bounds.size.height.into()),
            self.extent,
            kind,
            button,
        ) {
            let mut stats = self.stats.lock().unwrap();
            stats.record(&message);
            stats.pointer_events += 1;
        }
    }
    fn key(&mut self, key: &gpui::Keystroke, down: bool) {
        if let Some(message) = input::key(key, down) {
            let mut stats = self.stats.lock().unwrap();
            stats.record(&message);
            if down {
                stats.key_down += 1;
            } else {
                stats.key_up += 1;
            }
        }
    }
}
impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.diagnostic {
            window.request_animation_frame();
        }
        self.stats.lock().unwrap().render_calls += 1;
        let frame = self.mailbox.lock().unwrap().frame.take();
        if let Some(mut frame) = frame {
            for pixel in frame.rgba.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            let pixels = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba);
            if let Some(pixels) = pixels {
                let image = Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]));
                if let Some(previous) = self.frame.replace(image)
                    && let Err(error) = window.drop_image(previous)
                {
                    self.stats.lock().unwrap().error = Some(error.to_string());
                }
                self.extent = (frame.width, frame.height);
                let mut stats = self.stats.lock().unwrap();
                stats.submitted += 1;
                stats
                    .source_to_submit_ms
                    .push(frame.ready.elapsed().as_secs_f64() * 1000.0);
            } else {
                self.stats.lock().unwrap().error = Some("invalid RGBA frame dimensions".into());
            }
        }
        let summary = {
            let stats = self.stats.lock().unwrap();
            format!(
                "Frames submitted: {} · input messages: {}",
                stats.submitted, stats.inputs
            )
        };
        let native = self
            .manager
            .as_ref()
            .map(|manager| {
                let status = manager.status();
                format!(
                    "Isolated management: {} · {:?} · {} allowed peers",
                    status.phase,
                    status.role,
                    status.peers.len()
                )
            })
            .unwrap_or_else(|| "Management disabled; use an isolated --state-dir".into());
        let peers = self
            .manager
            .as_ref()
            .map(|manager| {
                manager
                    .status()
                    .peers
                    .into_iter()
                    .map(|peer| {
                        format!(
                            "{} · {:?} · {}",
                            peer.name,
                            peer.role,
                            if peer.online { "online" } else { "offline" }
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let bounds = self.bounds.clone();
        div().id("probe").size_full().flex().flex_col().gap_3().p_4().bg(rgb(0x101923)).text_color(rgb(0xe9f1f8))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, _| view.key(&event.keystroke, true)))
            .on_key_up(cx.listener(|view, event: &KeyUpEvent, _, _| view.key(&event.keystroke, false)))
            .child(if self.diagnostic { "GPUI diagnostics · local input observation" } else { "XSCC GPUI · development preview" })
            .child(summary).child(native)
            .children(peers)
            .child(div().id("video").relative().w_full().flex_1().min_h(px(200.0)).bg(rgb(0))
                .on_mouse_down(MouseButton::Left, cx.listener(|view, event: &MouseDownEvent, window, cx| { view.focus.focus(window, cx); view.pointer(event.position, 1, 1); }))
                .on_mouse_up(MouseButton::Left, cx.listener(|view, event: &MouseUpEvent, _, _| view.pointer(event.position, 2, 1)))
                .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _, _| view.pointer(event.position, 0, 0)))
                .children((!self.diagnostic).then_some(div().p_4().child("Remote sessions are not connected in this preview. Use --diagnostic for the local rendering test.")))
                .children(self.frame.as_ref().map(|image| img(ImageSource::Render(image.clone())).size_full().object_fit(ObjectFit::Contain)))
                .child(canvas(move |area, _, _| bounds.set(area), |_, _, _, _| {}).absolute().top_0().left_0().size_full()))
            .child("No remote input is sent. Hardware decode, IME, touch, and complete sessions require separate acceptance.")
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(all(target_os = "windows", feature = "live-session"))]
    if librustdesk::gpui_host::handle_background_command() {
        return Ok(());
    }
    let options = Options::parse()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    #[cfg(all(target_os = "windows", feature = "live-session"))]
    if !options.diagnostic {
        return client::run(&runtime);
    }
    let manager = options
        .state_dir
        .as_ref()
        .map(|path| management::open(path))
        .transpose()?;
    if let Some(manager) = &manager {
        let active = manager.clone();
        runtime.spawn(async move {
            if let Err(error) = active.resume().await {
                eprintln!("isolated management resume failed: {error}");
            }
        });
    }
    let mailbox = Arc::new(Mutex::new(video::Mailbox::default()));
    let source = if options.diagnostic {
        Some(video::Source::start(
            mailbox.clone(),
            options.width,
            options.height,
            options.fps,
            options.ivf.as_deref(),
        )?)
    } else {
        None
    };
    let stats = Arc::new(Mutex::new(Measurements::default()));
    let started = Instant::now();
    let app_mailbox = mailbox.clone();
    let app_stats = stats.clone();
    let app_manager = manager.clone();
    gpui_platform::application().run(move |cx: &mut App| {
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        if let Some(seconds) = options.seconds {
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(seconds))
                    .await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        }
        let bounds = Bounds::centered(None, size(px(1000.0), px(760.0)), cx);
        let result = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(
                        if options.diagnostic {
                            "xscc — GPUI validation"
                        } else {
                            "xscc — GPUI development preview"
                        }
                        .into(),
                    ),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    focus.focus(window, cx);
                    Probe {
                        focus,
                        bounds: Rc::new(Cell::new(Bounds::default())),
                        extent: (options.width, options.height),
                        mailbox: app_mailbox,
                        stats: app_stats.clone(),
                        frame: None,
                        manager: app_manager,
                        diagnostic: options.diagnostic,
                    }
                })
            },
        );
        if let Err(error) = result {
            app_stats.lock().unwrap().error = Some(error.to_string());
            cx.quit();
        }
        cx.activate(true);
    });
    drop(source);
    if let Some(manager) = manager {
        runtime.block_on(manager.shutdown())?;
    }
    let elapsed = started.elapsed().as_secs_f64();
    let mut stats = stats.lock().unwrap();
    let frames = mailbox.lock().unwrap();
    if let Some(error) = &frames.error {
        stats.error = Some(error.clone());
    }
    stats.source_to_submit_ms.sort_by(f64::total_cmp);
    let percentile = |p: f64| {
        stats
            .source_to_submit_ms
            .get(((stats.source_to_submit_ms.len().saturating_sub(1)) as f64 * p) as usize)
            .copied()
    };
    let report = serde_json::json!({
        "schema": 1, "framework": "gpui", "revision": REVISION, "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "renderer": "platform-gpu", "mode": if options.diagnostic { "diagnostic" } else { "preview" },
        "source": if !options.diagnostic { "none" } else if options.ivf.is_some() { "xscc-vp9-software-decoder" } else { "synthetic-cpu-rgba" },
        "requested_size": [options.width, options.height], "requested_fps": options.fps,
        "elapsed_seconds": elapsed, "source_frames": frames.produced, "mailbox_replacements": frames.replaced,
        "submitted_frames": stats.submitted, "submitted_fps": stats.submitted as f64 / elapsed,
        "render_callbacks": stats.render_calls,
        "source_ready_to_submit_ms": {"p50": percentile(0.5), "p95": percentile(0.95), "p99": percentile(0.99)},
        "input_messages": stats.inputs, "key_down": stats.key_down, "key_up": stats.key_up, "pointer_events": stats.pointer_events,
        "error": stats.error, "replacement_ready": false,
        "unverified": ["physical GPU decoding and native texture interop", "display scanout latency", "authenticated remote session and input injection", "IME and touch", "five-platform physical-device acceptance"]
    });
    let bytes = serde_json::to_vec_pretty(&report)?;
    if let Some(path) = options.report {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?
            .write_all(&bytes)?;
    }
    println!("{}", String::from_utf8(bytes)?);
    if stats.error.is_some() || (options.diagnostic && stats.submitted < 2) {
        return Err("GPUI validation failed; inspect the JSON report".into());
    }
    Ok(())
}
