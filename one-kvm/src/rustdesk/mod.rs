//! RustDesk peer protocol (hbbs / hbbr).

pub mod bytes_codec;
pub mod config;
pub mod connection;
pub mod crypto;
pub mod frame_adapters;
pub mod hid_adapter;
mod keyboard_mapping;
mod managed;
pub mod protocol;
pub mod punch;
pub mod rendezvous;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::RwLock;
use protobuf::Message;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::audio::AudioController;
use crate::hid::HidController;
use crate::utils::bind_tcp_listener;
use crate::video::stream_manager::VideoStreamManager;

use self::config::{RustDeskConfig, RustDeskMode};
use self::connection::ConnectionManager;
use self::crypto::SigningKeyPair;
use self::protocol::{make_local_addr, make_relay_response, make_request_relay};
use self::rendezvous::{AddrMangle, RendezvousMediator, RendezvousStatus};

const RELAY_CONNECT_TIMEOUT_MS: u64 = 10_000;
const SERVICE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PENDING_CONNECTION_ATTEMPTS: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub enum ServiceStatus {
    Stopped,
    Starting,
    Running,
    Error(String),
}

impl std::fmt::Display for ServiceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => write!(f, "stopped"),
            Self::Starting => write!(f, "starting"),
            Self::Running => write!(f, "running"),
            Self::Error(e) => write!(f, "error: {}", e),
        }
    }
}

pub struct RustDeskService {
    config: Arc<RwLock<RustDeskConfig>>,
    status: Arc<RwLock<ServiceStatus>>,
    rendezvous: Arc<RwLock<Option<Arc<RendezvousMediator>>>>,
    rendezvous_handle: Arc<RwLock<Option<JoinHandle<()>>>>,
    tcp_listener_handle: Arc<RwLock<Option<Vec<JoinHandle<()>>>>>,
    listener_start_lock: Arc<tokio::sync::Mutex<()>>,
    listen_port: Arc<RwLock<u16>>,
    connection_manager: Arc<ConnectionManager>,
    video_manager: Arc<VideoStreamManager>,
    hid: Arc<HidController>,
    audio: Arc<AudioController>,
    shutdown_tx: broadcast::Sender<()>,
}

impl RustDeskService {
    pub fn new(
        managed: Arc<one_kvm_management::ManagedKvm>,
        config: RustDeskConfig,
        video_manager: Arc<VideoStreamManager>,
        hid: Arc<HidController>,
        audio: Arc<AudioController>,
    ) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        let connection_manager = Arc::new(ConnectionManager::new(config.clone(), managed));
        let direct_access_port = config.direct_access_port;

