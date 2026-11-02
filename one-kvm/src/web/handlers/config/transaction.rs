use std::future::Future;

use tokio::sync::OwnedMutexGuard;

use crate::error::{AppError, Result};

pub(crate) fn require_current_grant(grant: &crate::management::AccessGrant) -> Result<()> {
    if !grant.is_valid() {
        return Err(AppError::Unauthorized);
    }
    Ok(())
}

/// An accepted device update owns its apply lock until commit or rollback finishes,
/// even when its HTTP caller is revoked or disconnected.
pub(crate) fn complete_device_update<T: Send + 'static>(
    guard: OwnedMutexGuard<()>,
    update: impl Future<Output = Result<T>> + Send + 'static,
) -> impl Future<Output = Result<T>> + Send {
    let task = tokio::spawn(async move {
        let _guard = guard;
        update.await
    });
    async move {
        task.await.map_err(|error| {
            AppError::Internal(format!("Device configuration task failed: {error}"))
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::{ConfigStore, ResourceRegistry},
        db::DatabasePool,
        runtime::try_apply_lock,
    };
    use std::{
        sync::{Arc, Mutex},
        task::Poll,
        time::Duration,
    };
    use tokio::sync::{oneshot, Mutex as AsyncMutex};

    #[tokio::test]
    async fn cancelled_caller_keeps_the_device_reserved_until_the_accepted_update_commits() {
        let directory = tempfile::tempdir().unwrap();
        let database_a = DatabasePool::new(&directory.path().join("a.db"))
            .await
            .unwrap();
        let database_b = DatabasePool::new(&directory.path().join("b.db"))
            .await
            .unwrap();
        database_a.init_schema().await.unwrap();
        database_b.init_schema().await.unwrap();
        let resources = Arc::new(ResourceRegistry::default());
        let a = ConfigStore::with_resources(database_a.clone_pool(), resources.clone(), "a".into())
            .unwrap();
        let b =
            ConfigStore::with_resources(database_b.clone_pool(), resources, "b".into()).unwrap();
        a.load().await.unwrap();
        b.load().await.unwrap();
        a.update(|config| config.video.device = Some("camera-a".into()))
            .await
            .unwrap();

        let mut blocked_connections = Vec::new();
        for _ in 0..database_a.pool().options().get_max_connections() {
            blocked_connections.push(database_a.pool().acquire().await.unwrap());
        }
        let runtime_device = Arc::new(Mutex::new(String::from("camera-a")));
        let apply_lock = Arc::new(AsyncMutex::new(()));
        let guard = try_apply_lock(&apply_lock, "video").unwrap();
        let (applied, observed_apply) = oneshot::channel();
        let mut committed = a.subscribe();
        let update_store = a.clone();
        let update_runtime = runtime_device.clone();
        let mut request = Box::pin(complete_device_update(guard, async move {
            let mut candidate = update_store.get().as_ref().clone();
            candidate.video.device = Some("camera-b".into());
            let _reservation = update_store.reserve_resources(&candidate)?;
            *update_runtime.lock().unwrap() = "camera-b".into();
            applied.send(()).unwrap();
            update_store
                .update(|config| config.video = candidate.video)
                .await
        }));
        std::future::poll_fn(|context| {
            assert!(matches!(request.as_mut().poll(context), Poll::Pending));
            Poll::Ready(())
        })
        .await;
        observed_apply.await.unwrap();
        drop(request);

        let mut candidate_b = b.get().as_ref().clone();
        candidate_b.video.device = Some("camera-b".into());
        assert!(matches!(
            b.reserve_resources(&candidate_b),
            Err(AppError::Conflict(_))
        ));
        assert!(try_apply_lock(&apply_lock, "video").is_err());
        assert_eq!(a.get().video.device.as_deref(), Some("camera-a"));
        drop(blocked_connections);
        tokio::time::timeout(Duration::from_secs(5), committed.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            a.get().video.device.as_deref(),
            Some(runtime_device.lock().unwrap().as_str())
        );
        assert!(b.reserve_resources(&candidate_b).is_err());
        b.update(|config| config.video.device = Some("camera-a".into()))
            .await
            .unwrap();
    }
}
