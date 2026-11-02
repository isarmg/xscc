use crate::{
    api,
    models::{DeviceCatalog, Platform, SetupStatus},
    ui,
};
use serde_json::{Map, Value};
use xcss_web::{html as h, prelude::*};

#[derive(Clone, Copy)]
enum Kind {
    Text,
    Secret,
    Number,
    Toggle,
    Choice(&'static [(&'static str, &'static str)]),
    Device(DeviceKind),
    HidBackend,
}

#[derive(Clone, Copy)]
enum DeviceKind {
    Video,
    Audio,
    Serial,
    Monitor,
}

#[derive(Clone, Copy)]
struct Field {
    path: &'static str,
    label: &'static str,
    kind: Kind,
}
const fn field(path: &'static str, label: &'static str, kind: Kind) -> Field {
    Field { path, label, kind }
}
use Kind::{Choice, Device, Number, Secret, Text, Toggle};

#[derive(Clone, Copy)]
struct Group {
    name: &'static str,
    path: &'static str,
    hint: &'static str,
    fields: &'static [Field],
}

const GROUPS: &[Group] = &[
    Group {
        name: "视频采集",
        path: "/api/config/video",
        hint: "选择当前实例使用的显示器或视频采集设备。源跟随型采集卡自动决定格式、分辨率和帧率。",
        fields: &[
            field("device", "显示器 / 视频采集设备", Device(DeviceKind::Video)),
            field(
                "format",
                "采集格式",
                Choice(&[
                    ("MJPEG", "MJPEG"),
                    ("YUYV", "YUYV"),
                    ("NV12", "NV12"),
                    ("RGB24", "RGB24"),
                    ("BGR24", "BGR24"),
                    ("JPEG", "JPEG"),
                    ("NV21", "NV21"),
                    ("YUV420", "YUV420"),
                ]),
            ),
            field("width", "宽度（像素）", Number),
            field("height", "高度（像素）", Number),
            field("fps", "采集帧率", Number),
            field("quality", "JPEG 质量（1–100）", Number),
        ],
    },
    Group {
        name: "视频传输",
        path: "/api/config/stream",
        hint: "WebRTC 通过浏览器原生加密通道传输音视频。STUN / TURN 留空使用现有部署默认值。",
        fields: &[
            field(
                "mode",
                "传输方式",
                Choice(&[("webrtc", "WebRTC"), ("mjpeg", "MJPEG")]),
            ),
            field(
                "encoder",
                "编码后端",
                Choice(&[
                    ("auto", "自动"),
                    ("software", "软件"),
                    ("vaapi", "VAAPI"),
                    ("nvenc", "NVENC"),
                    ("qsv", "Intel QSV"),
                    ("amf", "AMD AMF"),
                    ("rkmpp", "Rockchip MPP"),
                    ("v4l2m2m", "V4L2 M2M"),
                ]),
            ),
            field("stun_server", "STUN 服务地址", Text),
            field("turn_server", "TURN 服务地址", Text),
            field("turn_username", "TURN 用户名", Text),
            field("turn_password", "TURN 密码（留空且不修改则保留）", Secret),
        ],
    },
    Group {
        name: "键鼠 / HID",
        path: "/api/config/hid",
        hint: "为当前实例选择输入后端。Windows 桌面输入作用于登录用户的共享桌面，键盘和鼠标可分别启停；显示器映射不隔离多个 Windows 用户会话。修改 OTG 配置可能重新枚举 USB 设备。",
        fields: &[
            field("backend", "HID 后端", Kind::HidBackend),
            field(
                "windows_monitor",
                "Windows 输入映射显示器",
                Device(DeviceKind::Monitor),
            ),
            field("windows_keyboard", "Windows 键盘输入", Toggle),
            field("windows_mouse", "Windows 鼠标输入", Toggle),
            field("mouse_absolute", "绝对坐标鼠标", Toggle),
            field("mouse_macos_drag", "macOS 拖动兼容", Toggle),
            field("ch9329_port", "CH9329 串口路径", Device(DeviceKind::Serial)),
            field("ch9329_baudrate", "串口波特率", Number),
            field("ch9329_hybrid_mouse", "CH9329 混合鼠标", Toggle),
            field("ch9329_descriptor.vendor_id", "CH9329 厂商 ID", Number),
            field("ch9329_descriptor.product_id", "CH9329 产品 ID", Number),
            field("ch9329_descriptor.manufacturer", "CH9329 制造商", Text),
            field("ch9329_descriptor.product", "CH9329 产品名称", Text),
            field("ch9329_descriptor.serial_number", "CH9329 序列号", Text),
            field("otg_udc", "OTG UDC 名称", Text),
            field("otg_keyboard_leds", "读取键盘 LED", Toggle),
            field(
                "otg_profile",
                "OTG 键鼠配置",
                Choice(&[
                    ("full", "完整"),
                    ("full_no_consumer", "完整键鼠，无多媒体键"),
                    ("legacy_keyboard", "传统键盘"),
                    ("legacy_mouse_relative", "传统相对鼠标"),
                    ("custom", "自定义"),
                ]),
            ),
            field("otg_functions.keyboard", "OTG 键盘", Toggle),
            field("otg_functions.mouse_relative", "OTG 相对鼠标", Toggle),
            field("otg_functions.mouse_absolute", "OTG 绝对鼠标", Toggle),
            field("otg_functions.consumer", "OTG 多媒体键", Toggle),
            field("otg_descriptor.vendor_id", "OTG 厂商 ID", Number),
            field("otg_descriptor.product_id", "OTG 产品 ID", Number),
            field("otg_descriptor.manufacturer", "OTG 制造商", Text),
            field("otg_descriptor.product", "OTG 产品名称", Text),
            field("otg_descriptor.serial_number", "OTG 序列号", Text),
            field("bluetooth.adapter", "蓝牙适配器", Text),
            field("bluetooth.name", "蓝牙设备名称", Text),
        ],
    },
    Group {
        name: "电源 / ATX",
        path: "/api/config/atx",
        hint: "选择实际连接的驱动和引脚。短按、长按及复位按钮位于控制台。",
        fields: &[
            field("enabled", "启用 ATX", Toggle),
            field(
                "driver",
                "驱动",
                Choice(&[
                    ("none", "关闭"),
                    ("gpio", "GPIO"),
                    ("usbrelay", "USB 继电器"),
                    ("serial", "串口"),
                ]),
            ),
            field("device", "驱动设备", Text),
            field("baud_rate", "波特率", Number),
            field("wol_interface", "网络唤醒网卡（空为自动）", Text),
            field("power.enabled", "电源按键输出", Toggle),
            field("power.device", "电源 GPIO 设备", Text),
            field("power.pin", "电源引脚", Number),
            field(
                "power.active_level",
                "电源有效电平",
                Choice(&[("high", "高"), ("low", "低")]),
            ),
            field("reset.enabled", "复位按键输出", Toggle),
            field("reset.device", "复位 GPIO 设备", Text),
            field("reset.pin", "复位引脚", Number),
            field(
                "reset.active_level",
                "复位有效电平",
                Choice(&[("high", "高"), ("low", "低")]),
            ),
            field("led.enabled", "电源 LED 感应", Toggle),
            field("led.device", "LED GPIO 设备", Text),
            field("led.pin", "LED 引脚", Number),
            field(
                "led.active_level",
                "LED 有效电平",
                Choice(&[("high", "高"), ("low", "低")]),
            ),
            field("hdd.enabled", "磁盘 LED 感应", Toggle),
            field("hdd.device", "磁盘 LED 设备", Text),
            field("hdd.pin", "磁盘 LED 引脚", Number),
            field(
                "hdd.active_level",
                "磁盘 LED 有效电平",
                Choice(&[("high", "高"), ("low", "低")]),
            ),
        ],
    },
    Group {
        name: "虚拟媒体",
        path: "/api/config/msd",
        hint: "ISO/IMG 上传、挂载和虚拟磁盘位于媒体页。修改存储路径前先卸载介质。",
        fields: &[
            field("enabled", "启用 USB 存储", Toggle),
            field("msd_dir", "本机媒体存储目录", Text),
            field("flash_inquiry_string", "闪存设备名称", Text),
            field("cdrom_inquiry_string", "光驱设备名称", Text),
        ],
    },
    Group {
        name: "音频采集",
        path: "/api/config/audio",
        hint: "选择当前实例的音频采集源。Linux 使用 ALSA，Windows 使用 WASAPI；浏览器播放需要先操作音量控件。",
        fields: &[
            field("enabled", "启用音频采集", Toggle),
            field("device", "音频设备名称", Device(DeviceKind::Audio)),
            field(
                "quality",
                "音频质量",
                Choice(&[("voice", "语音"), ("balanced", "均衡"), ("high", "高质量")]),
            ),
        ],
    },
    Group {
        name: "麦克风转发",
        path: "/api/config/uac",
        hint: "将浏览器麦克风送到被控计算机的 USB Audio 设备。浏览器需要 HTTPS 和麦克风权限。",
        fields: &[
            field("enabled", "启用 USB Audio", Toggle),
            field("sample_rate", "采样率", Number),
            field("channels", "通道数", Number),
        ],
    },
    Group {
        name: "USB 网络",
        path: "/api/config/otg-network",
        hint: "变更可能使 USB 设备重新枚举。自定义 MAC 必须为本地管理单播地址。",
        fields: &[
            field("enabled", "启用 USB 网络", Toggle),
            field(
                "driver_mode",
                "驱动模式",
                Choice(&[("ncm", "NCM"), ("ecm", "ECM"), ("rndis", "RNDIS")]),
            ),
            field("bridge_interface", "桥接网络接口", Text),
            field("host_mac", "主机 MAC（空为自动）", Text),
            field("device_mac", "设备 MAC（空为自动）", Text),
        ],
    },
    Group {
        name: "原生 XSCC",
        path: "/api/config/rustdesk",
        hint: "原生 XSCC 从授权列表连接此设备。连接先完成 XSCS 身份证明，保存时固定启用受管直连。",
        fields: &[
            field("direct_access_port", "原生监听端口", Number),
            field(
                "codec",
                "原生视频编码",
                Choice(&[("h264", "H.264"), ("h265", "H.265")]),
            ),
        ],
    },
    Group {
        name: "Computer Use",
        path: "/api/config/computer-use",
        hint: "自动操作会将画面发送给指定模型服务。API 密钥仅提交到本机设备，留空不修改现有密钥。",
        fields: &[
            field("enabled", "启用自动操作", Toggle),
            field("base_url", "模型服务 API 地址", Text),
            field("model", "模型名称", Text),
            field("api_key", "API 密钥", Secret),
            field("clear_api_key", "删除保存的 API 密钥", Toggle),
        ],
    },
];

#[derive(Clone, Debug, PartialEq)]
struct Input {
    text: String,
    toggle: bool,
    changed: bool,
}

fn at<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(value, |value, part| value.get(part))
}

