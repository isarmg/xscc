use async_trait::async_trait;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{Notify, mpsc, watch};
use xscc::{
    host::{HostError, RustDeskHost},
    protocol::{DiagnosticSnapshot, HostAddress},
};
use xscc_native_management::{
    manager::{NativeHost, NativeManager, Status},
    private_fs::PrivateDirectory,
};

pub enum Command {
    Enroll { server: String, code: String },
    Resume,
    Unbind,
}

pub enum Event {
    Opened(Status),
    #[cfg(all(target_os = "ios", feature = "mobile-session"))]
    SessionReady(Arc<NativeManager>),
    Status(Status),
    Action(Result<(), String>),
    Lifecycle(Result<(), String>),
    Failed(String),
}

struct Controller;
#[async_trait]
impl RustDeskHost for Controller {
    async fn diagnostics(&self) -> Result<DiagnosticSnapshot, HostError> {
        Ok(DiagnosticSnapshot::awaiting_integration())
    }
}
#[async_trait]
impl NativeHost for Controller {
    async fn addresses(&self, _: &[HostAddress]) -> Vec<HostAddress> {
        Vec::new()
    }
}

#[cfg(target_os = "ios")]
pub fn application_directory() -> Result<PathBuf, String> {
    use objc2_foundation::{NSFileManager, NSSearchPathDirectory, NSSearchPathDomainMask};
    let url = NSFileManager::defaultManager()
        .URLForDirectory_inDomain_appropriateForURL_create_error(
            NSSearchPathDirectory::ApplicationSupportDirectory,
            NSSearchPathDomainMask::UserDomainMask,
            None,
            true,
        )
        .map_err(|error| format!("Application Support unavailable: {error}"))?;
    let path = url.path().ok_or("Application Support path unavailable")?;
    Ok(PathBuf::from(path.to_string()))
}

#[cfg(not(target_os = "ios"))]
pub fn application_directory() -> Result<PathBuf, String> {
    Err("This management preview is packaged for iOS and iPadOS only".into())
}

fn open(directory: PathBuf) -> Result<Arc<NativeManager>, String> {
    let root = PrivateDirectory::create(&directory, "xscc-managed")
        .map_err(|error| format!("Protected management storage unavailable: {error}"))?;
    let manager =
        NativeManager::open(root, Arc::new(Controller)).map_err(|error| error.to_string())?;
    #[cfg(all(target_os = "ios", feature = "mobile-session"))]
    librustdesk::platform::gpui_mobile::initialize(&directory, manager.clone())
        .map_err(|error| error.to_string())?;
    Ok(manager)
}

pub async fn run(
    directory: Result<PathBuf, String>,
    mut commands: mpsc::Receiver<Command>,
    mut lifecycle: watch::Receiver<bool>,
    events: std::sync::mpsc::Sender<Event>,
    wake: Arc<Notify>,
) {
    let publish = |event| {
        let sent = events.send(event).is_ok();
        wake.notify_one();
        sent
    };
    let manager = match directory {
        Ok(directory) => tokio::task::spawn_blocking(move || open(directory))
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result),
        Err(error) => Err(error),
    };
    let manager = match manager {
        Ok(manager) => manager,
        Err(error) => {
            publish(Event::Failed(error));
            return;
        }
    };
    #[cfg(all(target_os = "ios", feature = "mobile-session"))]
    publish(Event::SessionReady(manager.clone()));
    publish(Event::Opened(manager.status()));
    let mut active = *lifecycle.borrow_and_update();
    if active {
        publish(Event::Lifecycle(
            manager.resume().await.map_err(|error| error.to_string()),
        ));
    }
    let mut timer = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            biased;
            changed = lifecycle.changed() => {
                if changed.is_err() { break; }
                let next = *lifecycle.borrow_and_update();
                if next == active { continue; }
                active = next;
                let result = if active { manager.resume().await } else { manager.shutdown().await };
                publish(Event::Status(manager.status()));
                if !publish(Event::Lifecycle(result.map_err(|error| error.to_string()))) { break; }
            }
            command = commands.recv() => {
                let Some(command) = command else { break };
                let result = match command {
                    Command::Enroll { server, code } if active => {
                        manager.enroll(&server, &code, Vec::new()).await
                    }
                    Command::Resume if active => manager.resume().await,
                    Command::Unbind if active => manager.unbind().await,
                    _ => {
                        publish(Event::Action(Err("应用在后台，操作未执行".into())));
                        continue;
                    }
                };
                publish(Event::Status(manager.status()));
                if !publish(Event::Action(result.map_err(|error| error.to_string()))) {
                    break;
                }
            }
            _ = timer.tick(), if active => {
                if !publish(Event::Status(manager.status())) { break; }
            }
        }
    }
    if let Err(error) = manager.shutdown().await {
        eprintln!("Management shutdown failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reopening_preserves_installation_identity_and_single_owner() {
        let temporary = tempfile::tempdir().unwrap();
        let manager = open(temporary.path().to_owned()).unwrap();
        let path = temporary.path().join("xscc-managed/installation.uuid");
        let identity = std::fs::read(&path).unwrap();
        assert!(open(temporary.path().to_owned()).is_err());
        manager.shutdown().await.unwrap();
        drop(manager);
        let manager = open(temporary.path().to_owned()).unwrap();
        assert_eq!(identity, std::fs::read(path).unwrap());
        assert!(!manager.status().configured);
    }

    #[tokio::test]
    async fn invalid_enrollment_does_not_persist_configuration() {
        let temporary = tempfile::tempdir().unwrap();
        let manager = open(temporary.path().to_owned()).unwrap();
        assert!(
            manager
                .enroll("http://example.invalid", "invalid", vec![])
                .await
                .is_err()
        );
        assert!(!manager.status().configured);
        manager.unbind().await.unwrap();
        assert!(manager.status().binding.is_none());
        assert!(manager.status().peers.is_empty());
    }

    #[tokio::test]
    async fn latest_background_state_rejects_queued_user_operation() {
        let temporary = tempfile::tempdir().unwrap();
        let (commands, receiver) = mpsc::channel(1);
        let (lifecycle, phases) = watch::channel(true);
        lifecycle.send_replace(false);
        lifecycle.send_replace(true);
        lifecycle.send_replace(false);
        let (events, replies) = std::sync::mpsc::channel();
        let wake = Arc::new(Notify::new());
        commands.send(Command::Resume).await.unwrap();
        let worker = tokio::spawn(run(
            Ok(temporary.path().to_owned()),
            receiver,
            phases,
            events,
            wake.clone(),
        ));
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                wake.notified().await;
                for event in replies.try_iter() {
                    if let Event::Action(result) = event {
                        return result;
                    }
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(result.unwrap_err(), "应用在后台，操作未执行");
        drop(commands);
        drop(lifecycle);
        tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap();
    }
}
