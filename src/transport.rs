//! Product WSS transport. No generic Client platform, URL passthrough or Manager credential logging.
use crate::{
    engine::{ExecutionDeadline, Executor, Journal},
    host::RustDeskHost,
};
use futures_util::{SinkExt, StreamExt, stream::FuturesUnordered};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::{
    net::TcpStream,
    sync::watch,
    time::{Instant, MissedTickBehavior, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{
        self, Message,
        client::IntoClientRequest,
        http::{StatusCode, header},
        protocol::WebSocketConfig,
    },
};
use url::Url;

use xscs_protocol::{
    Binding, Capabilities, ClientMessage, DiagnosticSnapshot, MAX_MESSAGE_BYTES, ManagerMessage,
    Report, WEBSOCKET_SUBPROTOCOL, decode_manager_message,
};
use zeroize::Zeroizing;

fn session_log(binding: &Binding, event: &str, code: Option<&str>) {
    log::debug!(
        "xscc session: device={} event={} code={}",
        binding.device_id,
        event,
        code.unwrap_or("none")
    );
}

#[derive(Default)]
struct RetryBackoff {
    attempt: u32,
}
impl RetryBackoff {
    fn reset(&mut self) {
        self.attempt = 0;
    }
    fn next_delay(&mut self) -> Duration {
        let delay = Duration::from_secs(1u64 << self.attempt.min(6)).min(Duration::from_secs(60));
        self.attempt = self.attempt.saturating_add(1);
        delay
    }
}

pub use xscs_protocol::CONNECT_PATH;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const PEER_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("invalid Client WSS configuration")]
    Configuration,
    #[error("Client credential rejected or revoked")]
    Revoked,
    #[error("Manager WSS unavailable")]
    Disconnected,
    #[error("invalid Manager protocol message")]
    Protocol,
    #[error("Client shutdown left an execution outcome uncertain")]
    ShutdownIncomplete,
}

#[derive(Clone, Default)]
pub struct HealthObservation {
    pub diagnostics: Option<DiagnosticSnapshot>,
    pub observed_at: Option<Instant>,
}

/// The endpoint must come from protected local provisioning, never from an inbound task.
/// Credentials have no Debug/Serialize implementation and are never put into the URL.
pub struct ManagerConnection {
    endpoint: Url,
    credential: Zeroizing<String>,
}