fn read_fields(group: Group, response: &Value) -> Vec<Input> {
    group
        .fields
        .iter()
        .map(|field| {
            let value = at(response, field.path);
            let text = match field.kind {
                Secret => String::new(),
                Number => value
                    .and_then(Value::as_u64)
                    .map(|number| number.to_string())
                    .unwrap_or_default(),
                Text | Choice(_) | Device(_) | Kind::HidBackend => {
                    value.and_then(Value::as_str).unwrap_or_default().to_owned()
                }
                Toggle => String::new(),
            };
            Input {
                text,
                toggle: value.and_then(Value::as_bool).unwrap_or_default(),
                changed: false,
            }
        })
        .collect()
}

fn insert(object: &mut Map<String, Value>, path: &str, value: Value) {
    if let Some((first, rest)) = path.split_once('.') {
        let nested = object
            .entry(first.to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(map) = nested {
            insert(map, rest, value);
        }
    } else {
        object.insert(path.to_owned(), value);
    }
}

fn payload(group: Group, fields: &[Input]) -> Result<Value, String> {
    if group.path == "/api/config/audio" {
        let enabled = group
            .fields
            .iter()
            .zip(fields)
            .any(|(field, input)| field.path == "enabled" && input.toggle);
        let selected = group
            .fields
            .iter()
            .zip(fields)
            .find(|(field, _)| field.path == "device")
            .is_some_and(|(_, input)| !input.text.trim().is_empty());
        if enabled && !selected {
            return Err("请先为当前实例选择音频设备，再启用音频采集。".into());
        }
    }
    let mut object = Map::new();
    for (field, input) in group.fields.iter().zip(fields) {
        if !input.changed {
            continue;
        }
        if group.path == "/api/config/video" && field.path == "device" && input.text.is_empty() {
            object.insert("clear_device".into(), Value::Bool(true));
            continue;
        }
        let value = match field.kind {
            Toggle => Value::Bool(input.toggle),
            Number => {
                let text = input.text.trim();
                let number = if let Some(text) = text.strip_prefix("0x") {
                    u64::from_str_radix(text, 16)
                } else {
                    text.parse::<u64>()
                };
                Value::from(number.map_err(|_| format!("{}需要填写非负整数。", field.label))?)
            }
            Text | Secret | Choice(_) | Device(_) | Kind::HidBackend => {
                Value::String(input.text.clone())
            }
        };
        insert(&mut object, field.path, value);
    }
    if group.path == "/api/config/rustdesk" {
        object.insert("enabled".into(), Value::Bool(true));
        object.insert("mode".into(), Value::String("direct_ip".into()));
    }
    Ok(Value::Object(object))
}

fn device_options(kind: DeviceKind, devices: &DeviceCatalog) -> Vec<(String, String)> {
    match kind {
        DeviceKind::Video => devices
            .video
            .iter()
            .map(|device| {
                (
                    device.path.clone(),
                    format!("{} · {}", device.name, device.path),
                )
            })
            .collect(),
        DeviceKind::Audio => devices
            .audio
            .iter()
            .map(|device| (device.name.clone(), device.description.clone()))
            .collect(),
        DeviceKind::Serial => devices
            .serial
            .iter()
            .map(|device| (device.path.clone(), device.name.clone()))
            .collect(),
        DeviceKind::Monitor => devices
            .video
            .iter()
            .filter(|device| device.path.starts_with("desktop:"))
            .map(|device| (device.path.clone(), device.name.clone()))
            .collect(),
    }
}

fn editor(
    field: Field,
    index: usize,
    values: RwSignal<Vec<Input>>,
    catalog: RwSignal<Option<DeviceCatalog>>,
    platform: RwSignal<Platform>,
) -> AnyView {
    let read = move || {
        values.with(|values| {
            values
                .get(index)
                .map(|value| value.text.clone())
                .unwrap_or_default()
        })
    };
    let input = move |text: String| {
        values.update(|values| {
            if let Some(value) = values.get_mut(index) {
                value.text = text;
                value.changed = true;
            }
        })
    };
    let control = match field.kind {
        Device(kind) => View::new(
            h::select()
                .attr("aria-label", field.label)
                .prop("value", read)
                .on(xcss_web::ev::change, move |event| {
                    input(event_target_value(&event))
                })
                .child(move || {
                    let current = read();
                    let mut choices = catalog.with(|devices| {
                        devices
                            .as_ref()
                            .map(|devices| device_options(kind, devices))
                            .unwrap_or_default()
                    });
                    if !choices.iter().any(|(value, _)| value == &current) && !current.is_empty() {
                        choices.insert(
                            0,
                            (
                                current.clone(),
                                format!("{current}（当前设置，设备未发现）"),
                            ),
                        );
                    }
                    choices.insert(0, (String::new(), "未分配".into()));
                    choices
                        .into_iter()
                        .map(|(value, label)| {
                            h::option()
                                .prop("selected", value == current)
                                .value(value)
                                .child(label)
                        })
                        .collect::<Vec<_>>()
                }),
        )
        .into_any(),
        Kind::HidBackend => View::new(
            h::select()
                .attr("aria-label", field.label)
                .prop("value", read)
                .on(xcss_web::ev::change, move |event| {
                    input(event_target_value(&event))
                })
                .child(move || {
                    let current = read();
                    [
                        ("none", "关闭"),
                        ("otg", "USB OTG"),
                        ("ch9329", "CH9329 串口"),
                        ("bluetooth", "蓝牙"),
                        ("windows", "Windows 桌面键鼠"),
                    ]
                    .into_iter()
                    .filter(|(value, _)| platform.with(|platform| platform.supports_hid(value)))
                    .map(|(value, label)| {
                        h::option()
                            .prop("selected", value == current)
                            .value(value)
                            .child(label)
                    })
                    .collect::<Vec<_>>()
                }),
        )
        .into_any(),
        Toggle => View::new(
            h::input()
                .r#type("checkbox")
                .prop("checked", move || {
                    values.with(|values| values.get(index).is_some_and(|value| value.toggle))
                })
                .on(xcss_web::ev::change, move |event| {
                    let checked = event_target_checked(&event);
                    values.update(|values| {
                        if let Some(value) = values.get_mut(index) {
                            value.toggle = checked;
                            value.changed = true;
                        }
                    });
                }),
        )
        .into_any(),
        Choice(options) => View::new(
            h::select()
                .attr("aria-label", field.label)
                .prop("value", read)
                .on(xcss_web::ev::change, move |event| {
                    input(event_target_value(&event))
                })
                .child(
                    options
                        .iter()
                        .map(|(value, label)| h::option().value(*value).child(*label))
                        .collect::<Vec<_>>(),
                ),
        )
        .into_any(),
        Text | Secret | Number => View::new(
            h::input()
                .r#type(if matches!(field.kind, Secret) {
                    "password"
                } else {
                    "text"
                })
                .autocomplete("off")
                .prop("value", read)
                .on(xcss_web::ev::input, move |event| {
                    input(event_target_value(&event))
                }),
        )
        .into_any(),
    };
    View::new(
        h::label()
            .class("field")
            .hidden(move || {
                field.path.starts_with("windows_")
                    && platform.with(|platform| !platform.supports_hid("windows"))
            })
            .child((h::span().child(field.label), control)),
    )
    .into_any()
}

