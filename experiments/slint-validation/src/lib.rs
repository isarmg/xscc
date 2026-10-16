mod gpu;
mod input;
mod management;
#[path = "../../shared/video.rs"]
mod video;

use protobuf::Message as _;
use slint::{ComponentHandle, Timer, TimerMode};
use std::{
    cell::RefCell,
    fs::OpenOptions,
    io::{self, Write},
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

slint::include_modules!();

#[derive(Clone)]
struct Options {
    renderer: String,
    width: u32,
    height: u32,
    fps: u32,
    seconds: u64,
    report: Option<PathBuf>,
    snapshot: Option<PathBuf>,
    ivf: Option<PathBuf>,
    state_dir: Option<PathBuf>,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            renderer: "wgpu".into(),
            width: 1280,
            height: 720,
            fps: 60,
            seconds: 15,
            report: None,
            snapshot: None,
            ivf: None,
            state_dir: None,
        }
    }
}
impl Options {
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    fn parse() -> Result<Self, Box<dyn std::error::Error>> {
        let mut options = Self::default();
        let mut arguments = std::env::args().skip(1);
        while let Some(key) = arguments.next() {
            if key == "--help" {
                println!(
                    "xscc-slint-validation [--renderer wgpu|software] [--width N] [--height N] [--fps N] [--seconds 1..600] [--ivf VP9.ivf] [--state-dir /absolute/private/path] [--report new.json] [--snapshot new.png]"
                );
                std::process::exit(0);
            }
            let value = arguments.next().ok_or("each option requires a value")?;
            match key.as_str() {
                "--renderer" => options.renderer = value,
                "--width" => options.width = value.parse()?,
                "--height" => options.height = value.parse()?,
                "--fps" => options.fps = value.parse()?,
                "--seconds" => options.seconds = value.parse()?,
                "--report" => options.report = Some(value.into()),
                "--snapshot" => options.snapshot = Some(value.into()),
                "--ivf" => options.ivf = Some(value.into()),
                "--state-dir" => options.state_dir = Some(value.into()),
                _ => return Err(format!("unknown option: {key}").into()),
            }
        }
        if !["wgpu", "software"].contains(&options.renderer.as_str())
            || options.width == 0
            || options.width > 3840
            || options.height == 0
            || options.height > 2160
            || !(1..=120).contains(&options.fps)
            || !(1..=600).contains(&options.seconds)
        {
            return Err("invalid renderer, dimensions, FPS, or duration".into());
        }
        Ok(options)
    }
}

