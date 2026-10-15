//! RustDesk implements this interface inside its own process.
use async_trait::async_trait;
use xscs_protocol::DiagnosticSnapshot;

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("RustDesk host observation is unavailable")]
pub struct HostError;

#[async_trait]
pub trait RustDeskHost: Send + Sync + 'static {
    /// Observe the running host. Never infer readiness from a saved configuration.
    async fn diagnostics(&self) -> Result<DiagnosticSnapshot, HostError>;
}
