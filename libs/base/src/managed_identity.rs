//! Persistent installation identity for server-managed authorization.
//! An installation UUID identifies an installation; it is not an authentication secret.

use anyhow::{bail, Context, Result};
use hbb_common::uuid::{Uuid, Variant};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};

/// Load an existing UUIDv4, or atomically publish a new one if the file is absent.
///
/// The caller must supply an existing, trusted directory and the same path for
/// every process belonging to this installation, including a privileged service.
/// No per-user path, legacy ID, or hardware identifier is chosen here. Do not
/// delete this file on logout/unbind: replacing it creates a different identity.
/// This performs blocking filesystem I/O; async callers should use spawn_blocking.
pub fn load_or_create_installation_uuid(path: &Path) -> Result<Uuid> {
    match File::open(path) {
        Ok(file) => return read_uuid(file).with_context(|| format!("Read {}", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("Open {}", path.display())),
    }

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let uuid = Uuid::new_v4();
    let temporary = parent.join(format!(".installation-{}.tmp", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("Create installation identity in {}", parent.display()))?;
    let publish = (|| -> io::Result<()> {
        writeln!(file, "{}", uuid)?;
        file.sync_all()?;
        // A hard link publishes only a complete file and never replaces the
        // winner of concurrent initialization. Unsupported filesystems fail.
        fs::hard_link(&temporary, path)
    })();
    drop(file);
    fs::remove_file(&temporary)
        .with_context(|| format!("Remove temporary identity {}", temporary.display()))?;
    match publish {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err).with_context(|| format!("Persist {}", path.display())),
    }
    // Persist the directory entry as well as the file contents on Unix.
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("Sync identity directory {}", parent.display()))?;

    read_uuid(File::open(path).with_context(|| format!("Open {}", path.display()))?)
        .with_context(|| format!("Read {}", path.display()))
}

fn read_uuid(file: File) -> Result<Uuid> {
    // Bound reads: the format is exactly a hyphenated UUID and an optional LF.
    let mut value = String::new();
    file.take(38).read_to_string(&mut value)?;
    let value = value.strip_suffix('\n').unwrap_or(&value);
    let uuid = Uuid::parse_str(value).context("Invalid installation UUID")?;
    if value.len() != 36 || uuid.get_version_num() != 4 || uuid.get_variant() != Variant::RFC4122 {
        bail!("Installation identity must be a hyphenated UUIDv4");
    }
    Ok(uuid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Barrier, thread};

    #[test]
    fn identity_survives_reload_and_is_unique_per_installation() {
        let directory = std::env::temp_dir().join(Uuid::new_v4().to_string());
        fs::create_dir(&directory).unwrap();
        let path = directory.join("installation.uuid");
        let first = load_or_create_installation_uuid(&path).unwrap();
        assert_eq!(first.get_version_num(), 4);
        assert_eq!(first, load_or_create_installation_uuid(&path).unwrap());
        assert_ne!(
            first,
            load_or_create_installation_uuid(&directory.join("other.uuid")).unwrap()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_files_and_io_errors_do_not_replace_identity() {
        let directory = std::env::temp_dir().join(Uuid::new_v4().to_string());
        fs::create_dir(&directory).unwrap();
        let path = directory.join("installation.uuid");
        for invalid in ["", "broken", "00000000-0000-0000-0000-000000000000"] {
            fs::write(&path, invalid).unwrap();
            assert!(load_or_create_installation_uuid(&path).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
        }
        assert!(load_or_create_installation_uuid(&directory).is_err());
        assert!(load_or_create_installation_uuid(&directory.join("missing/id")).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn concurrent_initializers_share_one_complete_identity() {
        let directory = std::env::temp_dir().join(Uuid::new_v4().to_string());
        fs::create_dir(&directory).unwrap();
        let path = directory.join("installation.uuid");
        let barrier = Barrier::new(8);
        let identities = thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        load_or_create_installation_uuid(&path).unwrap()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        let persisted = load_or_create_installation_uuid(&path).unwrap();
        assert!(identities.iter().all(|uuid| *uuid == persisted));
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
