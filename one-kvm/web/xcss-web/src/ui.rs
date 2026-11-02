use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use xcss_web::{html as h, prelude::*};

pub fn error(value: RwSignal<String>) -> impl IntoView {
    h::p()
        .class("error")
        .attr("role", "alert")
        .hidden(move || value.with(String::is_empty))
        .child(move || value.get())
}

pub fn listen(name: &'static str, callback: impl FnMut(web_sys::Event) + 'static) {
    let callback =
        send_wrapper::SendWrapper::new(Closure::<dyn FnMut(web_sys::Event)>::new(callback));
    if let Some(window) = web_sys::window() {
        let _ = window.add_event_listener_with_callback(name, callback.as_ref().unchecked_ref());
    }
    on_cleanup(move || {
        if let Some(window) = web_sys::window() {
            let _ =
                window.remove_event_listener_with_callback(name, callback.as_ref().unchecked_ref());
        }
    });
}

pub fn js_error(value: JsValue) -> String {
    js_sys::Reflect::get(&value, &JsValue::from_str("message"))
        .ok()
        .and_then(|value| value.as_string())
        .or_else(|| value.as_string())
        .unwrap_or_else(|| "浏览器设备操作失败。".into())
}

pub fn action(
    label: &'static str,
    path: &'static str,
    method: &'static str,
    body: Option<serde_json::Value>,
    error: RwSignal<String>,
) -> impl IntoView {
    let body = StoredValue::new(body);
    let busy = RwSignal::new(false);
    h::button()
        .disabled(move || busy.get())
        .on(xcss_web::ev::click, move |_| {
            busy.set(true);
            error.set(String::new());
            xcss_web::task::spawn_local(async move {
                if let Err(value) = crate::api::mutate(path, method, body.get_value()).await {
                    error.set(value.to_string());
                }
                busy.set(false);
            });
        })
        .child(label)
}

pub fn input(label: &'static str, value: RwSignal<String>) -> impl IntoView {
    h::label().class("field").child((
        h::span().child(label),
        h::input()
            .prop("value", move || value.get())
            .on(xcss_web::ev::input, move |event| {
                value.set(event_target_value(&event))
            }),
    ))
}

pub fn poll<T: serde::de::DeserializeOwned + Clone + Send + Sync + 'static>(
    path: &'static str,
    interval: u32,
) -> (RwSignal<Option<T>>, RwSignal<String>) {
    let data = RwSignal::new(None);
    let error = RwSignal::new(String::new());
    let alive = std::rc::Rc::new(std::cell::Cell::new(true));
    let cleanup = send_wrapper::SendWrapper::new(alive.clone());
    on_cleanup(move || cleanup.set(false));
    xcss_web::task::spawn_local(async move {
        while alive.get() {
            let response = crate::api::get(path).await;
            if !alive.get() {
                break;
            }
            match response {
                Ok(value) => {
                    data.set(Some(value));
                    error.set(String::new());
                }
                Err(value) => error.set(value.to_string()),
            }
            gloo_timers::future::TimeoutFuture::new(interval).await;
        }
    });
    (data, error)
}
