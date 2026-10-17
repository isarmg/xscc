//! In-process management lifecycle and native session authorization.
pub use xscc_native_management::{binding_store as store, enrollment};

mod transport;
#[cfg(feature = "experimental-quic")]
pub(crate) mod quic;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) use transport::accept;
pub(crate) use transport::{check_controller, connect};
#[cfg(any(target_os = "android", target_os = "ios"))]
mod mobile;
#[cfg(any(target_os = "android", target_os = "ios"))]
pub(crate) use mobile::start_runtime as initialize_mobile;
pub use xscc_native_management::{
    manager::{Reply, Request},
    session::SessionPermit,
};

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use async_trait::async_trait;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use hbb_common::log;
use hbb_common::{bail, tokio, ResultType};
use std::sync::{Arc, OnceLock};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use xscc::{
    host::{HostError, RustDeskHost},
    protocol::{
        Backend, DiagnosticSnapshot, ExecutionMode, HostAddress, InstanceRole, IntegrationState,
    },
};
#[cfg(any(target_os = "android", target_os = "ios"))]
use xscc_native_management::manager::NativeManager;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use xscc_native_management::{
    manager::{NativeHost, NativeManager},
    private_fs::default_directory,
};

static MANAGER: OnceLock<Result<Arc<NativeManager>, ()>> = OnceLock::new();
static UI_RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();

pub(crate) fn attach_ui_runtime() {
    let _ = UI_RUNTIME.set(tokio::runtime::Handle::current());
}

// FRB 1.x runs normal exports on worker threads and does not await Rust futures.
pub(crate) fn request_from_ui(input: String) -> String {
    let reply = |error: &str| serde_json::json!({"error":error}).to_string();
    if input.len() > 8192 {
        return reply("Management request is too large");
    }
    let Ok(request) = serde_json::from_str::<Request>(&input) else {
        return reply("Invalid management request");
    };
    let Some(runtime) = UI_RUNTIME.get() else {
        return reply("Native management runtime is initializing");
    };
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    runtime.spawn(async move {
        let value = match rpc(request).await {
            Ok(value) => serde_json::json!({"value":value}).to_string(),
            Err(error) => serde_json::json!({"error":error.to_string()}).to_string(),
        };
        let _ = sender.send(value);
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(55))
        .unwrap_or_else(|_| reply("Native management request timed out"))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
struct Host;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[async_trait]
impl RustDeskHost for Host {
    async fn diagnostics(&self) -> Result<DiagnosticSnapshot, HostError> {
        Ok(DiagnosticSnapshot {
            backend: Backend::Rustdesk,
            execution_mode: ExecutionMode::Embedded,
            integration_state: if crate::direct_server::direct_server_running() {
                IntegrationState::Ready
            } else {
                IntegrationState::Unavailable
            },
            rustdesk_version: Some(crate::VERSION.to_owned()),
        })
    }
}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[async_trait]
impl NativeHost for Host {
    async fn addresses(&self, configured: &[HostAddress]) -> Vec<HostAddress> {
        if !crate::direct_server::direct_server_running() {
            return Vec::new();
        }
        if !configured.is_empty() {
            return configured.to_vec();
        }
        let port = crate::direct_server::get_direct_port();
        tokio::task::spawn_blocking(move || {
            let Ok(interface) = default_net::get_default_interface() else {
                return Vec::new();
            };
            interface
                .ipv4
                .into_iter()
                .map(|ip| std::net::IpAddr::V4(ip.addr))
                .chain(
                    interface
                        .ipv6
                        .into_iter()
                        .map(|ip| std::net::IpAddr::V6(ip.addr)),
                )
                .filter(|ip| {
                    !ip.is_loopback()
                        && !ip.is_unspecified()
                        && !ip.is_multicast()
                        && !matches!(ip, std::net::IpAddr::V6(ip) if ip.is_unicast_link_local())
                })
                .take(xscc::protocol::MAX_HOST_ADDRESSES)
                .map(|ip| HostAddress {
                    host: ip.to_string(),
                    port,
                })
                .collect()
        })
        .await
        .unwrap_or_default()
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) async fn start() {
    let opened = tokio::task::spawn_blocking(|| {
        NativeManager::open(default_directory().map_err(|_| ())?, Arc::new(Host)).map_err(|_| ())
    })
    .await
    .unwrap_or(Err(()));
    match MANAGER.set(opened) {
        Ok(()) => {
            if let Ok(manager) = owner() {
                if let Err(error) = manager.resume().await {
                    log::error!("Management startup failed: {error}");
                }
            } else {
                log::error!(
                    "Protected management storage unavailable; managed connections are disabled"
                );
            }
        }
        Err(_) => log::error!("Management owner was already initialized"),
    }
}

fn owner() -> ResultType<&'static Arc<NativeManager>> {
    match MANAGER.get() {
        Some(Ok(manager)) => Ok(manager),
        _ => bail!("Native management owner is unavailable"),
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) fn permits_host(permit: Option<&SessionPermit>) -> bool {
    #[cfg(feature = "gpui-host")]
    if permit.is_none() {
        return false;
    }
    if MANAGER.get().is_none() {
        return permit.is_none();
    }
    owner().is_ok_and(|manager| {
        permit.map_or_else(
            || !manager.configured(),
            |permit| manager.permits(permit, InstanceRole::Controlled),
        )
    })
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) async fn shutdown() {
    if let Ok(manager) = owner() {
        if let Err(error) = manager.shutdown().await {
            log::error!("Management shutdown failed: {error}");
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) async fn handle(request: Request) -> Reply {
    match owner() {
        Ok(manager) => manager.handle(request).await,
        Err(error) => Reply::error(error),
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) async fn rpc(request: Request) -> ResultType<serde_json::Value> {
    let timeout = if matches!(
        &request,
        Request::Enroll { .. } | Request::Unbind { .. } | Request::Resume {}
    ) {
        50_000
    } else {
        2000
    };
    let mut connection = crate::ipc::connect(1500, "").await?;
    crate::ipc::verify_managed_owner(&connection)?;
    connection.send(&crate::ipc::Data::Managed(request)).await?;
    match connection.next_timeout(timeout).await? {
        Some(crate::ipc::Data::ManagedReply(reply)) => match (reply.value, reply.error) {
            (Some(value), None) => Ok(value),
            (_, Some(error)) => bail!("{error}"),
            _ => bail!("Invalid management response"),
        },
        _ => bail!("Native management response unavailable"),
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
pub(crate) async fn rpc(request: Request) -> ResultType<serde_json::Value> {
    let reply = owner()?.handle(request).await;
    match (reply.value, reply.error) {
        (Some(value), None) => Ok(value),
        (_, Some(error)) => bail!("{error}"),
        _ => bail!("Invalid management response"),
    }
}

#[cfg(all(test, feature = "gpui-host"))]
mod host_tests {
    #[test]
    fn host_never_permits_a_session_without_management_authorization() {
        assert!(!super::permits_host(None));
    }
}
