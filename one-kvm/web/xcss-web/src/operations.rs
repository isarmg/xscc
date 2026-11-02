use crate::{
    models::{self, Info, Session},
    ui,
};
use serde::Deserialize;
use xcss_web::{html as h, prelude::*};

#[derive(Clone, Deserialize)]
struct UpdateStatus {
    phase: String,
    progress: f64,
    current_version: String,
    target_version: Option<String>,
    message: Option<String>,
    last_error: Option<String>,
}

fn metric(label: &'static str, value: String) -> impl IntoView {
    h::div()
        .class("metric")
        .child((h::span().child(label), h::span().child(value)))
}

pub fn page(session: RwSignal<Option<Session>>) -> impl IntoView {
    let (info, error) = ui::poll::<Info>("/api/info", 5000);
    let (update, update_error) = ui::poll::<UpdateStatus>("/api/update/status", 5000);
    h::div().child((
        h::h1().child("运行状态"),
        ui::error(error),
        h::section().class("card").child((
            h::h2().child("XSCS 授权身份"),
            h::div().child(move || {
                session.get().map(|session| {
                    h::div().child((
                        metric("控制端实例", session.controller.device_id),
                        metric("控制端安装", session.controller.installation_id),
                        metric("被控端实例", session.controlled.device_id),
                        metric("被控端安装", session.controlled.installation_id),
                        metric("管理服务身份", session.controlled.manager_id),
                        metric("控制端管理身份", session.controller.manager_id),
                        metric("会话最长有效期", format!("{} 秒", session.expires_in_secs)),
                    ))
                })
            }),
        )),
        h::section().class("card").child((
            h::h2().child("设备资源"),
            h::div().child(move || {
                info.get().map(|info| {
                    h::div().child((
                        metric("One-KVM 版本", info.version),
                        h::div().child(info.device_info.map(|device| {
                            h::div().child((
                                metric("主机名", device.hostname),
                                metric("处理器", device.cpu_model),
                                metric("处理器使用率", format!("{:.1}%", device.cpu_usage)),
                                metric(
                                    "内存",
                                    format!(
                                        "{} / {}",
                                        models::bytes(device.memory_used),
                                        models::bytes(device.memory_total)
                                    ),
                                ),
                            ))
                        })),
                    ))
                })
            }),
            h::p()
                .class("hint")
                .child("服务重启、Web 监听和系统维护由本机管理员完成。实例授权只允许操作本实例的设备和会话。"),
        )),
        h::section().class("card").child((
            h::h2().child("应用更新"),
            ui::error(update_error),
            h::p().class("hint").child(move || {
                update
                    .get()
                    .map(|update| {
                        format!(
                            "{} · {:.0}% · 当前 {} · 目标 {} · {}{}",
                            update.phase,
                            update.progress,
                            update.current_version,
                            update.target_version.unwrap_or_default(),
                            update.message.unwrap_or_default(),
                            update.last_error.unwrap_or_default()
                        )
                    })
                    .unwrap_or_default()
            }),
            h::p()
                .class("hint")
                .child("本定制产品使用经审核的离线安装包更新；上游自动更新入口已关闭。"),
        )),
    ))
}
