use crate::{api, ui};
use serde::Deserialize;
use serde_json::json;
use xcss_web::{html as h, prelude::*};

#[derive(Clone, Deserialize)]
struct UsbDevice {
    bus_num: u32,
    dev_num: u32,
    id_vendor: String,
    id_product: String,
    product: Option<String>,
}
#[derive(Clone, Deserialize)]
struct BluetoothStatus {
    ready: bool,
    connected: bool,
    adapter: String,
    peer: Option<String>,
    pairing_seconds: u32,
    error: Option<String>,
    devices: Vec<BluetoothDevice>,
}
#[derive(Clone, Deserialize)]
struct BluetoothDevice {
    address: String,
    name: String,
    paired: bool,
    connected: bool,
}
#[derive(Clone, Deserialize)]
struct SelfCheck {
    overall_ok: bool,
    error_count: u32,
    warning_count: u32,
    checks: Vec<Check>,
}
#[derive(Clone, Deserialize)]
struct Check {
    ok: bool,
    message: String,
    hint: Option<String>,
}

pub fn panel() -> impl IntoView {
    let error = RwSignal::new(String::new());
    let bluetooth = RwSignal::new(None::<BluetoothStatus>);
    let usb = RwSignal::new(Vec::<UsbDevice>::new());
    let check = RwSignal::new(None::<SelfCheck>);
    h::details().class("card").child((
        h::summary().child("USB / 蓝牙与硬件诊断"),
        ui::error(error),
        h::p()
            .class("hint")
            .child("按需读取设备状态，OTG 自检读取本机 USB 配置。USB 复位由本机管理员完成。"),
        h::div().class("actions").child((
            h::button()
                .on(xcss_web::ev::click, move |_| {
                    xcss_web::task::spawn_local(async move {
                        match api::get("/api/hid/bluetooth").await {
                            Ok(value) => bluetooth.set(Some(value)),
                            Err(value) => error.set(value.to_string()),
                        }
                    })
                })
                .child("读取蓝牙状态"),
            ui::action(
                "开放蓝牙配对 120 秒",
                "/api/hid/bluetooth",
                "POST",
                Some(json!({"action":"pair","seconds":120})),
                error,
            ),
            ui::action(
                "关闭蓝牙配对",
                "/api/hid/bluetooth",
                "POST",
                Some(json!({"action":"close"})),
                error,
            ),
            ui::action(
                "断开蓝牙设备",
                "/api/hid/bluetooth",
                "POST",
                Some(json!({"action":"disconnect"})),
                error,
            ),
            ui::action(
                "清除蓝牙配对",
                "/api/hid/bluetooth",
                "POST",
                Some(json!({"action":"forget"})),
                error,
            ),
            h::button()
                .on(xcss_web::ev::click, move |_| {
                    xcss_web::task::spawn_local(async move {
                        match api::get("/api/devices/usb").await {
                            Ok(value) => usb.set(value),
                            Err(value) => error.set(value.to_string()),
                        }
                    })
                })
                .child("读取 USB 设备"),
            h::button()
                .on(xcss_web::ev::click, move |_| {
                    xcss_web::task::spawn_local(async move {
                        match api::get("/api/hid/otg/self-check").await {
                            Ok(value) => check.set(Some(value)),
                            Err(value) => error.set(value.to_string()),
                        }
                    })
                })
                .child("运行 OTG 自检"),
        )),
        h::div().child(move || {
            bluetooth.get().map(|status| {
                h::div().child((
                    h::p().class("hint").child(format!(
                        "蓝牙 {} · 就绪 {} · 已连接 {} · 配对窗口 {} 秒 · 对端 {} · {}",
                        status.adapter,
                        status.ready,
                        status.connected,
                        status.pairing_seconds,
                        status.peer.unwrap_or_default(),
                        status.error.unwrap_or_default()
                    )),
                    h::div().child(
                        status
                            .devices
                            .into_iter()
                            .map(|device| {
                                h::p().class("hint").child(format!(
                                    "{} · {} · 配对 {} · 连接 {}",
                                    device.name, device.address, device.paired, device.connected
                                ))
                            })
                            .collect::<Vec<_>>(),
                    ),
                ))
            })
        }),
        h::div().child(move || {
            usb.get()
                .into_iter()
                .map(|device| {
                    h::div().class("file-row").child((h::span().child(format!(
                        "USB {}:{} · {}:{} · {}",
                        device.bus_num,
                        device.dev_num,
                        device.id_vendor,
                        device.id_product,
                        device.product.unwrap_or_default()
                    )),))
                })
                .collect::<Vec<_>>()
        }),
        h::div().child(move || {
            check.get().map(|check| {
                h::div().child((
                    h::p().class("hint").child(format!(
                        "OTG 自检 {} · {} 错误 · {} 警告",
                        if check.overall_ok {
                            "通过"
                        } else {
                            "发现问题"
                        },
                        check.error_count,
                        check.warning_count
                    )),
                    h::div().child(
                        check
                            .checks
                            .into_iter()
                            .map(|check| {
                                h::p().class(if check.ok { "hint" } else { "error" }).child(
                                    format!(
                                        "{} {} {}",
                                        if check.ok { "✓" } else { "!" },
                                        check.message,
                                        check.hint.unwrap_or_default()
                                    ),
                                )
                            })
                            .collect::<Vec<_>>(),
                    ),
                ))
            })
        }),
    ))
}
