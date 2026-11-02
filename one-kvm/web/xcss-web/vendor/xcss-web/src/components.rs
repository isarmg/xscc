use crate::{
    api::{self, AuthPhase},
    i18n::{switch_language, t},
    theme,
};
use gloo_events::EventListener;
use gloo_timers::callback::Timeout;
use serde_json::json;
use wasm_bindgen::JsCast;
use xcss_web::component::{component_props_builder, component_view};
use xcss_web::{portal::Portal, prelude::*};

#[component]
pub fn LoadingState() -> impl IntoView {
    View::new(
        xcss_web::html::div()
            .child(
                #[allow(unused_braces)]
                {
                    IntoRender::into_render({ t("正在加载…", "Loading…") })
                },
            )
            .class("xcss-loading")
            .role("status")
            .aria_live("polite"),
    )
}
#[component]
pub fn EmptyState(message: String) -> impl IntoView {
    View::new(
        xcss_web::html::div()
            .child(
                #[allow(unused_braces)]
                {
                    IntoRender::into_render(message)
                },
            )
            .class("xcss-empty"),
    )
}
#[component]
pub fn ErrorState(
    message: String,
    #[prop(optional_no_strip)] request_id: Option<String>,
) -> impl IntoView {
    View::new(
        xcss_web::html::div()
            .child((
                IntoRender::into_render(message),
                IntoRender::into_render({
                    request_id
                        .filter(|v| crate::contracts::safe_request_id(v))
                        .map(|id| {
                            View::new(
                                xcss_web::html::p()
                                    .child((
                                        IntoRender::into_render({
                                            t("请求标识：", "Request ID: ")
                                        }),
                                        xcss_web::html::code().child(
                                            #[allow(unused_braces)]
                                            {
                                                IntoRender::into_render({ id })
                                            },
                                        ),
                                    ))
                                    .class("xcss-request-id"),
                            )
                        })
                }),
            ))
            .class("xcss-error")
            .role("alert"),
    )
}

/// Native modal semantics retain focus containment and background inertness.
#[component]
pub fn Dialog(title: String, close: Callback<()>, children: ChildrenFn) -> impl IntoView {
    let dialog = NodeRef::<xcss_web::html::Dialog>::new();
    let previous = send_wrapper::SendWrapper::new(
        web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.active_element())
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok()),
    );
    Effect::new(move |_| {
        if let Some(dialog) = dialog.get() {
            let _ = dialog.show_modal();
            if let Ok(Some(element)) = dialog.query_selector("[data-xcss-initial-focus]")
                && let Ok(element) = element.dyn_into::<web_sys::HtmlElement>()
            {
                let _ = element.focus();
            }
        }
    });
    on_cleanup(move || {
        if let Some(dialog) = dialog.get_untracked() {
            dialog.close();
        }
        if let Some(previous) = previous.as_ref()
            && previous.is_connected()
        {
            let _ = previous.focus();
        }
    });
    View::new(
        xcss_web::html::dialog()
            .child((
                xcss_web::html::div()
                    .child((
                        xcss_web::html::h2().child(
                            #[allow(unused_braces)]
                            {
                                IntoRender::into_render({ title.clone() })
                            },
                        ),
                        xcss_web::html::button()
                            .child(
                                #[allow(unused_braces)]
                                {
                                    "×"
                                },
                            )
                            .class("xcss-button")
                            .r#type("button")
                            .aria_label(
                                ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(t(
                                    "关闭对话框",
                                    "Close dialog",
                                )),
                            )
                            .on(xcss_web::ev::click, move |_| close.run(())),
                    ))
                    .class("xcss-dialog-heading"),
                IntoRender::into_render(children()),
            ))
            .class("xcss-dialog")
            .node_ref(dialog)
            .aria_label(
                ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(title.clone()),
            )
            .tabindex("-1")
            .on(
                xcss_web::ev::Custom::new("cancel"),
                move |event: web_sys::Event| {
                    event.prevent_default();
                    close.run(());
                },
            )
            .on(xcss_web::ev::keydown, move |event| {
                if event.key() != "Tab" {
                    return;
                }
                let Some(dialog) = dialog.get_untracked() else {
                    return;
                };
                if dialog
                    .query_selector("video[controls],audio[controls]")
                    .ok()
                    .flatten()
                    .is_some()
                {
                    return;
                }
                let Ok(nodes) =
                    dialog.query_selector_all("button,input,select,textarea,a[href],[tabindex]")
                else {
                    return;
                };
                let focusable: Vec<_> = (0..nodes.length())
                    .filter_map(|i| nodes.item(i))
                    .filter_map(|n| n.dyn_into::<web_sys::HtmlElement>().ok())
                    .filter(|e| {
                        e.tab_index() >= 0
                            && !e.matches(":disabled,[hidden]").unwrap_or(true)
                            && e.get_client_rects().length() > 0
                    })
                    .collect();
                let active = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.active_element());
                if let (Some(first), Some(last)) = (focusable.first(), focusable.last()) {
                    if event.shift_key()
                        && (active.as_ref() == Some(first.as_ref())
                            || active.as_ref() == Some(dialog.as_ref()))
                    {
                        event.prevent_default();
                        let _ = last.focus();
                    } else if !event.shift_key() && active.as_ref() == Some(last.as_ref()) {
                        event.prevent_default();
                        let _ = first.focus();
                    }
                } else {
                    event.prevent_default();
                    let _ = dialog.focus();
                }
            }),
    )
}

