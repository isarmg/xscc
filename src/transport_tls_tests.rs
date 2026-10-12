//! Real TLS/WSS coverage of the production session and Task delivery paths.
use super::*;
use crate::{
    adapter::{AdapterError, Configuration},
    engine::{EffectIntent, ExecutionRecord, JournalError},
};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::net::TcpListener;
use tokio_rustls::{
    TlsAcceptor, TlsConnector,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject},
    },
};
use xscs_protocol::{ClientOs, Command, DeliveryMode, Permission, Task, Uncertainty};

mod tls_certificates {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/tls_certificates.rs"
    ));
}

type ManagerSocket = WebSocketStream<tokio_rustls::server::TlsStream<TcpStream>>;

async fn socket_pair() -> (ManagerConnection, Socket, ManagerSocket) {
    let temporary = tempfile::tempdir().unwrap();
    tls_certificates::certificates(temporary.path());
    let cert = CertificateDer::from_pem_file(temporary.path().join("server.pem")).unwrap();
    let key = PrivateKeyDer::from_pem_file(temporary.path().join("server.key")).unwrap();
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let manager = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let tls = acceptor.accept(stream).await.unwrap();
        #[allow(
            clippy::result_large_err,
            reason = "Tungstenite requires its fixed HTTP ErrorResponse in the handshake callback"
        )]
        let socket = tokio_tungstenite::accept_hdr_async(
            tls,
            |request: &tungstenite::handshake::server::Request,
             mut response: tungstenite::handshake::server::Response| {
                assert_eq!(request.uri().path(), CONNECT_PATH);
                assert_eq!(
                    request.headers()[header::SEC_WEBSOCKET_PROTOCOL],
                    WEBSOCKET_SUBPROTOCOL
                );
                assert_eq!(
                    request.headers()[header::AUTHORIZATION],
                    format!("Bearer {}", "a".repeat(64))
                );
                response.headers_mut().insert(
                    header::SEC_WEBSOCKET_PROTOCOL,
                    header::HeaderValue::from_static(WEBSOCKET_SUBPROTOCOL),
                );
                Ok(response)
            },
        )
        .await
        .unwrap();
        socket
    });
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from_pem_file(temporary.path().join("ca.pem")).unwrap())
        .unwrap();
    let connector = TlsConnector::from(Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ));
    let stream = TcpStream::connect(address).await.unwrap();
    let tls = connector
        .connect(ServerName::IpAddress(address.ip().into()), stream)
        .await
        .unwrap();
    let endpoint = format!("wss://{address}{CONNECT_PATH}");
    let mut request = endpoint.as_str().into_client_request().unwrap();
    request.headers_mut().insert(
        header::AUTHORIZATION,
        header::HeaderValue::from_str(&format!("Bearer {}", "a".repeat(64))).unwrap(),
    );
    request.headers_mut().insert(
        header::SEC_WEBSOCKET_PROTOCOL,
        header::HeaderValue::from_static(WEBSOCKET_SUBPROTOCOL),
    );
    let (client, response) = tokio_tungstenite::client_async(request, MaybeTlsStream::Rustls(tls))
        .await
        .unwrap();
    assert_eq!(
        response.headers()[header::SEC_WEBSOCKET_PROTOCOL],
        WEBSOCKET_SUBPROTOCOL
    );
    let connection = ManagerConnection::new(&endpoint, Zeroizing::new("a".repeat(64))).unwrap();
    (connection, client, manager.await.unwrap())
}

#[derive(Clone, Default)]
struct JournalFixture(Arc<Mutex<BTreeMap<String, ExecutionRecord>>>);
impl Journal for JournalFixture {
    fn pending_results(&mut self) -> Result<Vec<(String, ExecutionRecord)>, JournalError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, record)| !record.acknowledged)
            .map(|(id, record)| (id.clone(), record.clone()))
            .collect())
    }
    fn load(&mut self, id: &str) -> Result<Option<ExecutionRecord>, JournalError> {
        Ok(self.0.lock().unwrap().get(id).cloned())
    }
    fn create(&mut self, id: &str, record: &ExecutionRecord) -> Result<(), JournalError> {
        self.0.lock().unwrap().insert(id.into(), record.clone());
        Ok(())
    }
    fn replace(&mut self, id: &str, record: &ExecutionRecord) -> Result<(), JournalError> {
        self.create(id, record)
    }
}

