use crate::{
    api,
    models::{self, DriveFile, MsdImage, MsdStatus},
    ui,
};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;
use xcss_web::{html as h, prelude::*};

#[wasm_bindgen(js_namespace = ["globalThis", "kvmBrowser"])]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn upload(path: &str, input_id: &str) -> Result<JsValue, JsValue>;
}

fn encoded(value: &str) -> String {
    js_sys::encode_uri_component(value)
        .as_string()
        .unwrap_or_default()
}
fn drive_path(path: &str) -> String {
    path.split('/')
        .filter(|part| !part.is_empty())
        .map(encoded)
        .collect::<Vec<_>>()
        .join("/")
}

fn action(
    label: &'static str,
    path: String,
    method: &'static str,
    body: Option<Value>,
    error: RwSignal<String>,
) -> impl IntoView {
    let path = StoredValue::new(path);
    let body = StoredValue::new(body);
    let busy = RwSignal::new(false);
    h::button()
        .disabled(move || busy.get())
        .on(xcss_web::ev::click, move |_| {
            let path = path.get_value();
            busy.set(true);
            error.set(String::new());
            xcss_web::task::spawn_local(async move {
                if let Err(value) = api::mutate(&path, method, body.get_value()).await {
                    error.set(value.to_string());
                }
                busy.set(false);
            });
        })
        .child(label)
}

fn upload_button(
    label: &'static str,
    path: String,
    input_id: &'static str,
    progress: RwSignal<u32>,
    error: RwSignal<String>,
) -> impl IntoView {
    let path = StoredValue::new(path);
    let busy = RwSignal::new(false);
    h::button()
        .class("primary")
        .disabled(move || busy.get())
        .on(xcss_web::ev::click, move |_| {
            let path = path.get_value();
            busy.set(true);
            progress.set(0);
            error.set(String::new());
            xcss_web::task::spawn_local(async move {
                match upload(&path, input_id).await {
                    Ok(_) => progress.set(100),
                    Err(value) => error.set(ui::js_error(value)),
                }
                busy.set(false);
            });
        })
        .child(label)
}

fn image(image: MsdImage, error: RwSignal<String>) -> impl IntoView {
    let mount = format!("/api/msd/images/{}/mount", encoded(&image.id));
    let remove = format!("/api/msd/images/{}", encoded(&image.id));
    h::div().class("file-row").child((
        h::span().child(format!("{} · {}", image.name, models::bytes(image.size))),
        h::div().class("actions").child((
            action(
                "挂载光盘",
                mount.clone(),
                "POST",
                Some(json!({"cdrom":true,"read_only":true})),
                error,
            ),
            action(
                "挂载只读磁盘",
                mount.clone(),
                "POST",
                Some(json!({"cdrom":false,"read_only":true})),
                error,
            ),
            action(
                "挂载可写磁盘",
                mount.clone(),
                "POST",
                Some(json!({"cdrom":false,"read_only":false})),
                error,
            ),
            action("卸载", mount, "DELETE", None, error),
            action("删除镜像", remove, "DELETE", None, error),
        )),
    ))
}

fn load_files(path: String, files: RwSignal<Vec<DriveFile>>, error: RwSignal<String>) {
    xcss_web::task::spawn_local(async move {
        match api::get::<Vec<DriveFile>>(&format!("/api/msd/drive/files?path={}", encoded(&path)))
            .await
        {
            Ok(value) => {
                files.set(value);
                error.set(String::new());
            }
            Err(value) => error.set(value.to_string()),
        }
    });
}

