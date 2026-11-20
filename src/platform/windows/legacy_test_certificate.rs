use hbb_common::{log, ResultType};
use sha2::{Digest, Sha256};
use std::io;
use winreg::{
    enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, KEY_WRITE, REG_BINARY},
    RegKey,
};

const CERTIFICATE_STORES: &str = r"Software\Microsoft\SystemCertificates";
const LEGACY_TEST_CERTIFICATE_THUMBPRINT: &str = "D1DBB672D5A500B9809689CAEA1CE49E799767F0";
const LEGACY_TEST_CERTIFICATE_LENGTH: usize = 784;
const LEGACY_TEST_CERTIFICATE_SHA256: [u8; 32] = [
    0x74, 0x27, 0x55, 0x55, 0x50, 0xe9, 0x6a, 0x0c, 0x93, 0xfd, 0x48, 0xfc, 0xed, 0x17, 0x0b, 0x61,
    0x42, 0x84, 0xf1, 0x82, 0x34, 0x99, 0x95, 0xae, 0xb4, 0xad, 0xc9, 0xcf, 0xd2, 0xaf, 0xee, 0x80,
];

pub(super) fn uninstall() -> ResultType<()> {
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        remove_from_user_or_machine(&RegKey::predef(hive));
    }
    let users = RegKey::predef(HKEY_USERS);
    for name in users.enum_keys().flatten() {
        if let Ok(hive) = users.open_subkey(name) {
            remove_from_user_or_machine(&hive);
        }
    }
    Ok(())
}

fn remove_from_user_or_machine(hive: &RegKey) {
    let Ok(stores) = hive.open_subkey_with_flags(CERTIFICATE_STORES, KEY_READ | KEY_WRITE) else {
        return;
    };
    let names: Vec<_> = stores.enum_keys().flatten().collect();
    for name in names {
        let path = format!(r"{name}\Certificates\{LEGACY_TEST_CERTIFICATE_THUMBPRINT}");
        if let Err(error) = remove_pinned_certificate(&stores, &path) {
            if error.kind() != io::ErrorKind::NotFound {
                log::warn!("Could not remove the legacy Windows test certificate: {error}");
            }
        }
        // Early versions decoded the ANSI bytes of "ROOT" as UTF-16.
        if name.starts_with("佒呏") {
            remove_empty_legacy_store(&stores, &name);
        }
    }
}

fn remove_pinned_certificate(stores: &RegKey, path: &str) -> io::Result<()> {
    let certificate = stores.open_subkey(path)?;
    let value = certificate.get_raw_value("Blob")?;
    if value.vtype != REG_BINARY {
        return Ok(());
    }
    if let Some(der) = value.bytes.last_chunk::<LEGACY_TEST_CERTIFICATE_LENGTH>() {
        if Sha256::digest(der)[..] == LEGACY_TEST_CERTIFICATE_SHA256 {
            drop(certificate);
            stores.delete_subkey(path)?;
        }
    }
    Ok(())
}

fn remove_empty_legacy_store(stores: &RegKey, name: &str) {
    let Ok(store) = stores.open_subkey_with_flags(name, KEY_READ | KEY_WRITE) else {
        return;
    };
    let children: Vec<_> = store.enum_keys().flatten().collect();
    for child in children {
        if store
            .open_subkey(&child)
            .is_ok_and(|key| key.enum_keys().next().is_none() && key.enum_values().next().is_none())
        {
            let _ = store.delete_subkey(child);
        }
    }
    if store.enum_keys().next().is_none() && store.enum_values().next().is_none() {
        drop(store);
        let _ = stores.delete_subkey(name);
    }
}
