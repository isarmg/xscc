//! Protected storage owned by the actual host service, not its GUI process.
use super::enrollment::{BindingStore, EnrollmentError, SavedBinding, ServerOrigin};
use std::path::Path;
use uuid::Uuid;

type Result<T> = std::result::Result<T, EnrollmentError>;

#[cfg(unix)]
use {
    hbb_common::{
        libc,
        sha2::{Digest, Sha256},
    },
    serde::{Deserialize, Serialize},
    std::{
        fs::{self, File, OpenOptions},
        io::{ErrorKind, Read, Write},
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        path::PathBuf,
    },
    xscc::protocol::Binding,
    zeroize::{Zeroize, Zeroizing},
};

/// The caller supplies an existing trusted private service directory. Never accept
/// this path from GUI/IPC input or derive it from ordinary user preferences.
#[derive(Clone)]
pub struct FileBindingStore {
    #[cfg(unix)]
    directory: PathBuf,
    #[cfg(unix)]
    installation_id: Uuid,
}

impl FileBindingStore {
    /// Blocking filesystem I/O. Call from service initialization or spawn_blocking.
    pub fn open(service_directory: &Path, installation_id: Uuid) -> Result<Self> {
        if installation_id.is_nil() || !service_directory.is_absolute() {
            return Err(EnrollmentError::Configuration);
        }
        #[cfg(unix)]
        {
            verify_directory(service_directory)?;
            let directory = service_directory.join("management");
            match fs::DirBuilder::new().mode(0o700).create(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(_) => return Err(EnrollmentError::Storage),
            }
            verify_directory(&directory)?;
            File::open(service_directory)
                .and_then(|file| file.sync_all())
                .map_err(storage)?;
            Ok(Self {
                directory,
                installation_id,
            })
        }
        #[cfg(not(unix))]
        {
            // Do not assume inherited Windows ACLs are private. Enable only after
            // the service-owned directory/DACL contract has a verified adapter.
            let _ = service_directory;
            Err(EnrollmentError::Storage)
        }
    }
}

#[cfg(unix)]
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u8,
    enrolled: bool,
    server: String,
    binding: Binding,
    credential: String,
}

#[cfg(unix)]
impl Drop for Record {
    fn drop(&mut self) {
        self.credential.zeroize();
    }
}

#[cfg(unix)]
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

#[cfg(unix)]
impl FileBindingStore {
    fn path(&self, server: &ServerOrigin) -> PathBuf {
        self.directory.join(format!(
            "{}.json",
            hex::encode(Sha256::digest(server.as_str().as_bytes()))
        ))
    }

    fn read(&self, server: &ServerOrigin) -> Result<Option<Record>> {
        verify_directory(&self.directory)?;
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.path(server))
        {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(EnrollmentError::Storage),
        };
        let metadata = file.metadata().map_err(storage)?;
        if !metadata.is_file()
            || metadata.uid() != effective_uid()
            || metadata.mode() & 0o777 != 0o600
        {
            return Err(EnrollmentError::Storage);
        }
        let mut bytes = Zeroizing::new(Vec::new());
        file.take(4097).read_to_end(&mut bytes).map_err(storage)?;
        if bytes.len() > 4096 {
            return Err(EnrollmentError::Storage);
        }
        let record: Record = serde_json::from_slice(&bytes).map_err(storage)?;
        record.saved(server, self.installation_id)?;
        Ok(Some(record))
    }

    fn write(&self, saved: &SavedBinding, enrolled: bool) -> Result<()> {
        verify_directory(&self.directory)?;
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
        let temporary = self
            .directory
            .join(format!(".binding-{}.tmp", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temporary)
            .map_err(storage)?;
        let commit = (|| {
            if file.metadata().map_err(storage)?.mode() & 0o777 != 0o600 {
                return Err(EnrollmentError::Storage);
            }
            file.write_all(&bytes).map_err(storage)?;
            file.sync_all().map_err(storage)?;
            if enrolled {
                fs::rename(&temporary, self.path(saved.server())).map_err(storage)?;
            } else {
                fs::hard_link(&temporary, self.path(saved.server())).map_err(|error| {
                    if error.kind() == ErrorKind::AlreadyExists {
                        EnrollmentError::ExistingBinding
                    } else {
                        EnrollmentError::Storage
                    }
                })?;
            }
            Ok(())
        })();
        drop(file);
        match fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(error) if enrolled && commit.is_ok() && error.kind() == ErrorKind::NotFound => {}
            Err(_) => return Err(EnrollmentError::Storage),
        }
        // Even a failed directory sync leaves the original identity in place;
        // callers must recover it, never silently replace it after an error.
        File::open(&self.directory)
            .and_then(|file| file.sync_all())
            .map_err(storage)?;
        commit
    }
}

#[async_trait::async_trait]
impl BindingStore for FileBindingStore {
    async fn load(&self, server: &ServerOrigin) -> Result<Option<SavedBinding>> {
        #[cfg(unix)]
        {
            let store = self.clone();
            let server = server.clone();
            hbb_common::tokio::task::spawn_blocking(move || {
                store
                    .read(&server)?
                    .map(|record| record.saved(&server, store.installation_id))
                    .transpose()
            })
            .await
            .map_err(storage)?
        }
        #[cfg(not(unix))]
        {
            let _ = server;
            Err(EnrollmentError::Storage)
        }
    }

    async fn create(&self, pending: &SavedBinding) -> Result<()> {
        #[cfg(unix)]
        {
            let store = self.clone();
            let saved = SavedBinding::restore(
                pending.server().clone(),
                pending.binding().clone(),
                Zeroizing::new(pending.credential().to_owned()),
            )?;
            hbb_common::tokio::task::spawn_blocking(move || store.write(&saved, false))
                .await
                .map_err(storage)?
        }
        #[cfg(not(unix))]
        {
            let _ = pending;
            Err(EnrollmentError::Storage)
        }
    }

    async fn mark_enrolled(&self, saved: &SavedBinding) -> Result<()> {
        #[cfg(unix)]
        {
            let store = self.clone();
            let saved = SavedBinding::restore(
                saved.server().clone(),
                saved.binding().clone(),
                Zeroizing::new(saved.credential().to_owned()),
            )?;
            hbb_common::tokio::task::spawn_blocking(move || store.write(&saved, true))
                .await
                .map_err(storage)?
        }
        #[cfg(not(unix))]
        {
            let _ = saved;
            Err(EnrollmentError::Storage)
        }
    }
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    // geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn verify_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(storage)?;
    if !metadata.is_dir() || metadata.uid() != effective_uid() || metadata.mode() & 0o777 != 0o700 {
        return Err(EnrollmentError::Storage);
    }
    Ok(())
}

#[cfg(unix)]
fn storage<T>(_: T) -> EnrollmentError {
    EnrollmentError::Storage
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

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
        assert!(FileBindingStore::open(&directory, Uuid::new_v4())
            .unwrap()
            .load(saved.server())
            .await
            .is_err());
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
        assert_eq!(fs::read_dir(&store.directory).unwrap().count(), 1);
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
