use arc_swap::ArcSwap;
use sqlx::{Pool, Sqlite};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::Mutex;

use super::resources::{ResourceLease, ResourceRegistry, ResourceReservation};
use super::AppConfig;
use super::ConfigChange;
use crate::error::{AppError, Result};

#[derive(Clone)]
pub struct ConfigStore {
    pool: Pool<Sqlite>,
    cache: Arc<ArcSwap<AppConfig>>,
    change_tx: broadcast::Sender<ConfigChange>,
    write_lock: Arc<Mutex<()>>,
    resources: Option<Arc<ResourceLease>>,
}

impl ConfigStore {
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self {
            pool,
            cache: Arc::new(ArcSwap::from_pointee(AppConfig::default())),
            change_tx: broadcast::channel(16).0,
            write_lock: Arc::new(Mutex::new(())),
            resources: None,
        }
    }

    pub fn with_resources(
        pool: Pool<Sqlite>,
        registry: Arc<ResourceRegistry>,
        instance: String,
    ) -> Result<Self> {
        let mut store = Self::new(pool);
        store.resources = Some(registry.register(instance)?);
        Ok(store)
    }

    pub fn reserve_resources(&self, config: &AppConfig) -> Result<Option<ResourceReservation>> {
        self.resources
            .as_ref()
            .map(|lease| lease.reserve(config))
            .transpose()
    }

    pub async fn load(&self) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let (mut config, removed_legacy_totp) = Self::load_config(&self.pool).await?;
        if self.resources.is_some() {
            config.apply_platform_defaults();
        } else {
            config.enforce_invariants();
        }
        let reservation = self.reserve_resources(&config)?;
        if removed_legacy_totp {
            Self::save_config_to_db(&self.pool, &config).await?;
        }
        self.cache.store(Arc::new(config));
        if let Some(reservation) = reservation {
            reservation.commit();
        }
        Ok(())
    }

    async fn load_config(pool: &Pool<Sqlite>) -> Result<(AppConfig, bool)> {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT value FROM config WHERE key = 'app_config'")
                .fetch_optional(pool)
                .await?;

        match row {
            Some((json,)) => {
                let mut value: serde_json::Value =
                    serde_json::from_str(&json).map_err(|e| AppError::Config(e.to_string()))?;
                let mut removed = false;
                if let Some(auth) = value
                    .get_mut("auth")
                    .and_then(|value| value.as_object_mut())
                {
                    removed |= auth.remove("totp_enabled").is_some();
                    removed |= auth.remove("totp_secret").is_some();
                }
                let config =
                    serde_json::from_value(value).map_err(|e| AppError::Config(e.to_string()))?;
                Ok((config, removed))
            }
            None => {
                let config = AppConfig::default();
                Self::save_config_to_db(pool, &config).await?;
                Ok((config, false))
            }
        }
    }

    async fn save_config_to_db(pool: &Pool<Sqlite>, config: &AppConfig) -> Result<()> {
        let json = serde_json::to_string(config)?;

        sqlx::query(
            r#"
            INSERT INTO config (key, value, updated_at)
            VALUES ('app_config', ?1, datetime('now'))
            ON CONFLICT(key) DO UPDATE SET value = ?1, updated_at = datetime('now')
            "#,
        )
        .bind(&json)
        .execute(pool)
        .await?;

        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub fn hid_bonds(&self) -> crate::db::hid_bonds::HidBondStore {
        crate::db::hid_bonds::HidBondStore(self.pool.clone())
    }

    pub fn get(&self) -> Arc<AppConfig> {
        self.cache.load_full()
    }

    pub async fn set(&self, config: AppConfig) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let mut config = config;
        config.enforce_invariants();
        let reservation = self.reserve_resources(&config)?;
        Self::save_config_to_db(&self.pool, &config).await?;
        self.cache.store(Arc::new(config));
        if let Some(reservation) = reservation {
            reservation.commit();
        }

        let _ = self.change_tx.send(ConfigChange {
            key: "app_config".to_string(),
        });

        Ok(())
    }

    pub async fn update<F>(&self, f: F) -> Result<()>
    where
        F: FnOnce(&mut AppConfig),
    {
        let _guard = self.write_lock.lock().await;
        let current = self.cache.load();
        let mut config = (**current).clone();
        f(&mut config);
        config.enforce_invariants();
        let reservation = self.reserve_resources(&config)?;

        Self::save_config_to_db(&self.pool, &config).await?;

        self.cache.store(Arc::new(config));
        if let Some(reservation) = reservation {
            reservation.commit();
        }

        let _ = self.change_tx.send(ConfigChange {
            key: "app_config".to_string(),
        });

        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ConfigChange> {
        self.change_tx.subscribe()
    }

    pub fn is_initialized(&self) -> bool {
        self.cache.load().initialized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DatabasePool;
    use tempfile::tempdir;

    #[tokio::test]
    async fn resource_conflict_and_persistence_failure_keep_previous_assignment() {
        let dir = tempdir().unwrap();
        let db_a = DatabasePool::new(&dir.path().join("a.db")).await.unwrap();
        let db_b = DatabasePool::new(&dir.path().join("b.db")).await.unwrap();
        db_a.init_schema().await.unwrap();
        db_b.init_schema().await.unwrap();
        let registry = Arc::new(ResourceRegistry::default());
        let a =
            ConfigStore::with_resources(db_a.clone_pool(), registry.clone(), "a".into()).unwrap();
        let b = ConfigStore::with_resources(db_b.clone_pool(), registry, "b".into()).unwrap();
        a.load().await.unwrap();
        b.load().await.unwrap();
        a.update(|config| config.video.device = Some("camera-a".into()))
            .await
            .unwrap();
        assert!(matches!(
            b.update(|config| config.video.device = Some("camera-a".into()))
                .await,
            Err(AppError::Conflict(_))
        ));
        assert!(b.get().video.device.is_none());
        db_a.pool().close().await;
        assert!(a
            .update(|config| config.video.device = Some("camera-b".into()))
            .await
            .is_err());
        assert_eq!(a.get().video.device.as_deref(), Some("camera-a"));
        b.update(|config| config.video.device = Some("camera-b".into()))
            .await
            .unwrap();
        assert!(b
            .update(|config| config.video.device = Some("camera-a".into()))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn startup_rejects_persisted_conflicts_before_hardware_can_open() {
        let dir = tempdir().unwrap();
        let db_a = DatabasePool::new(&dir.path().join("a.db")).await.unwrap();
        let db_b = DatabasePool::new(&dir.path().join("b.db")).await.unwrap();
        db_a.init_schema().await.unwrap();
        db_b.init_schema().await.unwrap();
        for db in [&db_a, &db_b] {
            ConfigStore::new(db.clone_pool())
                .update(|config| {
                    config.hid.backend = super::super::HidBackend::Ch9329;
                    config.hid.ch9329_port = "COM7".into();
                })
                .await
                .unwrap();
        }
        let registry = Arc::new(ResourceRegistry::default());
        let a =
            ConfigStore::with_resources(db_a.clone_pool(), registry.clone(), "a".into()).unwrap();
        let b = ConfigStore::with_resources(db_b.clone_pool(), registry, "b".into()).unwrap();
        a.load().await.unwrap();
        assert!(matches!(b.load().await, Err(AppError::Conflict(_))));
    }

    #[tokio::test]
    async fn test_config_store() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let db = DatabasePool::new(&db_path).await.unwrap();
        db.init_schema().await.unwrap();

        let store = ConfigStore::new(db.clone_pool());
        store.load().await.unwrap();

        let config = store.get();
        assert!(!config.initialized);

        store
            .update(|c| {
                c.initialized = true;
                c.web.http_port = 9000;
            })
            .await
            .unwrap();

        let config = store.get();
        assert!(config.initialized);
        assert_eq!(config.web.http_port, 9000);

        let store2 = ConfigStore::new(db.clone_pool());
        store2.load().await.unwrap();
        let config = store2.get();
        assert!(config.initialized);
        assert_eq!(config.web.http_port, 9000);
    }

    #[tokio::test]
    async fn failed_watchdog_persistence_does_not_update_cache() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let db = DatabasePool::new(&db_path).await.unwrap();
        db.init_schema().await.unwrap();
        let store = ConfigStore::new(db.clone_pool());
        store.load().await.unwrap();

        sqlx::query("DROP TABLE config")
            .execute(&db.clone_pool())
            .await
            .unwrap();
        assert!(store
            .update(|config| config.watchdog.enabled = true)
            .await
            .is_err());
        assert!(!store.get().watchdog.enabled);
    }

    #[tokio::test]
    async fn load_removes_legacy_totp_fields_from_persisted_config() {
        let dir = tempdir().unwrap();
        let db = DatabasePool::new(&dir.path().join("test.db"))
            .await
            .unwrap();
        db.init_schema().await.unwrap();
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        let auth = value.get_mut("auth").unwrap().as_object_mut().unwrap();
        auth.insert("totp_enabled".to_string(), serde_json::json!(true));
        auth.insert(
            "totp_secret".to_string(),
            serde_json::json!("legacy-secret"),
        );
        sqlx::query("INSERT INTO config (key, value) VALUES ('app_config', ?1)")
            .bind(value.to_string())
            .execute(db.pool())
            .await
            .unwrap();

        let store = ConfigStore::new(db.clone_pool());
        store.load().await.unwrap();
        let (persisted,): (String,) =
            sqlx::query_as("SELECT value FROM config WHERE key = 'app_config'")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert!(!persisted.contains("totp_enabled"));
        assert!(!persisted.contains("totp_secret"));
    }
}
