use crate::private_fs::PrivateDirectory;
use async_trait::async_trait;
use std::{
    fs,
    sync::{Arc, Mutex},
};
use xscc::{
    engine::{ExecutionRecord, Journal, JournalError, MAX_PENDING_RESULTS, MAX_RECORD_BYTES},
    protocol::Binding,
};

const MAX_RETAINED: usize = 131_072;
#[derive(Clone)]
pub struct FileJournal {
    directory: PrivateDirectory,
    gate: Arc<Mutex<()>>,
    _lock: Arc<std::fs::File>,
}
impl FileJournal {
    pub fn open(root: &PrivateDirectory, binding: &Binding) -> Result<Self, JournalError> {
        let name = format!(
            "journal-{}-{}-{}",
            binding.manager_id, binding.device_id, binding.installation_id
        );
        let directory = root.child(&name).map_err(storage)?;
        let lock = directory.lock(".lock").map_err(storage)?;
        Ok(Self {
            directory,
            gate: Arc::new(Mutex::new(())),
            _lock: Arc::new(lock),
        })
    }
    fn name(id: &str) -> Result<String, JournalError> {
        let uuid = id
            .strip_prefix("op_")
            .and_then(|s| uuid::Uuid::parse_str(s).ok())
            .ok_or(JournalError::Storage)?;
        if uuid.is_nil() || format!("op_{uuid}") != id {
            return Err(JournalError::Storage);
        }
        Ok(format!("{id}.json"))
    }
    fn read(&self, id: &str) -> Result<Option<ExecutionRecord>, JournalError> {
        self.directory
            .read(&Self::name(id)?, MAX_RECORD_BYTES)
            .map_err(storage)?
            .map(|b| serde_json::from_slice(&b).map_err(storage))
            .transpose()
    }
    fn pending(&self) -> Result<(usize, Vec<(String, ExecutionRecord)>), JournalError> {
        let mut count = 0;
        let mut records = Vec::new();
        for entry in fs::read_dir(self.directory.path()).map_err(storage)? {
            let name = entry.map_err(storage)?.file_name();
            let name = name.to_str().ok_or(JournalError::Storage)?;
            let Some(id) = name.strip_suffix(".json") else {
                continue;
            };
            count += 1;
            if count > MAX_RETAINED {
                return Err(JournalError::Full);
            }
            let record = self.read(id)?.ok_or(JournalError::Storage)?;
            if !record.acknowledged {
                records.push((id.to_owned(), record));
                if records.len() > MAX_PENDING_RESULTS {
                    return Err(JournalError::Full);
                }
            }
        }
        Ok((count, records))
    }
    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(Self) -> Result<T, JournalError> + Send + 'static,
    ) -> Result<T, JournalError> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = store.gate.lock().map_err(storage)?;
            work(store.clone())
        })
        .await
        .map_err(storage)?
    }
    async fn write(
        &self,
        id: &str,
        record: &ExecutionRecord,
        create: bool,
    ) -> Result<(), JournalError> {
        let name = Self::name(id)?;
        let record = record.clone();
        let id = id.to_owned();
        self.blocking(move |store| {
            let old = store.read(&id)?;
            if create {
                let (count, pending) = store.pending()?;
                if count >= MAX_RETAINED || pending.len() >= MAX_PENDING_RESULTS {
                    return Err(JournalError::Full);
                }
                if old.is_some() {
                    return Err(JournalError::Storage);
                }
            } else {
                let old = old.ok_or(JournalError::Storage)?;
                if old.binding != record.binding
                    || old.fingerprint != record.fingerprint
                    || old.acknowledged
                {
                    return Err(JournalError::Storage);
                }
            }
            let bytes = serde_json::to_vec(&record).map_err(storage)?;
            if bytes.len() > MAX_RECORD_BYTES {
                return Err(JournalError::Storage);
            }
            store
                .directory
                .write(&name, &bytes, create)
                .map_err(storage)
        })
        .await
    }
}
fn storage<T>(_: T) -> JournalError {
    JournalError::Storage
}
#[async_trait]
impl Journal for FileJournal {
    async fn pending_results(&self) -> Result<Vec<(String, ExecutionRecord)>, JournalError> {
        self.blocking(|s| Ok(s.pending()?.1)).await
    }
    async fn load(&self, id: &str) -> Result<Option<ExecutionRecord>, JournalError> {
        let id = id.to_owned();
        self.blocking(move |s| s.read(&id)).await
    }
    async fn create(&self, id: &str, record: &ExecutionRecord) -> Result<(), JournalError> {
        self.write(id, record, true).await
    }
    async fn replace(&self, id: &str, record: &ExecutionRecord) -> Result<(), JournalError> {
        self.write(id, record, false).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn committed_receipts_survive_reopen_and_cannot_be_overwritten() {
        let temporary = tempfile::tempdir().unwrap();
        let root = PrivateDirectory::create(temporary.path(), "native").unwrap();
        let binding = Binding {
            manager_id: uuid::Uuid::new_v4(),
            device_id: uuid::Uuid::new_v4(),
            installation_id: uuid::Uuid::new_v4(),
        };
        let id = format!("op_{}", uuid::Uuid::new_v4());
        let mut record = ExecutionRecord {
            binding: binding.clone(),
            fingerprint: "a".repeat(64),
            report: None,
            acknowledged: false,
            accepted_digest: None,
        };
        let journal = FileJournal::open(&root, &binding).unwrap();
        assert!(FileJournal::open(&root, &binding).is_err());
        journal.create(&id, &record).await.unwrap();
        assert_eq!(journal.pending_results().await.unwrap().len(), 1);
        let mut changed = record.clone();
        changed.fingerprint = "b".repeat(64);
        assert!(journal.replace(&id, &changed).await.is_err());
        record.acknowledged = true;
        record.accepted_digest = Some("c".repeat(64));
        journal.replace(&id, &record).await.unwrap();
        drop(journal);
        let journal = FileJournal::open(&root, &binding).unwrap();
        assert_eq!(journal.load(&id).await.unwrap(), Some(record.clone()));
        assert!(journal.pending_results().await.unwrap().is_empty());
        assert!(journal.create(&id, &record).await.is_err());
        assert!(journal.replace(&id, &record).await.is_err());
        let mut other = binding;
        other.installation_id = uuid::Uuid::new_v4();
        assert!(
            FileJournal::open(&root, &other)
                .unwrap()
                .load(&id)
                .await
                .unwrap()
                .is_none()
        );
    }
}
