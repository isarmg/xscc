use axum::{
    Json, Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use futures_util::StreamExt;
use one_kvm_management::{
    ManagedKvm, SessionPermit, Transcript, nonce,
    protocol::{
        self, AccessPeer, AccessPolicy, Backend, Binding, ClientMessage, HostAddress, InstanceRole,
        ManagerMessage, SessionKey,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::watch;
use uuid::Uuid;

struct Row {
    binding: Option<Binding>,
    credential: Option<String>,
    online: bool,
    role: InstanceRole,
}
struct Mock {
    manager: Uuid,
    host: Uuid,
    second_host: Uuid,
    controller: Uuid,
    rows: Mutex<BTreeMap<Uuid, Row>>,
    allowed: Mutex<bool>,
    unbind_fails: AtomicBool,
    updates: watch::Sender<u64>,
}
impl Mock {
    fn new() -> Arc<Self> {
        let host = Uuid::new_v4();
        let controller = Uuid::new_v4();
        let second_host = Uuid::new_v4();
        let (updates, _) = watch::channel(0);
        Arc::new(Self {
            manager: Uuid::new_v4(),
            host,
            second_host,
            controller,
            rows: Mutex::new(BTreeMap::from([
                (
                    second_host,
                    Row {
                        binding: None,
                        credential: None,
                        online: false,
                        role: InstanceRole::Controlled,
                    },
                ),
                (
                    host,
                    Row {
                        binding: None,
                        credential: None,
                        online: false,
                        role: InstanceRole::Controlled,
                    },
                ),
                (
                    controller,
                    Row {
                        binding: None,
                        credential: None,
                        online: false,
                        role: InstanceRole::Controller,
                    },
                ),
            ])),
            allowed: Mutex::new(true),
            unbind_fails: AtomicBool::new(false),
            updates,
        })
    }
    fn update(&self) {
        self.updates.send_modify(|revision| *revision += 1);
    }
    fn id_for_code(&self, code: &str) -> Option<Uuid> {
        match code {
            value if value == "a".repeat(36) => Some(self.host),
            value if value == "b".repeat(36) => Some(self.controller),
            value if value == "c".repeat(36) => Some(self.second_host),
            _ => None,
        }
    }
    fn bearer(&self, headers: &HeaderMap) -> Option<Uuid> {
        let value = headers
            .get("authorization")?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")?;
        self.rows
            .lock()
            .unwrap()
            .iter()
            .find(|(_, row)| row.credential.as_deref() == Some(value))
            .map(|(id, _)| *id)
    }
    fn policy(&self, id: Uuid) -> AccessPolicy {
        let rows = self.rows.lock().unwrap();
        let own = &rows[&id];
        let peers = if *self.allowed.lock().unwrap() {
            rows.iter()
                .filter(|(other, row)| {
                    **other != id
                        && row.role != own.role
                        && row.binding.is_some()
                        && row.credential.is_some()
                })
                .map(|(_, row)| {
                    let own_credential = own.credential.as_deref().unwrap_or_default();
                    let peer_credential = row.credential.as_deref().unwrap();
                    let (controller, controlled) = if own.role == InstanceRole::Controller {
                        (own_credential, peer_credential)
                    } else {
                        (peer_credential, own_credential)
                    };
                    let bytes: [u8; 32] = Sha256::digest(
                        [controller.as_bytes(), b"\0", controlled.as_bytes()].concat(),
                    )
                    .into();
                    AccessPeer {
                        binding: row.binding.clone().unwrap(),
                        session_key: SessionKey::from_bytes(bytes),
                        name: "peer".into(),
                        role: row.role,
                        online: row.online,
                        addresses: Vec::new(),
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        AccessPolicy {
            binding: own.binding.clone().unwrap(),
            role: own.role,
            valid_for_ms: protocol::ACCESS_LEASE_MS,
            peers,
        }
    }
}
async fn pairing(
    State(state): State<Arc<Mock>>,
    Json(input): Json<Value>,
) -> axum::response::Response {
    let Some(id) = input["authorization_code"]
        .as_str()
        .and_then(|code| state.id_for_code(code))
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let rows = state.rows.lock().unwrap();
    Json(json!({"manager_id":state.manager,"device_id":id,"role":rows[&id].role,"backend":"one_kvm"})).into_response()
}
async fn enroll(
    State(state): State<Arc<Mock>>,
    Json(input): Json<Value>,
) -> axum::response::Response {
    let Some(id) = input["token"]
        .as_str()
        .and_then(|code| state.id_for_code(code))
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let installation = Uuid::parse_str(input["installation_id"].as_str().unwrap()).unwrap();
    let binding = Binding {
        manager_id: state.manager,
        device_id: id,
        installation_id: installation,
    };
    let mut rows = state.rows.lock().unwrap();
    let row = rows.get_mut(&id).unwrap();
    if row.credential.is_some() {
        return StatusCode::CONFLICT.into_response();
    }
    row.binding = Some(binding.clone());
    row.credential = Some(input["credential"].as_str().unwrap().into());
    Json(binding).into_response()
}
async fn identity(State(state): State<Arc<Mock>>, headers: HeaderMap) -> axum::response::Response {
    let Some(id) = state.bearer(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    Json(state.rows.lock().unwrap()[&id].binding.clone().unwrap()).into_response()
}
async fn unbind(State(state): State<Arc<Mock>>, headers: HeaderMap) -> axum::response::Response {
    if state.unbind_fails.load(Ordering::Acquire) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let Some(id) = state.bearer(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    {
        let mut rows = state.rows.lock().unwrap();
        let row = rows.get_mut(&id).unwrap();
        row.credential = None;
        row.online = false;
    }
    state.update();
    StatusCode::NO_CONTENT.into_response()
}
async fn connect(
    State(state): State<Arc<Mock>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> axum::response::Response {
    let Some(id) = state.bearer(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    upgrade
        .protocols([protocol::WEBSOCKET_SUBPROTOCOL])
        .on_upgrade(move |socket| socket_loop(socket, state, id))
}
async fn socket_loop(mut socket: WebSocket, state: Arc<Mock>, id: Uuid) {
    let Some(Ok(Message::Text(hello))) = socket.next().await else {
        return;
    };
    let ClientMessage::Hello {
        binding,
        capabilities,
        ..
    } = serde_json::from_str(&hello).unwrap()
    else {
        panic!("expected hello")
    };
    assert_eq!(capabilities.backend, Backend::OneKvm);
    assert_eq!(
        state.rows.lock().unwrap()[&id].binding.as_ref(),
        Some(&binding)
    );
    state.rows.lock().unwrap().get_mut(&id).unwrap().online = true;
    let mut updates = state.updates.subscribe();
    state.update();
    let mut tick = tokio::time::interval(Duration::from_secs(2));
    loop {
        let message = ManagerMessage::AccessPolicy {
            policy: Box::new(state.policy(id)),
        };
        if socket
            .send(Message::Text(
                serde_json::to_string(&message).unwrap().into(),
            ))
            .await
            .is_err()
        {
            break;
        }
        tokio::select! {
            message=socket.next()=>match message {Some(Ok(Message::Ping(bytes)))=>{if socket.send(Message::Pong(bytes)).await.is_err(){break;}},Some(Ok(Message::Text(_)))|Some(Ok(Message::Pong(_)))=>{},_=>break},
            _=updates.changed()=>{},
            _=tick.tick()=>{}
        }
    }
    state.rows.lock().unwrap().get_mut(&id).unwrap().online = false;
    state.update();
}
async fn fixture() -> (Arc<Mock>, String, tokio::task::JoinHandle<()>) {
    let state = Mock::new();
    let router = Router::new()
        .route(protocol::PAIRING_PATH, post(pairing))
        .route(protocol::ENROLL_PATH, post(enroll))
        .route(protocol::IDENTITY_PATH, get(identity))
        .route(protocol::UNBIND_PATH, post(unbind))
        .route(protocol::CONNECT_PATH, get(connect))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (state, format!("http://{address}"), server)
}

#[tokio::test]
async fn bilateral_browser_authorization_revocation_logout_and_restart_recovery() {
    let (state, server, task) = fixture().await;
    let temp = tempfile::tempdir().unwrap();
    let manager = ManagedKvm::open(temp.path()).await.unwrap();
    assert!(!manager.public_status().configured);
    assert!(matches!(
        manager
            .set_addresses(vec![HostAddress {
                host: "http://bad".into(),
                port: 0
            }])
            .await,
        Err(one_kvm_management::Error::Invalid)
    ));
    assert!(manager.login(&"b".repeat(36)).await.is_err());
    manager
        .bind_device(&server, &"a".repeat(36), Vec::<HostAddress>::new())
        .await
        .unwrap();
    assert!(matches!(
        manager.login("invalid code").await,
        Err(one_kvm_management::Error::Invalid)
    ));
    assert!(matches!(
        manager.login(&"c".repeat(36)).await,
        Err(one_kvm_management::Error::Denied)
    ));
    assert!(matches!(
        manager.login(&"a".repeat(36)).await,
        Err(one_kvm_management::Error::Denied)
    ));
    assert_eq!(
        std::fs::read_dir(temp.path().join("xscs-managed/controllers"))
            .unwrap()
            .count(),
        0
    );
    let login = manager.login(&"b".repeat(36)).await.unwrap();
    assert_eq!(login.controller.device_id, state.controller);
    assert_eq!(login.controlled.device_id, state.host);
    let addresses = vec![HostAddress {
        host: "127.0.0.1".into(),
        port: 21118,
    }];
    manager.set_addresses(addresses.clone()).await.unwrap();
    assert_eq!(manager.device_status().addresses, addresses);
    let grant = manager.authorize(&login.cookie_token).unwrap();
    assert!(grant.csrf_matches(&login.csrf_token));
    assert!(!grant.csrf_matches("wrong"));
    let serialized = serde_json::to_string(&login).unwrap();
    assert!(!serialized.contains(&login.cookie_token));
    assert!(!serialized.contains("session_key"));
    let permit = SessionPermit {
        controller: login.controller.clone(),
        controlled: login.controlled.clone(),
        epoch: grant.permit().epoch.clone(),
    };
    let key = manager.native_grant(&permit).unwrap();
    let transcript = Transcript {
        permit,
        client_nonce: nonce().unwrap(),
        host_nonce: nonce().unwrap(),
        transport_host_key: nonce().unwrap(),
    };
    transcript
        .verify(&key, true, &transcript.proof(&key, true).unwrap())
        .unwrap();
    *state.allowed.lock().unwrap() = false;
    state.update();
    tokio::time::timeout(Duration::from_secs(3), grant.cancelled())
        .await
        .unwrap();
    assert!(manager.authorize(&login.cookie_token).is_err());
    *state.allowed.lock().unwrap() = true;
    state.update();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !manager.native_permits(&transcript.permit) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!grant.is_valid());
    assert!(manager.authorize(&login.cookie_token).is_err());
    let reauthorized = manager.login(&"b".repeat(36)).await.unwrap();
    assert_eq!(reauthorized.controller, login.controller);
    assert_ne!(reauthorized.cookie_token, login.cookie_token);
    assert!(manager.authorize(&login.cookie_token).is_err());
    assert!(!grant.is_valid());
    assert!(
        manager
            .authorize(&reauthorized.cookie_token)
            .unwrap()
            .is_valid()
    );
    manager.shutdown().await.unwrap();
    drop(grant);
    drop(manager);
    let manager = ManagedKvm::open(temp.path()).await.unwrap();
    assert!(manager.authorize(&login.cookie_token).is_err());
    assert!(manager.authorize(&reauthorized.cookie_token).is_err());
    let recovered = manager.login(&"b".repeat(36)).await.unwrap();
    assert_eq!(recovered.controller, login.controller);
    assert_eq!(recovered.controlled, login.controlled);
    let grant = manager.authorize(&recovered.cookie_token).unwrap();
    manager.logout(&recovered.cookie_token).await.unwrap();
    assert!(!grant.is_valid());
    assert!(
        state.rows.lock().unwrap()[&state.controller]
            .credential
            .is_none()
    );
    let old_permit = grant.permit().clone();
    let rebound = manager.login(&"b".repeat(36)).await.unwrap();
    assert_eq!(rebound.controller, recovered.controller);
    assert_ne!(rebound.cookie_token, recovered.cookie_token);
    assert_ne!(rebound.csrf_token, recovered.csrf_token);
    assert!(manager.authorize(&recovered.cookie_token).is_err());
    assert!(!grant.is_valid());
    assert!(!manager.native_permits(&old_permit));
    assert!(manager.authorize(&rebound.cookie_token).unwrap().is_valid());
    manager.shutdown().await.unwrap();
    task.abort();
}

#[tokio::test]
async fn failed_logout_stays_local_revoked_and_finishes_after_restart() {
    let (state, server, task) = fixture().await;
    let temp = tempfile::tempdir().unwrap();
    let manager = ManagedKvm::open(temp.path()).await.unwrap();
    manager
        .bind_device(&server, &"a".repeat(36), Vec::new())
        .await
        .unwrap();
    let login = manager.login(&"b".repeat(36)).await.unwrap();
    let grant = manager.authorize(&login.cookie_token).unwrap();
    state.unbind_fails.store(true, Ordering::Release);
    assert!(manager.logout(&login.cookie_token).await.is_err());
    assert!(!grant.is_valid());
    manager.shutdown().await.unwrap();
    drop(grant);
    drop(manager);
    state.unbind_fails.store(false, Ordering::Release);
    let manager = ManagedKvm::open(temp.path()).await.unwrap();
    assert!(manager.authorize(&login.cookie_token).is_err());
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let pending = std::fs::read_dir(temp.path().join("xscs-managed/controllers"))
                .unwrap()
                .any(|entry| entry.unwrap().path().join("logout.pending").exists());
            if state.rows.lock().unwrap()[&state.controller]
                .credential
                .is_none()
                && !pending
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    manager.shutdown().await.unwrap();
    task.abort();
}

#[tokio::test]
async fn shared_controller_identity_keeps_instance_sessions_isolated() {
    let (state, server, task) = fixture().await;
    let root = tempfile::tempdir().unwrap();
    let second_dir = root.path().join("second");
    std::fs::create_dir(&second_dir).unwrap();
    let pool = one_kvm_management::BrowserControllerPool::open(root.path())
        .await
        .unwrap();
    let first = ManagedKvm::open_with_controllers(root.path(), pool.clone())
        .await
        .unwrap();
    let second = ManagedKvm::open_with_controllers(&second_dir, pool.clone())
        .await
        .unwrap();
    first
        .bind_device(&server, &"a".repeat(36), Vec::new())
        .await
        .unwrap();
    second
        .bind_device(&server, &"c".repeat(36), Vec::new())
        .await
        .unwrap();
    let a = first.login(&"b".repeat(36)).await.unwrap();
    let grant_a = first.authorize(&a.cookie_token).unwrap();
    let b = second.login(&"b".repeat(36)).await.unwrap();
    assert_eq!(a.controller, b.controller);
    assert_ne!(a.controlled, b.controlled);
    assert_eq!(b.controlled.device_id, state.second_host);
    assert!(grant_a.is_valid());
    assert!(first.authorize(&b.cookie_token).is_err());
    assert!(second.authorize(&a.cookie_token).is_err());
    assert!(first.logout(&b.cookie_token).await.is_err());
    let grant_b = second.authorize(&b.cookie_token).unwrap();
    first.logout(&a.cookie_token).await.unwrap();
    assert!(!grant_a.is_valid());
    assert!(grant_b.is_valid());
    assert!(
        state.rows.lock().unwrap()[&state.controller]
            .credential
            .is_some()
    );
    second.logout(&b.cookie_token).await.unwrap();
    assert!(!grant_b.is_valid());
    assert!(
        state.rows.lock().unwrap()[&state.controller]
            .credential
            .is_none()
    );
    first.shutdown().await.unwrap();
    second.shutdown().await.unwrap();
    pool.shutdown().await.unwrap();
    task.abort();
}