#[derive(Clone)]
struct ToastRecord {
    id: u64,
    message: String,
}
#[derive(Clone, Copy)]
struct Toasts {
    records: RwSignal<Vec<ToastRecord>>,
    sequence: RwSignal<u64>,
}
pub fn notify(message: String) {
    // Async completions have no reactive Owner. Dispatch on the current
    // document rather than trying to resolve a context after an await.
    let Some(window) = web_sys::window() else {
        return;
    };
    let init = web_sys::CustomEventInit::new();
    init.set_detail(&wasm_bindgen::JsValue::from_str(&message));
    if let Ok(event) = web_sys::CustomEvent::new_with_event_init_dict("xcss-notify", &init) {
        let _ = window.dispatch_event(&event);
    }
}
fn push_toast(toasts: Toasts, message: String) {
    toasts.sequence.update(|n| *n += 1);
    let id = toasts.sequence.get_untracked();
    toasts.records.update(|records| {
        if records.len() >= 5 {
            records.remove(0);
        }
        records.push(ToastRecord {
            id,
            message: message.chars().take(512).collect(),
        });
    });
    Timeout::new(5_000, move || {
        let _ = toasts
            .records
            .try_update(|records| records.retain(|r| r.id != id));
    })
    .forget();
}

#[derive(Clone, Copy)]
struct HeaderTargets {
    actions: NodeRef<xcss_web::html::Div>,
    navigation: NodeRef<xcss_web::html::Div>,
}

#[component]
pub fn HeaderActions(children: ChildrenFn) -> impl IntoView {
    let target = use_context::<HeaderTargets>().map(|targets| targets.actions);
    move || {
        let children = children.clone();
        target.and_then(|r| r.get()).map(|element| {
            View::new({
                #[allow(unreachable_code)]
                #[allow(unused_mut)]
                #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
                component_view(
                    #[allow(clippy::needless_borrows_for_generic_args)]
                    &Portal,
                    {
                        let mut props = component_props_builder(&Portal)
                            .mount(
                                #[allow(unused_braces)]
                                {
                                    { element.unchecked_into::<web_sys::Element>() }
                                },
                            )
                            .children({
                                ::xcss_web::children::ToChildren::to_children(move || {
                                    IntoRender::into_render(children())
                                })
                            })
                            .build();
                        props
                    },
                )
            })
        })
    }
}
#[component]
pub fn HeaderNavigation(
    children: ChildrenFn,
    #[prop(default=t("实例工作区", "Instance workspace"))] label: String,
) -> impl IntoView {
    let label = StoredValue::new(label);
    let target = use_context::<HeaderTargets>().map(|targets| targets.navigation);
    move || {
        let children = children.clone();
        target.and_then(|r| r.get()).map(|element| {
            View::new({
                #[allow(unreachable_code)]
                #[allow(unused_mut)]
                #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
                component_view(
                    #[allow(clippy::needless_borrows_for_generic_args)]
                    &Portal,
                    {
                        let mut props = component_props_builder(&Portal)
                            .mount(
                                #[allow(unused_braces)]
                                {
                                    { element.unchecked_into::<web_sys::Element>() }
                                },
                            )
                            .children({
                                ::xcss_web::children::ToChildren::to_children(move || {
                                    xcss_web::html::nav()
                            .child(
                                #[allow(unused_braces)]
                                {
                                    IntoRender::into_render(children())
                                },
                            )
                            .class("xcss-header-navigation")
                            .aria_label(
                                ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(
                                    label.get_value(),
                                ),
                            )
                                })
                            })
                            .build();
                        props
                    },
                )
            })
        })
    }
}

#[component]
pub fn ThemeToggle() -> impl IntoView {
    let current = RwSignal::new(theme::current());
    if let Some(window) = web_sys::window() {
        let listener = EventListener::new(&window, "xcss-theme-change", move |_| {
            let _ = current.try_set(theme::current());
        });
        let listener = send_wrapper::SendWrapper::new(listener);
        on_cleanup(move || drop(listener));
    }
    View::new(
    xcss_web::html::button()
        .child(
            #[allow(unused_braces)]
            {
                xcss_web::svg::svg()
                    .child(
                        #[allow(unused_braces)]
                        {
                            xcss_web::svg::path()
                                .attr(
                                    "d",
                                    ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(move ||
                                    {
                                        if current.get() == "light" {
                                            "M20.8 13A9 9 0 0 1 11 3.2 9 9 0 1 0 20.8 13Z"
                                        } else {
                                            "M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8M12 2v2m0 16v2M2 12h2m16 0h2M5 5l1.4 1.4m11.2 11.2L19 19M5 19l1.4-1.4M17.6 6.4 19 5"
                                        }
                                    }),
                                )
                        },
                    )
                    .attr("aria-hidden", "true")
                    .attr("width", "1em")
                    .attr("height", "1em")
                    .attr("viewBox", "0 0 24 24")
                    .attr("fill", "none")
                    .attr("stroke", "currentColor")
                    .attr("stroke-width", "1.7")
            },
        )
        .class("xcss-button")
        .r#type("button")
        .title(
            ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(move || {
                if current.get() == "light" {
                    t("切换到深色模式", "Switch to dark mode")
                } else {
                    t("切换到浅色模式", "Switch to light mode")
                }
            }),
        )
        .aria_label(
            ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(move || {
                if current.get() == "light" {
                    t("切换到深色模式", "Switch to dark mode")
                } else {
                    t("切换到浅色模式", "Switch to light mode")
                }
            }),
        )
        .on(xcss_web::ev::click, move |_| theme::toggle()),
)
}