struct UnusedSunshine(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl Sunshine for UnusedSunshine {
    async fn read(&mut self) -> Result<Configuration, AdapterError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(AdapterError::UnsupportedCapability)
    }
    async fn save(&mut self, _: &Configuration) -> Result<(), AdapterError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(AdapterError::UnsupportedCapability)
    }
    async fn restart(&mut self) -> Result<(), AdapterError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(AdapterError::UnsupportedCapability)
    }
}

fn task() -> Task {
    let task = Task {
        protocol: xscs_protocol::TASK_PROTOCOL.into(),
        operation_id: format!("op_{}", uuid::Uuid::from_u128(4)),
        binding: Binding {
            manager_id: uuid::Uuid::from_u128(1),
            device_id: uuid::Uuid::from_u128(2),
            installation_id: uuid::Uuid::from_u128(3),
        },
        permission: Permission::ManageApplications,
        command: Command::CloseApplication {
            administrator_confirmed: true,
        },
    };
    task.validate(&task.binding, &capabilities()).unwrap();
    task
}

fn capabilities() -> Capabilities {
    Capabilities {
        protocol: PROTOCOL.into(),
        client_version: "test".into(),
        os: ClientOs::LinuxX86_64,
        sunshine_version: xscs_protocol::SUNSHINE_VERSION.into(),
        configuration_overwrite: true,
        pending_pairing_listing: true,
        restart_allowed: false,
        managed_fields: xscs_protocol::config::FIELD_DEFINITIONS
            .iter()
            .map(|field| field.key.into())
            .collect(),
        application_management: true,
        application_host_commands_allowed: false,
        moonlight_pairing_management: true,
        diagnostics: true,
        maintenance: true,
        service_control: false,
    }
}

fn executor(
    task: &Task,
    journal: JournalFixture,
) -> (
    Arc<Executor<UnusedSunshine, JournalFixture>>,
    Arc<AtomicUsize>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    (
        Arc::new(Executor::new(
            task.binding.clone(),
            UnusedSunshine(calls.clone()),
            journal,
        )),
        calls,
    )
}

async fn receive(socket: &mut ManagerSocket) -> ClientMessage {
    timeout(Duration::from_secs(2), async {
        loop {
            match socket.next().await.unwrap().unwrap() {
                Message::Text(text) => return serde_json::from_str(&text).unwrap(),
                Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await.unwrap(),
                Message::Pong(_) => {}
                message => panic!("unexpected WSS frame: {message:?}"),
            }
        }
    })
    .await
    .expect("response must arrive before the 15-second heartbeat")
}

async fn transmit(socket: &mut ManagerSocket, message: ManagerMessage) {
    socket
        .send(Message::Text(
            serde_json::to_string(&message).unwrap().into(),
        ))
        .await
        .unwrap();
}

async fn inspect(socket: &mut ManagerSocket, task: &Task) {
    transmit(
        socket,
        ManagerMessage::Task {
            mode: DeliveryMode::InspectOnly,
            task: Box::new(task.clone()),
            expires_at_unix_ms: u64::MAX / 2,
            remaining_ms: 900_000,
        },
    )
    .await;
    assert_eq!(
        receive(socket).await,
        ClientMessage::Progress {
            operation_id: task.operation_id.clone(),
            fingerprint: task.fingerprint().unwrap()
        }
    );
}

async fn consume_startup(socket: &mut ManagerSocket, task: &Task, replay: Option<&Report>) {
    assert!(matches!(receive(socket).await, ClientMessage::Hello { .. }));
    if let Some(report) = replay {
        assert_eq!(
            receive(socket).await,
            ClientMessage::Result {
                operation_id: task.operation_id.clone(),
                report: report.clone()
            }
        );
        // The first immediate tick can replay the durable journal once more.
        loop {
            match receive(socket).await {
                ClientMessage::Result {
                    operation_id,
                    report: observed,
                } => {
                    assert_eq!(operation_id, task.operation_id);
                    assert_eq!(&observed, report);
                }
                ClientMessage::Heartbeat { .. } => break,
                message => panic!("unexpected startup message: {message:?}"),
            }
        }
    } else {
        assert!(matches!(
            receive(socket).await,
            ClientMessage::Heartbeat { .. }
        ));
    }
}

