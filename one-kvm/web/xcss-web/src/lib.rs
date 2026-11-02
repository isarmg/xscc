mod api;
mod computer_use;
mod console;
mod hardware;
mod hid;
mod models;
mod operations;
mod settings;
mod setup;
mod storage;
mod ui;

use models::{ManagementStatus, Session};
use serde_json::json;
use wasm_bindgen::prelude::*;
use xcss_web::{html as h, prelude::*};

#[wasm_bindgen(js_namespace = ["globalThis", "kvmBrowser"])]
extern "C" {
    #[wasm_bindgen(js_name = toggleTheme)]
    fn toggle_theme();
}

#[wasm_bindgen(start)]
pub fn start() {
    xcss_web::mount_server("root", app);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Console,
    Storage,
    Settings,
    Operations,
}

fn current_page() -> Page {
    let Some(window) = web_sys::window() else {
        return Page::Console;
    };
    let hash = window.location().hash().unwrap_or_default();
    let path = if hash.is_empty() {
        let pathname = window.location().pathname().unwrap_or_default();
        if let Some(instance) = pathname.strip_prefix("/instances/") {
            instance
                .split_once('/')
                .map(|(_, path)| format!("/{path}"))
                .unwrap_or_else(|| "/".into())
        } else {
            pathname
        }
    } else {
        hash
    };
    match path.as_str() {
        "#media" | "/media" => Page::Storage,
        "#settings" | "/settings" => Page::Settings,
        "#system" | "/system" => Page::Operations,
        _ => Page::Console,
    }
}

fn app() -> impl IntoView {
    let session = RwSignal::new(None::<Session>);
    let status = RwSignal::new(None::<ManagementStatus>);
    let restoring = RwSignal::new(true);
    let initialized = RwSignal::new(None::<bool>);
    let platform = RwSignal::new(models::Platform::default());
    let page = RwSignal::new(current_page());
    let error = RwSignal::new(String::new());
    ui::listen("kvm-unauthorized", move |_| {
        let was_authenticated = session.with_untracked(Option::is_some);
        session.set(None);
        if was_authenticated {
            error.set("授权已失效，请确认 XSCS 授权关系后重新登录。".into());
        }
    });
    ui::listen("hashchange", move |_| page.set(current_page()));
    xcss_web::task::spawn_local(async move {
        if let Ok(value) = api::get::<ManagementStatus>("/api/management/status").await {
            status.set(Some(value));
        }
        if let Ok(value) = api::get::<Session>("/api/auth/check").await
            && value.authenticated
        {
            api::csrf(&value.csrf_token);
            session.set(Some(value));
        }
        restoring.set(false);
    });
    Effect::new(move |_| {
        if session.with(Option::is_some) {
            xcss_web::task::spawn_local(async move {
                match api::get::<models::SetupStatus>("/api/setup").await {
                    Ok(value) => {
                        platform.set(value.platform);
                        initialized.set(Some(value.initialized && !value.needs_setup));
                    }
                    Err(value) => error.set(value.to_string()),
                }
            });
        } else {
            initialized.set(None);
        }
    });
    let alive = std::rc::Rc::new(std::cell::Cell::new(true));
    let cleanup = send_wrapper::SendWrapper::new(alive.clone());
    on_cleanup(move || cleanup.set(false));
    xcss_web::task::spawn_local(async move {
        while alive.get() {
            gloo_timers::future::TimeoutFuture::new(5000).await;
            if session.with_untracked(Option::is_none) {
                continue;
            }
            match api::get::<Session>("/api/auth/check").await {
                Ok(value) if value.authenticated => {}
                Ok(_) => {
                    api::csrf("");
                    session.set(None);
                }
                Err(value) => {
                    api::csrf("");
                    session.set(None);
                    error.set(value.to_string());
                }
            }
        }
    });
    h::main().class("shell").child((
        h::header().child((
            h::div().class("brand").child((
                "ONE-KVM",
                h::small().child(format!("实例 {}", api::instance_id())),
            )),
            h::nav()
                .hidden(move || session.with(Option::is_none))
                .child(
                    [
                        ("#console", "控制台"),
                        ("#media", "虚拟媒体"),
                        ("#settings", "设备设置"),
                        ("#system", "运行状态"),
                    ]
                    .into_iter()
                    .map(|(href, label)| {
                        h::a()
                            .href(href)
                            .hidden(move || {
                                href == "#media"
                                    && platform.with(|platform| !platform.supports_media())
                            })
                            .child(label)
                    })
                    .collect::<Vec<_>>(),
                ),
            h::div().class("actions").child((
                h::a().class("button").href("/").child("切换实例"),
                h::button()
                    .on(xcss_web::ev::click, move |_| toggle_theme())
                    .child("明暗外观"),
                h::button()
                    .hidden(move || session.with(Option::is_none))
                    .on(xcss_web::ev::click, move |_| {
                        xcss_web::task::spawn_local(async move {
                            match api::mutate("/api/auth/logout", "POST", Some(json!({}))).await {
                                Ok(()) => {
                                    api::csrf("");
                                    session.set(None);
                                }
                                Err(value) => error.set(value.to_string()),
                            }
                        });
                    })
                    .child("退出授权会话"),
            )),
        )),
        ui::error(error),
        h::div().child(move || {
            if restoring.get() {
                return View::new(h::p().class("hint").child("正在恢复管理会话…")).into_any();
            }
            if session.with(Option::is_none) {
                return View::new(login(session, status)).into_any();
            }
            if initialized.get() == Some(false) {
                return View::new(setup::page(initialized)).into_any();
            }
            if initialized.get().is_none() {
                return View::new(h::p().class("hint").child("正在读取硬件初始化状态…")).into_any();
            }
            match page.get() {
                Page::Console => View::new(console::page(platform)).into_any(),
                Page::Storage if platform.with(|platform| platform.supports_media()) => {
                    View::new(storage::page()).into_any()
                }
                Page::Storage => {
                    View::new(h::p().class("hint").child("当前平台不支持 USB 虚拟媒体。"))
                        .into_any()
                }
                Page::Settings => View::new(settings::page()).into_any(),
                Page::Operations => View::new(operations::page(session)).into_any(),
            }
        }),
    ))
}