pub fn page() -> impl IntoView {
    let error = RwSignal::new(String::new());
    let progress = RwSignal::new(0u32);
    let url = RwSignal::new(String::new());
    let filename = RwSignal::new(String::new());
    let size = RwSignal::new("4096".to_owned());
    let path = RwSignal::new("/".to_owned());
    let directory = RwSignal::new(String::new());
    let files = RwSignal::new(Vec::<DriveFile>::new());
    let (images, images_error) = ui::poll::<Vec<MsdImage>>("/api/msd/images", 3500);
    let (status, status_error) = ui::poll::<MsdStatus>("/api/msd/status", 3500);
    ui::listen("kvm-upload", move |event| {
        if let Ok(value) = js_sys::Reflect::get(event.as_ref(), &JsValue::from_str("detail"))
            && let Some(value) = value.as_f64()
        {
            progress.set(value as u32);
        }
    });
    load_files("/".into(), files, error);
    h::div().child((
        h::h1().child("虚拟媒体"), ui::error(error), ui::error(status_error),
        h::section().class("card").child((
            h::h2().child("USB 存储状态"),
            h::p().class("hint").child(move || status.get().map(|status| format!("{} · 模式 {} · 已挂载 {}/{}{}", if status.available { "可用" } else { "暂不可用" }, status.state.disk_mode, status.state.mounted_count, status.state.slot_capacity, if status.state.usb_reenumerating { " · USB 重新枚举中" } else { "" })).unwrap_or_default()),
            h::div().child(move || status.get().map(|status| status.state.mounted_media.into_iter().map(|media| h::p().class("hint").child(format!("{} · {} · {} · {} · {} · {}", media.id, media.kind, media.name, models::bytes(media.size), if media.cdrom { "光盘" } else { "磁盘" }, if media.read_only { "只读" } else { "可写" }))).collect::<Vec<_>>())),
            h::div().class("actions").child((ui::action("单盘模式", "/api/msd/disk-mode", "PUT", Some(json!({"disk_mode":"single"})), error), ui::action("多盘模式", "/api/msd/disk-mode", "PUT", Some(json!({"disk_mode":"multi"})), error))),
        )),
        h::section().class("card").child((
            h::h2().child("镜像库"), h::p().class("hint").child("上传 ISO 或 IMG 后挂载到被控计算机。删除前请先卸载对应镜像。"),
            ui::error(images_error),
            h::div().class("actions").child((h::input().id("kvm-image-upload").r#type("file").accept(".iso,.img,.bin"), upload_button("上传镜像", "/api/msd/images".into(), "kvm-image-upload", progress, error))),
            h::progress().max("100").value(move || progress.get().to_string()),
            h::div().child(move || images.get().unwrap_or_default().into_iter().map(|value| image(value, error)).collect::<Vec<_>>()),
            h::details().child((h::summary().child("从地址下载镜像"), ui::input("镜像下载 URL", url), ui::input("文件名（可选）", filename), h::button().on(xcss_web::ev::click, move |_| {
                let url = url.get_untracked(); let filename = filename.get_untracked();
                xcss_web::task::spawn_local(async move { if let Err(value) = api::mutate("/api/msd/images/download", "POST", Some(json!({"url":url,"filename":if filename.is_empty() { None } else { Some(filename) }}))).await { error.set(value.to_string()); } });
            }).child("开始下载"))),
        )),
        h::section().class("card").child((
            h::h2().child("虚拟 U 盘"), h::p().class("hint").child("被控计算机连接虚拟 U 盘时不能同时从这里操作其文件系统。初始化会创建新的虚拟磁盘，请先确认介质已卸载。"),
            h::div().class("actions").child((ui::input("容量（MiB）", size), h::button().on(xcss_web::ev::click, move |_| {
                let Ok(size) = size.get_untracked().parse::<u32>() else { error.set("请填写有效磁盘容量。".into()); return; };
                xcss_web::task::spawn_local(async move { if let Err(value) = api::mutate("/api/msd/drive/init", "POST", Some(json!({"size_mb":size}))).await { error.set(value.to_string()); } });
            }).child("初始化虚拟磁盘"), ui::action("连接 U 盘", "/api/msd/drive/mount", "POST", Some(json!({})), error), ui::action("断开 U 盘", "/api/msd/drive/mount", "DELETE", None, error))),
            h::div().class("actions").child((ui::input("当前目录", path), h::button().on(xcss_web::ev::click, move |_| load_files(path.get_untracked(), files, error)).child("读取目录"))),
            h::div().child(move || files.get().into_iter().map(|file| {
                let target = StoredValue::new(file.path.clone());
                let download = format!("/api/msd/drive/files/{}", drive_path(&file.path));
                h::div().class("file-row").child((
                    h::button().on(xcss_web::ev::click, move |_| { if file.is_dir { path.set(target.get_value()); load_files(target.get_value(), files, error); } }).child(format!("{} {} · {}", if file.is_dir { "▸" } else { "·" }, file.name, models::bytes(file.size))),
                    h::div().class("actions").child((h::a().class("button").hidden(file.is_dir).href(api::url(&download)).target("_blank").rel("noopener").child("下载"), action("删除", download, "DELETE", None, error))),
                ))
            }).collect::<Vec<_>>()),
            ui::input("新目录完整路径", directory), h::button().on(xcss_web::ev::click, move |_| {
                let directory = drive_path(&directory.get_untracked());
                xcss_web::task::spawn_local(async move { if let Err(value) = api::mutate(&format!("/api/msd/drive/mkdir/{directory}"), "POST", Some(json!({}))).await { error.set(value.to_string()); } });
            }).child("创建目录"),
            h::input().id("kvm-drive-upload").r#type("file"),
            h::button().on(xcss_web::ev::click, move |_| { let path = path.get_untracked(); xcss_web::task::spawn_local(async move { if let Err(value) = upload(&format!("/api/msd/drive/files?path={}", encoded(&path)), "kvm-drive-upload").await { error.set(ui::js_error(value)); } }); }).child("上传文件到当前目录"),
        )),
    ))
}