        Self {
            config: Arc::new(RwLock::new(config)),
            status: Arc::new(RwLock::new(ServiceStatus::Stopped)),
            rendezvous: Arc::new(RwLock::new(None)),
            rendezvous_handle: Arc::new(RwLock::new(None)),
            tcp_listener_handle: Arc::new(RwLock::new(None)),
            listener_start_lock: Arc::new(tokio::sync::Mutex::new(())),
            listen_port: Arc::new(RwLock::new(direct_access_port)),
            connection_manager,
            video_manager,
            hid,
            audio,
            shutdown_tx,
        }
    }

    pub fn listen_port(&self) -> u16 {
        *self.listen_port.read()
    }

    pub fn status(&self) -> ServiceStatus {
        self.status.read().clone()
    }

    pub fn config(&self) -> RustDeskConfig {
        self.config.read().clone()
    }

    pub fn update_config(&self, config: RustDeskConfig) {
        self.connection_manager.update_config(config.clone());
        *self.config.write() = config;
    }

    pub fn rendezvous_status(&self) -> Option<RendezvousStatus> {
        self.rendezvous.read().as_ref().map(|r| r.status())
    }

    pub fn device_id(&self) -> String {
        self.config.read().device_id.clone()
    }

    pub fn connection_count(&self) -> usize {
        self.connection_manager.connection_count()
    }

    pub fn is_listening(&self) -> bool {
        self.tcp_listener_handle.read().is_some()
    }

    pub fn is_running(&self) -> bool {
        self.status() == ServiceStatus::Running
    }

    /// Prepare the stable direct transport identity so the caller can persist
    /// newly generated keys before opening a listener.
    pub fn prepare_direct_identity(&self) -> anyhow::Result<Option<RustDeskConfig>> {
        let mut config = self.config.write();
        if config.mode != RustDeskMode::DirectIp {
            return Ok(None);
        }
        crypto::init()?;
        let generated = config.signing_public_key.is_none() && config.signing_private_key.is_none();
        let identity = direct_signing_identity(&mut config)?;
        self.connection_manager.set_signing_keypair(identity);
        Ok(generated.then(|| config.clone()))
    }

    pub async fn start(self: &Arc<Self>) -> anyhow::Result<()> {
        let config = self.config.read().clone();

        if !config.enabled {
            info!("RustDesk service is disabled");
            return Ok(());
        }

        if !config.is_valid() {
            warn!("RustDesk configuration is incomplete");
            return Ok(());
        }

        if self.status() == ServiceStatus::Running {
            warn!("RustDesk service is already running");
            return Ok(());
        }

        *self.status.write() = ServiceStatus::Starting;
        info!(
            "Starting RustDesk service in {:?} mode with ID: {}",
            config.mode, config.device_id,
        );

        if let Err(e) = crypto::init() {
            error!("Failed to initialize crypto: {}", e);
            *self.status.write() = ServiceStatus::Error(e.to_string());
            return Err(e.into());
        }

        self.connection_manager.set_hid(self.hid.clone());

        self.connection_manager.set_audio(self.audio.clone());

        self.connection_manager
            .set_video_manager(self.video_manager.clone());

        if config.mode == RustDeskMode::DirectIp {
            if let Err(error) = self.prepare_direct_identity() {
                *self.status.write() = ServiceStatus::Error(error.to_string());
                return Err(error);
            }
            let listen_port = match self
                .ensure_tcp_listener(config.direct_access_port, false)
                .await
            {
                Ok(result) => result,
                Err(err) => {
                    *self.status.write() = ServiceStatus::Error(err.to_string());
                    return Err(err);
                }
            };
            *self.listen_port.write() = listen_port;
            *self.status.write() = ServiceStatus::Running;
            return Ok(());
        }

        let mediator = Arc::new(RendezvousMediator::new(config.clone()));

        let keypair = mediator.ensure_keypair();
        self.connection_manager.set_keypair(keypair);

        let signing_keypair = mediator.ensure_signing_keypair();
        self.connection_manager.set_signing_keypair(signing_keypair);

        *self.rendezvous.write() = Some(mediator.clone());

        let connection_manager = self.connection_manager.clone();
        let service_config = self.config.clone();
        let connection_attempts = Arc::new(Semaphore::new(MAX_PENDING_CONNECTION_ATTEMPTS));

        mediator.set_punch_callback(Arc::new({
            let connection_manager = connection_manager.clone();
            let service_config = service_config.clone();
            let connection_attempts = connection_attempts.clone();
            move |peer_addr, rendezvous_addr, relay_server, uuid, socket_addr, device_id| {
                let conn_mgr = connection_manager.clone();
                let config = service_config.clone();
                let attempts = connection_attempts.clone();
                tokio::spawn(async move {
                    let Ok(_permit) = attempts.try_acquire_owned() else {
                        warn!(
                            "Dropping RustDesk punch request: too many pending connection attempts"
                        );
                        return;
                    };
                    if let Some(addr) = peer_addr {
                        info!("Attempting P2P direct connection to {}", addr);
                        match punch::try_direct_connection(addr).await {
                            punch::PunchResult::DirectConnection(stream) => {
                                info!("P2P direct connection succeeded to {}", addr);
                                if let Err(e) = conn_mgr.accept_connection(stream, addr).await {
                                    error!("Failed to accept P2P connection: {}", e);
                                }
                                return;
                            }
                            punch::PunchResult::NeedRelay => {
                                info!("P2P direct connection failed, falling back to relay");
                            }
                        }
                    }

                    let relay_key = rustdesk_relay_key(&config);
                    if let Err(e) = handle_relay_request(
                        &rendezvous_addr,
                        &relay_server,
                        &uuid,
                        &socket_addr,
                        &device_id,
                        &relay_key,
                        conn_mgr,
                    )
                    .await
                    {
                        error!("Failed to handle relay request: {}", e);
                    }
                });
            }
        }));

        mediator.set_relay_callback(Arc::new({
            let connection_manager = connection_manager.clone();
            let service_config = service_config.clone();
            let connection_attempts = connection_attempts.clone();
            move |rendezvous_addr, relay_server, uuid, socket_addr, device_id| {
                let conn_mgr = connection_manager.clone();
                let config = service_config.clone();
                let attempts = connection_attempts.clone();
                tokio::spawn(async move {
                    let Ok(_permit) = attempts.try_acquire_owned() else {
                        warn!(
                            "Dropping RustDesk relay request: too many pending connection attempts"
                        );
                        return;
                    };
                    let relay_key = rustdesk_relay_key(&config);
                    if let Err(e) = handle_relay_request(
                        &rendezvous_addr,
                        &relay_server,
                        &uuid,
                        &socket_addr,
                        &device_id,
                        &relay_key,
                        conn_mgr,
                    )
                    .await
                    {
                        error!("Failed to handle relay request: {}", e);
                    }
                });
            }
        }));

        let weak_service = Arc::downgrade(self);
        let intranet_attempts = connection_attempts.clone();
        mediator.set_intranet_callback(Arc::new(
            move |rendezvous_addr, peer_socket_addr, local_ip, relay_server, device_id| {
                let weak_service = weak_service.clone();
                let attempts = intranet_attempts.clone();
                tokio::spawn(async move {
                    let Ok(_permit) = attempts.try_acquire_owned() else {
                        warn!("Dropping RustDesk intranet request: too many pending connection attempts");
                        return;
                    };
                    let Some(service) = weak_service.upgrade() else {
                        return;
                    };
                    let preferred_port = service.config.read().direct_access_port;
                    let listen_port = match service
                        .ensure_tcp_listener(preferred_port, true)
                        .await
                    {
                        Ok(port) => port,
                        Err(error) => {
                            error!("Failed to start on-demand RustDesk listener: {}", error);
                            return;
                        }
                    };
                    let local_addr = SocketAddr::new(local_ip, listen_port);
                    if let Err(e) = handle_intranet_request(
                        &rendezvous_addr,
                        &peer_socket_addr,
                        local_addr,
                        &relay_server,
                        &device_id,
                        service.connection_manager.clone(),
                    )
                    .await
                    {
                        error!("Failed to handle intranet request: {}", e);
                    }
                });
            },
        ));

        let handle = tokio::spawn(async move {
            loop {
                match mediator.start().await {
                    Ok(_) => {
                        info!("Rendezvous mediator stopped normally");
                        break;
                    }
                    Err(e) => {
                        error!("Rendezvous mediator error: {}", e);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });

        *self.rendezvous_handle.write() = Some(handle);

        *self.status.write() = ServiceStatus::Running;

        Ok(())
    }

    async fn ensure_tcp_listener(
        self: &Arc<Self>,
        preferred_port: u16,
        allow_ephemeral_fallback: bool,
    ) -> anyhow::Result<u16> {
        let _guard = self.listener_start_lock.lock().await;
        if self.tcp_listener_handle.read().is_some() {
            return Ok(*self.listen_port.read());
        }
        if self.status() == ServiceStatus::Stopped {
            anyhow::bail!("RustDesk service stopped before listener could start");
        }

        let (listeners, listen_port) = match self.bind_direct_listeners(preferred_port) {
            Ok(result) => result,
            Err(error) if allow_ephemeral_fallback => {
                warn!(
                    "RustDesk port {} unavailable for on-demand listening: {}; using an ephemeral port",
                    preferred_port, error
                );
                self.bind_direct_listeners(0)?
            }
            Err(error) => return Err(error),
        };

        *self.listen_port.write() = listen_port;

        let connection_manager = self.connection_manager.clone();
        let mut handles = Vec::new();

        for listener in listeners {
            let local_addr = listener.local_addr()?;
            info!("RustDesk TCP listener started on {}", local_addr);

            let conn_mgr = connection_manager.clone();
            let mut shutdown_rx = self.shutdown_tx.subscribe();
            let handle = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        result = listener.accept() => {
                            match result {
                                Ok((stream, peer_addr)) => {
                                    info!("Accepted direct connection from {}", peer_addr);
                                    if let Err(e) = conn_mgr.accept_listener_connection(stream, peer_addr).await {
                                        warn!("Rejected direct connection from {}: {}", peer_addr, e);
                                    }
                                }
                                Err(e) => {
                                    error!("TCP accept error: {}", e);
                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                }
                            }
                        }
                        _ = shutdown_rx.recv() => {
                            info!("TCP listener shutting down");
                            break;
                        }
                    }
                }
            });
            handles.push(handle);
        }

        *self.tcp_listener_handle.write() = Some(handles);
        Ok(listen_port)
    }

    fn bind_direct_listeners(&self, port: u16) -> anyhow::Result<(Vec<TcpListener>, u16)> {
        let v4_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
        let v4_listener = bind_tcp_listener(v4_addr)?;
        let listen_port = v4_listener.local_addr()?.port();

        let mut listeners = vec![TcpListener::from_std(v4_listener)?];

        let v6_addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), listen_port);
        match bind_tcp_listener(v6_addr) {
            Ok(v6_listener) => {
                listeners.push(TcpListener::from_std(v6_listener)?);
            }
            Err(err) => {
                warn!(
                    "IPv6 listener unavailable on port {}: {}, continuing with IPv4 only",
                    listen_port, err
                );
            }
        }

        Ok((listeners, listen_port))
    }

    pub async fn stop(&self) -> anyhow::Result<()> {
        if self.status() == ServiceStatus::Stopped {
            return Ok(());
        }

        info!("Stopping RustDesk service");

        let _ = self.shutdown_tx.send(());
        let _listener_guard = self.listener_start_lock.lock().await;
        *self.status.write() = ServiceStatus::Stopped;

        if let Some(mediator) = self.rendezvous.read().as_ref() {
            mediator.stop();
        }

        let rendezvous_handle = self.rendezvous_handle.write().take();
        if let Some(handle) = rendezvous_handle {
            wait_for_service_task(handle, "rendezvous").await;
        }

        let tcp_listener_handles = self.tcp_listener_handle.write().take();
        if let Some(handles) = tcp_listener_handles {
            for handle in handles {
                wait_for_service_task(handle, "TCP listener").await;
            }
        }

        // No listener can admit a new session after this point.
        self.connection_manager.close_all().await;

        *self.rendezvous.write() = None;

        Ok(())
    }

    pub async fn restart(self: &Arc<Self>, config: RustDeskConfig) -> anyhow::Result<()> {
        self.stop().await?;
        self.update_config(config);
        self.start().await
    }

    pub fn save_credentials(&self) -> Option<RustDeskConfig> {
        {
            let config = self.config.read();
            if config.mode == RustDeskMode::DirectIp {
                return Some(config.clone());
            }
        }
        if let Some(mediator) = self.rendezvous.read().as_ref() {
            let kp = mediator.ensure_keypair();
            let skp = mediator.ensure_signing_keypair();
            let mut config = self.config.write();
            let mut changed = false;

            let pk = kp.public_key_base64();
            let sk = kp.secret_key_base64();
            if config.public_key.as_ref() != Some(&pk) || config.private_key.as_ref() != Some(&sk) {
                config.public_key = Some(pk);
                config.private_key = Some(sk);
                changed = true;
            }

            let signing_pk = skp.public_key_base64();
            let signing_sk = skp.secret_key_base64();
            if config.signing_public_key.as_ref() != Some(&signing_pk)
                || config.signing_private_key.as_ref() != Some(&signing_sk)
            {
                config.signing_public_key = Some(signing_pk);
                config.signing_private_key = Some(signing_sk);
                changed = true;
            }

            if mediator.uuid_needs_save() {
                let mediator_config = mediator.config();
                if let Some(uuid) = mediator_config.uuid {
                    if config.uuid.as_ref() != Some(&uuid) {
                        config.uuid = Some(uuid);
                        changed = true;
                    }
                }
                mediator.mark_uuid_saved();
            }

            if changed {
                return Some(config.clone());
            }
        }
        None
    }

    #[deprecated(note = "Use save_credentials instead")]
    pub fn save_keypair(&self) {
        let _ = self.save_credentials();
    }
}

