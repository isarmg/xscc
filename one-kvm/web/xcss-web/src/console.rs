use crate::{api, hid, models::StreamStatus, ui};
use serde_json::json;
use wasm_bindgen::{JsCast, prelude::*};
use xcss_web::{html as h, prelude::*};

#[derive(Clone, serde::Deserialize)]
struct Codecs {
    codecs: Vec<Codec>,
}
#[derive(Clone, serde::Deserialize)]
struct Codec {
    id: String,
    name: String,
    available: bool,
}
#[derive(Clone, serde::Deserialize)]
struct StreamConstraints {
    allowed_codecs: Vec<String>,
    disallow_mjpeg: bool,
    reason: String,
}

fn usable(codec: &Codec, constraints: &StreamConstraints) -> bool {
    codec.available
        && codec_supported(&codec.id)
        && if codec.id == "mjpeg" {
            !constraints.disallow_mjpeg
        } else {
            constraints.allowed_codecs.contains(&codec.id)
        }
}

#[wasm_bindgen(js_namespace = ["globalThis", "kvmBrowser"])]
extern "C" {
    #[wasm_bindgen(catch, js_name = connectVideo)]
    async fn connect_video(mode: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = disconnectVideo)]
    async fn disconnect_video() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = sendPacket)]
    fn send_packet(value: &[u8]) -> Result<(), JsValue>;
    #[wasm_bindgen(catch, js_name = setVolume)]
    async fn set_volume(value: f64) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = toggleMicrophone)]
    async fn toggle_microphone() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = codecSupported)]
    fn codec_supported(value: &str) -> bool;
    #[wasm_bindgen(catch)]
    async fn fullscreen() -> Result<JsValue, JsValue>;
}

fn send(value: &[u8], error: RwSignal<String>) {
    if let Err(value) = send_packet(value) {
        error.set(ui::js_error(value));
    }
}

fn key(event: web_sys::KeyboardEvent, down: bool, error: RwSignal<String>) {
    if let Some(usage) = hid::usage(&event.code()) {
        event.prevent_default();
        if event.repeat() {
            return;
        }
        let modifier = u8::from(event.ctrl_key())
            | (u8::from(event.shift_key()) << 1)
            | (u8::from(event.alt_key()) << 2)
            | (u8::from(event.meta_key()) << 3);
        send(&hid::keyboard(down, usage, modifier), error);
    }
}

fn position(event: &web_sys::MouseEvent) -> Option<(i16, i16)> {
    let target = event
        .current_target()?
        .dyn_into::<web_sys::Element>()
        .ok()?;
    let rect = target.get_bounding_client_rect();
    if rect.width() == 0. || rect.height() == 0. {
        return None;
    }
    let document = web_sys::window()?.document()?;
    let video = document
        .get_element_by_id("kvm-video")?
        .dyn_into::<web_sys::HtmlVideoElement>()
        .ok()?;
    let image = document
        .get_element_by_id("kvm-image")?
        .dyn_into::<web_sys::HtmlImageElement>()
        .ok()?;
    let (width, height) = if image.hidden() {
        (video.video_width(), video.video_height())
    } else {
        (image.natural_width(), image.natural_height())
    };
    if width == 0 || height == 0 {
        return None;
    }
    let scale = (rect.width() / f64::from(width)).min(rect.height() / f64::from(height));
    let rendered_width = f64::from(width) * scale;
    let rendered_height = f64::from(height) * scale;
    let left = rect.left() + (rect.width() - rendered_width) / 2.;
    let top = rect.top() + (rect.height() - rendered_height) / 2.;
    Some((
        ((f64::from(event.client_x()) - left) / rendered_width * 32767.).clamp(0., 32767.) as i16,
        ((f64::from(event.client_y()) - top) / rendered_height * 32767.).clamp(0., 32767.) as i16,
    ))
}

fn shortcut(
    label: &'static str,
    usage: u8,
    modifier: u8,
    error: RwSignal<String>,
) -> impl IntoView {
    h::button()
        .on(xcss_web::ev::click, move |_| {
            send(&hid::keyboard(true, usage, modifier), error);
            send(&hid::keyboard(false, usage, 0), error);
        })
        .child(label)
}