fn form(
    group: Group,
    catalog: RwSignal<Option<DeviceCatalog>>,
    platform: RwSignal<Platform>,
) -> impl IntoView {
    let values = RwSignal::new(Vec::<Input>::new());
    let error = RwSignal::new(String::new());
    let loading = RwSignal::new(true);
    let busy = RwSignal::new(false);
    let saved = RwSignal::new(false);
    xcss_web::task::spawn_local(async move {
        match api::get::<Value>(group.path).await {
            Ok(value) => {
                values.set(read_fields(group, &value));
            }
            Err(value) => error.set(value.to_string()),
        }
        loading.set(false);
    });
    h::section().class("card").child((
        h::h2().child(group.name),
        h::p().class("hint").child(group.hint),
        ui::error(error),
        h::p()
            .class("notice")
            .hidden(move || !saved.get())
            .child("设置已保存。"),
        h::p()
            .class("hint")
            .hidden(move || !loading.get())
            .child("正在读取设置…"),
        h::form()
            .on(xcss_web::ev::submit, move |event| {
                event.prevent_default();
                if busy.get_untracked() {
                    return;
                }
                let value = match payload(group, &values.get_untracked()) {
                    Ok(value) => value,
                    Err(value) => {
                        error.set(value);
                        return;
                    }
                };
                busy.set(true);
                saved.set(false);
                error.set(String::new());
                xcss_web::task::spawn_local(async move {
                    match api::mutate(group.path, "PATCH", Some(value)).await {
                        Ok(()) => saved.set(true),
                        Err(value) => error.set(value.to_string()),
                    }
                    busy.set(false);
                });
            })
            .child((
                h::div().class("fields").child(
                    group
                        .fields
                        .iter()
                        .copied()
                        .enumerate()
                        .map(|(index, field)| editor(field, index, values, catalog, platform))
                        .collect::<Vec<_>>(),
                ),
                h::button()
                    .class("primary")
                    .r#type("submit")
                    .disabled(move || busy.get() || loading.get() || values.with(Vec::is_empty))
                    .child("保存设置"),
            )),
    ))
}