impl ManagerConnection {
    pub fn new(endpoint: &str, credential: Zeroizing<String>) -> Result<Self, TransportError> {
        let endpoint = Url::parse(endpoint).map_err(|_| TransportError::Configuration)?;
        validate_manager_endpoint(&endpoint)?;
        // Enrollment provisions one independent 256-bit random credential, encoded as lowercase hex.
        if credential.len() != 64
            || !credential
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(TransportError::Configuration);
        }
        Ok(Self {
            endpoint,
            credential,
        })
    }

    async fn connect(&self) -> Result<Socket, TransportError> {
        let mut request = self
            .endpoint
            .as_str()
            .into_client_request()
            .map_err(|_| TransportError::Configuration)?;
        let raw = Zeroizing::new(format!("Bearer {}", self.credential.as_str()));
        let mut value = header::HeaderValue::from_str(raw.as_str())
            .map_err(|_| TransportError::Configuration)?;
        value.set_sensitive(true);
        request.headers_mut().insert(header::AUTHORIZATION, value);
        request.headers_mut().insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            header::HeaderValue::from_static(WEBSOCKET_SUBPROTOCOL),
        );
        let mut config = WebSocketConfig::default();
        config.max_message_size = Some(MAX_MESSAGE_BYTES);
        config.max_frame_size = Some(MAX_MESSAGE_BYTES);
        config.write_buffer_size = 0;
        config.max_write_buffer_size = MAX_MESSAGE_BYTES * 2;
        let (socket, response) = timeout(
            IO_TIMEOUT,
            connect_async_with_config(request, Some(config), false),
        )
        .await
        .map_err(|_| TransportError::Disconnected)?
        .map_err(classify_handshake)?;
        if response
            .headers()
            .get(header::SEC_WEBSOCKET_PROTOCOL)
            .and_then(|value| value.to_str().ok())
            != Some(WEBSOCKET_SUBPROTOCOL)
        {
            return Err(TransportError::Protocol);
        }
        Ok(socket)
    }

    /// Terminal authentication failure stops reconnecting. Other reconnects use bounded backoff.
    /// The caller owns protected credential/revocation state and retains the Executor across sessions.
    pub async fn run<A: RustDeskHost + 'static, J: Journal + 'static>(
        &self,
        binding: Binding,
        capabilities: Capabilities,
        executor: Arc<Executor<A, J>>,
        health: watch::Receiver<HealthObservation>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), TransportError> {
        if capabilities.validate().is_err() {
            return Err(TransportError::Configuration);
        }
        let mut execution = ExecutionState::default();
        for (id, record) in executor
            .pending_results()
            .await
            .map_err(|_| TransportError::Configuration)?
        {
            if let Some(report) = record.report {
                execution.completed.insert(id, (record.fingerprint, report));
            }
        }
        let mut backoff = RetryBackoff::default();
        loop {
            if *shutdown.borrow() {
                return execution.shutdown().await;
            }
            let started = Instant::now();
            let result = tokio::select! {
                biased;
                _ = shutdown.changed() => return execution.shutdown().await,
                result = self.session(&binding, &capabilities, executor.clone(), &health, &mut execution) => result,
            };
            let (event, code) = match result {
                Err(TransportError::Revoked) => {
                    ("xscc.session.authorization_failed", "credential_rejected")
                }
                Err(TransportError::Protocol) => {
                    ("xscc.session.protocol_failed", "protocol_invalid")
                }
                Err(TransportError::Configuration) => {
                    ("xscc.session.state_failed", "state_invalid")
                }
                _ => ("xscc.session.disconnected", "connection_failed"),
            };
            session_log(&binding, event, Some(code));
            if matches!(
                result,
                Err(TransportError::Revoked
                    | TransportError::Configuration
                    | TransportError::Protocol)
            ) {
                return result;
            }
            if started.elapsed() >= Duration::from_secs(60) {
                backoff.reset();
            }
            let delay = backoff.next_delay();
            tokio::select! {
                _ = shutdown.changed() => return execution.shutdown().await,
                _ = tokio::time::sleep(delay) => {},
            }
        }
    }

    async fn session<A: RustDeskHost + 'static, J: Journal + 'static>(
        &self,
        binding: &Binding,
        capabilities: &Capabilities,
        executor: Arc<Executor<A, J>>,
        health: &watch::Receiver<HealthObservation>,
        execution: &mut ExecutionState,
    ) -> Result<(), TransportError> {
        let socket = self.connect().await?;
        self.session_connected(socket, binding, capabilities, executor, health, execution)
            .await
    }

    // Keep the authenticated socket loop independently testable with a private
    // trusted TLS fixture; production always obtains its socket via connect().
    async fn session_connected<A: RustDeskHost + 'static, J: Journal + 'static>(
        &self,
        mut socket: Socket,
        binding: &Binding,
        capabilities: &Capabilities,
        executor: Arc<Executor<A, J>>,
        health: &watch::Receiver<HealthObservation>,
        execution: &mut ExecutionState,
    ) -> Result<(), TransportError> {
        send(
            &mut socket,
            ClientMessage::Hello {
                binding: binding.clone(),
                capabilities: capabilities.clone(),
                active_operation: execution.active.as_ref().map(|active| {
                    xscs_protocol::ActiveOperation {
                        operation_id: active.id.clone(),
                        fingerprint: active.fingerprint.clone(),
                    }
                }),
            },
        )
        .await?;
        session_log(binding, "xscc.session.connected", None);
        let mut tick = tokio::time::interval(HEARTBEAT_INTERVAL);
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_peer = Instant::now();
        // JoinHandle is retained by run() across sessions; dropping a socket cannot cancel effects.
        for (operation_id, (_, report)) in &execution.completed {
            send(
                &mut socket,
                ClientMessage::Result {
                    operation_id: operation_id.clone(),
                    report: report.clone(),
                },
            )
            .await?;
        }
        let mut last_snapshot = None;
        // One inspection per operation per session can supersede an earlier Unknown.
        // Keep only that sent report's identity until the new report is acknowledged.
        let mut superseded = BTreeMap::<String, (String, String)>::new();
        loop {
            tokio::select! {
                // Credential revocation/connection close wins over publishing another result.
                biased;
                incoming = socket.next() => {
                    let frame = incoming.ok_or(TransportError::Disconnected)?.map_err(|_| TransportError::Disconnected)?;
                    last_peer = Instant::now();
                    match frame {
                        Message::Text(text) => match decode_manager_message(text.as_bytes()).map_err(|_| TransportError::Protocol)? {
                            ManagerMessage::Revoked {} => return Err(TransportError::Revoked),
                            ManagerMessage::ResultAccepted { operation_id, fingerprint, report_digest, finalized } => {
                                if receipt_requires_commit(execution.completed.get(&operation_id), superseded.get(&operation_id), &fingerprint, &report_digest, finalized)? {
                                    let executor = executor.clone();
                                    execution.acknowledging.push(tokio::spawn(async move {
                                        let result = executor.acknowledge_final(&operation_id, &fingerprint, &report_digest, finalized).await;
                                        (operation_id, report_digest, result)
                                    }));
                                }
                            }
                            ManagerMessage::Task { mode, task, expires_at_unix_ms, remaining_ms } => {
                                execution.deliver_task(&mut socket, executor.clone(), mode, task,
                                    (expires_at_unix_ms, remaining_ms), &mut superseded).await?;
                            }
                        },
                        Message::Ping(bytes) => send_frame(&mut socket, Message::Pong(bytes)).await?,
                        Message::Pong(_) => {},
                        Message::Close(Some(frame)) if matches!(frame.code, tungstenite::protocol::frame::coding::CloseCode::Protocol | tungstenite::protocol::frame::coding::CloseCode::Unsupported | tungstenite::protocol::frame::coding::CloseCode::Policy) => return Err(TransportError::Protocol),
                        Message::Close(_) => return Err(TransportError::Disconnected),
                        _ => return Err(TransportError::Protocol),
                    }
                },
                result = async { (&mut execution.active.as_mut().expect("guarded execution").handle).await }, if execution.active.is_some() => {
                    let active = execution.active.take().expect("completed execution");
                    let report = result.map_err(|_| TransportError::Configuration)?;
                    execution.completed.insert(active.id.clone(), (active.fingerprint, report.clone()));
                    send(&mut socket, ClientMessage::Result { operation_id: active.id, report }).await?;
                },
                Some(result) = execution.acknowledging.next(), if !execution.acknowledging.is_empty() => {
                    let (id, digest, result) = result.map_err(|_| TransportError::Configuration)?;
                    result.map_err(|_| TransportError::Configuration)?;
                    if execution.completed.get(&id).is_some_and(|(_, report)| xscs_protocol::report_digest(report).ok().as_deref() == Some(&digest)) {
                        execution.completed.remove(&id);
                        superseded.remove(&id);
                    }
                },
                _ = tick.tick() => {
                    if last_peer.elapsed() >= PEER_TIMEOUT { return Err(TransportError::Disconnected); }
                    if execution.active.is_none() {
                        // Fetch final receipts after a human resolution, preserving uncertain intent
                        // until the Manager confirms that xscs has a terminal outcome.
                        for (id, record) in executor.pending_results().await.map_err(|_| TransportError::Configuration)? {
                            if let Some(report) = record.report {
                                execution.completed.entry(id.clone()).or_insert((record.fingerprint, report.clone()));
                                send(&mut socket, ClientMessage::Result { operation_id: id, report }).await?;
                            }
                        }
                    }
                    let observation = health.borrow().clone();
                    let fresh = observation.observed_at.is_some_and(|at| at.elapsed() < Duration::from_secs(30));
                    if let Some(active) = &execution.active {
                        send(&mut socket, ClientMessage::Progress { operation_id: active.id.clone(), fingerprint: active.fingerprint.clone() }).await?;
                    }
                    let diagnostics = if fresh { observation.diagnostics.filter(|snapshot| last_snapshot.as_ref() != Some(snapshot)) } else { None };
                    if let Some(snapshot) = &diagnostics { last_snapshot = Some(snapshot.clone()); }
                    send(&mut socket, ClientMessage::Heartbeat { diagnostics }).await?;
                    send_frame(&mut socket, Message::Ping(Vec::new().into())).await?;
                },
            }
        }
    }
}