fn virtual_keyboard(error: RwSignal<String>) -> impl IntoView {
    let modifiers = RwSignal::new(0u8);
    h::details().child((
        h::summary().child("屏幕键盘"),
        h::div().class("actions").child(
            [
                ("Ctrl", 1u8, 224u8),
                ("Shift", 2, 225),
                ("Alt", 4, 226),
                ("Meta", 8, 227),
            ]
            .into_iter()
            .map(|(label, bit, usage)| {
                h::button()
                    .attr("aria-pressed", move || {
                        (modifiers.get() & bit != 0).to_string()
                    })
                    .on(xcss_web::ev::click, move |_| {
                        modifiers.update(|value| *value ^= bit);
                        let modifier = modifiers.get_untracked();
                        send(&hid::keyboard(modifier & bit != 0, usage, modifier), error);
                    })
                    .child(label)
            })
            .collect::<Vec<_>>(),
        ),
        h::div().class("kbd").child(
            [
                "1234567890-=",
                "qwertyuiop[]",
                "asdfghjkl;'",
                "zxcvbnm,./\\",
            ]
            .into_iter()
            .flat_map(|row| row.chars())
            .map(|character| {
                let (usage, _) = hid::character(character)
                    .expect("invariant: screen keyboard contains mapped ASCII keys");
                h::button()
                    .on(xcss_web::ev::click, move |_| {
                        let modifier = modifiers.get_untracked();
                        send(&hid::keyboard(true, usage, modifier), error);
                        send(&hid::keyboard(false, usage, modifier), error);
                    })
                    .child(character.to_ascii_uppercase().to_string())
            })
            .collect::<Vec<_>>(),
        ),
        h::div().class("kbd").child(
            [
                "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
            ]
            .into_iter()
            .enumerate()
            .map(|(index, label)| shortcut(label, 58 + index as u8, 0, error))
            .chain(
                [
                    ("←", 80),
                    ("↑", 82),
                    ("↓", 81),
                    ("→", 79),
                    ("Tab", 43),
                    ("Enter", 40),
                    ("Space", 44),
                    ("Backspace", 42),
                    ("Delete", 76),
                    ("Caps Lock", 57),
                    ("Home", 74),
                    ("End", 77),
                ]
                .into_iter()
                .map(|(label, usage)| shortcut(label, usage, 0, error)),
            )
            .collect::<Vec<_>>(),
        ),
        h::button()
            .on(xcss_web::ev::click, move |_| {
                modifiers.set(0);
                xcss_web::task::spawn_local(async move {
                    if let Err(value) = api::mutate("/api/hid/reset", "POST", Some(json!({}))).await
                    {
                        let _ = error.try_set(value.to_string());
                    }
                });
            })
            .child("释放键盘修饰键"),
    ))
}