#[component]
pub fn LanguageToggle() -> impl IntoView {
    let label = t("切换为英文", "Switch to Chinese");
    View::new(
    xcss_web::html::button()
        .child(
            #[allow(unused_braces)]
            {
                xcss_web::svg::svg()
                    .child(
                        #[allow(unused_braces)]
                        {
                            xcss_web::svg::path()
                                .attr(
                                    "d",
                                    "M3 5h12M9 3v2M6 5c0 5 4 9 8 11M13 5c0 5-4 9-9 12m10 4 4-10 4 10m-6.5-4h5",
                                )
                        },
                    )
                    .attr("aria-hidden", "true")
                    .attr("width", "1em")
                    .attr("height", "1em")
                    .attr("viewBox", "0 0 24 24")
                    .attr("fill", "none")
                    .attr("stroke", "currentColor")
                    .attr("stroke-width", "1.7")
            },
        )
        .class("xcss-button")
        .r#type("button")
        .aria_label(
            ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(
                label.clone(),
            ),
        )
        .title(::xcss_web::prelude::IntoAttributeValue::into_attribute_value(label))
        .on(xcss_web::ev::click, move |_| switch_language()),
)
}

#[component]
pub fn AdminShell(
    product: &'static str,
    #[prop(optional)] landing: &'static str,
    children: ChildrenFn,
) -> impl IntoView {
    let auth = RwSignal::new(api::snapshot());
    let pending_logout = RwSignal::new(false);
    let toasts = Toasts {
        records: RwSignal::new(Vec::new()),
        sequence: RwSignal::new(0),
    };
    provide_context(toasts);
    let targets = HeaderTargets {
        actions: NodeRef::new(),
        navigation: NodeRef::new(),
    };
    provide_context(targets);
    let notifications = send_wrapper::SendWrapper::new(EventListener::new(
        &web_sys::window().unwrap(),
        "xcss-notify",
        move |event| {
            if let Some(message) = event
                .dyn_ref::<web_sys::CustomEvent>()
                .and_then(|event| event.detail().as_string())
            {
                push_toast(toasts, message);
            }
        },
    ));
    on_cleanup(move || drop(notifications));
    let listener = send_wrapper::SendWrapper::new(EventListener::new(
        &web_sys::window().unwrap(),
        api::AUTH_CHANGE_EVENT,
        move |_| {
            let next = api::snapshot();
            if next.phase != AuthPhase::Authenticated {
                toasts.records.set(Vec::new());
            }
            // Retain mounted forms and their pending/failure state when only
            // the private authentication generation changed.
            if auth.get_untracked() != next {
                auth.set(next);
            }
        },
    ));
    on_cleanup(move || drop(listener));
    wasm_bindgen_futures::spawn_local(async {
        let _ = api::restore().await;
    });
    IntoRender::into_render({
        move || {
            let current = auth.get();
            if current.phase == AuthPhase::Loading {
                return View::new(
                    xcss_web::html::div()
                        .class("xcss-application-boot")
                        .role("status")
                        .aria_busy("true")
                        .aria_label(
                            ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(t(
                                "正在准备应用…",
                                "Preparing application…",
                            )),
                        ),
                )
                .into_any();
            }
            if current.phase != AuthPhase::Authenticated {
                return View::new(
    xcss_web::html::div()
        .child((
            xcss_web::html::div()
                .child(
                    #[allow(unused_braces)]
                    {
                        xcss_web::html::div()
                            .child((
                                {
                                    #[allow(unreachable_code)] #[allow(unused_mut)]
                                    #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
                                    component_view(
                                        #[allow(clippy::needless_borrows_for_generic_args)]
                                        &LanguageToggle,
                                        {
                                            let mut props = component_props_builder(
                                                    &LanguageToggle,
                                                )
                                                .build();
                                            props
                                        },
                                    )
                                },
                                {
                                    #[allow(unreachable_code)] #[allow(unused_mut)]
                                    #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
                                    component_view(
                                        #[allow(clippy::needless_borrows_for_generic_args)]
                                        &ThemeToggle,
                                        {
                                            let mut props = component_props_builder(
                                                    &ThemeToggle,
                                                )
                                                .build();
                                            props
                                        },
                                    )
                                },
                            ))
                            .class("xcss-login-controls")
                            .role("group")
                            .aria_label(
                                ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(
                                    t("显示设置", "Display settings"),
                                ),
                            )
                    },
                )
                .class("xcss-auth-language")
                .style(("position", "absolute"))
                .style(("inset-block-start", "1rem"))
                .style(("inset-inline-end", "1rem")),
            xcss_web::html::div()
                .child((
                    IntoRender::into_render({
                        current
                            .account_updated
                            .then(|| {
                                View::new(
    xcss_web::html::p()
        .child(
            #[allow(unused_braces)]
            {
                IntoRender::into_render({
                    t(
                        "账号已更新，请使用新账号信息登录。",
                        "Account updated. Sign in with your updated credentials.",
                    )
                })
            },
        )
        .role("status"),
)
                            })
                    }),
                    IntoRender::into_render({
                        (current.phase == AuthPhase::LogoutUnconfirmed)
                            .then(|| {
                                View::new(
    xcss_web::html::div()
        .child((
            {
                #[allow(unreachable_code)] #[allow(unused_mut)]
                #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
                component_view(
                    #[allow(clippy::needless_borrows_for_generic_args)]
                    &ErrorState,
                    {
                        let mut props = component_props_builder(
                                &ErrorState,
                            )
                            .message(
                                #[allow(unused_braces)]
                                {
                                    t(
                                        "本地已退出，但无法确认服务器会话已注销。请重试退出以确认注销。",
                                        "Signed out locally, but the server session could not be confirmed as revoked. Retry sign out to confirm revocation.",
                                    )
                                },
                            )
                            .request_id(
                                #[allow(unused_braces)]
                                {
                                    current.error.as_ref().and_then(|e| e.request_id.clone())
                                },
                            )
                            .build();
                        props
                    },
                )
            },
            xcss_web::html::button()
                .child(
                    #[allow(unused_braces)]
                    {
                        IntoRender::into_render({
                            t("重试退出", "Retry sign out")
                        })
                    },
                )
                .class("xcss-button")
                .on(
                    xcss_web::ev::click,
                    move |_| {
                        wasm_bindgen_futures::spawn_local(async {
                            let _ = api::logout().await;
                        });
                    },
                ),
        )),
)
                            })
                    }),
                    xcss_web::html::div()
                        .child((
                            xcss_web::html::div()
                                .child(
                                    #[allow(unused_braces)]
                                    {
                                        xcss_web::html::strong()
                                            .child(
                                                #[allow(unused_braces)]
                                                { IntoRender::into_render({ product }) },
                                            )
                                    },
                                )
                                .class("xcss-product-identity"),
                            IntoRender::into_render({
                                if current.phase == AuthPhase::Error {
                                    View::new(
    xcss_web::html::div()
        .child((
            {
                #[allow(unreachable_code)] #[allow(unused_mut)]
                #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
                component_view(
                    #[allow(clippy::needless_borrows_for_generic_args)]
                    &ErrorState,
                    {
                        let mut props = component_props_builder(
                                &ErrorState,
                            )
                            .message(
                                #[allow(unused_braces)]
                                {
                                    t(
                                        "无法恢复管理员会话。",
                                        "Unable to restore administrator session.",
                                    )
                                },
                            )
                            .request_id(
                                #[allow(unused_braces)]
                                { current.error.and_then(|e| e.request_id) },
                            )
                            .build();
                        props
                    },
                )
            },
            xcss_web::html::button()
                .child(
                    #[allow(unused_braces)]
                    {
                        IntoRender::into_render({
                            t("重试", "Retry")
                        })
                    },
                )
                .class("xcss-button")
                .on(
                    xcss_web::ev::click,
                    move |_| {
                        wasm_bindgen_futures::spawn_local(async {
                            let _ = api::restore().await;
                        });
                    },
                ),
        )),
)
                                        .into_any()
                                } else {
                                    View::new({
    #[allow(unreachable_code)] #[allow(unused_mut)] #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
    component_view(
        #[allow(clippy::needless_borrows_for_generic_args)]
        &LoginPage,
        {
            let mut props = component_props_builder(&LoginPage)
                .landing(#[allow(unused_braces)] { landing })
                .build();
            props
        },
    )
})
                                        .into_any()
                                }
                            }),
                        ))
                        .class("xcss-auth-card"),
                ))
                .class("xcss-auth-content"),
        ))
        .class("xcss-auth-shell"),
)
                .into_any();
            }
            let children = children.clone();
            View::new(
    xcss_web::html::div()
        .child((
            xcss_web::html::a()
                .child(
                    #[allow(unused_braces)]
                    {
                        IntoRender::into_render({
                            t("跳至正文", "Skip to content")
                        })
                    },
                )
                .class("xcss-skip-link")
                .href("#xcss-main-content")
                .on(
                    xcss_web::ev::click,
                    move |event| {
                        event.prevent_default();
                        if let Some(main) = web_sys::window()
                            .unwrap()
                            .document()
                            .unwrap()
                            .get_element_by_id("xcss-main-content")
                            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
                        {
                            let _ = main.focus();
                        }
                    },
                ),
            xcss_web::html::header()
                .child((
                    xcss_web::html::div()
                        .child(
                            #[allow(unused_braces)]
                            {
                                xcss_web::html::div()
                                    .child((
                                        xcss_web::html::div()
                                            .child(
                                                #[allow(unused_braces)]
                                                {
                                                    xcss_web::html::strong()
                                                        .child(
                                                            #[allow(unused_braces)]
                                                            { IntoRender::into_render({ product }) },
                                                        )
                                                },
                                            )
                                            .class("xcss-product-identity"),
                                        xcss_web::html::div()
                                            .node_ref(targets.navigation)
                                            .style(("display", "contents")),
                                    ))
                                    .class("xcss-header-brand-navigation")
                            },
                        )
                        .class("xcss-header-navigation-slot"),
                    xcss_web::html::div()
                        .child((
                            xcss_web::html::div()
                                .node_ref(targets.actions)
                                .style(("display", "contents")),
                            {
                                #[allow(unreachable_code)] #[allow(unused_mut)]
                                #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
                                component_view(
                                    #[allow(clippy::needless_borrows_for_generic_args)]
                                    &LanguageToggle,
                                    {
                                        let mut props = component_props_builder(
                                                &LanguageToggle,
                                            )
                                            .build();
                                        props
                                    },
                                )
                            },
                            {
                                #[allow(unreachable_code)] #[allow(unused_mut)]
                                #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
                                component_view(
                                    #[allow(clippy::needless_borrows_for_generic_args)]
                                    &ThemeToggle,
                                    {
                                        let mut props = component_props_builder(
                                                &ThemeToggle,
                                            )
                                            .build();
                                        props
                                    },
                                )
                            },
                            xcss_web::html::button()
                                .child(
                                    #[allow(unused_braces)]
                                    {
                                        xcss_web::svg::svg()
                                            .child(
                                                #[allow(unused_braces)]
                                                {
                                                    xcss_web::svg::path()
                                                        .attr("d", "M9 4H4v16h5m5-12 4 4-4 4M8 12h12")
                                                },
                                            )
                                            .attr("aria-hidden", "true")
                                            .attr("width", "1em")
                                            .attr("height", "1em")
                                            .attr("viewBox", "0 0 24 24")
                                            .attr("fill", "none")
                                            .attr("stroke", "currentColor")
                                            .attr("stroke-width", "1.7")
                                    },
                                )
                                .class("xcss-button")
                                .disabled(
                                    ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(move ||
                                    pending_logout.get()),
                                )
                                .aria_label(
                                    ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(move ||
                                    {
                                        if pending_logout.get() {
                                            t("正在退出…", "Signing out…")
                                        } else {
                                            t("退出", "Sign out")
                                        }
                                    }),
                                )
                                .title(
                                    ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(
                                        t("退出", "Sign out"),
                                    ),
                                )
                                .on(
                                    xcss_web::ev::click,
                                    move |_| {
                                        if pending_logout.get_untracked() {
                                            return;
                                        }
                                        pending_logout.set(true);
                                        wasm_bindgen_futures::spawn_local(async move {
                                            let _ = api::logout().await;
                                            let _ = pending_logout.try_set(false);
                                        });
                                    },
                                ),
                            xcss_web::html::button()
                                .child(
                                    #[allow(unused_braces)]
                                    {
                                        xcss_web::svg::svg()
                                            .child((
                                                xcss_web::svg::circle()
                                                    .attr("cx", "12")
                                                    .attr("cy", "8")
                                                    .attr("r", "3.5"),
                                                xcss_web::svg::path()
                                                    .attr("d", "M4.5 21v-2a7.5 7.5 0 0 1 15 0v2"),
                                            ))
                                            .attr("viewBox", "0 0 24 24")
                                            .attr("width", "1em")
                                            .attr("height", "1em")
                                            .attr("fill", "none")
                                            .attr("stroke", "currentColor")
                                            .attr("stroke-width", "1.7")
                                            .attr("aria-hidden", "true")
                                    },
                                )
                                .class("xcss-button xcss-account-entry")
                                .attr("data-xcss-server-corner", "none")
                                .style(("border-radius", "50%"))
                                .style(("aspect-ratio", "1"))
                                .aria_label(
                                    ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(
                                        t("账号设置", "Account settings"),
                                    ),
                                )
                                .title(
                                    ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(
                                        t("账号设置", "Account settings"),
                                    ),
                                )
                                .on(
                                    xcss_web::ev::click,
                                    move |_| {
                                        let _ = web_sys::window()
                                            .unwrap()
                                            .location()
                                            .set_hash("account");
                                    },
                                ),
                        ))
                        .class("xcss-header-actions")
                        .role("group")
                        .aria_label(
                            ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(
                                t("全局操作", "Global actions"),
                            ),
                        ),
                ))
                .class("xcss-page-header"),
            {
                #[allow(unreachable_code)] #[allow(unused_mut)]
                #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
                component_view(
                    #[allow(clippy::needless_borrows_for_generic_args)]
                    &Show,
                    {
                        let mut props = component_props_builder(
                                &Show,
                            )
                            .when(
                                #[allow(unused_braces)]
                                { move || !toasts.records.get().is_empty() },
                            )
                            .children({
                                ::xcss_web::children::ToChildren::to_children(move || {
                                    xcss_web::html::div()
                                        .child(
                                            #[allow(unused_braces)]
                                            {
                                                {
                                                    #[allow(unreachable_code)] #[allow(unused_mut)]
                                                    #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
                                                    component_view(
                                                        #[allow(clippy::needless_borrows_for_generic_args)]
                                                        &For,
                                                        {
                                                            let mut props = component_props_builder(
                                                                    &For,
                                                                )
                                                                .each(
                                                                    #[allow(unused_braces)]
                                                                    { move || toasts.records.get() },
                                                                )
                                                                .key(#[allow(unused_braces)] { |r| r.id })
                                                                .children(
                                                                    #[allow(unused_braces)]
                                                                    {
                                                                        move |r| {
                                                                            let id = r.id;
                                                                            View::new(
    xcss_web::html::div()
        .child((
            xcss_web::html::span()
                .child(
                    #[allow(unused_braces)]
                    { IntoRender::into_render({ r.message }) },
                ),
            xcss_web::html::button()
                .child(#[allow(unused_braces)] { "×" })
                .class("xcss-button")
                .aria_label(
                    ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(
                        t("关闭通知", "Dismiss notification"),
                    ),
                )
                .on(
                    xcss_web::ev::click,
                    move |_| toasts.records.update(|v| v.retain(|r| r.id != id)),
                ),
        ))
        .class("xcss-toast")
        .role("status")
        .aria_live("polite")
        .aria_atomic("true"),
)
                                                                        }
                                                                    },
                                                                )
                                                                .build();
                                                            props
                                                        },
                                                    )
                                                }
                                            },
                                        )
                                        .class("xcss-toast-stack")
                                        .role("region")
                                        .aria_label(
                                            ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(
                                                t("通知", "Notifications"),
                                            ),
                                        )
                                })
                            })
                            .build();
                        props
                    },
                )
            },
            xcss_web::html::div()
                .child(
                    #[allow(unused_braces)]
                    {
                        xcss_web::html::main()
                            .child(
                                #[allow(unused_braces)]
                                {
                                    IntoRender::into_render(children())
                                },
                            )
                            .class("xcss-shell-main")
                            .id("xcss-main-content")
                            .tabindex("-1")
                    },
                )
                .class("xcss-shell-layout xcss-shell-layout--full"),
        ))
        .class("xcss-admin-shell")
        .style(("--xcss-header-icon-size", "1em")),
)
            .into_any()
        }
    })
}

#[component]
pub fn LoginPage(#[prop(optional)] landing: &'static str) -> impl IntoView {
    let username = NodeRef::<xcss_web::html::Input>::new();
    let password = NodeRef::<xcss_web::html::Input>::new();
    let pending = RwSignal::new(false);
    let failure = RwSignal::new(None::<(String, Option<String>)>);
    View::new(
        xcss_web::html::form()
            .child((
                xcss_web::html::h1().child(
                    #[allow(unused_braces)]
                    {
                        IntoRender::into_render({ t("管理员登录", "Administrator sign in") })
                    },
                ),
                xcss_web::html::label()
                    .child((
                        xcss_web::html::span().child(
                            #[allow(unused_braces)]
                            {
                                IntoRender::into_render({ t("用户名", "Username") })
                            },
                        ),
                        xcss_web::html::input()
                            .class("xcss-input")
                            .node_ref(username)
                            .name("username")
                            .autocomplete("username")
                            .required(true)
                            .maxlength("64")
                            .readonly(
                                ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(
                                    move || pending.get(),
                                ),
                            ),
                    ))
                    .class("xcss-form-field"),
                xcss_web::html::label()
                    .child((
                        xcss_web::html::span().child(
                            #[allow(unused_braces)]
                            {
                                IntoRender::into_render({ t("密码", "Password") })
                            },
                        ),
                        xcss_web::html::input()
                            .class("xcss-input")
                            .node_ref(password)
                            .name("password")
                            .r#type("password")
                            .autocomplete("current-password")
                            .required(true)
                            .maxlength("1024")
                            .readonly(
                                ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(
                                    move || pending.get(),
                                ),
                            ),
                    ))
                    .class("xcss-form-field"),
                IntoRender::into_render({
                    move || {
                        failure.get().map(|(message, id)| {
                            View::new({
                                #[allow(unreachable_code)]
                                #[allow(unused_mut)]
                                #[allow(
                                    clippy::let_and_return,
                                    clippy::let_unit_value,
                                    clippy::unit_arg
                                )]
                                component_view(
                                    #[allow(clippy::needless_borrows_for_generic_args)]
                                    &ErrorState,
                                    {
                                        let mut props = component_props_builder(&ErrorState)
                                            .message(
                                                #[allow(unused_braces)]
                                                {
                                                    message
                                                },
                                            )
                                            .request_id(
                                                #[allow(unused_braces)]
                                                {
                                                    id
                                                },
                                            )
                                            .build();
                                        props
                                    },
                                )
                            })
                        })
                    }
                }),
                xcss_web::html::button()
                    .child(
                        #[allow(unused_braces)]
                        {
                            IntoRender::into_render({
                                move || {
                                    if pending.get() {
                                        t("正在登录…", "Signing in…")
                                    } else {
                                        t("登录", "Sign in")
                                    }
                                }
                            })
                        },
                    )
                    .class("xcss-button")
                    .r#type("submit")
                    .disabled(
                        ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(move || {
                            pending.get()
                        }),
                    ),
            ))
            .novalidate(true)
            .aria_busy(
                ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(move || {
                    pending.get().to_string()
                }),
            )
            .on(xcss_web::ev::input, move |_| failure.set(None))
            .on(xcss_web::ev::submit, move |event| {
                event.prevent_default();
                if pending.get_untracked() {
                    return;
                }
                let name = username.get_untracked().unwrap();
                let secret = password.get_untracked().unwrap();
                if name.value().is_empty() {
                    failure.set(Some((t("请输入用户名。", "Enter your username."), None)));
                    let _ = name.focus();
                    return;
                }
                if secret.value().is_empty() {
                    failure.set(Some((t("请输入密码。", "Enter your password."), None)));
                    let _ = secret.focus();
                    return;
                }
                pending.set(true);
                failure.set(None);
                wasm_bindgen_futures::spawn_local(async move {
                    match api::login(&name.value(), &secret.value()).await {
                        Ok(_) => {
                            if !landing.is_empty() {
                                let window = web_sys::window().unwrap();
                                let current = window.location().href().unwrap();
                                if let Ok(url) = web_sys::Url::new_with_base(landing, &current) {
                                    if web_sys::Url::new(&current)
                                        .is_ok_and(|current| current.search_params().has("lang"))
                                    {
                                        url.search_params().set("lang", &crate::i18n::locale());
                                    }
                                    let _ = window.history().unwrap().replace_state_with_url(
                                        &wasm_bindgen::JsValue::NULL,
                                        "",
                                        Some(&url.href()),
                                    );
                                    let _ = window.dispatch_event(
                                        &web_sys::Event::new("hashchange").unwrap(),
                                    );
                                }
                            }
                        }
                        Err(error) => {
                            let _ = failure.try_set(Some((
                                t(
                                    "登录失败，请检查用户名和密码后重试。",
                                    "Sign in failed. Check your credentials and try again.",
                                ),
                                error.request_id,
                            )));
                            secret.set_value("");
                            let _ = secret.focus();
                        }
                    }
                    let _ = pending.try_set(false);
                });
            }),
    )
}

#[component]
pub fn AccountPage() -> impl IntoView {
    let username = NodeRef::<xcss_web::html::Input>::new();
    let current = NodeRef::<xcss_web::html::Input>::new();
    let new_password = NodeRef::<xcss_web::html::Input>::new();
    let confirmation = NodeRef::<xcss_web::html::Input>::new();
    let pending = RwSignal::new(false);
    let failure = RwSignal::new(None::<String>);
    let initial = api::current_session()
        .map(|s| s.username)
        .unwrap_or_default();
    on_cleanup(move || {
        for input in [current, new_password, confirmation] {
            if let Some(input) = input.get_untracked() {
                input.set_value("");
            }
        }
    });
    View::new(
    xcss_web::html::section()
        .child(
            #[allow(unused_braces)]
            {
                xcss_web::html::div()
                    .child(
                        #[allow(unused_braces)]
                        {
                            xcss_web::html::form()
                                .child((
                                    xcss_web::html::div()
                                        .child((
                                            xcss_web::html::label()
                                                .child((
                                                    xcss_web::html::span()
                                                        .child(
                                                            #[allow(unused_braces)]
                                                            {
                                                                IntoRender::into_render({
                                                                    t("用户名", "Username")
                                                                })
                                                            },
                                                        ),
                                                    xcss_web::html::input()
                                                        .class("xcss-input")
                                                        .node_ref(username)
                                                        .name("username")
                                                        .value(
                                                            ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(
                                                                initial,
                                                            ),
                                                        )
                                                        .autocomplete("username")
                                                        .required(true)
                                                        .minlength("3")
                                                        .maxlength("64")
                                                        .readonly(
                                                            ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(move ||
                                                            pending.get()),
                                                        ),
                                                ))
                                                .class("xcss-form-field"),
                                            xcss_web::html::label()
                                                .child((
                                                    xcss_web::html::span()
                                                        .child(
                                                            #[allow(unused_braces)]
                                                            {
                                                                IntoRender::into_render({
                                                                    t("当前密码", "Current password")
                                                                })
                                                            },
                                                        ),
                                                    xcss_web::html::input()
                                                        .class("xcss-input")
                                                        .node_ref(current)
                                                        .name("current_password")
                                                        .r#type("password")
                                                        .autocomplete("current-password")
                                                        .required(true)
                                                        .maxlength("1024")
                                                        .readonly(
                                                            ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(move ||
                                                            pending.get()),
                                                        ),
                                                ))
                                                .class("xcss-form-field"),
                                            xcss_web::html::label()
                                                .child((
                                                    xcss_web::html::span()
                                                        .child(
                                                            #[allow(unused_braces)]
                                                            {
                                                                IntoRender::into_render({
                                                                    t("新密码", "New password")
                                                                })
                                                            },
                                                        ),
                                                    xcss_web::html::input()
                                                        .class("xcss-input")
                                                        .node_ref(new_password)
                                                        .name("new_password")
                                                        .r#type("password")
                                                        .autocomplete("new-password")
                                                        .maxlength("1024")
                                                        .readonly(
                                                            ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(move ||
                                                            pending.get()),
                                                        ),
                                                ))
                                                .class("xcss-form-field"),
                                            xcss_web::html::label()
                                                .child((
                                                    xcss_web::html::span()
                                                        .child(
                                                            #[allow(unused_braces)]
                                                            {
                                                                IntoRender::into_render({
                                                                    t("确认新密码", "Confirm new password")
                                                                })
                                                            },
                                                        ),
                                                    xcss_web::html::input()
                                                        .class("xcss-input")
                                                        .node_ref(confirmation)
                                                        .name("confirm_password")
                                                        .r#type("password")
                                                        .autocomplete("new-password")
                                                        .maxlength("1024")
                                                        .readonly(
                                                            ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(move ||
                                                            pending.get()),
                                                        ),
                                                ))
                                                .class("xcss-form-field"),
                                        ))
                                        .class("xcss-account-fields"),
                                    IntoRender::into_render({
                                        move || {
                                            failure
                                                .get()
                                                .map(|message| {
                                                    View::new({
    #[allow(unreachable_code)] #[allow(unused_mut)] #[allow(clippy::let_and_return, clippy::let_unit_value, clippy::unit_arg)]
    component_view(
        #[allow(clippy::needless_borrows_for_generic_args)]
        &ErrorState,
        {
            let mut props = component_props_builder(&ErrorState)
                .message(#[allow(unused_braces)] { message })
                .build();
            props
        },
    )
})
                                                })
                                        }
                                    }),
                                    xcss_web::html::div()
                                        .child(
                                            #[allow(unused_braces)]
                                            {
                                                xcss_web::html::button()
                                                    .child(
                                                        #[allow(unused_braces)]
                                                        {
                                                            IntoRender::into_render({
                                                                move || {
                                                                    if pending.get() {
                                                                        t("保存中…", "Saving…")
                                                                    } else {
                                                                        t("保存", "Save")
                                                                    }
                                                                }
                                                            })
                                                        },
                                                    )
                                                    .class("xcss-button")
                                                    .r#type("submit")
                                                    .disabled(
                                                        ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(move ||
                                                        pending.get()),
                                                    )
                                            },
                                        )
                                        .class("xcss-actions"),
                                ))
                                .class("xcss-account-form")
                                .novalidate(true)
                                .aria_busy(
                                    ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(move ||
                                    pending.get().to_string()),
                                )
                                .on(
                                    xcss_web::ev::input,
                                    move |_| failure.set(None),
                                )
                                .on(
                                    xcss_web::ev::submit,
                                    move |event| {
                                        event.prevent_default();
                                        if pending.get_untracked() {
                                            return;
                                        }
                                        let name = username.get_untracked().unwrap();
                                        let secret = current.get_untracked().unwrap();
                                        let password = new_password.get_untracked().unwrap();
                                        let confirm = confirmation.get_untracked().unwrap();
                                        if name.value().len() < 3 || name.value().len() > 64 {
                                            failure
                                                .set(
                                                    Some(
                                                        t(
                                                            "请输入有效的用户名。",
                                                            "Enter a valid username.",
                                                        ),
                                                    ),
                                                );
                                            let _ = name.focus();
                                            return;
                                        }
                                        if secret.value().is_empty() {
                                            failure
                                                .set(
                                                    Some(
                                                        t(
                                                            "请输入当前密码。",
                                                            "Enter your current password.",
                                                        ),
                                                    ),
                                                );
                                            let _ = secret.focus();
                                            return;
                                        }
                                        if !password.value().is_empty()
                                            && !crate::contracts::valid_password(&password.value())
                                        {
                                            failure
                                                .set(
                                                    Some(
                                                        t(
                                                            "新密码长度不符合要求。",
                                                            "The new password length is invalid.",
                                                        ),
                                                    ),
                                                );
                                            let _ = password.focus();
                                            return;
                                        }
                                        if password.value() != confirm.value() {
                                            failure
                                                .set(
                                                    Some(
                                                        t(
                                                            "两次输入的新密码不一致。",
                                                            "The new passwords do not match.",
                                                        ),
                                                    ),
                                                );
                                            let _ = confirm.focus();
                                            return;
                                        }
                                        let mut input = json!(
                                            { "username" : name.value(), "current_password" : secret
                                            .value() }
                                        );
                                        if !password.value().is_empty() {
                                            input["new_password"] = json!(password.value());
                                        }
                                        pending.set(true);
                                        failure.set(None);
                                        wasm_bindgen_futures::spawn_local(async move {
                                            if let Err(error) = api::update_account(input).await {
                                                let message = match error.code.as_str() {
                                                    "admin.current_password_invalid" => {
                                                        t(
                                                            "当前密码不正确。",
                                                            "The current password is incorrect.",
                                                        )
                                                    }
                                                    "admin.conflict" => {
                                                        t(
                                                            "用户名已被使用。",
                                                            "The username is already in use.",
                                                        )
                                                    }
                                                    _ => {
                                                        t(
                                                            "账号未能更新，请检查输入并重试。",
                                                            "The account could not be updated. Check the input and retry.",
                                                        )
                                                    }
                                                };
                                                let _ = failure.try_set(Some(message));
                                                secret.set_value("");
                                                password.set_value("");
                                                confirm.set_value("");
                                                let _ = if error.code == "admin.conflict" {
                                                    name.focus()
                                                } else {
                                                    secret.focus()
                                                };
                                            } else {
                                                secret.set_value("");
                                                password.set_value("");
                                                confirm.set_value("");
                                            }
                                            let _ = pending.try_set(false);
                                        });
                                    },
                                )
                        },
                    )
                    .class("xcss-content-panel")
            },
        )
        .class("xcss-content-stack xcss-account-page")
        .aria_label(
            ::xcss_web::prelude::IntoAttributeValue::into_attribute_value(
                t("账号设置", "Account settings"),
            ),
        ),
)
}
