use crate::{
    binding_store::FileBindingStore,
    enrollment::{BindingStore, EnrollmentClient, EnrollmentError, ServerOrigin},
    journal::FileJournal,
    private_fs::PrivateDirectory,
    session::{self, SessionPermit, Transcript},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::{
    sync::{Mutex, watch},
    task::JoinHandle,
    time::Instant,
};
use xscc::{
    engine::Executor,
    host::{HostError, RustDeskHost},
    protocol::{
        self, AccessPolicy, Backend, Binding, Capabilities, ClientOs, DiagnosticSnapshot,
        ExecutionMode, HostAddress, InstanceRole, SessionKey,
    },
    transport::{AccessObservation, HealthObservation, TransportError},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("protected management storage unavailable")]
    Storage,
    #[error("invalid management input")]
    Invalid,
    #[error("management authorization unavailable")]
    Unavailable,
    #[error("management authorization denied")]
    Denied,
    #[error(transparent)]
    Enrollment(#[from] EnrollmentError),
}
type Result<T> = std::result::Result<T, Error>;
#[async_trait]
pub trait NativeHost: RustDeskHost {
    async fn addresses(&self, configured: &[HostAddress]) -> Vec<HostAddress>;
}
struct Host(Arc<dyn NativeHost>);
#[async_trait]
impl RustDeskHost for Host {
    async fn diagnostics(&self) -> std::result::Result<DiagnosticSnapshot, HostError> {
        self.0.diagnostics().await
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    version: u8,
    server: String,
    addresses: Vec<HostAddress>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerView {
    pub binding: Binding,
    pub name: String,
    pub role: InstanceRole,
    pub online: bool,
    pub addresses: Vec<HostAddress>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Status {
    pub configured: bool,
    pub server: Option<String>,
    pub phase: String,
    pub binding: Option<Binding>,
    pub role: Option<InstanceRole>,
    pub addresses: Vec<HostAddress>,
    pub peers: Vec<PeerView>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Target {
    pub permit: SessionPermit,
    pub addresses: Vec<HostAddress>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Status {},
    Enroll {
        server: String,
        authorization_code: String,
        addresses: Vec<HostAddress>,
    },
    Resume {},
    Unbind {},
    SetAddresses {
        addresses: Vec<HostAddress>,
    },
    Target {
        device_id: uuid::Uuid,
    },
    Prove {
        transcript: Transcript,
        host_proof: String,
    },
    Check {
        permit: Option<SessionPermit>,
    },
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ManagementRequest(<redacted>)")
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reply {
    pub value: Option<serde_json::Value>,
    pub error: Option<String>,
}
impl Reply {
    pub fn error(value: impl ToString) -> Self {
        Self {
            value: None,
            error: Some(value.to_string()),
        }
    }
}
struct Owner {
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}
pub struct NativeManager {
    root: PrivateDirectory,
    store: FileBindingStore,
    installation_id: uuid::Uuid,
    host: Arc<dyn NativeHost>,
    settings: RwLock<Option<Settings>>,
    phase: RwLock<String>,
    binding: RwLock<Option<Binding>>,
    access: RwLock<Option<watch::Receiver<Option<AccessObservation>>>>,
    gate: Mutex<()>,
    owner: Mutex<Option<Owner>>,
    _lock: File,
}
impl NativeManager {
    pub fn open(root: PrivateDirectory, host: Arc<dyn NativeHost>) -> Result<Arc<Self>> {
        let lock = root.lock(".owner.lock").map_err(storage)?;
        let installation_id = match root.read("installation.uuid", 37).map_err(storage)? {
            Some(bytes) => {
                let text = std::str::from_utf8(&bytes).map_err(storage)?;
                let id = uuid::Uuid::parse_str(text).map_err(storage)?;
                if id.get_version_num() != 4 || id.to_string() != text {
                    return Err(Error::Storage);
                }
                id
            }
            None => {
                let id = uuid::Uuid::new_v4();
                root.write("installation.uuid", id.to_string().as_bytes(), true)
                    .map_err(storage)?;
                id
            }
        };
        let settings: Option<Settings> = root
            .read("active.json", 4096)
            .map_err(storage)?
            .map(|b| serde_json::from_slice(&b).map_err(storage))
            .transpose()?;
        if let Some(s) = &settings {
            if s.version != 1 {
                return Err(Error::Storage);
            }
            ServerOrigin::parse(&s.server)?;
            protocol::validate_addresses(&s.addresses).map_err(storage)?;
        }
        let store = FileBindingStore::open(root.path(), installation_id)?;
        Ok(Arc::new(Self {
            root,
            store,
            installation_id,
            host,
            settings: RwLock::new(settings),
            phase: RwLock::new("offline".into()),
            binding: RwLock::new(None),
            access: RwLock::new(None),
            gate: Mutex::new(()),
            owner: Mutex::new(None),
            _lock: lock,
        }))
    }
    pub fn configured(&self) -> bool {
        self.settings.read().unwrap().is_some()
    }
    pub fn policy(&self) -> Option<AccessPolicy> {
        self.access
            .read()
            .unwrap()
            .as_ref()
            .and_then(|receiver| receiver.borrow().as_ref().and_then(|o| o.policy().cloned()))
    }
    pub fn status(&self) -> Status {
        let settings = self.settings.read().unwrap().clone();
        let policy = self.policy();
        Status {
            configured: settings.is_some(),
            server: settings.as_ref().map(|s| s.server.clone()),
            phase: if policy.is_some() {
                "online".into()
            } else {
                self.phase.read().unwrap().clone()
            },
            binding: self.binding.read().unwrap().clone(),
            role: policy.as_ref().map(|p| p.role),
            addresses: settings.map(|s| s.addresses).unwrap_or_default(),
            peers: policy
                .map(|p| {
                    p.peers
                        .into_iter()
                        .map(|p| PeerView {
                            binding: p.binding,
                            name: p.name,
                            role: p.role,
                            online: p.online,
                            addresses: p.addresses,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
    pub fn target(&self, id: uuid::Uuid) -> Result<Target> {
        let policy = self.policy().ok_or(Error::Unavailable)?;
        if policy.role != InstanceRole::Controller {
            return Err(Error::Denied);
        }
        let peer = policy
            .peers
            .iter()
            .find(|p| p.binding.device_id == id && p.online && !p.addresses.is_empty())
            .ok_or(Error::Denied)?;
        Ok(Target {
            permit: SessionPermit {
                controller: policy.binding,
                controlled: peer.binding.clone(),
                epoch: session::epoch(&peer.session_key),
            },
            addresses: peer.addresses.clone(),
        })
    }
    pub fn grant(&self, permit: &SessionPermit, role: InstanceRole) -> Result<SessionKey> {
        if cfg!(any(target_os = "android", target_os = "ios")) && role != InstanceRole::Controller {
            return Err(Error::Denied);
        }
        let policy = self.policy().ok_or(Error::Unavailable)?;
        let (own, other) = if role == InstanceRole::Controller {
            (&permit.controller, &permit.controlled)
        } else {
            (&permit.controlled, &permit.controller)
        };
        if policy.role != role || policy.binding != *own {
            return Err(Error::Denied);
        }
        let peer = policy
            .peers
            .iter()
            .find(|p| p.binding == *other && p.online)
            .ok_or(Error::Denied)?;
        if session::epoch(&peer.session_key) != permit.epoch {
            return Err(Error::Denied);
        }
        Ok(peer.session_key.clone())
    }
    pub fn permits(&self, permit: &SessionPermit, role: InstanceRole) -> bool {
        self.grant(permit, role).is_ok()
    }
    pub async fn handle(self: &Arc<Self>, request: Request) -> Reply {
        let result: Result<serde_json::Value> = async {
            Ok(match request {
                Request::Status {} => serde_json::to_value(self.status()).map_err(storage)?,
                Request::Target { device_id } => {
                    serde_json::to_value(self.target(device_id)?).map_err(storage)?
                }
                Request::Check { permit } => serde_json::json!(permit.as_ref().map_or_else(
                    || !self.configured(),
                    |permit| self.permits(permit, InstanceRole::Controller)
                )),
                Request::Prove {
                    transcript,
                    host_proof,
                } => {
                    let key = self.grant(&transcript.permit, InstanceRole::Controller)?;
                    transcript
                        .verify(&key, true, &host_proof)
                        .map_err(|_| Error::Denied)?;
                    serde_json::json!(transcript.proof(&key, false).map_err(|_| Error::Denied)?)
                }
                Request::Enroll {
                    server,
                    authorization_code,
                    addresses,
                } => {
                    self.enroll(&server, &authorization_code, addresses).await?;
                    serde_json::to_value(self.status()).map_err(storage)?
                }
                Request::Resume {} => {
                    self.resume().await?;
                    serde_json::to_value(self.status()).map_err(storage)?
                }
                Request::SetAddresses { addresses } => {
                    self.set_addresses(addresses).await?;
                    serde_json::to_value(self.status()).map_err(storage)?
                }
                Request::Unbind {} => {
                    self.unbind().await?;
                    serde_json::to_value(self.status()).map_err(storage)?
                }
            })
        }
        .await;
        match result {
            Ok(value) => Reply {
                value: Some(value),
                error: None,
            },
            Err(error) => Reply::error(error),
        }
    }
    async fn save_settings(&self, settings: Option<Settings>) -> Result<()> {
        let root = self.root.clone();
        let value = settings.clone();
        tokio::task::spawn_blocking(move || match value {
            Some(s) => root.write(
                "active.json",
                &serde_json::to_vec(&s).map_err(std::io::Error::other)?,
                false,
            ),
            None => root.remove("active.json"),
        })
        .await
        .map_err(storage)?
        .map_err(storage)?;
        *self.settings.write().unwrap() = settings;
        Ok(())
    }
    async fn stop_owner(&self) -> Result<()> {
        let owner = self.owner.lock().await.take();
        if let Some(owner) = owner {
            owner.shutdown.send_replace(true);
            owner.task.await.map_err(storage)?;
        }
        *self.access.write().unwrap() = None;
        *self.phase.write().unwrap() = "offline".into();
        Ok(())
    }
    pub async fn shutdown(&self) -> Result<()> {
        let _guard = self.gate.lock().await;
        self.stop_owner().await
    }
    pub async fn enroll(
        self: &Arc<Self>,
        server: &str,
        code: &str,
        addresses: Vec<HostAddress>,
    ) -> Result<()> {
        let origin = ServerOrigin::parse(server)?;
        protocol::validate_addresses(&addresses).map_err(|_| Error::Invalid)?;
        if !protocol::is_valid_authorization_code(code) {
            return Err(Error::Invalid);
        }
        let _guard = self.gate.lock().await;
        if self
            .settings
            .read()
            .unwrap()
            .as_ref()
            .is_some_and(|s| s.server != origin.as_str())
        {
            return Err(Error::Denied);
        }
        self.stop_owner().await?;
        self.save_settings(Some(Settings {
            version: 1,
            server: origin.as_str().into(),
            addresses,
        }))
        .await?;
        let client = EnrollmentClient::new(origin.clone())?;
        let binding = if self.store.load(&origin).await?.is_some() {
            client.retry_enrollment(&self.store, code).await?
        } else {
            client
                .enroll_new(&self.store, self.installation_id, code)
                .await?
        };
        *self.binding.write().unwrap() = Some(binding);
        self.start_owner().await;
        Ok(())
    }
    pub async fn resume(self: &Arc<Self>) -> Result<()> {
        let _guard = self.gate.lock().await;
        self.stop_owner().await?;
        if self.configured() {
            self.start_owner().await;
        }
        Ok(())
    }
    pub async fn set_addresses(&self, addresses: Vec<HostAddress>) -> Result<()> {
        protocol::validate_addresses(&addresses).map_err(|_| Error::Invalid)?;
        let _guard = self.gate.lock().await;
        let mut s = self.settings.read().unwrap().clone().ok_or(Error::Denied)?;
        s.addresses = addresses;
        self.save_settings(Some(s)).await
    }
    pub async fn unbind(&self) -> Result<()> {
        let _guard = self.gate.lock().await;
        self.stop_owner().await?;
        let Some(settings) = self.settings.read().unwrap().clone() else {
            return Ok(());
        };
        let origin = ServerOrigin::parse(&settings.server)?;
        if let Some(saved) = self.store.load(&origin).await? {
            EnrollmentClient::new(origin.clone())?
                .unbind(&saved)
                .await?;
            self.store.forget(&origin).await?;
        }
        self.save_settings(None).await?;
        *self.binding.write().unwrap() = None;
        *self.phase.write().unwrap() = "unbound".into();
        Ok(())
    }
    async fn start_owner(self: &Arc<Self>) {
        let (shutdown, receiver) = watch::channel(false);
        let this = self.clone();
        let task = tokio::spawn(async move {
            this.run_owner(receiver).await;
        });
        *self.owner.lock().await = Some(Owner { shutdown, task });
    }
    async fn run_owner(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let result = self.run(&mut shutdown).await;
        *self.access.write().unwrap() = None;
        *self.phase.write().unwrap() = match result {
            Ok(()) => "offline",
            Err(Error::Enrollment(EnrollmentError::Unauthorized)) => "revoked",
            Err(_) => "error",
        }
        .into();
    }
    async fn run(self: &Arc<Self>, shutdown: &mut watch::Receiver<bool>) -> Result<()> {
        let settings = self.settings.read().unwrap().clone().ok_or(Error::Denied)?;
        let origin = ServerOrigin::parse(&settings.server)?;
        let client = EnrollmentClient::new(origin.clone())?;
        loop {
            let recovered = tokio::select! {_=shutdown.changed()=>return Ok(()),value=client.recover(&self.store)=>value};
            match recovered {
                Ok(binding) => {
                    *self.binding.write().unwrap() = Some(binding);
                    break;
                }
                Err(EnrollmentError::Unavailable) => {}
                Err(e) => return Err(e.into()),
            }
            tokio::select! {_=shutdown.changed()=>return Ok(()),_=tokio::time::sleep(Duration::from_secs(5))=>{}}
        }
        let saved = self.store.load(&origin).await?.ok_or(Error::Denied)?;
        let connection = saved.connection()?;
        let access = connection.access_policy();
        *self.access.write().unwrap() = Some(access);
        let root = self.root.clone();
        let binding = saved.binding().clone();
        let journal = tokio::task::spawn_blocking(move || FileJournal::open(&root, &binding))
            .await
            .map_err(storage)?
            .map_err(storage)?;
        let caps = capabilities()?;
        let executor = Arc::new(Executor::new_with_capabilities(
            saved.binding().clone(),
            caps.clone(),
            Host(self.host.clone()),
            journal,
        ));
        let (sender, health) = watch::channel(HealthObservation::default());
        let sampler = async {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            loop {
                tick.tick().await;
                let configured = self
                    .settings
                    .read()
                    .unwrap()
                    .as_ref()
                    .map(|s| s.addresses.clone())
                    .unwrap_or_default();
                let diagnostics = self.host.diagnostics().await.ok();
                let addresses = self.host.addresses(&configured).await;
                sender.send_replace(HealthObservation {
                    diagnostics,
                    addresses,
                    observed_at: Some(Instant::now()),
                });
            }
        };
        *self.phase.write().unwrap() = "connecting".into();
        tokio::select! {result=connection.run(saved.binding().clone(),caps,executor,health,shutdown.clone())=>match result{Ok(())=>Ok(()),Err(TransportError::Revoked)=>Err(EnrollmentError::Unauthorized.into()),Err(_)=>Err(Error::Unavailable)},_=sampler=>Err(Error::Unavailable)}
    }
}
fn capabilities() -> Result<Capabilities> {
    let os = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        ClientOs::LinuxX86_64
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        ClientOs::WindowsX86_64
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        ClientOs::MacosAarch64
    } else if cfg!(all(target_os = "ios", target_arch = "aarch64")) {
        ClientOs::IosAarch64
    } else if cfg!(all(target_os = "android", target_arch = "aarch64")) {
        ClientOs::AndroidAarch64
    } else {
        return Err(Error::Unavailable);
    };
    Ok(Capabilities {
        protocol: protocol::PROTOCOL.into(),
        client_version: env!("CARGO_PKG_VERSION").into(),
        os,
        backend: Backend::Rustdesk,
        execution_mode: ExecutionMode::Embedded,
        diagnostics: true,
    })
}
fn storage<T>(_: T) -> Error {
    Error::Storage
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enrollment::SavedBinding;
    use zeroize::Zeroizing;

    struct UnavailableHost;
    #[async_trait]
    impl RustDeskHost for UnavailableHost {
        async fn diagnostics(&self) -> std::result::Result<DiagnosticSnapshot, HostError> {
            Err(HostError)
        }
    }
    #[async_trait]
    impl NativeHost for UnavailableHost {
        async fn addresses(&self, _: &[HostAddress]) -> Vec<HostAddress> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn owner_is_exclusive_and_recovers_pending_identity_without_exposing_secrets() {
        let temporary = tempfile::tempdir().unwrap();
        let root = PrivateDirectory::create(temporary.path(), "native").unwrap();
        let manager = NativeManager::open(root.clone(), Arc::new(UnavailableHost)).unwrap();
        assert!(NativeManager::open(root.clone(), Arc::new(UnavailableHost)).is_err());
        assert!(!manager.status().configured);
        assert_eq!(
            manager.handle(Request::Check { permit: None }).await.value,
            Some(serde_json::json!(true))
        );
        let origin = ServerOrigin::parse("https://example.invalid").unwrap();
        let binding = Binding {
            manager_id: uuid::Uuid::new_v4(),
            device_id: uuid::Uuid::new_v4(),
            installation_id: manager.installation_id,
        };
        let saved = SavedBinding::restore(
            origin.clone(),
            binding.clone(),
            Zeroizing::new("a".repeat(64)),
        )
        .unwrap();
        manager.store.create(&saved).await.unwrap();
        manager
            .save_settings(Some(Settings {
                version: 1,
                server: origin.as_str().into(),
                addresses: Vec::new(),
            }))
            .await
            .unwrap();
        assert_eq!(
            manager.handle(Request::Check { permit: None }).await.value,
            Some(serde_json::json!(false))
        );
        drop(manager);
        let manager = NativeManager::open(root.clone(), Arc::new(UnavailableHost)).unwrap();
        assert_eq!(manager.installation_id, binding.installation_id);
        assert_eq!(
            manager
                .store
                .load(&origin)
                .await
                .unwrap()
                .unwrap()
                .binding(),
            &binding
        );
        let status = serde_json::to_string(&manager.status()).unwrap();
        assert!(manager.status().configured && manager.status().peers.is_empty());
        assert!(!status.contains(&"a".repeat(64)) && !status.contains("session_key"));
        assert_eq!(
            format!(
                "{:?}",
                Request::Enroll {
                    server: origin.as_str().into(),
                    authorization_code: "secret".into(),
                    addresses: Vec::new()
                }
            ),
            "ManagementRequest(<redacted>)"
        );
        drop(manager);
        root.write("active.json", b"corrupt", false).unwrap();
        assert!(NativeManager::open(root, Arc::new(UnavailableHost)).is_err());
    }
}
