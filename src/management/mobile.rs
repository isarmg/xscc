use super::{attach_ui_runtime, MANAGER};
use async_trait::async_trait;
use hbb_common::{bail, tokio, ResultType};
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};
use xscc::{
    host::{HostError, RustDeskHost},
    protocol::{Backend, DiagnosticSnapshot, ExecutionMode, HostAddress, IntegrationState},
};
use xscc_native_management::{
    manager::{NativeHost, NativeManager},
    private_fs::PrivateDirectory,
};

static STARTED: OnceLock<Result<(), ()>> = OnceLock::new();

struct Host;
#[async_trait]
impl RustDeskHost for Host {
    async fn diagnostics(&self) -> Result<DiagnosticSnapshot, HostError> {
        Ok(DiagnosticSnapshot {
            backend: Backend::Rustdesk,
            execution_mode: ExecutionMode::Embedded,
            integration_state: IntegrationState::Unavailable,
            rustdesk_version: Some(crate::VERSION.into()),
        })
    }
}
#[async_trait]
impl NativeHost for Host {
    async fn addresses(&self, _: &[HostAddress]) -> Vec<HostAddress> {
        Vec::new()
    }
}

// Mobile has no desktop IPC runtime. This application startup boundary owns one
// process-wide loop; the manager library and individual requests never create runtimes.
pub(crate) fn start_runtime(directory: String) -> ResultType<()> {
    let result = STARTED.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("xscc-management".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        let _ = sender.send(Err(()));
                        return;
                    }
                };
                runtime.block_on(async move {
                    attach_ui_runtime();
                    let opened = tokio::task::spawn_blocking(move || {
                        NativeManager::open(
                            PrivateDirectory::create(&PathBuf::from(directory), "xscc-managed")
                                .map_err(|_| ())?,
                            Arc::new(Host),
                        )
                        .map_err(|_| ())
                    })
                    .await
                    .unwrap_or(Err(()));
                    if MANAGER.set(opened).is_err() {
                        let _ = sender.send(Err(()));
                        return;
                    }
                    let result = match MANAGER.get() {
                        Some(Ok(manager)) => manager.resume().await.map_err(|_| ()),
                        _ => Err(()),
                    };
                    let succeeded = result.is_ok();
                    let _ = sender.send(result);
                    if succeeded {
                        std::future::pending::<()>().await;
                    }
                });
            })
            .map_err(|_| ())?;
        receiver
            .recv_timeout(Duration::from_secs(55))
            .map_err(|_| ())?
    });
    match result {
        Ok(()) => Ok(()),
        Err(()) => bail!("Protected native management storage unavailable"),
    }
}
