//! XSCS owns instance identities and ACLs; browser credentials stay in this process.
use rand::{TryRng, rngs::SysRng};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{
        Arc, RwLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::{
    sync::{Mutex, watch},
    task::JoinHandle,
};
use xscc::{
    host::{HostError, RustDeskHost},
    protocol::{Backend, DiagnosticSnapshot, ExecutionMode, IntegrationState},
};
use xscc_native_management::{
    enrollment::{EnrollmentClient, EnrollmentError},
    manager::{Error as NativeError, NativeHost, NativeManager},
    private_fs::PrivateDirectory,
};
use zeroize::Zeroizing;

pub use xscc::protocol;
pub use xscc::protocol::{Binding, HostAddress, InstanceRole, SessionKey};
pub use xscc_native_management::enrollment::ServerOrigin;
pub use xscc_native_management::manager::{PeerView, Status as DeviceStatus};
pub use xscc_native_management::session::{SessionPermit, Transcript, WireMessage, nonce};

pub const COOKIE_NAME: &str = "one_kvm_session";
pub const CSRF_HEADER: &str = "x-csrf-token";
pub const SESSION_TTL_SECS: u64 = 3600;
const MAX_ACTIVE_CONTROLLERS: usize = 8;
const MAX_CONTROLLER_PROFILES: usize = 64;
const POLICY_WAIT: Duration = Duration::from_secs(12);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("protected management storage unavailable: {0}")]
    Storage(#[from] std::io::Error),
    #[error("invalid management input")]
    Invalid,
    #[error("KVM is not bound to XSCS; use the local management CLI")]
    Unconfigured,
    #[error("XSCS authorization is unavailable or expired")]
    Unavailable,
    #[error("XSCS does not authorize this controller for this KVM")]
    Denied,
    #[error("browser controller capacity exhausted; log out unused controllers")]
    Capacity,
    #[error(transparent)]
    Enrollment(#[from] EnrollmentError),
    #[error(transparent)]
    Management(#[from] xscc_native_management::manager::Error),
    #[error("management task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}
pub type Result<T> = std::result::Result<T, Error>;

fn login_enrollment_error(error: EnrollmentError) -> Error {
    match error {
        EnrollmentError::Configuration => Error::Invalid,
        EnrollmentError::Rejected
        | EnrollmentError::Unauthorized
        | EnrollmentError::RoleUnsupported
        | EnrollmentError::InstanceMismatch => Error::Denied,
        error => Error::Enrollment(error),
    }
}

fn login_management_error(error: NativeError) -> Error {
    match error {
        NativeError::Invalid => Error::Invalid,
        NativeError::Denied => Error::Denied,
        NativeError::Enrollment(error) => login_enrollment_error(error),
        error => Error::Management(error),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PublicStatus {
    pub configured: bool,
    pub phase: String,
}

#[derive(Clone, Serialize)]
pub struct BrowserLogin {
    pub authenticated: bool,
    pub csrf_token: String,
    pub expires_in_secs: u64,
    pub controller: Binding,
    pub controlled: Binding,
    #[serde(skip)]
    pub cookie_token: String,
}
impl std::fmt::Debug for BrowserLogin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrowserLogin")
            .field("controller", &self.controller)
            .field("controlled", &self.controlled)
            .finish_non_exhaustive()
    }
}

struct KvmHost {
    role: InstanceRole,
}
#[async_trait::async_trait]
impl RustDeskHost for KvmHost {
    async fn diagnostics(&self) -> std::result::Result<DiagnosticSnapshot, HostError> {
        Ok(DiagnosticSnapshot {
            backend: Backend::OneKvm,
            execution_mode: ExecutionMode::Embedded,
            integration_state: IntegrationState::Ready,
            rustdesk_version: None,
            one_kvm_version: Some(env!("CARGO_PKG_VERSION").into()),
        })
    }
}
#[async_trait::async_trait]
impl NativeHost for KvmHost {
    async fn addresses(&self, configured: &[HostAddress]) -> Vec<HostAddress> {
        match self.role {
            InstanceRole::Controlled => configured.to_vec(),
            InstanceRole::Controller => Vec::new(),
        }
    }
}

struct ControllerOwner {
    manager: Arc<NativeManager>,
    directory: PrivateDirectory,
    retiring: AtomicBool,
}
struct BrowserSession {
    owner: Arc<ControllerOwner>,
    device: Arc<NativeManager>,
    permit: SessionPermit,
    csrf: Zeroizing<String>,
    expires_at: Instant,
    active: AtomicBool,
}
impl BrowserSession {
    fn is_valid(&self) -> bool {
        if !self.active.load(Ordering::Acquire) {
            return false;
        }
        let permitted = Instant::now() < self.expires_at
            && self
                .owner
                .manager
                .permits(&self.permit, InstanceRole::Controller)
            && self.device.permits(&self.permit, InstanceRole::Controlled);
        if !permitted {
            self.active.store(false, Ordering::Release);
        }
        permitted && self.active.load(Ordering::Acquire)
    }
}

/// A request/stream permit. Check `is_valid` before actions or await `cancelled`.
#[derive(Clone)]
pub struct AccessGrant(Arc<BrowserSession>);
impl AccessGrant {
    pub fn is_valid(&self) -> bool {
        self.0.is_valid()
    }
    pub fn controller(&self) -> &Binding {
        &self.0.permit.controller
    }
    pub fn controlled(&self) -> &Binding {
        &self.0.permit.controlled
    }
    pub fn permit(&self) -> &SessionPermit {
        &self.0.permit
    }
    pub fn csrf_matches(&self, supplied: &str) -> bool {
        self.0.csrf.as_bytes().ct_eq(supplied.as_bytes()).into()
    }
    pub fn login_view(&self) -> BrowserLogin {
        BrowserLogin {
            authenticated: true,
            csrf_token: self.0.csrf.to_string(),
            expires_in_secs: self
                .0
                .expires_at
                .saturating_duration_since(Instant::now())
                .as_secs(),
            controller: self.0.permit.controller.clone(),
            controlled: self.0.permit.controlled.clone(),
            cookie_token: String::new(),
        }
    }
    pub async fn cancelled(&self) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            if !self.is_valid() {
                return;
            }
        }
    }
}

/// Browser controller identities are shared by the instances of one local host.
/// Sessions remain bound to their exact controlled device and private cookie.
pub struct BrowserControllerPool {
    profiles: PrivateDirectory,
    controllers: RwLock<BTreeMap<String, Arc<ControllerOwner>>>,
    sessions: RwLock<BTreeMap<String, Arc<BrowserSession>>>,
    gate: Mutex<()>,
    stop: watch::Sender<bool>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl BrowserControllerPool {
    pub async fn open(data_dir: &Path) -> Result<Arc<Self>> {
        let path = data_dir.to_owned();
        let (profiles, pending) = tokio::task::spawn_blocking(move || -> Result<_> {
            let path = path.canonicalize()?;
            let root = PrivateDirectory::create(&path, "xscs-managed")?;
            let profiles = root.child("controllers")?;
            let mut pending = BTreeMap::new();
            let entries = std::fs::read_dir(profiles.path())?
                .take(MAX_CONTROLLER_PROFILES + 1)
                .collect::<std::io::Result<Vec<_>>>()?;
            if entries.len() > MAX_CONTROLLER_PROFILES {
                return Err(Error::Capacity);
            }
            for entry in entries {
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| Error::Invalid)?;
                if name.len() != 64
                    || !name
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                {
                    return Err(Error::Invalid);
                }
                let directory = PrivateDirectory::open(&entry.path())?;
                if directory.read("logout.pending", 1)?.is_some() {
                    let manager = NativeManager::open_for_role(
                        directory.clone(),
                        Arc::new(KvmHost {
                            role: InstanceRole::Controller,
                        }),
                        InstanceRole::Controller,
                    )?;
                    pending.insert(
                        name,
                        Arc::new(ControllerOwner {
                            manager,
                            directory,
                            retiring: AtomicBool::new(true),
                        }),
                    );
                }
            }
            Ok((profiles, pending))
        })
        .await??;
        let (stop, receiver) = watch::channel(false);
        let this = Arc::new(Self {
            profiles,
            controllers: RwLock::new(pending),
            sessions: RwLock::new(BTreeMap::new()),
            gate: Mutex::new(()),
            stop,
            worker: Mutex::new(None),
        });
        let weak = Arc::downgrade(&this);
        *this.worker.lock().await = Some(tokio::spawn(Self::run_cleanup(weak, receiver)));
        Ok(this)
    }

    fn has_live_session(&self, owner: &Arc<ControllerOwner>) -> bool {
        self.sessions
            .read()
            .expect("browser session lock poisoned")
            .values()
            .any(|session| Arc::ptr_eq(owner, &session.owner) && session.is_valid())
    }

    fn invalidate_device(&self, device: &Arc<NativeManager>) {
        for session in self
            .sessions
            .read()
            .expect("browser session lock poisoned")
            .values()
        {
            if Arc::ptr_eq(device, &session.device) {
                session.active.store(false, Ordering::Release);
            }
        }
    }
}

pub struct ManagedKvm {
    device: Arc<NativeManager>,
    browser: Arc<BrowserControllerPool>,
    owns_controllers: bool,
}

impl ManagedKvm {
    pub async fn open(data_dir: &Path) -> Result<Arc<Self>> {
        let browser = BrowserControllerPool::open(data_dir).await?;
        Self::open_inner(data_dir, browser, true).await
    }

    pub async fn open_with_controllers(
        data_dir: &Path,
        browser: Arc<BrowserControllerPool>,
    ) -> Result<Arc<Self>> {
        Self::open_inner(data_dir, browser, false).await
    }

    async fn open_inner(
        data_dir: &Path,
        browser: Arc<BrowserControllerPool>,
        owns_controllers: bool,
    ) -> Result<Arc<Self>> {
        let path = data_dir.to_owned();
        let device = tokio::task::spawn_blocking(move || -> Result<_> {
            let path = path.canonicalize()?;
            let root = PrivateDirectory::create(&path, "xscs-managed")?;
            Ok(NativeManager::open_for_role(
                root.child("device")?,
                Arc::new(KvmHost {
                    role: InstanceRole::Controlled,
                }),
                InstanceRole::Controlled,
            )?)
        })
        .await??;
        let this = Arc::new(Self {
            device,
            browser,
            owns_controllers,
        });
        this.device.resume().await?;
        Ok(this)
    }
    pub fn public_status(&self) -> PublicStatus {
        PublicStatus {
            configured: self.device.configured(),
            phase: self.device.connection_phase(),
        }
    }
    pub fn device_status(&self) -> DeviceStatus {
        self.device.status()
    }
    pub async fn bind_device(
        self: &Arc<Self>,
        server: &str,
        code: &str,
        addresses: Vec<HostAddress>,
    ) -> Result<DeviceStatus> {
        self.device.enroll(server, code, addresses).await?;
        Ok(self.device.status())
    }
    pub async fn unbind_device(&self) -> Result<()> {
        self.browser.invalidate_device(&self.device);
        self.device.unbind().await?;
        Ok(())
    }
    pub async fn set_addresses(&self, addresses: Vec<HostAddress>) -> Result<()> {
        self.device
            .set_addresses(addresses)
            .await
            .map_err(|error| match error {
                NativeError::Invalid => Error::Invalid,
                error => Error::Management(error),
            })
    }
    pub fn native_grant(&self, permit: &SessionPermit) -> Result<SessionKey> {
        self.device
            .grant(permit, InstanceRole::Controlled)
            .map_err(Error::Management)
    }
    pub fn native_permits(&self, permit: &SessionPermit) -> bool {
        self.device.permits(permit, InstanceRole::Controlled)
    }
    pub fn authorize(&self, cookie: &str) -> Result<AccessGrant> {
        let sessions = self
            .browser
            .sessions
            .read()
            .expect("browser session lock poisoned");
        sessions
            .get(cookie)
            .filter(|session| Arc::ptr_eq(&self.device, &session.device) && session.is_valid())
            .cloned()
            .map(AccessGrant)
            .ok_or(Error::Denied)
    }
    pub fn browser_status(&self, cookie: &str) -> Result<BrowserLogin> {
        Ok(self.authorize(cookie)?.login_view())
    }
    pub async fn login(self: &Arc<Self>, code: &str) -> Result<BrowserLogin> {
        if !protocol::is_valid_authorization_code(code) {
            return Err(Error::Invalid);
        }
        let _guard = self.browser.gate.lock().await;
        let server = self.device.status().server.ok_or(Error::Unconfigured)?;
        let profile = hex::encode(Sha256::digest(
            [server.as_bytes(), b"\0", code.as_bytes()].concat(),
        ));
        let existing = self
            .browser
            .controllers
            .read()
            .expect("controller lock poisoned")
            .get(&profile)
            .cloned();
        {
            let sessions = self
                .browser
                .sessions
                .read()
                .expect("browser session lock poisoned");
            let now = Instant::now();
            let live = |session: &&Arc<BrowserSession>| {
                Arc::ptr_eq(&self.device, &session.device)
                    && session.active.load(Ordering::Acquire)
                    && now < session.expires_at
            };
            let count = sessions.values().filter(live).count();
            let replacing = existing.as_ref().is_some_and(|owner| {
                sessions
                    .values()
                    .filter(live)
                    .any(|session| Arc::ptr_eq(owner, &session.owner))
            });
            if count >= MAX_ACTIVE_CONTROLLERS && !replacing {
                return Err(Error::Capacity);
            }
        }
        let owner = match existing {
            Some(owner) => owner,
            None => {
                let check_path = self.browser.profiles.path().join(&profile);
                let exists = tokio::task::spawn_blocking(move || -> std::io::Result<bool> {
                    match std::fs::symlink_metadata(check_path) {
                        Ok(metadata) => Ok(metadata.is_dir() && !metadata.file_type().is_symlink()),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                        Err(error) => Err(error),
                    }
                })
                .await??;
                if !exists {
                    EnrollmentClient::for_kvm_role(
                        ServerOrigin::parse(&server)?,
                        InstanceRole::Controller,
                    )?
                    .check_pairing_code(code)
                    .await
                    .map_err(login_enrollment_error)?;
                }
                let profiles = self.browser.profiles.clone();
                let profile_name = profile.clone();
                let owner = tokio::task::spawn_blocking(move || -> Result<_> {
                    let candidate = profiles.path().join(&profile_name);
                    let count = std::fs::read_dir(profiles.path())?
                        .take(MAX_CONTROLLER_PROFILES + 1)
                        .count();
                    if !candidate.exists() && count >= MAX_CONTROLLER_PROFILES {
                        return Err(Error::Capacity);
                    }
                    let directory = profiles.child(&profile_name)?;
                    let manager = NativeManager::open_for_role(
                        directory.clone(),
                        Arc::new(KvmHost {
                            role: InstanceRole::Controller,
                        }),
                        InstanceRole::Controller,
                    )?;
                    Ok(Arc::new(ControllerOwner {
                        manager,
                        directory,
                        retiring: AtomicBool::new(false),
                    }))
                })
                .await??;
                self.browser
                    .controllers
                    .write()
                    .expect("controller lock poisoned")
                    .insert(profile.clone(), owner.clone());
                owner
            }
        };
        let pending = {
            let directory = owner.directory.clone();
            tokio::task::spawn_blocking(move || directory.read("logout.pending", 1))
                .await??
                .is_some()
        };
        if pending || owner.retiring.load(Ordering::Acquire) {
            owner.retiring.store(true, Ordering::Release);
            BrowserControllerPool::retire(&owner).await?;
        }
        if !self.browser.has_live_session(&owner)
            && let Err(error) = owner.manager.enroll(&server, code, Vec::new()).await
        {
            owner.manager.shutdown().await?;
            return Err(login_management_error(error));
        }
        let deadline = Instant::now() + POLICY_WAIT;
        let permit = loop {
            if let Some(permit) = self.browser_permit(&owner.manager) {
                break permit;
            }
            if Instant::now() >= deadline {
                if !self.browser.has_live_session(&owner) {
                    owner.manager.shutdown().await?;
                }
                return Err(Error::Denied);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        for session in self
            .browser
            .sessions
            .write()
            .expect("browser session lock poisoned")
            .values()
            .filter(|session| {
                session.permit.controller == permit.controller
                    && session.permit.controlled == permit.controlled
            })
        {
            session.active.store(false, Ordering::Release);
        }
        self.browser
            .sessions
            .write()
            .expect("browser session lock poisoned")
            .retain(|_, session| session.active.load(Ordering::Acquire));
        let cookie = random_secret()?;
        let csrf = random_secret()?;
        let session = Arc::new(BrowserSession {
            owner,
            device: self.device.clone(),
            permit,
            csrf: Zeroizing::new(csrf),
            expires_at: Instant::now() + Duration::from_secs(SESSION_TTL_SECS),
            active: AtomicBool::new(true),
        });
        let grant = AccessGrant(session.clone());
        let mut response = grant.login_view();
        response.cookie_token = cookie.clone();
        self.browser
            .sessions
            .write()
            .expect("browser session lock poisoned")
            .insert(cookie, session);
        Ok(response)
    }
    fn browser_permit(&self, controller: &NativeManager) -> Option<SessionPermit> {
        let device = self.device.status().binding?;
        let permit = controller.permit_to(&device).ok()?;
        if !self.device.permits(&permit, InstanceRole::Controlled) {
            return None;
        }
        Some(permit)
    }
    pub async fn logout(&self, cookie: &str) -> Result<()> {
        let _guard = self.browser.gate.lock().await;
        self.authorize(cookie)?;
        let session = self
            .browser
            .sessions
            .write()
            .expect("browser session lock poisoned")
            .remove(cookie)
            .ok_or(Error::Denied)?;
        session.active.store(false, Ordering::Release);
        if self.browser.has_live_session(&session.owner) {
            return Ok(());
        }
        BrowserControllerPool::mark_retiring(&session.owner).await?;
        BrowserControllerPool::retire(&session.owner).await
    }
    pub async fn shutdown(&self) -> Result<()> {
        self.browser.invalidate_device(&self.device);
        self.device.shutdown().await?;
        if self.owns_controllers {
            self.browser.shutdown().await?;
        }
        Ok(())
    }
}

impl BrowserControllerPool {
    async fn mark_retiring(owner: &ControllerOwner) -> Result<()> {
        owner.retiring.store(true, Ordering::Release);
        let directory = owner.directory.clone();
        let persisted =
            tokio::task::spawn_blocking(move || directory.write("logout.pending", b"1", false))
                .await?;
        owner.manager.shutdown().await?;
        persisted?;
        Ok(())
    }
    async fn retire(owner: &ControllerOwner) -> Result<()> {
        owner.manager.unbind().await?;
        let directory = owner.directory.clone();
        tokio::task::spawn_blocking(move || directory.remove("logout.pending")).await??;
        owner.retiring.store(false, Ordering::Release);
        Ok(())
    }
    fn invalidate_all(&self) {
        for session in self
            .sessions
            .read()
            .expect("browser session lock poisoned")
            .values()
        {
            session.active.store(false, Ordering::Release);
        }
    }
    async fn run_cleanup(weak: Weak<Self>, mut shutdown: watch::Receiver<bool>) {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = shutdown.changed() => return,
                _ = tick.tick() => {}
            }
            let Some(this) = weak.upgrade() else {
                return;
            };
            let _guard = this.gate.lock().await;
            let expired: Vec<_> = {
                let mut sessions = this
                    .sessions
                    .write()
                    .expect("browser session lock poisoned");
                let expired: Vec<_> = sessions
                    .iter()
                    .filter(|(_, session)| !session.is_valid())
                    .map(|(cookie, _)| cookie.clone())
                    .collect();
                expired
                    .into_iter()
                    .filter_map(|cookie| sessions.remove(&cookie))
                    .collect()
            };
            for session in expired {
                session.active.store(false, Ordering::Release);
                if !this.has_live_session(&session.owner)
                    && let Err(error) = session.owner.manager.shutdown().await
                {
                    tracing_error(&Error::Management(error));
                }
            }
            drop(_guard);
            let owners: Vec<_> = this
                .controllers
                .read()
                .expect("controller lock poisoned")
                .values()
                .filter(|owner| owner.retiring.load(Ordering::Acquire))
                .cloned()
                .collect();
            for owner in owners {
                let _guard = tokio::select! {
                    _ = shutdown.changed() => return,
                    guard = this.gate.lock() => guard,
                };
                if !owner.retiring.load(Ordering::Acquire) {
                    continue;
                }
                let retired = tokio::select! {
                    _ = shutdown.changed() => return,
                    result = Self::retire(&owner) => result,
                };
                if let Err(error) = retired {
                    tracing_error(&error);
                    continue;
                }
            }
        }
    }
    pub async fn shutdown(&self) -> Result<()> {
        self.stop.send_replace(true);
        if let Some(worker) = self.worker.lock().await.take() {
            worker.await?;
        }
        self.invalidate_all();
        let owners: Vec<_> = self
            .controllers
            .read()
            .expect("controller lock poisoned")
            .values()
            .cloned()
            .collect();
        for owner in owners {
            owner.manager.shutdown().await?;
        }
        Ok(())
    }
}
fn random_secret() -> Result<String> {
    let mut bytes = [0; 32];
    SysRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| Error::Unavailable)?;
    Ok(hex::encode(bytes))
}
fn tracing_error(error: &Error) {
    log::warn!("browser controller cleanup: {error}");
}
