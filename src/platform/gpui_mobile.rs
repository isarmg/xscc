//! Attach the existing application-owned manager; this entry creates no runtime or identity.
use hbb_common::{bail, config, ResultType};
use std::{path::Path, sync::Arc};
use xscc_native_management::manager::NativeManager;

pub fn initialize(directory: &Path, manager: Arc<NativeManager>) -> ResultType<()> {
    if !directory.is_absolute() || !directory.is_dir() {
        bail!("A native sandbox directory is required");
    }
    let directory = directory
        .to_str()
        .ok_or_else(|| hbb_common::anyhow::anyhow!("Invalid sandbox path"))?;
    crate::management::attach_gpui_owner(manager)?;
    *config::APP_DIR.write().unwrap() = directory.to_owned();
    Ok(())
}
