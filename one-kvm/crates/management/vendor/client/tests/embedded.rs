use async_trait::async_trait;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use xscc::{
    engine::{ExecutionRecord, Executor, Journal, JournalError},
    host::{HostError, RustDeskHost},
    protocol::*,
};

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
    async fn create(&self, id: &str, record: &ExecutionRecord) -> Result<(), JournalError> {
        let mut facts = self.0.lock().unwrap();
        if facts.contains_key(id) {
            return Err(JournalError::Storage);
        }
        facts.insert(id.into(), record.clone());
        Ok(())
    }
    async fn replace(&self, id: &str, record: &ExecutionRecord) -> Result<(), JournalError> {
        self.0.lock().unwrap().insert(id.into(), record.clone());
        Ok(())
    }
}
struct Host(Arc<AtomicUsize>);
#[async_trait]
impl RustDeskHost for Host {
    async fn diagnostics(&self) -> Result<DiagnosticSnapshot, HostError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(DiagnosticSnapshot::awaiting_integration())
    }
}
fn fixture() -> (Binding, Capabilities, Task) {
    let binding = Binding {
        manager_id: uuid::Uuid::new_v4(),
        device_id: uuid::Uuid::new_v4(),
        installation_id: uuid::Uuid::new_v4(),
    };
    let capabilities = Capabilities {
        protocol: PROTOCOL.into(),
        client_version: env!("CARGO_PKG_VERSION").into(),
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
    (binding, capabilities, task)
}
#[tokio::test]
async fn embedded_host_result_survives_executor_recreation_and_acknowledgement() {
    let (binding, caps, task) = fixture();
    let facts = Facts::default();
    let calls = Arc::new(AtomicUsize::new(0));
    let executor = Executor::new_with_capabilities(
        binding.clone(),
        caps.clone(),
        Host(calls.clone()),
        facts.clone(),
    );
    let report = executor.deliver(&task, DeliveryMode::Execute).await;
    assert_eq!(
        report,
        Report::DiagnosticsRead {
            snapshot: DiagnosticSnapshot::awaiting_integration()
        }
    );
    drop(executor);
    let executor = Executor::new_with_capabilities(binding, caps, Host(calls.clone()), facts);
    assert_eq!(executor.deliver(&task, DeliveryMode::Execute).await, report);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let fingerprint = task.fingerprint().unwrap();
    let digest = report_digest(&report).unwrap();
    assert!(
        executor
            .acknowledge_final(&task.operation_id, &fingerprint, &"f".repeat(64), true)
            .await
            .is_err()
    );
    assert_eq!(executor.pending_results().await.unwrap().len(), 1);
    executor
        .acknowledge_final(&task.operation_id, &fingerprint, &digest, true)
        .await
        .unwrap();
    assert!(executor.pending_results().await.unwrap().is_empty());
    assert_eq!(
        executor.deliver(&task, DeliveryMode::Execute).await,
        Report::Rejected {
            reason: Rejection::OperationAlreadyCompleted
        }
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn mismatched_binding_and_expired_delivery_do_not_observe_the_host() {
    let (binding, caps, task) = fixture();
    let calls = Arc::new(AtomicUsize::new(0));
    let executor =
        Executor::new_with_capabilities(binding, caps, Host(calls.clone()), Facts::default());
    let mut wrong = task.clone();
    wrong.binding.installation_id = uuid::Uuid::new_v4();
    assert_eq!(
        executor.deliver(&wrong, DeliveryMode::Execute).await,
        Report::Rejected {
            reason: Rejection::BindingMismatch
        }
    );
    assert_eq!(
        executor
            .deliver_before(
                &task,
                DeliveryMode::Execute,
                Some(xscc::engine::ExecutionDeadline::received(1, 0))
            )
            .await,
        Report::Rejected {
            reason: Rejection::ExecutionExpired
        }
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn inspection_without_host_execution_facts_stays_unknown() {
    let (binding, caps, task) = fixture();
    let calls = Arc::new(AtomicUsize::new(0));
    let executor =
        Executor::new_with_capabilities(binding, caps, Host(calls.clone()), Facts::default());
    assert_eq!(
        executor.deliver(&task, DeliveryMode::InspectOnly).await,
        Report::Unknown {
            reason: Uncertainty::NoExecutionRecord
        }
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
#[test]
fn manager_endpoint_remains_a_protected_tls_channel() {
    use xscc::transport::{ManagerConnection, validate_manager_endpoint};
    for endpoint in [
        "ws://manager.example/xscc/v2/connect",
        "wss://user:password@manager.example/xscc/v2/connect",
        "wss://manager.example/xscc/v1/connect",
    ] {
        assert!(validate_manager_endpoint(&url::Url::parse(endpoint).unwrap()).is_err());
    }
    assert!(
        ManagerConnection::new(
            "wss://manager.example/xscc/v2/connect",
            zeroize::Zeroizing::new("a".repeat(64))
        )
        .is_ok()
    );
}
