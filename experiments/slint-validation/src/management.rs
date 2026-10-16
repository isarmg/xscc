use crate::ProbeWindow;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::sync::Arc;
use xscc_native_management::manager::{NativeManager, Request};
#[path = "../../shared/management.rs"]
mod shared;
pub use shared::open;
pub fn attach(ui: &ProbeWindow, manager: Arc<NativeManager>, runtime: &tokio::runtime::Runtime) {
    let weak = ui.as_weak();
    let active = manager.clone();
    runtime.spawn(async move {
        if let Err(error) = active.resume().await {
            let message = error.to_string();
            if weak
                .upgrade_in_event_loop(move |ui| ui.set_native_summary(message.into()))
                .is_err()
            {
                return;
            }
        }
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            interval.tick().await;
            let status = active.status();
            if weak
                .upgrade_in_event_loop(move |ui| {
                    ui.set_native_summary(
                        format!(
                            "Isolated management: {} · {:?} · {} allowed peers",
                            status.phase,
                            status.role,
                            status.peers.len()
                        )
                        .into(),
                    );
                    ui.set_peers(ModelRc::new(VecModel::from(
                        status
                            .peers
                            .into_iter()
                            .map(|peer| {
                                SharedString::from(format!(
                                    "{} · {:?} · {}",
                                    peer.name,
                                    peer.role,
                                    if peer.online { "online" } else { "offline" }
                                ))
                            })
                            .collect::<Vec<_>>(),
                    )));
                })
                .is_err()
            {
                break;
            }
        }
    });
    let handle = runtime.handle().clone();
    let weak = ui.as_weak();
    let active = manager.clone();
    ui.on_enroll(move |server, code| {
        let manager = active.clone();
        let weak = weak.clone();
        if let Some(ui) = weak.upgrade() {
            ui.set_enrollment_code("".into());
        }
        handle.spawn(async move {
            let reply = manager
                .handle(Request::Enroll {
                    server: server.to_string(),
                    authorization_code: code.to_string(),
                    addresses: Vec::new(),
                })
                .await;
            let text = reply
                .error
                .unwrap_or_else(|| "Enrollment request completed; awaiting policy".into());
            if weak
                .upgrade_in_event_loop(move |ui| ui.set_native_summary(text.into()))
                .is_err()
            {
                eprintln!("management result arrived after validation window closed");
            }
        });
    });
    let handle = runtime.handle().clone();
    let weak = ui.as_weak();
    ui.on_retry(move || {
        let manager = manager.clone();
        let weak = weak.clone();
        handle.spawn(async move {
            if let Err(error) = manager.resume().await
                && weak
                    .upgrade_in_event_loop(move |ui| {
                        ui.set_native_summary(error.to_string().into())
                    })
                    .is_err()
            {
                eprintln!("management result arrived after validation window closed");
            }
        });
    });
}