fn direct_signing_identity(config: &mut RustDeskConfig) -> anyhow::Result<SigningKeyPair> {
    match (&config.signing_public_key, &config.signing_private_key) {
        (Some(public), Some(secret)) => SigningKeyPair::from_base64(public, secret)
            .map_err(|error| anyhow::anyhow!("Stored native signing identity is damaged: {error}")),
        (None, None) => {
            let identity = SigningKeyPair::generate();
            config.signing_public_key = Some(identity.public_key_base64());
            config.signing_private_key = Some(identity.secret_key_base64());
            Ok(identity)
        }
        _ => anyhow::bail!("Stored native signing identity is incomplete"),
    }
}

async fn wait_for_service_task(mut handle: JoinHandle<()>, task_name: &str) {
    match tokio::time::timeout(SERVICE_SHUTDOWN_TIMEOUT, &mut handle).await {
        Ok(Ok(())) => {}
        Ok(Err(err)) if err.is_cancelled() => {}
        Ok(Err(err)) => warn!("RustDesk {} task ended with error: {}", task_name, err),
        Err(_) => {
            warn!("Timed out waiting for RustDesk {} task to stop", task_name);
            handle.abort();
            let _ = handle.await;
        }
    }
}

fn rustdesk_relay_key(config: &Arc<RwLock<RustDeskConfig>>) -> String {
    config.read().relay_key.clone().unwrap_or_default()
}

