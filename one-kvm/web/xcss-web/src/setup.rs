use crate::{
    api,
    models::{DeviceCatalog, Platform, SetupStatus},
    ui,
};
use serde_json::{Map, Value, json};
use xcss_web::{html as h, prelude::*};

fn initialize(
    value: Value,
    initialized: RwSignal<Option<bool>>,
    busy: RwSignal<bool>,
    error: RwSignal<String>,
) {
    busy.set(true);
    error.set(String::new());
    xcss_web::task::spawn_local(async move {
        match api::mutate("/api/setup/init", "POST", Some(value)).await {
            Ok(()) => initialized.set(Some(true)),
            Err(value) => error.set(value.to_string()),
        }
        let _ = busy.try_set(false);
    });
}

pub fn page(initialized: RwSignal<Option<bool>>) -> impl IntoView {
    let video = RwSignal::new(String::new());
    let audio = RwSignal::new(String::new());
    let hid = RwSignal::new(String::new());
    let port = RwSignal::new(String::new());
    let udc = RwSignal::new(String::new());
    let monitor = RwSignal::new(String::new());
    let windows_keyboard = RwSignal::new(true);
    let windows_mouse = RwSignal::new(true);
    let platform = RwSignal::new(Platform::default());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(String::new());
    let (devices, devices_error) = ui::poll::<DeviceCatalog>("/api/devices", 10_000);
    xcss_web::task::spawn_local(async move {
        match api::get::<SetupStatus>("/api/setup").await {
            Ok(status) => platform.set(status.platform),
            Err(value) => error.set(value.to_string()),
        }
    });
    h::section().class("card login").child((
        h::p().class("eyebrow").child("AUTHORIZED HARDWARE SETUP"),
        h::h1().child("初始化 KVM 硬件"),
        h::p().class("hint").child(format!("当前实例：{}。选择本实例的显示器、视频采集、键鼠和音频设备；也可以保留当前配置后再到设备设置调整。", api::instance_id())),
        ui::error(error), ui::error(devices_error),
        h::form().on(xcss_web::ev::submit, move |event| {
            event.prevent_default();
            let mut body = Map::new();
            for (key, value) in [("video_device", video.get_untracked()), ("audio_device", audio.get_untracked()), ("hid_backend", hid.get_untracked()), ("hid_ch9329_port", port.get_untracked()), ("hid_otg_udc", udc.get_untracked())] {
                if !value.is_empty() { body.insert(key.to_owned(), Value::String(value)); }
            }
            if hid.get_untracked() == "windows" {
                let selected = monitor.get_untracked();
                if selected.is_empty() { error.set("请选择 Windows 输入映射显示器。".into()); return; }
                body.insert("hid_windows_monitor".into(), Value::String(selected));
                body.insert("hid_windows_keyboard".into(), Value::Bool(windows_keyboard.get_untracked()));
                body.insert("hid_windows_mouse".into(), Value::Bool(windows_mouse.get_untracked()));
            }
            initialize(Value::Object(body), initialized, busy, error);
        }).child((
            h::label().class("field").child((h::span().child("显示器 / 视频采集设备"), h::select().attr("aria-label", "显示器 / 视频采集设备").on(xcss_web::ev::change, move |event| {
                let selected = event_target_value(&event);
                if selected.starts_with("desktop:") { monitor.set(selected.clone()); }
                video.set(selected);
            }).child((h::option().value("").child("保留当前设置"), move || devices.get().map(|devices| devices.video.into_iter().map(|device| h::option().value(device.path.clone()).child(format!("{} · {}", device.name, device.path))).collect::<Vec<_>>()))))),
            h::label().class("field").child((h::span().child("HID 后端"), h::select().attr("aria-label", "HID 后端").on(xcss_web::ev::change, move |event| hid.set(event_target_value(&event))).child(move || [( "", "保留当前设置"), ("none", "关闭"), ("otg", "USB OTG"), ("ch9329", "CH9329"), ("bluetooth", "蓝牙"), ("windows", "Windows 桌面键鼠")].into_iter().filter(|(value, _)| value.is_empty() || platform.with(|platform| platform.supports_hid(value))).map(|(value,label)| h::option().value(value).child(label)).collect::<Vec<_>>()))),
            h::div().hidden(move || hid.get() != "windows").child((
                h::p().class("hint").child("Windows 键鼠作用于登录用户的共享桌面。显示器映射限定鼠标坐标，不提供独立 Windows 用户会话或实体键鼠隔离。"),
                h::label().class("field").child((h::span().child("Windows 输入映射显示器"), h::select().attr("aria-label", "Windows 输入映射显示器").prop("value", move || monitor.get()).on(xcss_web::ev::change, move |event| monitor.set(event_target_value(&event))).child((h::option().value("").child("选择显示器"), move || devices.get().map(|devices| devices.video.into_iter().filter(|device| device.path.starts_with("desktop:")).map(|device| h::option().value(device.path).child(device.name)).collect::<Vec<_>>()))))),
                h::label().class("field").child((h::span().child("Windows 键盘输入"), h::input().r#type("checkbox").prop("checked", move || windows_keyboard.get()).on(xcss_web::ev::change, move |event| windows_keyboard.set(event_target_checked(&event))))),
                h::label().class("field").child((h::span().child("Windows 鼠标输入"), h::input().r#type("checkbox").prop("checked", move || windows_mouse.get()).on(xcss_web::ev::change, move |event| windows_mouse.set(event_target_checked(&event))))),
            )),
            h::div().hidden(move || hid.get() != "ch9329").child(ui::input("CH9329 串口路径（使用串口时填写）", port)),
            h::label().class("field").hidden(move || hid.get() != "otg").child((h::span().child("OTG UDC"), h::select().attr("aria-label", "OTG UDC").on(xcss_web::ev::change, move |event| udc.set(event_target_value(&event))).child((h::option().value("").child("保留当前设置"), move || devices.get().map(|devices| devices.udc.into_iter().map(|device| h::option().value(device.name.clone()).child(device.name)).collect::<Vec<_>>()))))),
            h::label().class("field").child((h::span().child("音频采集设备"), h::select().attr("aria-label", "音频采集设备").on(xcss_web::ev::change, move |event| audio.set(event_target_value(&event))).child((h::option().value("").child("保留当前设置"), move || devices.get().map(|devices| devices.audio.into_iter().map(|device| h::option().value(device.name).child(device.description)).collect::<Vec<_>>()))))),
            h::div().class("actions").child((h::button().class("primary").r#type("submit").disabled(move || busy.get()).child("保存选择并初始化"), h::button().r#type("button").disabled(move || busy.get()).on(xcss_web::ev::click, move |_| initialize(json!({}), initialized, busy, error)).child("初始化并保留当前硬件配置"))),
        )),
    ))
}
