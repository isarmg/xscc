//! Instance management contract. Media and hardware mutations stay in the host.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
mod access;
pub use access::*;

pub const PROTOCOL: &str = "xscs-management/2";
pub const TASK_PROTOCOL: &str = "xscs-task/2";
pub const WEBSOCKET_SUBPROTOCOL: &str = "xscs-management.v2";
pub const LIVE_HEALTH_PATH: &str = "/healthz";
pub const READY_HEALTH_PATH: &str = "/readyz";
pub const CLIENT_NAMESPACE: &str = "/xscc";
pub const ENROLL_PATH: &str = "/xscc/v2/enroll";
pub const PAIRING_PATH: &str = "/xscc/v2/pairing";
pub const IDENTITY_PATH: &str = "/xscc/v2/identity";
pub const CONNECT_PATH: &str = "/xscc/v2/connect";
pub const ACCESS_PATH: &str = "/xscc/v2/access";
pub const UNBIND_PATH: &str = "/xscc/v2/unbind";
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_REPORT_BYTES: usize = MAX_MESSAGE_BYTES - 1024;
pub const MAX_EXECUTION_SECONDS: u64 = 10;
pub const AUTHORIZATION_CODE_LENGTH: usize = 36;
pub const MAX_CLIENT_VERSION_BYTES: usize = 64;
pub const REVISION_HEX_BYTES: usize = 64;

