//! Durable credentials stay in the owning native process's private directory.
use crate::{
    enrollment::{BindingStore, EnrollmentError, SavedBinding, ServerOrigin},
    private_fs::PrivateDirectory,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
#[cfg(all(test, unix))]
use std::path::PathBuf;
use uuid::Uuid;
use xscc::protocol::Binding;
use zeroize::{Zeroize, Zeroizing};
type Result<T> = std::result::Result<T, EnrollmentError>;
#[derive(Clone)]
pub struct FileBindingStore {
    directory: PrivateDirectory,
    installation_id: Uuid,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    enrolled: bool,
    server: String,
    binding: Binding,
    credential: String,
}
impl Drop for Record {
    fn drop(&mut self) {
        self.credential.zeroize();
    }
}
impl Record {
    fn from_saved(saved: &SavedBinding, enrolled: bool) -> Self {
        Self {
            version: 1,
            enrolled,
            server: saved.server().as_str().to_owned(),
            binding: saved.binding().clone(),
            credential: saved.credential().to_owned(),
        }
    }
    fn saved(&self, server: &ServerOrigin, installation_id: Uuid) -> Result<SavedBinding> {
        if self.version != 1
            || self.server != server.as_str()
            || self.binding.installation_id != installation_id
        {
            return Err(EnrollmentError::Storage);
        }
        SavedBinding::restore(
            server.clone(),
            self.binding.clone(),
            Zeroizing::new(self.credential.clone()),
        )
        .map_err(storage)
    }
}
impl FileBindingStore {
    pub fn open(directory: &Path, installation_id: Uuid) -> Result<Self> {
        if installation_id.is_nil() {
            return Err(EnrollmentError::Configuration);
        }
        let root = PrivateDirectory::open(directory).map_err(storage)?;
        Ok(Self {
            directory: root.child("management").map_err(storage)?,
            installation_id,
        })
    }
    fn name(&self, server: &ServerOrigin) -> String {
        format!(
            "{}.json",
            hex::encode(Sha256::digest(server.as_str().as_bytes()))
        )
    }
    #[cfg(all(test, unix))]
    fn path(&self, server: &ServerOrigin) -> PathBuf {
        self.directory.path().join(self.name(server))
    }
    fn read(&self, server: &ServerOrigin) -> Result<Option<Record>> {
        self.directory
            .read(&self.name(server), 4096)
            .map_err(storage)?
            .map(|b| {
                let r: Record = serde_json::from_slice(&b).map_err(storage)?;
                r.saved(server, self.installation_id)?;
                Ok(r)
            })
            .transpose()
    }
    fn write(&self, saved: &SavedBinding, enrolled: bool) -> Result<()> {
        if saved.binding().installation_id != self.installation_id {
            return Err(EnrollmentError::Storage);
        }
        if enrolled {
            let current = self
                .read(saved.server())?
                .ok_or(EnrollmentError::MissingBinding)?;
            if current.binding != *saved.binding() || current.credential != saved.credential() {
                return Err(EnrollmentError::BindingMismatch);
            }
            if current.enrolled {
                return Ok(());
            }
        }
        let bytes = Zeroizing::new(
            serde_json::to_vec(&Record::from_saved(saved, enrolled)).map_err(storage)?,
        );
        if bytes.len() > 4096 {
            return Err(EnrollmentError::Storage);
        }
        self.directory
            .write(&self.name(saved.server()), &bytes, !enrolled)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    EnrollmentError::ExistingBinding
                } else {
                    EnrollmentError::Storage
                }
            })
    }
    pub async fn forget(&self, server: &ServerOrigin) -> Result<()> {
        let store = self.clone();
        let server = server.clone();
        tokio::task::spawn_blocking(move || {
            store
                .directory
                .remove(&store.name(&server))
                .map_err(storage)
        })
        .await
        .map_err(storage)?
    }
}
#[async_trait::async_trait]
impl BindingStore for FileBindingStore {
    async fn load(&self, server: &ServerOrigin) -> Result<Option<SavedBinding>> {
        let store = self.clone();
        let server = server.clone();
        tokio::task::spawn_blocking(move || {
            store
                .read(&server)?
                .map(|r| r.saved(&server, store.installation_id))
                .transpose()
        })
        .await
        .map_err(storage)?
    }
    async fn create(&self, pending: &SavedBinding) -> Result<()> {
        let store = self.clone();
        let saved = SavedBinding::restore(
            pending.server().clone(),
            pending.binding().clone(),
            Zeroizing::new(pending.credential().to_owned()),
        )?;
        tokio::task::spawn_blocking(move || store.write(&saved, false))
            .await
            .map_err(storage)?
    }
    async fn mark_enrolled(&self, saved: &SavedBinding) -> Result<()> {
        let store = self.clone();
        let saved = SavedBinding::restore(
            saved.server().clone(),
            saved.binding().clone(),
            Zeroizing::new(saved.credential().to_owned()),
        )?;
        tokio::task::spawn_blocking(move || store.write(&saved, true))
            .await
            .map_err(storage)?
    }
}
fn storage<T>(_: T) -> EnrollmentError {
    EnrollmentError::Storage
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, PermissionsExt},
    };

    fn fixture() -> (PathBuf, Uuid, SavedBinding) {
        let directory = std::env::temp_dir().join(format!("managed-store-{}", Uuid::new_v4()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .unwrap();
        let id = Uuid::new_v4();
        let saved = SavedBinding::restore(
            ServerOrigin::parse("https://example.invalid").unwrap(),
            Binding {
                manager_id: Uuid::new_v4(),
                device_id: Uuid::new_v4(),
                installation_id: id,
            },
            Zeroizing::new("a".repeat(64)),
        )
        .unwrap();
        (directory, id, saved)
    }

    #[tokio::test]
    async fn pending_and_enrolled_survive_reopen_without_rotating_identity() {
        let (directory, id, saved) = fixture();
        let store = FileBindingStore::open(&directory, id).unwrap();
        store.create(&saved).await.unwrap();
        let reopened = FileBindingStore::open(&directory, id).unwrap();
        let loaded = reopened.load(saved.server()).await.unwrap().unwrap();
        assert_eq!(loaded.binding(), saved.binding());
        assert_eq!(loaded.credential(), saved.credential());
        reopened.mark_enrolled(&loaded).await.unwrap();
        assert!(reopened.read(saved.server()).unwrap().unwrap().enrolled);
        assert_eq!(
            reopened
                .load(saved.server())
                .await
                .unwrap()
                .unwrap()
                .credential(),
            saved.credential()
        );
        assert!(
            FileBindingStore::open(&directory, Uuid::new_v4())
                .unwrap()
                .load(saved.server())
                .await
                .is_err()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn create_never_overwrites_existing_or_corrupt_binding() {
        let (directory, id, saved) = fixture();
        let store = FileBindingStore::open(&directory, id).unwrap();
        store.create(&saved).await.unwrap();
        assert_eq!(
            store.create(&saved).await,
            Err(EnrollmentError::ExistingBinding)
        );
        fs::write(store.path(saved.server()), "corrupt").unwrap();
        assert!(store.load(saved.server()).await.is_err());
        assert_eq!(
            store.create(&saved).await,
            Err(EnrollmentError::ExistingBinding)
        );
        assert_eq!(
            fs::read_to_string(store.path(saved.server())).unwrap(),
            "corrupt"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn insecure_permissions_and_failed_atomic_publication_fail_closed() {
        let (directory, id, saved) = fixture();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(FileBindingStore::open(&directory, id).is_err());
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let store = FileBindingStore::open(&directory, id).unwrap();
        fs::create_dir(store.path(saved.server())).unwrap();
        assert!(store.create(&saved).await.is_err());
        assert!(store.load(saved.server()).await.is_err());
        assert_eq!(fs::read_dir(store.directory.path()).unwrap().count(), 1);
        fs::remove_dir(store.path(saved.server())).unwrap();
        store.create(&saved).await.unwrap();
        fs::set_permissions(
            store.path(saved.server()),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(store.load(saved.server()).await.is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
