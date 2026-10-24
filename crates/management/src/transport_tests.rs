use super::*;
use crate::{
    engine::{ExecutionRecord, JournalError},
    host::HostError,
};
use async_trait::async_trait;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{net::TcpListener, sync::Notify};
use tokio_tungstenite::{accept_async, connect_async};
use xscs_protocol::*;
#[test]
fn cleartext_management_connections_are_limited_to_loopback() {
    for endpoint in [
        "wss://manager.example/xscc/v2/connect",
        "ws://localhost:18104/xscc/v2/connect",
        "ws://127.0.0.1:18104/xscc/v2/connect",
        "ws://[::1]:18104/xscc/v2/connect",
    ] {
        assert!(ManagerConnection::new(endpoint, Zeroizing::new("a".repeat(64))).is_ok());
    }
    for endpoint in [
        "ws://manager.example/xscc/v2/connect",
        "ws://192.168.1.1:18104/xscc/v2/connect",
        "ws://localhost.example.com:18104/xscc/v2/connect",
        "ws://user:secret@localhost:18104/xscc/v2/connect",
        "ws://localhost:18104/xscc/v2/connect?token=x",
        "ws://localhost:18104/other",
    ] {
        assert!(ManagerConnection::new(endpoint, Zeroizing::new("a".repeat(64))).is_err());
    }
}

