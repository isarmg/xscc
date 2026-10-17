#[cfg(any(
    test,
    all(
        any(target_os = "ios", target_os = "android"),
        feature = "mobile-session"
    )
))]
mod display_state;
#[cfg(any(
    test,
    all(
        any(target_os = "ios", target_os = "android"),
        feature = "mobile-session"
    )
))]
mod keyboard_action;
#[cfg(any(
    test,
    all(
        any(target_os = "ios", target_os = "android"),
        feature = "mobile-session"
    )
))]
mod keyboard_input;
mod management;
mod panel;
#[cfg(all(
    any(target_os = "ios", target_os = "android"),
    feature = "mobile-session"
))]
#[path = "../../gpui-validation/src/presentation.rs"]
mod presentation;
#[cfg(all(
    any(target_os = "ios", target_os = "android"),
    feature = "mobile-session"
))]
mod remote_keyboard;
#[cfg(all(
    any(target_os = "ios", target_os = "android"),
    feature = "mobile-session"
))]
mod session_view;
#[path = "../../gpui-validation/src/text_field.rs"]
mod text_field;
#[path = "../../gpui-validation/src/theme.rs"]
mod theme;
#[cfg(any(
    test,
    all(
        any(target_os = "ios", target_os = "android"),
        feature = "mobile-session"
    )
))]
mod touch_input;

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
        eprintln!("XSCC_GPUI_LAUNCH_CALLBACK_REACHED");
        text_field::init(cx);
        if let Err(error) = cx.open_window(WindowOptions::default(), |window, cx| {
            cx.new(|cx| panel::Panel::new(directory, handle, window, cx))
        }) {
            eprintln!("Could not open the management preview: {error}");
            cx.quit();
        } else {
            eprintln!("XSCC_GPUI_WINDOW_CREATED");
        }
    });
    runtime.shutdown_timeout(std::time::Duration::from_secs(3));
    Ok(())
}