pub fn page(platform: RwSignal<crate::models::Platform>) -> impl IntoView {
    let mode = RwSignal::new("h264".to_owned());
    let relative = RwSignal::new(false);
    let state = RwSignal::new("点击连接开始查看画面。".to_owned());
    let error = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let microphone = RwSignal::new(false);
    let microphone_busy = RwSignal::new(false);
    let paste = RwSignal::new(String::new());
    let pasting = RwSignal::new(false);
    let wol = RwSignal::new(String::new());
    let (status, status_error) = ui::poll::<StreamStatus>("/api/stream/status", 2500);
    let (codecs, codec_error) = ui::poll::<Codecs>("/api/stream/codecs", 30_000);
    let (constraints, constraints_error) =
        ui::poll::<StreamConstraints>("/api/stream/constraints", 2500);
    Effect::new(move |_| {
        let (Some(codecs), Some(constraints)) = (codecs.get(), constraints.get()) else {
            return;
        };
        if codecs
            .codecs
            .iter()
            .any(|codec| codec.id == mode.get_untracked() && usable(codec, &constraints))
        {
            return;
        }
        if let Some(codec) = codecs
            .codecs
            .into_iter()
            .find(|codec| usable(codec, &constraints))
        {
            mode.set(codec.id);
        }
    });
    ui::listen("kvm-media", move |event| {
        let detail = js_sys::Reflect::get(event.as_ref(), &JsValue::from_str("detail")).ok();
        if let Some(detail) = detail {
            let message = js_sys::Reflect::get(&detail, &JsValue::from_str("detail"))
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_default();
            let phase = js_sys::Reflect::get(&detail, &JsValue::from_str("state"))
                .ok()
                .and_then(|value| value.as_string());
            if phase.as_deref() == Some("error") {
                error.set(message);
            } else {
                state.set(message);
            }
        }
    });
    on_cleanup(move || {
        xcss_web::task::spawn_local(async move {
            let _ = disconnect_video().await;
        })
    });
    h::div().child((
        h::div().class("console-top").child((
            h::h1().child("远程控制台"),
            h::div().class("actions").child((
                h::select()
                    .attr("aria-label", "视频传输方式")
                    .on(xcss_web::ev::change, move |event| {
                        mode.set(event_target_value(&event))
                    })
                    .prop("value", move || mode.get())
                    .child(move || {
                        codecs
                            .get()
                            .zip(constraints.get())
                            .map(|(codecs, constraints)| {
                                codecs
                                    .codecs
                                    .into_iter()
                                    .filter(|codec| usable(codec, &constraints))
                                    .map(|codec| h::option().value(codec.id).child(codec.name))
                                    .collect::<Vec<_>>()
                            })
                    }),
                h::button()
                    .class("primary")
                    .disabled(move || busy.get() || constraints.get().is_none())
                    .on(xcss_web::ev::click, move |_| {
                        busy.set(true);
                        error.set(String::new());
                        let mode = mode.get_untracked();
                        xcss_web::task::spawn_local(async move {
                            if let Err(value) = connect_video(&mode).await {
                                error.set(ui::js_error(value));
                            }
                            busy.set(false);
                        });
                    })
                    .child("连接画面"),
                h::button()
                    .on(xcss_web::ev::click, move |_| {
                        xcss_web::task::spawn_local(async move {
                            if let Err(value) = disconnect_video().await {
                                error.set(ui::js_error(value));
                            }
                        })
                    })
                    .child("断开"),
                h::button()
                    .on(xcss_web::ev::click, move |_| {
                        xcss_web::task::spawn_local(async move {
                            if let Err(value) = fullscreen().await {
                                error.set(ui::js_error(value));
                            }
                        })
                    })
                    .child("全屏"),
                h::a()
                    .class("button")
                    .href(api::url("/api/snapshot"))
                    .target("_blank")
                    .rel("noopener")
                    .child("截图"),
            )),
        )),
        ui::error(error),
        ui::error(codec_error),
        ui::error(constraints_error),
        h::p()
            .class("hint")
            .child(move || constraints.get().map(|value| value.reason)),
        h::div()
            .id("kvm-viewport")
            .class("viewport")
            .tabindex(0)
            .on(xcss_web::ev::keydown, move |event| key(event, true, error))
            .on(xcss_web::ev::keyup, move |event| key(event, false, error))
            .on(xcss_web::ev::contextmenu, |event| event.prevent_default())
            .on(xcss_web::ev::pointermove, move |event| {
                if relative.get_untracked() {
                    send(
                        &hid::mouse(
                            0,
                            event.movement_x().clamp(-32768, 32767) as i16,
                            event.movement_y().clamp(-32768, 32767) as i16,
                            0,
                        ),
                        error,
                    );
                } else if let Some((x, y)) = position(&event) {
                    send(&hid::mouse(1, x, y, 0), error);
                }
            })
            .on(xcss_web::ev::pointerdown, move |event| {
                event.prevent_default();
                if let Some(target) = event
                    .current_target()
                    .and_then(|target| target.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = target.focus();
                    let _ = target.set_pointer_capture(event.pointer_id());
                }
                send(&hid::mouse(2, 0, 0, event.button() as i8), error);
            })
            .on(xcss_web::ev::pointerup, move |event| {
                event.prevent_default();
                send(&hid::mouse(3, 0, 0, event.button() as i8), error);
            })
            .on(xcss_web::ev::wheel, move |event| {
                event.prevent_default();
                send(
                    &hid::mouse(4, 0, 0, if event.delta_y() > 0. { -1 } else { 1 }),
                    error,
                );
            })
            .child((
                h::video()
                    .id("kvm-video")
                    .autoplay(true)
                    .playsinline(true)
                    .muted(true),
                h::img().id("kvm-image").alt("KVM 画面").hidden(true),
            )),
        h::p().class("status-line").child(move || state.get()),
        h::p().class("hint").child(move || {
            status
                .get()
                .map(|value| {
                    format!(
                        "采集 {} · {} FPS · {} 个连接 · {} · {}",
                        value.state,
                        value.fps,
                        value.clients,
                        value.device.unwrap_or_default(),
                        value
                            .resolution
                            .map(|(width, height)| format!("{width}×{height}"))
                            .unwrap_or_default()
                    )
                })
                .unwrap_or_default()
        }),
        ui::error(status_error),
        h::section().class("card").child((
            h::div().class("actions").child((
                h::label().class("field").child((
                    "播放音量",
                    h::input()
                        .class("volume")
                        .r#type("range")
                        .min("0")
                        .max("100")
                        .value("0")
                        .on(xcss_web::ev::input, move |event| {
                            let value = event_target_value(&event);
                            let level = value.parse::<f64>().unwrap_or_default() / 100.;
                            xcss_web::task::spawn_local(async move {
                                if let Err(value) = set_volume(level).await {
                                    error.set(ui::js_error(value));
                                }
                            });
                        }),
                )),
                h::button()
                    .disabled(move || microphone_busy.get())
                    .hidden(move || platform.with(|platform| !platform.supports_otg()))
                    .on(xcss_web::ev::click, move |_| {
                        microphone_busy.set(true);
                        xcss_web::task::spawn_local(async move {
                            match toggle_microphone().await {
                                Ok(value) => {
                                    let _ = microphone.try_set(value.as_bool().unwrap_or_default());
                                }
                                Err(value) => {
                                    let _ = error.try_set(ui::js_error(value));
                                }
                            }
                            let _ = microphone_busy.try_set(false);
                        })
                    })
                    .child(move || {
                        if microphone.get() {
                            "关闭麦克风转发"
                        } else {
                            "开启麦克风转发"
                        }
                    }),
                h::label().child((
                    h::input()
                        .r#type("checkbox")
                        .on(xcss_web::ev::change, move |event| {
                            relative.set(event_target_checked(&event))
                        }),
                    "相对鼠标",
                )),
                shortcut("Ctrl+Alt+Del", 76, 5, error),
                shortcut("Alt+Tab", 43, 4, error),
                shortcut("Win / Meta", 227, 0, error),
                shortcut("Escape", 41, 0, error),
                ui::action(
                    "释放所有按键",
                    "/api/hid/reset",
                    "POST",
                    Some(json!({})),
                    error,
                ),
            )),
            h::div().class("actions").child(
                [
                    ("Speed", "低延迟"),
                    ("Balanced", "均衡"),
                    ("Quality", "高画质"),
                ]
                .into_iter()
                .map(|(preset, label)| {
                    ui::action(
                        label,
                        "/api/stream/bitrate",
                        "POST",
                        Some(json!({"bitrate_preset":{"type":preset}})),
                        error,
                    )
                })
                .collect::<Vec<_>>(),
            ),
        )),
        h::div().class("grid").child((
            h::section().class("card").child((
                h::h2().child("电源与网络唤醒"),
                h::div().class("actions").child((
                    ui::action(
                        "短按电源",
                        "/api/atx/power",
                        "POST",
                        Some(json!({"action":"short"})),
                        error,
                    ),
                    ui::action(
                        "长按电源",
                        "/api/atx/power",
                        "POST",
                        Some(json!({"action":"long"})),
                        error,
                    ),
                    ui::action(
                        "复位",
                        "/api/atx/power",
                        "POST",
                        Some(json!({"action":"reset"})),
                        error,
                    ),
                )),
                ui::input("网络唤醒 MAC 地址", wol),
                h::button()
                    .on(xcss_web::ev::click, move |_| {
                        let mac = wol.get_untracked();
                        xcss_web::task::spawn_local(async move {
                            if let Err(value) = api::mutate(
                                "/api/atx/wol",
                                "POST",
                                Some(json!({"mac_address":mac})),
                            )
                            .await
                            {
                                error.set(value.to_string());
                            }
                        });
                    })
                    .child("发送唤醒包"),
                h::p()
                    .class("hint")
                    .child("长按电源和复位可能导致未保存内容丢失。"),
            )),
            h::section().class("card").child((
                h::h2().child("键盘输入与粘贴"),
                h::textarea()
                    .attr("aria-label", "粘贴文本")
                    .placeholder("通过 USB 键盘依次输入 ASCII 文本")
                    .prop("value", move || paste.get())
                    .on(xcss_web::ev::input, move |event| {
                        paste.set(event_target_value(&event))
                    }),
                h::button()
                    .disabled(move || pasting.get())
                    .on(xcss_web::ev::click, move |_| {
                        let text = paste.get_untracked();
                        if text.chars().any(|character| {
                            character != '\r' && hid::character(character).is_none()
                        }) {
                            error.set("键盘粘贴支持 ASCII 字符；请移除无法映射的字符。".into());
                            return;
                        }
                        pasting.set(true);
                        xcss_web::task::spawn_local(async move {
                            for character in text.chars() {
                                if pasting.try_get_untracked().is_none_or(|value| !value) {
                                    break;
                                }
                                if let Some((usage, modifier)) = hid::character(character) {
                                    if let Err(value) =
                                        send_packet(&hid::keyboard(true, usage, modifier))
                                    {
                                        error.set(ui::js_error(value));
                                        break;
                                    }
                                    gloo_timers::future::TimeoutFuture::new(12).await;
                                    if let Err(value) = send_packet(&hid::keyboard(false, usage, 0))
                                    {
                                        error.set(ui::js_error(value));
                                        break;
                                    }
                                    gloo_timers::future::TimeoutFuture::new(12).await;
                                }
                            }
                            let _ = pasting.try_set(false);
                        });
                    })
                    .child("输入到被控计算机"),
                h::button()
                    .hidden(move || !pasting.get())
                    .on(xcss_web::ev::click, move |_| pasting.set(false))
                    .child("停止粘贴"),
                virtual_keyboard(error),
            )),
        )),
        crate::computer_use::panel(),
    ))
}