async fn handle_relay_request(
    rendezvous_addr: &str,
    relay_server: &str,
    uuid: &str,
    socket_addr: &[u8],
    device_id: &str,
    relay_key: &str,
    connection_manager: Arc<ConnectionManager>,
) -> anyhow::Result<()> {
    info!(
        "Handling relay request: rendezvous={}, relay={}, uuid={}",
        rendezvous_addr, relay_server, uuid
    );

    let rendezvous_socket_addr: SocketAddr = tokio::net::lookup_host(rendezvous_addr)
        .await?
        .next()
        .ok_or_else(|| {
            anyhow::anyhow!("Failed to resolve rendezvous server: {}", rendezvous_addr)
        })?;

    let mut rendezvous_stream = tokio::time::timeout(
        Duration::from_millis(RELAY_CONNECT_TIMEOUT_MS),
        TcpStream::connect(rendezvous_socket_addr),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Rendezvous connection timeout"))??;

    debug!(
        "Connected to rendezvous server at {}",
        rendezvous_socket_addr
    );

    // Rendezvous looks up our pk by device id (must set `id`, not raw pk on wire).
    let relay_response = make_relay_response(uuid, socket_addr, relay_server, device_id);
    let bytes = relay_response
        .write_to_bytes()
        .map_err(|e| anyhow::anyhow!("Failed to encode: {}", e))?;
    bytes_codec::write_frame(&mut rendezvous_stream, &bytes).await?;
    debug!("Sent RelayResponse to rendezvous server for uuid={}", uuid);

    drop(rendezvous_stream);

    let relay_addr: SocketAddr = tokio::net::lookup_host(relay_server)
        .await?
        .next()
        .ok_or_else(|| anyhow::anyhow!("Failed to resolve relay server: {}", relay_server))?;

    let mut stream = tokio::time::timeout(
        Duration::from_millis(RELAY_CONNECT_TIMEOUT_MS),
        TcpStream::connect(relay_addr),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Relay connection timeout"))??;

    info!("Connected to relay server at {}", relay_addr);

    // Relay pairs peers by uuid + mangled peer socket_addr (required when hbbr uses -k).
    let request_relay = make_request_relay(uuid, relay_key, socket_addr);
    let bytes = request_relay
        .write_to_bytes()
        .map_err(|e| anyhow::anyhow!("Failed to encode: {}", e))?;
    bytes_codec::write_frame(&mut stream, &bytes).await?;
    debug!("Sent RequestRelay to relay server for uuid={}", uuid);

    let peer_addr = rendezvous::AddrMangle::decode(socket_addr).unwrap_or(relay_addr);

    connection_manager
        .accept_connection(stream, peer_addr)
        .await?;
    info!(
        "Relay connection established for uuid={}, peer={}",
        uuid, peer_addr
    );

    Ok(())
}

async fn handle_intranet_request(
    rendezvous_addr: &str,
    peer_socket_addr: &[u8],
    local_addr: SocketAddr,
    relay_server: &str,
    device_id: &str,
    connection_manager: Arc<ConnectionManager>,
) -> anyhow::Result<()> {
    info!(
        "Handling intranet request: rendezvous={}, local_addr={}, device_id={}",
        rendezvous_addr, local_addr, device_id
    );

    let peer_addr = AddrMangle::decode(peer_socket_addr);
    debug!("Peer address from FetchLocalAddr: {:?}", peer_addr);

    let mut stream =
        tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(rendezvous_addr))
            .await
            .map_err(|_| anyhow::anyhow!("Timeout connecting to rendezvous server"))??;

    info!(
        "Connected to rendezvous server for intranet: {}",
        rendezvous_addr
    );

    let local_addr_bytes = AddrMangle::encode(local_addr);
    let msg = make_local_addr(
        peer_socket_addr,
        &local_addr_bytes,
        relay_server,
        device_id,
        env!("CARGO_PKG_VERSION"),
    );
    let bytes = msg
        .write_to_bytes()
        .map_err(|e| anyhow::anyhow!("Failed to encode: {}", e))?;

    bytes_codec::write_frame(&mut stream, &bytes).await?;

    info!("Sent LocalAddr to rendezvous server, waiting for peer connection");

    let effective_peer_addr = peer_addr.unwrap_or_else(|| {
        rendezvous_addr
            .parse()
            .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap())
    });

    connection_manager
        .accept_connection(stream, effective_peer_addr)
        .await?;
    info!("Intranet connection established via rendezvous server proxy");

    Ok(())
}