pub fn page() -> impl IntoView {
    let selected = RwSignal::new(0usize);
    let catalog = RwSignal::new(None::<DeviceCatalog>);
    let platform = RwSignal::new(Platform::default());
    let load_error = RwSignal::new(String::new());
    xcss_web::task::spawn_local(async move {
        match api::get::<DeviceCatalog>("/api/devices").await {
            Ok(value) => catalog.set(Some(value)),
            Err(value) => load_error.set(value.to_string()),
        }
        match api::get::<SetupStatus>("/api/setup").await {
            Ok(value) => platform.set(value.platform),
            Err(value) => load_error.set(value.to_string()),
        }
    });
    h::div().child((
        h::h1().child("设备设置"),
        h::p()
            .class("hint")
            .child(format!("当前实例：{}。设备选择和设置只应用于本实例。XSCS 服务地址和设备身份由本机管理命令维护。", api::instance_id())),
        ui::error(load_error),
        h::div().class("actions settings-nav").child(
            GROUPS
                .iter()
                .enumerate()
                .map(|(index, group)| {
                    h::button()
                        .hidden(move || platform.with(|platform| match group.path {
                            "/api/config/msd" => !platform.supports_media(),
                            "/api/config/uac" | "/api/config/otg-network" => !platform.supports_otg(),
                            _ => false,
                        }))
                        .on(xcss_web::ev::click, move |_| selected.set(index))
                        .child(group.name)
                })
                .collect::<Vec<_>>(),
        ),
        h::div().child(move || form(GROUPS[selected.get()], catalog, platform)),
        devices(),
        native_addresses(),
        h::div().hidden(move || platform.with(|platform| platform.mode == "windows")).child(crate::hardware::panel()),
    ))
}

