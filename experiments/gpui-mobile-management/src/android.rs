#![cfg(target_os = "android")]

#[cfg(feature = "mobile-session")]
mod display_state;

#[cfg(feature = "mobile-session")]
mod keyboard_action;
#[cfg(feature = "mobile-session")]
mod keyboard_input;
mod management;
mod panel;
#[cfg(feature = "mobile-session")]
mod pointer_input;
#[cfg(feature = "mobile-session")]
#[path = "../../gpui-validation/src/presentation.rs"]
mod presentation;
#[cfg(feature = "mobile-session")]
mod remote_cursor;
#[cfg(feature = "mobile-session")]
mod remote_image;
#[cfg(feature = "mobile-session")]
mod remote_keyboard;
#[cfg(feature = "mobile-session")]
mod session_chrome;
#[cfg(feature = "mobile-session")]
mod session_input;
#[cfg(feature = "mobile-session")]
mod session_layout;
#[cfg(feature = "mobile-session")]
mod session_view;
#[path = "../../gpui-validation/src/text_field.rs"]
mod text_field;
mod theme;
#[cfg(feature = "mobile-session")]
mod touch_input;

use gpui::{App, AppContext, WindowOptions};

#[unsafe(no_mangle)]
fn android_main(app: gpui::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_tag("XSCC-GPUI")
            .with_max_level(log::LevelFilter::Info),
    );
    if let Err(error) = run(app) {
        log::error!("Cannot start the management preview: {error}");
    }
}

fn run(app: gpui::AndroidApp) -> Result<(), Box<dyn std::error::Error>> {
    // Android supplies the application sandbox; never infer it from HOME or cwd.
    let directory = app
        .internal_data_path()
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "Android private application directory unavailable".to_owned());
    #[cfg(feature = "mobile-session")]
    if let Err(error) =
        unsafe { librustdesk::gpui_session::initialize_android_decoders(app.vm_as_ptr()) }
    {
        log::warn!(
            "Android hardware decoder discovery failed; continuing with software codecs: {error:#}"
        );
    }
    // Share APK assets with the native EditText instead of embedding a second
    // copy of these full CJK fonts in the native library.
    let assets = app.asset_manager();
    let mut fonts = Vec::with_capacity(2);
    for name in [c"MapleMonoFull-Regular.ttf", c"MapleMonoFull-Bold.ttf"] {
        let mut asset = assets
            .open(name)
            .ok_or_else(|| format!("Bundled Maple font is missing: {}", name.to_string_lossy()))?;
        fonts.push(std::borrow::Cow::Owned(asset.buffer()?.to_vec()));
    }
    let application = gpui::android(app)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let handle = runtime.handle().clone();
    application.run(move |cx: &mut App| {
        if let Err(error) = cx.text_system().add_fonts(fonts) {
            log::error!("Could not load the bundled Maple fonts: {error}");
            cx.quit();
            return;
        }
        text_field::init(cx);
        if let Err(error) = cx.open_window(WindowOptions::default(), |window, cx| {
            cx.new(|cx| panel::Panel::new(directory, handle, window, cx))
        }) {
            log::error!("Could not open the management preview: {error}");
            cx.quit();
        }
    });
    runtime.shutdown_timeout(std::time::Duration::from_secs(3));
    Ok(())
}