#[derive(Default)]
struct Measurements {
    submitted_frames: u64,
    rendered_callbacks: u64,
    input_messages: u64,
    key_down: u64,
    key_up: u64,
    pointer_events: u64,
    text_edits: u64,
    secondary_windows: u64,
    rgba_texture_import: bool,
    planar_texture_import_rejected: bool,
    adapter: Option<String>,
    source_to_submit_ms: Vec<f64>,
    error: Option<String>,
    snapshot_saved: bool,
}
impl Measurements {
    fn input(&mut self, message: &base::message_proto::Message) {
        match message
            .write_to_bytes()
            .and_then(|bytes| base::message_proto::Message::parse_from_bytes(&bytes))
        {
            Ok(_) => self.input_messages += 1,
            Err(error) => self.error = Some(format!("input message serialization: {error}")),
        }
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let options = Options::default();
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let options = Options::parse()?;
    run_probe(options)
}

fn run_probe(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let manager = options
        .state_dir
        .as_ref()
        .map(|path| management::open(path))
        .transpose()?;
    let selector = slint::BackendSelector::new();
    if options.renderer == "wgpu" {
        selector.require_wgpu_30(Default::default()).select()?;
    } else {
        selector.renderer_name("software".into()).select()?;
    }
    let ui = ProbeWindow::new()?;
    if let Some(manager) = &manager {
        management::attach(&ui, manager.clone(), &runtime);
    }
    let mailbox = Arc::new(Mutex::new(video::Mailbox::default()));
    let source = video::Source::start(
        mailbox.clone(),
        options.width,
        options.height,
        options.fps,
        options.ivf.as_deref(),
    )?;
    let measurements = Rc::new(RefCell::new(Measurements::default()));
    let dimensions = Rc::new(RefCell::new((options.width, options.height)));
    let pending = Rc::new(RefCell::new(None::<video::Frame>));

    let active = measurements.clone();
    ui.on_keyboard(move |text, down, control, alt, shift, meta| {
        if let Some(message) = input::key(text.as_str(), down, [control, alt, shift, meta]) {
            let mut stats = active.borrow_mut();
            stats.input(&message);
            if down {
                stats.key_down += 1;
            } else {
                stats.key_up += 1;
            }
        }
    });
    let active = measurements.clone();
    let extent = dimensions.clone();
    ui.on_pointer(move |x, y, width, height, kind, button| {
        if let Some(message) = input::pointer(x, y, (width, height), *extent.borrow(), kind, button)
        {
            let mut stats = active.borrow_mut();
            stats.input(&message);
            stats.pointer_events += 1;
        }
    });
    let active = measurements.clone();
    ui.on_text_edited(move || {
        active.borrow_mut().text_edits += 1;
    });
    let second = Rc::new(RefCell::new(None::<SecondaryWindow>));
    let secondary = second.clone();
    let active = measurements.clone();
    ui.on_second_window(move || {
        match SecondaryWindow::new().and_then(|window| {
            window.show()?;
            Ok(window)
        }) {
            Ok(window) => {
                *secondary.borrow_mut() = Some(window);
                active.borrow_mut().secondary_windows += 1;
            }
            Err(error) => active.borrow_mut().error = Some(error.to_string()),
        }
    });

    if options.renderer == "wgpu" {
        let weak = ui.as_weak();
        let active = measurements.clone();
        let pending = pending.clone();
        let extent = dimensions.clone();
        let mut uploader = None::<gpu::Upload>;
        ui.window()
            .set_rendering_notifier(move |state, api| match state {
                slint::RenderingState::RenderingSetup => {
                    if let slint::GraphicsAPI::WGPU30 { device, queue, .. } = api {
                        let info = device.adapter_info();
                        active.borrow_mut().adapter = Some(format!(
                            "{} / {:?} / {:?}",
                            info.name, info.device_type, info.backend
                        ));
                        uploader = Some(gpu::Upload::new(device.clone(), queue.clone()));
                    } else {
                        active.borrow_mut().error =
                            Some("requested WGPU30 context was not supplied".into());
                    }
                }
                slint::RenderingState::BeforeRendering => {
                    if let (Some(ui), Some(uploader), Some(frame)) = (
                        weak.upgrade(),
                        uploader.as_mut(),
                        pending.borrow_mut().take(),
                    ) {
                        match uploader.frame(&frame) {
                            Ok(image) => {
                                *extent.borrow_mut() = (frame.width, frame.height);
                                ui.set_frame(image);
                                let mut stats = active.borrow_mut();
                                stats.submitted_frames += 1;
                                stats.rgba_texture_import = uploader.rgba_import_checked;
                                stats.planar_texture_import_rejected =
                                    uploader.planar_import_rejected;
                                stats
                                    .source_to_submit_ms
                                    .push(frame.ready.elapsed().as_secs_f64() * 1000.0);
                            }
                            Err(error) => active.borrow_mut().error = Some(error),
                        }
                    }
                }
                slint::RenderingState::AfterRendering => {
                    active.borrow_mut().rendered_callbacks += 1;
                }
                slint::RenderingState::RenderingTeardown => {
                    uploader = None;
                }
                _ => {}
            })?;
    }
    let started = Instant::now();
    let timer = Timer::default();
    let weak = ui.as_weak();
    let active = measurements.clone();
    let frames = mailbox.clone();
    let software = options.renderer == "software";
    let snapshot = options.snapshot.clone();
    let duration = Duration::from_secs(options.seconds);
    timer.start(
        TimerMode::Repeated,
        Duration::from_secs_f64(1.0 / f64::from(options.fps)),
        move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let frame = frames.lock().unwrap().frame.take();
            if let Some(frame) = frame {
                if software {
                    *dimensions.borrow_mut() = (frame.width, frame.height);
                    ui.set_frame(slint::Image::from_rgba8(slint::SharedPixelBuffer::<
                        slint::Rgba8Pixel,
                    >::clone_from_slice(
                        &frame.rgba,
                        frame.width,
                        frame.height,
                    )));
                    let mut stats = active.borrow_mut();
                    stats.submitted_frames += 1;
                    stats
                        .source_to_submit_ms
                        .push(frame.ready.elapsed().as_secs_f64() * 1000.0);
                } else {
                    *pending.borrow_mut() = Some(frame);
                    ui.window().request_redraw();
                }
            }
            {
                let stats = active.borrow();
                ui.set_status(
                    format!(
                        "Frames submitted: {} · input messages: {} · text edits: {}",
                        stats.submitted_frames, stats.input_messages, stats.text_edits
                    )
                    .into(),
                );
            }
            if started.elapsed() >= duration {
                if let Some(path) = &snapshot {
                    match ui
                        .window()
                        .take_snapshot()
                        .map_err(|e| e.to_string())
                        .and_then(|pixels| {
                            image::save_buffer(
                                path,
                                pixels.as_bytes(),
                                pixels.width(),
                                pixels.height(),
                                image::ColorType::Rgba8,
                            )
                            .map_err(|e| e.to_string())
                        }) {
                        Ok(()) => active.borrow_mut().snapshot_saved = true,
                        Err(error) => {
                            active.borrow_mut().error = Some(format!("snapshot: {error}"))
                        }
                    }
                }
                if let Err(error) = slint::quit_event_loop() {
                    active.borrow_mut().error = Some(error.to_string());
                }
            }
        },
    );
    let result = ui.run();
    timer.stop();
    drop(source);
    if let Some(manager) = manager {
        runtime.block_on(manager.shutdown())?;
    }
    let elapsed = started.elapsed().as_secs_f64();
    let mut stats = measurements.borrow_mut();
    if let Err(error) = &result {
        stats.error = Some(error.to_string());
    }
    let source_stats = mailbox.lock().unwrap();
    if let Some(error) = &source_stats.error {
        stats.error = Some(error.clone());
    }
    stats.source_to_submit_ms.sort_by(f64::total_cmp);
    let percentile = |p: f64| -> Option<f64> {
        stats
            .source_to_submit_ms
            .get(((stats.source_to_submit_ms.len().saturating_sub(1)) as f64 * p) as usize)
            .copied()
    };
    let report = serde_json::json!({
        "schema": 1, "framework": "slint", "slint": "1.18.1", "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "renderer": options.renderer, "source": if options.ivf.is_some() { "xscc-vp9-software-decoder" } else { "synthetic-cpu-rgba" },
        "requested_size": [options.width, options.height], "requested_fps": options.fps,
        "elapsed_seconds": elapsed, "source_frames": source_stats.produced,
        "mailbox_replacements": source_stats.replaced,
        "submitted_frames": stats.submitted_frames, "submitted_fps": stats.submitted_frames as f64 / elapsed,
        "render_callbacks": stats.rendered_callbacks,
        "source_ready_to_submit_ms": {"p50": percentile(0.5), "p95": percentile(0.95), "p99": percentile(0.99)},
        "rgba_texture_import": stats.rgba_texture_import,
        "planar_texture_import_rejected": stats.planar_texture_import_rejected,
        "adapter": stats.adapter,
        "cpu_adapters_allowed": std::env::var_os("SLINT_WGPU_CPU").is_some(),
        "input_messages": stats.input_messages, "key_down": stats.key_down, "key_up": stats.key_up,
        "pointer_events": stats.pointer_events, "text_edits": stats.text_edits,
        "secondary_windows": stats.secondary_windows, "snapshot_saved": stats.snapshot_saved,
        "error": stats.error, "replacement_ready": false,
        "unverified": ["physical GPU decoding and native texture interop", "display scanout latency", "authenticated remote session and input injection", "five-platform physical-device acceptance"]
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
    result?;
    if stats.error.is_some() || stats.submitted_frames < 2 {
        return Err(io::Error::other("Slint validation failed; inspect the JSON report").into());
    }
    Ok(())
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: slint::android::AndroidApp) {
    if let Err(error) = slint::android::init(app)
        .map_err(|e| e.to_string())
        .and_then(|_| run().map_err(|e| e.to_string()))
    {
        eprintln!("Slint validation failed: {error}");
    }
}