fn devices() -> impl IntoView {
    let catalog = RwSignal::new(None::<crate::models::DeviceCatalog>);
    let error = RwSignal::new(String::new());
    xcss_web::task::spawn_local(async move {
        match api::get("/api/devices").await {
            Ok(value) => catalog.set(Some(value)),
            Err(value) => error.set(value.to_string()),
        }
    });
    h::section().class("card").child((
        h::h2().child("本机设备目录"),
        ui::error(error),
        h::div().child(move || {
            catalog.get().map(|catalog| {
                h::div().child((
                    h::h3().child("视频采集"),
                    h::div().child(
                        catalog
                            .video
                            .into_iter()
                            .map(|device| {
                                h::p()
                                    .class("hint")
                                    .child(format!("{}  {}", device.name, device.path))
                            })
                            .collect::<Vec<_>>(),
                    ),
                    h::h3().child("串口"),
                    h::div().child(
                        catalog
                            .serial
                            .into_iter()
                            .map(|device| {
                                h::p()
                                    .class("hint")
                                    .child(format!("{}  {}", device.name, device.path))
                            })
                            .collect::<Vec<_>>(),
                    ),
                ))
            })
        }),
    ))
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Address {
    host: String,
    port: u16,
}
#[derive(serde::Deserialize)]
struct DeviceStatus {
    addresses: Vec<Address>,
}
#[derive(Clone)]
struct AddressInput {
    host: String,
    port: String,
}

fn native_addresses() -> impl IntoView {
    let inputs = RwSignal::new(vec![
        AddressInput {
            host: String::new(),
            port: String::new()
        };
        4
    ]);
    let error = RwSignal::new(String::new());
    let loading = RwSignal::new(true);
    let busy = RwSignal::new(false);
    let saved = RwSignal::new(false);
    xcss_web::task::spawn_local(async move {
        match api::get::<DeviceStatus>("/api/management/device").await {
            Ok(status) => inputs.update(|inputs| {
                for (input, address) in inputs.iter_mut().zip(status.addresses) {
                    input.host = address.host;
                    input.port = address.port.to_string();
                }
            }),
            Err(value) => {
                let _ = error.try_set(value.to_string());
            }
        }
        let _ = loading.try_set(false);
    });
    h::section().class("card").child((
        h::h2().child("向 XSCS 报告的原生可达地址"),
        h::p().class("hint").child("最多四个 IP 或域名及端口。NAT 映射的外部端口可以与本机监听端口不同。全部留空时，由设备自动发现本机地址。"),
        ui::error(error), h::p().class("notice").hidden(move || !saved.get()).child("可达地址已保存，将由管理连接同步到控制端。"),
        h::form().on(xcss_web::ev::submit, move |event| {
            event.prevent_default();
            let mut addresses = Vec::new();
            for input in inputs.get_untracked() {
                let host = input.host.trim();
                if host.is_empty() { continue; }
                let Ok(port) = input.port.trim().parse::<u16>() else { error.set("每个地址都需要有效端口（1–65535）。".into()); return; };
                if port == 0 { error.set("端口不能为零。".into()); return; }
                addresses.push(Address { host: host.to_owned(), port });
            }
            busy.set(true); saved.set(false); error.set(String::new());
            xcss_web::task::spawn_local(async move {
                match api::mutate("/api/management/addresses", "PATCH", Some(serde_json::json!({"addresses":addresses}))).await {
                    Ok(()) => { let _ = saved.try_set(true); }, Err(value) => { let _ = error.try_set(value.to_string()); },
                }
                let _ = busy.try_set(false);
            });
        }).child((
            h::div().class("fields").child((0usize..4).map(|index| h::div().child((
                h::label().class("field").child((h::span().child(format!("地址 {}", index+1)), h::input().placeholder("IP 或域名").prop("value", move || inputs.with(|inputs| inputs[index].host.clone())).on(xcss_web::ev::input, move |event| inputs.update(|inputs| inputs[index].host = event_target_value(&event))))),
                h::label().class("field").child((h::span().child("外部可达端口"), h::input().r#type("number").min("1").max("65535").prop("value", move || inputs.with(|inputs| inputs[index].port.clone())).on(xcss_web::ev::input, move |event| inputs.update(|inputs| inputs[index].port = event_target_value(&event))))),
            ))).collect::<Vec<_>>()),
            h::button().class("primary").r#type("submit").disabled(move || loading.get() || busy.get()).child("保存上报地址"),
        )),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_submit_only_changed_fields_and_keep_nested_types() {
        let group = GROUPS[2];
        let mut fields = read_fields(
            group,
            &serde_json::json!({"backend":"otg","otg_descriptor":{"vendor_id":4660},"mouse_absolute":true}),
        );
        assert_eq!(payload(group, &fields).unwrap(), serde_json::json!({}));
        let index = group
            .fields
            .iter()
            .position(|field| field.path == "otg_descriptor.vendor_id")
            .unwrap();
        fields[index].text = "0x1234".into();
        fields[index].changed = true;
        assert_eq!(
            payload(group, &fields).unwrap(),
            serde_json::json!({"otg_descriptor":{"vendor_id":4660}})
        );
        fields[index].text = "bad".into();
        assert!(payload(group, &fields).is_err());
    }

    #[test]
    fn windows_input_fields_preserve_independent_keyboard_and_mouse_choices() {
        let group = GROUPS[2];
        let mut fields = read_fields(
            group,
            &serde_json::json!({
                "backend":"windows", "windows_monitor":"desktop:DISPLAY2",
                "windows_keyboard":true, "windows_mouse":true,
            }),
        );
        let keyboard = group
            .fields
            .iter()
            .position(|field| field.path == "windows_keyboard")
            .unwrap();
        fields[keyboard].toggle = false;
        fields[keyboard].changed = true;
        assert_eq!(
            payload(group, &fields).unwrap(),
            serde_json::json!({"windows_keyboard":false})
        );
    }

    #[test]
    fn input_mapping_lists_only_desktop_monitors_and_keeps_device_ids() {
        let catalog: DeviceCatalog = serde_json::from_value(serde_json::json!({
            "video":[{"path":"desktop:DISPLAY2","name":"Display 2"},{"path":"dshow:camera","name":"USB HDMI"}],
            "audio":[{"name":"wasapi:microphone","description":"Microphone"}],
            "serial":[], "udc":[],
        })).unwrap();
        assert_eq!(
            device_options(DeviceKind::Monitor, &catalog),
            vec![("desktop:DISPLAY2".into(), "Display 2".into())]
        );
        assert_eq!(
            device_options(DeviceKind::Audio, &catalog),
            vec![("wasapi:microphone".into(), "Microphone".into())]
        );
    }

    #[test]
    fn enabling_audio_requires_an_explicit_instance_device() {
        let group = GROUPS
            .iter()
            .find(|group| group.path == "/api/config/audio")
            .copied()
            .unwrap();
        let fields = read_fields(
            group,
            &serde_json::json!({"enabled":true,"device":"","quality":"balanced"}),
        );
        assert!(payload(group, &fields).is_err());
        let fields = read_fields(
            group,
            &serde_json::json!({"enabled":true,"device":"wasapi:monitor-2","quality":"balanced"}),
        );
        assert_eq!(payload(group, &fields).unwrap(), serde_json::json!({}));
    }

    #[test]
    fn unassigning_video_explicitly_releases_the_instance_source() {
        let group = GROUPS[0];
        let mut fields = read_fields(group, &serde_json::json!({"device":"desktop:DISPLAY2"}));
        let device = group
            .fields
            .iter()
            .position(|field| field.path == "device")
            .unwrap();
        fields[device].text.clear();
        fields[device].changed = true;
        assert_eq!(
            payload(group, &fields).unwrap(),
            serde_json::json!({"clear_device":true})
        );
    }
}