#[tokio::test]
async fn fresh_inspection_sends_progress_then_result_before_heartbeat() {
    let (connection, client, mut manager) = socket_pair().await;
    let task = task();
    let (executor, calls) = executor(&task, JournalFixture::default());
    let binding = task.binding.clone();
    let session = tokio::spawn(async move {
        let (_health_tx, health) = watch::channel(HealthObservation::default());
        let mut execution = ExecutionState::default();
        connection
            .session_connected(
                client,
                &binding,
                &capabilities(),
                executor,
                &health,
                &mut execution,
            )
            .await
    });
    consume_startup(&mut manager, &task, None).await;
    inspect(&mut manager, &task).await;
    assert_eq!(
        receive(&mut manager).await,
        ClientMessage::Result {
            operation_id: task.operation_id.clone(),
            report: Report::Unknown {
                reason: Uncertainty::NoExecutionRecord
            }
        }
    );
    transmit(&mut manager, ManagerMessage::Revoked {}).await;
    assert_eq!(session.await.unwrap(), Err(TransportError::Revoked));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cached_terminal_inspection_sends_progress_before_cached_result() {
    let (connection, client, mut manager) = socket_pair().await;
    let task = task();
    let report = Report::ApplicationClosed {};
    let (executor, calls) = executor(&task, JournalFixture::default());
    let mut execution = ExecutionState::default();
    execution.completed.insert(
        task.operation_id.clone(),
        (task.fingerprint().unwrap(), report.clone()),
    );
    let binding = task.binding.clone();
    let session = tokio::spawn(async move {
        let (_health_tx, health) = watch::channel(HealthObservation::default());
        connection
            .session_connected(
                client,
                &binding,
                &capabilities(),
                executor,
                &health,
                &mut execution,
            )
            .await
    });
    consume_startup(&mut manager, &task, Some(&report)).await;
    inspect(&mut manager, &task).await;
    assert_eq!(
        receive(&mut manager).await,
        ClientMessage::Result {
            operation_id: task.operation_id.clone(),
            report
        }
    );
    transmit(&mut manager, ManagerMessage::Revoked {}).await;
    assert_eq!(session.await.unwrap(), Err(TransportError::Revoked));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn delayed_old_unknown_receipt_preserves_new_unknown_and_session() {
    let (connection, client, mut manager) = socket_pair().await;
    let task = task();
    let fingerprint = task.fingerprint().unwrap();
    let previous = Report::Unknown {
        reason: Uncertainty::EffectNotConfirmed,
    };
    let mut journal = JournalFixture::default();
    journal.0.lock().unwrap().insert(
        task.operation_id.clone(),
        ExecutionRecord {
            fingerprint: fingerprint.clone(),
            effect: Some(EffectIntent::Unobservable),
            report: Some(previous.clone()),
            acknowledged: false,
            binding: Some(task.binding.clone()),
            accepted_digest: None,
        },
    );
    let (executor, calls) = executor(&task, journal.clone());
    let mut execution = ExecutionState::default();
    execution.completed.insert(
        task.operation_id.clone(),
        (fingerprint.clone(), previous.clone()),
    );
    let binding = task.binding.clone();
    let session = tokio::spawn(async move {
        let (_health_tx, health) = watch::channel(HealthObservation::default());
        let result = connection
            .session_connected(
                client,
                &binding,
                &capabilities(),
                executor,
                &health,
                &mut execution,
            )
            .await;
        (result, execution)
    });
    consume_startup(&mut manager, &task, Some(&previous)).await;
    inspect(&mut manager, &task).await;
    let current = Report::Unknown {
        reason: Uncertainty::SideEffectNotConfirmed,
    };
    assert_eq!(
        receive(&mut manager).await,
        ClientMessage::Result {
            operation_id: task.operation_id.clone(),
            report: current.clone()
        }
    );
    transmit(
        &mut manager,
        ManagerMessage::ResultAccepted {
            operation_id: task.operation_id.clone(),
            fingerprint: fingerprint.clone(),
            report_digest: xscs_protocol::report_digest(&previous).unwrap(),
            finalized: false,
        },
    )
    .await;
    transmit(
        &mut manager,
        ManagerMessage::ResultAccepted {
            operation_id: task.operation_id.clone(),
            fingerprint: fingerprint.clone(),
            report_digest: xscs_protocol::report_digest(&current).unwrap(),
            finalized: false,
        },
    )
    .await;
    transmit(&mut manager, ManagerMessage::Revoked {}).await;
    let (result, execution) = session.await.unwrap();
    assert_eq!(
        result,
        Err(TransportError::Revoked),
        "late ACK must not disconnect as Protocol"
    );
    assert_eq!(
        execution.completed.get(&task.operation_id),
        Some(&(fingerprint, current.clone()))
    );
    let record = journal.load(&task.operation_id).unwrap().unwrap();
    assert_eq!(record.report, Some(current));
    assert!(!record.acknowledged);
    assert!(record.accepted_digest.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn conflicting_cached_fingerprint_is_rejected_before_progress() {
    let (_, mut client, mut manager) = socket_pair().await;
    let task = task();
    let (executor, calls) = executor(&task, JournalFixture::default());
    let mut execution = ExecutionState::default();
    execution.completed.insert(
        task.operation_id.clone(),
        ("0".repeat(64), Report::ApplicationClosed {}),
    );
    let mut superseded = BTreeMap::new();
    assert_eq!(
        execution
            .deliver_task(
                &mut client,
                executor,
                DeliveryMode::InspectOnly,
                Box::new(task.clone()),
                (0, 0),
                &mut superseded
            )
            .await,
        Err(TransportError::Protocol)
    );
    assert!(
        timeout(Duration::from_millis(100), manager.next())
            .await
            .is_err(),
        "invalid task must not emit Progress"
    );
    assert_eq!(execution.completed[&task.operation_id].0, "0".repeat(64));
    assert!(execution.active.is_none());
    assert!(superseded.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failed_progress_send_preserves_unknown_outbox_and_journal() {
    let (_, mut client, _manager) = socket_pair().await;
    client.close(None).await.unwrap();
    let task = task();
    let fingerprint = task.fingerprint().unwrap();
    let previous = Report::Unknown {
        reason: Uncertainty::EffectNotConfirmed,
    };
    let record = ExecutionRecord {
        fingerprint: fingerprint.clone(),
        effect: Some(EffectIntent::Unobservable),
        report: Some(previous.clone()),
        acknowledged: false,
        binding: Some(task.binding.clone()),
        accepted_digest: None,
    };
    let mut journal = JournalFixture::default();
    journal
        .0
        .lock()
        .unwrap()
        .insert(task.operation_id.clone(), record.clone());
    let (executor, calls) = executor(&task, journal.clone());
    let mut execution = ExecutionState::default();
    execution.completed.insert(
        task.operation_id.clone(),
        (fingerprint.clone(), previous.clone()),
    );
    let mut superseded = BTreeMap::new();
    assert_eq!(
        execution
            .deliver_task(
                &mut client,
                executor,
                DeliveryMode::InspectOnly,
                Box::new(task.clone()),
                (0, 0),
                &mut superseded
            )
            .await,
        Err(TransportError::Disconnected)
    );
    assert_eq!(
        execution.completed[&task.operation_id],
        (fingerprint, previous)
    );
    assert_eq!(journal.load(&task.operation_id).unwrap(), Some(record));
    assert!(execution.active.is_none());
    assert!(superseded.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn duplicate_active_failed_progress_send_preserves_original_execution() {
    let (_, mut client, _manager) = socket_pair().await;
    client.close(None).await.unwrap();
    let task = task();
    let (executor, calls) = executor(&task, JournalFixture::default());
    let mut execution = ExecutionState {
        active: Some(ActiveExecution {
            id: task.operation_id.clone(),
            fingerprint: task.fingerprint().unwrap(),
            mutation: true,
            handle: tokio::spawn(std::future::pending::<Report>()),
        }),
        ..ExecutionState::default()
    };
    let mut superseded = BTreeMap::new();
    assert_eq!(
        execution
            .deliver_task(
                &mut client,
                executor,
                DeliveryMode::InspectOnly,
                Box::new(task),
                (0, 0),
                &mut superseded
            )
            .await,
        Err(TransportError::Disconnected)
    );
    let active = execution
        .active
        .take()
        .expect("original execution must remain owned");
    assert!(!active.handle.is_finished());
    active.handle.abort();
    let _ = active.handle.await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(superseded.is_empty());
}