// Validate against the current observation or the one this session superseded.
// Unknown receipts cannot compact intent until the Manager supplies a final outcome.
fn receipt_requires_commit(
    current: Option<&(String, Report)>,
    superseded: Option<&(String, String)>,
    fingerprint: &str,
    digest: &str,
    finalized: bool,
) -> Result<bool, TransportError> {
    let current_digest = current.and_then(|(_, report)| xscs_protocol::report_digest(report).ok());
    if superseded.is_some_and(|(previous, sent)| previous == fingerprint && sent == digest)
        && current_digest.as_deref() != Some(digest)
    {
        // A delayed old receipt must neither acknowledge nor reject the new report.
        return Ok(false);
    }
    let Some((expected, report)) = current else {
        return Ok(false);
    };
    if expected != fingerprint || current_digest.as_deref() != Some(digest) {
        return Err(TransportError::Protocol);
    }
    // Unknown remains in the journal and is polled regardless of accepted_digest.
    // Avoid racing a redundant receipt write against fresh inspection persistence.
    Ok(finalized || !matches!(report, Report::Unknown { .. }))
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type ReceiptCompletion = (String, String, Result<(), crate::engine::JournalError>);
#[derive(Default)]
struct ExecutionState {
    active: Option<ActiveExecution>,
    completed: BTreeMap<String, (String, Report)>,
    acknowledging: FuturesUnordered<tokio::task::JoinHandle<ReceiptCompletion>>,
}
impl ExecutionState {
    // This is the production Task branch. Socket order is established before
    // changing the outbox or spawning work; a failed send leaves both intact.
    async fn deliver_task<A: RustDeskHost + 'static, J: Journal + 'static>(
        &mut self,
        socket: &mut Socket,
        executor: Arc<Executor<A, J>>,
        mode: xscs_protocol::DeliveryMode,
        task: Box<xscs_protocol::Task>,
        budget: (u64, u64),
        superseded: &mut BTreeMap<String, (String, String)>,
    ) -> Result<(), TransportError> {
        let fingerprint = task.fingerprint().map_err(|_| TransportError::Protocol)?;
        if let Some(active) = &self.active {
            // A retry can arrive before the first execution finishes. Keep
            // the original execution and its eventual result, never run twice.
            if active.id == task.operation_id && active.fingerprint == fingerprint {
                send(
                    socket,
                    ClientMessage::Progress {
                        operation_id: active.id.clone(),
                        fingerprint: active.fingerprint.clone(),
                    },
                )
                .await?;
                return Ok(());
            }
            return Err(TransportError::Protocol);
        }
        // Do not confirm a task that conflicts with a retained result.
        if self
            .completed
            .get(&task.operation_id)
            .is_some_and(|(previous, _)| previous != &fingerprint)
        {
            return Err(TransportError::Protocol);
        }
        if mode == xscs_protocol::DeliveryMode::InspectOnly {
            // On this ordered socket, Progress separates earlier replayed
            // results from the result of this inspection, including cache hits.
            send(
                socket,
                ClientMessage::Progress {
                    operation_id: task.operation_id.clone(),
                    fingerprint: fingerprint.clone(),
                },
            )
            .await?;
        }
        if let Some((_, report)) = self.completed.get(&task.operation_id) {
            if mode != xscs_protocol::DeliveryMode::InspectOnly
                || !matches!(report, Report::Unknown { .. })
            {
                send(
                    socket,
                    ClientMessage::Result {
                        operation_id: task.operation_id.clone(),
                        report: report.clone(),
                    },
                )
                .await?;
                return Ok(());
            }
            superseded.insert(
                task.operation_id.clone(),
                (
                    fingerprint.clone(),
                    xscs_protocol::report_digest(report).map_err(|_| TransportError::Protocol)?,
                ),
            );
            // An inspect request must collect fresh evidence.
            self.completed.remove(&task.operation_id);
        }
        let id = task.operation_id.clone();
        let deadline = (mode == xscs_protocol::DeliveryMode::Execute)
            .then(|| ExecutionDeadline::received(budget.0, budget.1));
        let handle =
            tokio::spawn(async move { executor.deliver_before(&task, mode, deadline).await });
        self.active = Some(ActiveExecution {
            id,
            fingerprint,
            handle,
        });

        Ok(())
    }

    async fn shutdown(&mut self) -> Result<(), TransportError> {
        // Stop receiving tasks first, then let the already-owned execution and
        // receipts reach their existing journal commits. Never replay an effect.
        let completed = timeout(Duration::from_secs(30), async {
            if let Some(active) = self.active.as_mut() {
                let result = (&mut active.handle).await;
                self.active = None;
                let report = result.map_err(|_| TransportError::ShutdownIncomplete)?;
                if matches!(
                    report,
                    Report::Unknown {
                        reason: xscs_protocol::Uncertainty::PersistenceFailure
                    }
                ) {
                    return Err(TransportError::ShutdownIncomplete);
                }
            }
            while let Some(receipt) = self.acknowledging.next().await {
                receipt
                    .map_err(|_| TransportError::ShutdownIncomplete)?
                    .2
                    .map_err(|_| TransportError::ShutdownIncomplete)?;
            }
            Ok(())
        })
        .await
        .unwrap_or(Err(TransportError::ShutdownIncomplete));
        if completed.is_err() {
            // Intent already saved by the executor remains available for later
            // inspection. Cancellation must not be reported as clean shutdown.
            if let Some(active) = self.active.take() {
                active.handle.abort();
                let _ = active.handle.await;
            }
            for receipt in self.acknowledging.iter() {
                receipt.abort();
            }
            while self.acknowledging.next().await.is_some() {}
        }
        completed
    }
}

struct ActiveExecution {
    id: String,
    fingerprint: String,
    handle: tokio::task::JoinHandle<Report>,
}

async fn send(socket: &mut Socket, message: ClientMessage) -> Result<(), TransportError> {
    let bytes = serde_json::to_string(&message).map_err(|_| TransportError::Protocol)?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(TransportError::Protocol);
    }
    send_frame(socket, Message::Text(bytes.into())).await
}

async fn send_frame(socket: &mut Socket, message: Message) -> Result<(), TransportError> {
    timeout(IO_TIMEOUT, socket.send(message))
        .await
        .map_err(|_| TransportError::Disconnected)?
        .map_err(|_| TransportError::Disconnected)
}

pub fn validate_manager_endpoint(url: &Url) -> Result<(), TransportError> {
    if url.scheme() != "wss"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != CONNECT_PATH
    {
        return Err(TransportError::Configuration);
    }
    Ok(())
}

fn classify_handshake(error: tungstenite::Error) -> TransportError {
    match error {
        // Only the Manager's own authentication response proves the device
        // credential is invalid. A proxy can generate unrelated 401 or 403.
        tungstenite::Error::Http(response)
            if response.status() == StatusCode::UNAUTHORIZED
                && response
                    .headers()
                    .get("x-error-code")
                    .is_some_and(|value| value.as_bytes() == b"unauthorized") =>
        {
            TransportError::Revoked
        }
        _ => TransportError::Disconnected,
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
