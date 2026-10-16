use async_trait::async_trait;
use std::{path::Path, sync::Arc};
use xscc::{
    host::{HostError, RustDeskHost},
    protocol::{DiagnosticSnapshot, HostAddress},
};
use xscc_native_management::{
    manager::{NativeHost, NativeManager},
    private_fs::PrivateDirectory,
};

struct ProbeHost;
#[async_trait]
impl RustDeskHost for ProbeHost {
    async fn diagnostics(&self) -> Result<DiagnosticSnapshot, HostError> {
        Ok(DiagnosticSnapshot::awaiting_integration())
    }
}
#[async_trait]
impl NativeHost for ProbeHost {
    async fn addresses(&self, _: &[HostAddress]) -> Vec<HostAddress> {
        Vec::new()
    }
}
pub fn open(path: &Path) -> Result<Arc<NativeManager>, Box<dyn std::error::Error>> {
    let root = if path.exists() {
        PrivateDirectory::open(path)?
    } else {
        let parent = path.parent().ok_or("state directory needs a parent")?;
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or("invalid state directory name")?;
        PrivateDirectory::create(parent, name)?
    };
    Ok(NativeManager::open(root, Arc::new(ProbeHost))?)
}