pub fn is_valid_authorization_code(value: &str) -> bool {
    value.len() == AUTHORIZATION_CODE_LENGTH
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub manager_id: Uuid,
    pub device_id: Uuid,
    pub installation_id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    ReadDiagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClientOs {
    LinuxX86_64,
    LinuxAarch64,
    LinuxArmv7,
    WindowsX86_64,
    MacosX86_64,
    MacosAarch64,
    IosAarch64,
    AndroidAarch64,
}
impl ClientOs {
    pub fn supports_role(self, role: InstanceRole) -> bool {
        !matches!(self, Self::IosAarch64 | Self::AndroidAarch64) || role == InstanceRole::Controller
    }
}

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    #[default]
    Rustdesk,
    OneKvm,
}
impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rustdesk => "rustdesk",
            Self::OneKvm => "one_kvm",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    Embedded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub protocol: String,
    pub client_version: String,
    pub os: ClientOs,
    pub backend: Backend,
    pub execution_mode: ExecutionMode,
    pub diagnostics: bool,
}
impl Capabilities {
    pub fn validate(&self) -> Result<(), InvalidTask> {
        if self.protocol != PROTOCOL || !self.diagnostics {
            return Err(InvalidTask);
        }
        if matches!(self.os, ClientOs::LinuxAarch64 | ClientOs::LinuxArmv7)
            && self.backend != Backend::OneKvm
        {
            return Err(InvalidTask);
        }
        validate_text(&self.client_version, MAX_CLIENT_VERSION_BYTES)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationState {
    AwaitingIntegration,
    Ready,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticSnapshot {
    pub backend: Backend,
    pub execution_mode: ExecutionMode,
    pub integration_state: IntegrationState,
    pub rustdesk_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_kvm_version: Option<String>,
}
impl DiagnosticSnapshot {
    pub fn awaiting_integration() -> Self {
        Self {
            backend: Backend::Rustdesk,
            execution_mode: ExecutionMode::Embedded,
            integration_state: IntegrationState::AwaitingIntegration,
            rustdesk_version: None,
            one_kvm_version: None,
        }
    }
    pub fn validate(&self) -> Result<(), InvalidTask> {
        let version = match self.backend {
            Backend::Rustdesk if self.one_kvm_version.is_none() => &self.rustdesk_version,
            Backend::OneKvm if self.rustdesk_version.is_none() => &self.one_kvm_version,
            Backend::Rustdesk | Backend::OneKvm => return Err(InvalidTask),
        };
        if let Some(version) = version {
            validate_text(version, 64)?;
        }
        if self.integration_state == IntegrationState::Ready && version.is_none() {
            return Err(InvalidTask);
        }
        if self.integration_state == IntegrationState::AwaitingIntegration && version.is_some() {
            return Err(InvalidTask);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    ReadDiagnostics {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub protocol: String,
    pub operation_id: String,
    pub binding: Binding,
    pub permission: Permission,
    pub command: Command,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    Execute,
    InspectOnly,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManagerMessage {
    AccessPolicy {
        policy: Box<AccessPolicy>,
    },
    Task {
        mode: DeliveryMode,
        task: Box<Task>,
        expires_at_unix_ms: u64,
        remaining_ms: u64,
    },
    ResultAccepted {
        operation_id: String,
        fingerprint: String,
        report_digest: String,
        finalized: bool,
    },
    Revoked {},
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Report {
    DiagnosticsRead { snapshot: DiagnosticSnapshot },
    Rejected { reason: Rejection },
    Unknown { reason: Uncertainty },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rejection {
    InvalidTask,
    BindingMismatch,
    PermissionDenied,
    OperationIdReused,
    UnsupportedCapability,
    ExecutionExpired,
    ResultTooLarge,
    JournalFull,
    OperationAlreadyCompleted,
    RustdeskUnavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Uncertainty {
    NoExecutionRecord,
    PersistenceFailure,
    ResultTooLarge,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActiveOperation {
    pub operation_id: String,
    pub fingerprint: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    Progress {
        operation_id: String,
        fingerprint: String,
    },
    Hello {
        binding: Binding,
        capabilities: Capabilities,
        #[serde(default)]
        active_operation: Option<ActiveOperation>,
    },
    Heartbeat {
        diagnostics: Option<DiagnosticSnapshot>,
        addresses: Vec<HostAddress>,
    },
    Result {
        operation_id: String,
        report: Report,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid instance management contract")]
pub struct InvalidTask;

impl Task {
    pub fn validate(
        &self,
        binding: &Binding,
        capabilities: &Capabilities,
    ) -> Result<(), Rejection> {
        if &self.binding != binding {
            return Err(Rejection::BindingMismatch);
        }
        self.validate_identity()
            .map_err(|_| Rejection::InvalidTask)?;
        capabilities
            .validate()
            .map_err(|_| Rejection::InvalidTask)?;
        Ok(())
    }
    fn validate_identity(&self) -> Result<(), InvalidTask> {
        let id = self.operation_id.strip_prefix("op_").ok_or(InvalidTask)?;
        let uuid = Uuid::parse_str(id).map_err(|_| InvalidTask)?;
        if self.protocol != TASK_PROTOCOL
            || uuid.is_nil()
            || uuid.to_string() != id
            || self.binding.manager_id.is_nil()
            || self.binding.device_id.is_nil()
            || self.binding.installation_id.is_nil()
        {
            return Err(InvalidTask);
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<String, InvalidTask> {
        self.validate_identity()?;
        let bytes = serde_json::to_vec(self).map_err(|_| InvalidTask)?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }
}
pub fn report_digest(report: &Report) -> Result<String, InvalidTask> {
    let bytes = serde_json::to_vec(report).map_err(|_| InvalidTask)?;
    if bytes.len() > MAX_REPORT_BYTES {
        return Err(InvalidTask);
    }
    Ok(hex::encode(Sha256::digest(bytes)))
}
pub fn validate_revision(value: &str) -> Result<(), InvalidTask> {
    if value.len() == REVISION_HEX_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(InvalidTask)
    }
}
fn validate_text(value: &str, maximum: usize) -> Result<(), InvalidTask> {
    if value.is_empty()
        || value.len() > maximum
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        Err(InvalidTask)
    } else {
        Ok(())
    }
}
pub fn decode_manager_message(bytes: &[u8]) -> Result<ManagerMessage, InvalidTask> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(InvalidTask);
    }
    serde_json::from_slice(bytes).map_err(|_| InvalidTask)
}
pub fn validate_report(_command: &Command, report: &Report) -> Result<(), InvalidTask> {
    report_digest(report)?;
    if let Report::DiagnosticsRead { snapshot } = report {
        snapshot.validate()?;
    }
    Ok(())
}
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StoredRequest {
    pub binding: Binding,
    pub command: Command,
}
#[derive(Serialize)]
pub struct CommandStorageContract {
    pub kind: &'static str,
    pub action: &'static str,
    pub resource: &'static str,
}
pub const COMMAND_STORAGE_CONTRACTS: &[CommandStorageContract] = &[CommandStorageContract {
    kind: "read_diagnostics",
    action: "rustdesk.diagnostics.read",
    resource: "read",
}];
pub fn command_storage_contract(_command: &Command) -> &'static CommandStorageContract {
    &COMMAND_STORAGE_CONTRACTS[0]
}
pub fn persistent_json_definitions() -> serde_json::Value {
    use serde_json::json;
    let mut capabilities =
        serde_json::to_value(schemars::schema_for!(Capabilities)).expect("capability schema");
    capabilities["properties"]["protocol"] = json!({"type":"string","const":PROTOCOL});
    capabilities["properties"]["diagnostics"] = json!({"type":"boolean","const":true});
    let snapshot =
        serde_json::to_value(schemars::schema_for!(DiagnosticSnapshot)).expect("diagnostic schema");
    let sample = DiagnosticSnapshot::awaiting_integration();
    json!({"snapshot":{"schema":snapshot,"rules":[],"cases":[{"name":"awaiting-integration","input":sample,"valid":true}]},
        "capabilities":{"schema":capabilities,"rules":[{"kind":"string-bytes","fields":["/client_version"],"arguments":{"minimum":1,"maximum":64}},
        {"kind":"string-trim-no-control","fields":["/client_version"],"arguments":{}}],"cases":[{"name":"embedded-rustdesk","valid":true,"input":{
        "protocol":PROTOCOL,"client_version":"2.0.0-dev","os":"linux_x86_64","backend":"rustdesk","execution_mode":"embedded","diagnostics":true}}]}})
}