#[derive(Clone, Default)]
struct Facts(Arc<Mutex<BTreeMap<String, ExecutionRecord>>>);
#[async_trait]
impl Journal for Facts {
    async fn pending_results(&self) -> Result<Vec<(String, ExecutionRecord)>, JournalError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, v)| !v.acknowledged)
            .map(|(id, v)| (id.clone(), v.clone()))
            .collect())
    }
    async fn load(&self, id: &str) -> Result<Option<ExecutionRecord>, JournalError> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    async fn create(&self, id: &str, r: &ExecutionRecord) -> Result<(), JournalError> {
        assert!(
            self.0
                .lock()
                .unwrap()
                .insert(id.into(), r.clone())
                .is_none()
        );
        Ok(())
    }
    async fn replace(&self, id: &str, r: &ExecutionRecord) -> Result<(), JournalError> {
        self.0.lock().unwrap().insert(id.into(), r.clone());
        Ok(())
    }
}
struct Host {
    started: Arc<Notify>,
    release: Arc<Notify>,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl RustDeskHost for Host {
    async fn diagnostics(&self) -> Result<DiagnosticSnapshot, HostError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        self.release.notified().await;
        Ok(DiagnosticSnapshot::awaiting_integration())
    }
}
type Peer = WebSocketStream<TcpStream>;
async fn sockets() -> (Socket, Peer) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (client, server) = tokio::join!(connect_async(format!("ws://{address}")), async {
        let (stream, _) = listener.accept().await.unwrap();
        accept_async(stream).await.unwrap()
    });
    (client.unwrap().0, server)
}
async fn receive(peer: &mut Peer) -> ClientMessage {
    loop {
        let frame = peer.next().await.unwrap().unwrap();
        if let Message::Text(text) = frame {
            let message = serde_json::from_str(&text).unwrap();
            if !matches!(message, ClientMessage::Heartbeat { .. }) {
                return message;
            }
        }
    }
}
async fn transmit(peer: &mut Peer, message: ManagerMessage) {
    peer.send(Message::Text(
        serde_json::to_string(&message).unwrap().into(),
    ))
    .await
    .unwrap()
}
#[tokio::test]
async fn embedded_execution_survives_socket_replacement_and_commits_the_receipt() {
    timeout(Duration::from_secs(15), async {
        let binding = Binding {
            manager_id: uuid::Uuid::new_v4(),
            device_id: uuid::Uuid::new_v4(),
            installation_id: uuid::Uuid::new_v4(),
        };
        let caps = Capabilities {
            protocol: PROTOCOL.into(),
            client_version: "test".into(),
            os: ClientOs::LinuxX86_64,
            backend: Backend::Rustdesk,
            execution_mode: ExecutionMode::Embedded,
            diagnostics: true,
        };
        let task = Task {
            protocol: TASK_PROTOCOL.into(),
            operation_id: format!("op_{}", uuid::Uuid::new_v4()),
            binding: binding.clone(),
            permission: Permission::ReadDiagnostics,
            command: Command::ReadDiagnostics {},
        };
        let facts = Facts::default();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let executor = Arc::new(Executor::new_with_capabilities(
            binding.clone(),
            caps.clone(),
            Host {
                started: started.clone(),
                release: release.clone(),
                calls: calls.clone(),
            },
            facts.clone(),
        ));
        let connection = ManagerConnection::new(
            "wss://manager.example/xscc/v2/connect",
            Zeroizing::new("a".repeat(64)),
        )
        .unwrap();
        let (_health, receiver) = watch::channel(HealthObservation::default());
        let mut access_receiver = connection.access_policy();
        let policy = AccessPolicy {
            binding: binding.clone(),
            role: InstanceRole::Controller,
            valid_for_ms: ACCESS_LEASE_MS,
            peers: vec![],
        };
        let (client, mut peer) = sockets().await;
        let first = tokio::spawn(async move {
            let mut state = ExecutionState::default();
            let outcome = connection
                .session_connected(
                    client,
                    &binding,
                    &caps,
                    executor.clone(),
                    &receiver,
                    &mut state,
                )
                .await;
            assert!(outcome.is_err());
            (state, connection, binding, caps, executor, receiver)
        });
        assert!(matches!(
            receive(&mut peer).await,
            ClientMessage::Hello { .. }
        ));
        transmit(
            &mut peer,
            ManagerMessage::AccessPolicy {
                policy: Box::new(policy),
            },
        )
        .await;
        timeout(Duration::from_secs(1), async {
            loop {
                access_receiver.changed().await.unwrap();
                if access_receiver.borrow().is_some() {
                    break;
                }
            }
        })
        .await
        .unwrap();
        let retained = access_receiver.borrow().clone().unwrap();
        assert!(retained.policy().is_some());
        let expires = (std::time::SystemTime::now() + Duration::from_secs(10))
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        transmit(
            &mut peer,
            ManagerMessage::Task {
                mode: DeliveryMode::Execute,
                task: Box::new(task.clone()),
                expires_at_unix_ms: expires,
                remaining_ms: 10_000,
            },
        )
        .await;
        // Execute may arrive after the first heartbeat; observe actual host entry
        // instead of requiring a periodic Progress frame before its 10s budget.
        started.notified().await;
        drop(peer);
        let (mut state, connection, binding, caps, executor, receiver) = first.await.unwrap();
        assert!(state.active.is_some());
        assert!(access_receiver.borrow().is_none());
        assert!(retained.policy().is_none());
        release.notify_one();
        let (client, mut peer) = sockets().await;
        let second = tokio::spawn(async move {
            let result = connection
                .session_connected(client, &binding, &caps, executor, &receiver, &mut state)
                .await;
            state.shutdown().await.unwrap();
            result
        });
        assert!(matches!(
            receive(&mut peer).await,
            ClientMessage::Hello {
                active_operation: Some(_),
                ..
            }
        ));
        let ClientMessage::Result {
            operation_id,
            report,
        } = receive(&mut peer).await
        else {
            panic!("expected persisted result")
        };
        assert_eq!(operation_id, task.operation_id);
        assert_eq!(
            report,
            Report::DiagnosticsRead {
                snapshot: DiagnosticSnapshot::awaiting_integration()
            }
        );
        transmit(
            &mut peer,
            ManagerMessage::ResultAccepted {
                operation_id: operation_id.clone(),
                fingerprint: task.fingerprint().unwrap(),
                report_digest: xscs_protocol::report_digest(&report).unwrap(),
                finalized: true,
            },
        )
        .await;
        transmit(&mut peer, ManagerMessage::Revoked {}).await;
        assert!(matches!(
            second.await.unwrap(),
            Err(TransportError::Revoked)
        ));
        assert!(
            facts
                .load(&operation_id)
                .await
                .unwrap()
                .unwrap()
                .acknowledged
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    })
    .await
    .unwrap();
}

#[test]
fn permission_observations_expire_and_replacements_invalidate_copies() {
    let binding = Binding {
        manager_id: uuid::Uuid::new_v4(),
        device_id: uuid::Uuid::new_v4(),
        installation_id: uuid::Uuid::new_v4(),
    };
    let peer = Binding {
        manager_id: binding.manager_id,
        device_id: uuid::Uuid::new_v4(),
        installation_id: uuid::Uuid::new_v4(),
    };
    let policy = AccessPolicy {
        binding,
        role: InstanceRole::Controlled,
        valid_for_ms: ACCESS_LEASE_MS,
        peers: vec![AccessPeer {
            binding: peer.clone(),
            name: "Controller".into(),
            session_key: SessionKey::from_bytes([7; 32]),
            role: InstanceRole::Controller,
            online: true,
            addresses: vec![],
        }],
    };
    let observation = AccessObservation {
        policy,
        received_at: Instant::now(),
        current: Arc::new(AtomicBool::new(true)),
    };
    assert!(observation.permits(&peer));
    let wrong = Binding {
        installation_id: uuid::Uuid::new_v4(),
        ..peer.clone()
    };
    assert!(!observation.permits(&wrong));
    let expired = AccessObservation {
        received_at: Instant::now() - Duration::from_millis(ACCESS_LEASE_MS),
        ..observation.clone()
    };
    assert!(!expired.permits(&peer));
    let (sender, _receiver) = watch::channel(Some(observation.clone()));
    publish_policy(&sender, None);
    assert!(!observation.permits(&peer));
}

#[tokio::test]
async fn address_heartbeats_follow_role_and_observation_freshness() {
    async fn scenario(role: InstanceRole, fresh: bool) {
        let binding = Binding {
            manager_id: uuid::Uuid::new_v4(),
            device_id: uuid::Uuid::new_v4(),
            installation_id: uuid::Uuid::new_v4(),
        };
        let caps = Capabilities {
            protocol: PROTOCOL.into(),
            client_version: "test".into(),
            os: ClientOs::LinuxX86_64,
            backend: Backend::Rustdesk,
            execution_mode: ExecutionMode::Embedded,
            diagnostics: true,
        };
        let executor = Arc::new(Executor::new_with_capabilities(
            binding.clone(),
            caps.clone(),
            Host {
                started: Arc::new(Notify::new()),
                release: Arc::new(Notify::new()),
                calls: Arc::new(AtomicUsize::new(0)),
            },
            Facts::default(),
        ));
        let connection = ManagerConnection::new(
            "wss://manager.example/xscc/v2/connect",
            Zeroizing::new("a".repeat(64)),
        )
        .unwrap();
        let access = connection.access_policy();
        let addresses = vec![HostAddress {
            host: "host.example.com".into(),
            port: 21118,
        }];
        let (_health, receiver) = watch::channel(HealthObservation {
            diagnostics: None,
            addresses: addresses.clone(),
            observed_at: Some(
                Instant::now()
                    - if fresh {
                        Duration::ZERO
                    } else {
                        Duration::from_secs(31)
                    },
            ),
        });
        let (client, mut peer) = sockets().await;
        let policy = AccessPolicy {
            binding: binding.clone(),
            role,
            valid_for_ms: ACCESS_LEASE_MS,
            peers: vec![],
        };
        let running = tokio::spawn(async move {
            connection
                .session_connected(
                    client,
                    &binding,
                    &caps,
                    executor,
                    &receiver,
                    &mut ExecutionState::default(),
                )
                .await
        });
        // Drain the initial heartbeat, which precedes a role supplied by xscs.
        loop {
            let frame = peer.next().await.unwrap().unwrap();
            if let Message::Text(text) = frame
                && matches!(
                    serde_json::from_str::<ClientMessage>(&text).unwrap(),
                    ClientMessage::Heartbeat { .. }
                )
            {
                break;
            }
        }
        transmit(
            &mut peer,
            ManagerMessage::AccessPolicy {
                policy: Box::new(policy),
            },
        )
        .await;
        loop {
            match peer.next().await.unwrap().unwrap() {
                Message::Text(text) => {
                    if let ClientMessage::Heartbeat {
                        addresses: actual, ..
                    } = serde_json::from_str(&text).unwrap()
                    {
                        assert_eq!(
                            actual,
                            if fresh && role == InstanceRole::Controlled {
                                addresses
                            } else {
                                vec![]
                            }
                        );
                        break;
                    }
                }
                Message::Ping(bytes) => {
                    peer.send(Message::Pong(bytes)).await.unwrap();
                }
                _ => {}
            }
        }
        transmit(&mut peer, ManagerMessage::Revoked {}).await;
        assert_eq!(running.await.unwrap(), Err(TransportError::Revoked));
        assert!(access.borrow().is_none());
    }
    timeout(Duration::from_secs(25), async {
        tokio::join!(
            scenario(InstanceRole::Controlled, true),
            scenario(InstanceRole::Controller, true),
            scenario(InstanceRole::Controlled, false)
        );
    })
    .await
    .unwrap();
}
