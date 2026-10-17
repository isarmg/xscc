mod management;
mod panel;
#[path = "../../gpui-validation/src/text_field.rs"]
mod text_field;
#[path = "../../gpui-validation/src/theme.rs"]
mod theme;

use gpui::{App, AppContext, WindowOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Runtime ownership belongs to the executable, never to a request or the manager library.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let directory = management::application_directory();
    let handle = runtime.handle().clone();
    gpui::application().run(move |cx: &mut App| {
        text_field::init(cx);
        if let Err(error) = cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|cx| panel::Panel::new(directory, handle, cx))
        }) {
            eprintln!("Could not open the management preview: {error}");
            cx.quit();
        }
    });
    runtime.shutdown_timeout(std::time::Duration::from_secs(3));
    Ok(())
}