#[cfg(test)]
mod managed_identity_tests {
    use super::*;

    #[tokio::test]
    async fn native_signing_identity_survives_persisted_config_reload() {
        let directory = tempfile::tempdir().unwrap();
        let managed = one_kvm_management::ManagedKvm::open(directory.path())
            .await
            .unwrap();
        let make_service = |config| {
            #[cfg(unix)]
            let hid = HidController::new(crate::hid::HidBackendType::None, None);
            #[cfg(not(unix))]
            let hid = HidController::new(crate::hid::HidBackendType::None);
            RustDeskService::new(
                managed.clone(),
                config,
                VideoStreamManager::with_webrtc_streamer(
                    crate::video::Streamer::new(),
                    crate::webrtc::WebRtcStreamer::new(),
                ),
                Arc::new(hid),
                Arc::new(AudioController::new(Default::default())),
            )
        };
        let first = make_service(RustDeskConfig {
            mode: RustDeskMode::DirectIp,
            ..Default::default()
        });
        assert!(first.prepare_direct_identity().unwrap().is_some());
        let saved = serde_json::to_vec(&first.save_credentials().unwrap()).unwrap();
        let restored = make_service(serde_json::from_slice(&saved).unwrap());
        assert!(restored.prepare_direct_identity().unwrap().is_none());
        assert_eq!(
            first
                .connection_manager
                .ensure_signing_keypair()
                .public_key_bytes(),
            restored
                .connection_manager
                .ensure_signing_keypair()
                .public_key_bytes()
        );
        assert_eq!(
            saved,
            serde_json::to_vec(&restored.save_credentials().unwrap()).unwrap()
        );
        assert!(!first.is_listening() && !restored.is_listening());
        managed.shutdown().await.unwrap();
    }

    #[test]
    fn native_signing_identity_rejects_damage_without_replacing_keys() {
        crypto::init().unwrap();
        let first = SigningKeyPair::generate();
        let other = SigningKeyPair::generate();
        for (public, secret) in [
            (Some(first.public_key_base64()), None),
            (None, Some(first.secret_key_base64())),
            (Some("damaged".into()), Some(first.secret_key_base64())),
            (
                Some(first.public_key_base64()),
                Some(other.secret_key_base64()),
            ),
        ] {
            let mut config = RustDeskConfig {
                signing_public_key: public,
                signing_private_key: secret,
                ..Default::default()
            };
            let before = serde_json::to_vec(&config).unwrap();
            assert!(direct_signing_identity(&mut config).is_err());
            assert_eq!(before, serde_json::to_vec(&config).unwrap());
        }
    }
}