fn login(
    session: RwSignal<Option<Session>>,
    status: RwSignal<Option<ManagementStatus>>,
) -> impl IntoView {
    let code = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    h::section().class("card login").child((
        h::p()
            .class("eyebrow")
            .child("BOUND DEVICE · AUTHORIZED CONTROLLER"),
        h::h1().child("连接你的 KVM"),
        h::p()
            .class("hint")
            .child("输入 XSCS 中控制端实例的授权码。该实例需要获得当前 KVM 被控实例的控制权限。"),
        h::p().class("hint").child(move || {
            status
                .get()
                .map(|value| {
                    if value.configured {
                        format!("设备已绑定 · {}", value.phase)
                    } else {
                        "设备尚未绑定，请由本机管理员使用管理命令绑定 XSCS 被控实例。".into()
                    }
                })
                .unwrap_or_else(|| "正在读取设备绑定状态…".into())
        }),
        ui::error(error),
        h::form()
            .on(xcss_web::ev::submit, move |event| {
                event.prevent_default();
                if busy.get_untracked() {
                    return;
                }
                let authorization_code = code.get_untracked().trim().to_owned();
                code.set(String::new());
                busy.set(true);
                error.set(String::new());
                xcss_web::task::spawn_local(async move {
                    match api::request::<Session>(
                        "/api/management/login",
                        "POST",
                        Some(json!({"authorization_code":authorization_code})),
                    )
                    .await
                    {
                        Ok(value) if value.authenticated => {
                            api::csrf(&value.csrf_token);
                            session.set(Some(value));
                        }
                        Ok(_) => error.set("控制端未获得授权。".into()),
                        Err(value) => error.set(value.to_string()),
                    }
                    busy.set(false);
                });
            })
            .child((
                h::label().class("field").child((
                    h::span().child("控制端实例授权码"),
                    h::input()
                        .r#type("password")
                        .autocomplete("off")
                        .required(true)
                        .maxlength(128)
                        .prop("value", move || code.get())
                        .on(xcss_web::ev::input, move |event| {
                            code.set(event_target_value(&event))
                        }),
                )),
                h::p()
                    .class("hint")
                    .child("授权码只用于建立当前浏览器会话，不存入浏览器本地存储。"),
                h::button()
                    .class("primary")
                    .r#type("submit")
                    .disabled(move || {
                        busy.get()
                            || status
                                .with(|value| value.as_ref().is_none_or(|value| !value.configured))
                    })
                    .child("验证授权并连接"),
            )),
    ))
}
