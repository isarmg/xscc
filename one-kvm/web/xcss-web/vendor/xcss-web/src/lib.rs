//! Shared browser-only support. The Linux server crate remains independent.
pub mod api;
pub mod components;
pub mod contracts;
pub mod i18n;
pub mod server_corners;
pub mod theme;

extern crate self as xcss_web;

pub use wasm_bindgen;
pub use wasm_bindgen_futures;
pub use web_sys;
pub use xcss_web_core::*;

use wasm_bindgen::{JsCast, JsValue};

/// Mount only after fonts have decoded, preserving the existing startup contract.
pub fn mount<F, V>(root_id: &str, app: F)
where
    F: FnOnce() -> V + 'static,
    V: IntoView + 'static,
{
    mount_application(root_id, app, false);
}

/// Mount a Server view with the shared Rust corner controller.
pub fn mount_server<F, V>(root_id: &str, app: F)
where
    F: FnOnce() -> V + 'static,
    V: IntoView + 'static,
{
    mount_application(root_id, app, true);
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    // Keep the controller alive for this document; its pagehide listener releases
    // observers, styles and URLs before an uncached page leaves.
    static SERVER_CORNERS: std::cell::RefCell<Option<server_corners::ServerCornersController>> = const { std::cell::RefCell::new(None) };
}

fn mount_application<F, V>(root_id: &str, app: F, server_corners: bool)
where
    F: FnOnce() -> V + 'static,
    V: IntoView + 'static,
{
    console_error_panic_hook::set_once();
    i18n::initialize();
    theme::initialize();
    let root_id = root_id.to_owned();
    wasm_bindgen_futures::spawn_local(async move {
        let result: Result<(), JsValue> = async {
            let document = web_sys::window()
                .and_then(|window| window.document())
                .ok_or_else(|| JsValue::from_str("Browser document is unavailable"))?;
            let fonts = document.fonts();
            // Loading both weights up front also covers glyphs absent from the first view.
            let regular = fonts.load("16px \"Sarmg Maple\"");
            let bold = fonts.load("bold 16px \"Sarmg Maple\"");
            let (regular, bold) = futures::try_join!(
                wasm_bindgen_futures::JsFuture::from(regular),
                wasm_bindgen_futures::JsFuture::from(bold)
            )?;
            if js_sys::Array::from(&regular).length() == 0
                || js_sys::Array::from(&bold).length() == 0
            {
                return Err(JsValue::from_str("Application font styles are missing"));
            }
            wasm_bindgen_futures::JsFuture::from(fonts.ready()?).await?;
            let root = document
                .get_element_by_id(&root_id)
                .ok_or_else(|| JsValue::from_str("Application root is missing"))?
                .dyn_into::<web_sys::HtmlElement>()?;
            let html = document
                .document_element()
                .ok_or_else(|| JsValue::from_str("Browser document root is missing"))?;
            // A timed-out startup cannot later replace its visible failure page.
            if html.get_attribute("data-xcss-startup").as_deref() == Some("failed") {
                return Err(JsValue::from_str("Application startup has already failed"));
            }
            xcss_web::mount::mount_to(root, app).forget();
            html.set_attribute("data-xcss-appearance", "content-blocks")?;
            html.set_attribute("data-xcss-selection", "underline")?;
            #[cfg(target_arch = "wasm32")]
            if server_corners {
                let controller = crate::server_corners::start(&document)?;
                SERVER_CORNERS.with(|value| *value.borrow_mut() = Some(controller));
            }
            #[cfg(not(target_arch = "wasm32"))]
            let _ = server_corners;
            html.set_attribute("data-xcss-fonts", "ready")?;
            html.set_attribute("data-xcss-runtime", "xcss-web")?;
            document.dispatch_event(&web_sys::Event::new("xcss-startup-ready")?)?;
            Ok(())
        }
        .await;
        if let Err(error) = result {
            web_sys::console::error_2(&JsValue::from_str("Application startup failed"), &error);
            if let Some(document) = web_sys::window().and_then(|window| window.document())
                && let Ok(event) = web_sys::Event::new("xcss-startup-failed")
            {
                let _ = document.dispatch_event(&event);
            }
        }
    });
}
