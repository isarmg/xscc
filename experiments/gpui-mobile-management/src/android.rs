#![cfg(target_os = "android")]

mod management;
mod panel;
#[path = "../../gpui-validation/src/text_field.rs"]
mod text_field;
#[path = "../../gpui-validation/src/theme.rs"]
mod theme;

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
    let application = gpui::android(app)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let handle = runtime.handle().clone();
    application.run(move |cx: &mut App| {
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
