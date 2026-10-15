use crate::message_proto::{message, Message, PublicKey};
use hbb_common::{
    anyhow::anyhow,
    bail,
    config::{CONNECT_TIMEOUT, READ_TIMEOUT},
    protobuf::Message as _,
    sodiumoxide::crypto::sign,
    timeout, ResultType, Stream,
};

pub const DIRECT_PORT: u16 = 21118;

pub fn load_direct_identity(path: &std::path::Path) -> ResultType<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(key) if key.len() == sign::PUBLICKEYBYTES => Ok(Some(key)),
        Ok(_) => bail!("Saved device identity is damaged; verify the device before resetting it"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn persist_direct_identity(path: &std::path::Path, identity: &[u8]) -> ResultType<()> {
    use std::io::Write;
    if identity.len() != sign::PUBLICKEYBYTES {
        bail!("Invalid device identity");
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Invalid identity storage path"))?;
    std::fs::create_dir_all(parent)?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(identity)?;
            file.sync_all()?;
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    if load_direct_identity(path)?.as_deref() != Some(identity) {
        bail!("Device identity changed or could not be saved; connection refused");
    }
    Ok(())
}

pub fn normalize_direct_endpoint(input: &str) -> ResultType<String> {
    let input = input.trim();
    if let Ok(addr) = input.parse::<std::net::SocketAddr>() {
        if addr.port() == 0 {
            bail!("Direct connection port must be between 1 and 65535");
        }
        return Ok(addr.to_string());
    }
    if let Ok(ip) = input.parse::<std::net::IpAddr>() {
        return Ok(std::net::SocketAddr::new(ip, DIRECT_PORT).to_string());
    }
    if input.starts_with('[') && input.ends_with(']') {
        let ip = input[1..input.len() - 1].parse::<std::net::Ipv6Addr>()?;
        return Ok(std::net::SocketAddr::new(ip.into(), DIRECT_PORT).to_string());
    }
    let (host, port) = match input.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>()?),
        None => (input, DIRECT_PORT),
    };
    if port == 0
        || host.is_empty()
        || host.len() > 253
        || host.bytes().all(|b| b.is_ascii_digit())
        || !host.trim_end_matches('.').split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        bail!("Enter a reachable IP address or hostname, optionally followed by a port. Device IDs and relay connections are not supported.");
    }
    Ok(format!(
        "{}:{}",
        host.trim_end_matches('.').to_ascii_lowercase(),
        port
    ))
}

pub async fn connect_handshake(conn: &mut Stream, pinned: &[u8]) -> ResultType<Vec<u8>> {
    let bytes = timeout(READ_TIMEOUT, conn.next())
        .await?
        .ok_or_else(|| anyhow!("Peer closed before the encrypted handshake"))??;
    let msg = Message::parse_from_bytes(&bytes)?;
    let Some(message::Union::SignedId(identity)) = msg.union else {
        bail!("Peer does not support the required encrypted direct handshake");
    };
    let signing_key = sign::PublicKey::from_slice(&identity.signing_key)
        .ok_or_else(|| anyhow!("Invalid direct peer identity key"))?;
    let (_, ephemeral, advertised) = super::decode_peer_identity(&identity.id, &signing_key)?;
    if !pinned.is_empty() && pinned != signing_key.0 {
        bail!("Device identity changed. Verify its fingerprint on the remote device before removing the saved identity and reconnecting.");
    }
    let picked = hbb_common::tcp::KX_VERSION_LATEST;
    if advertised < picked {
        bail!("Peer does not support the required encrypted key exchange");
    }
    let (asymmetric_value, symmetric_value, key) = super::create_symmetric_key_msg(ephemeral);
    let mut msg = Message::new();
    msg.set_public_key(PublicKey {
        asymmetric_value: asymmetric_value.clone(),
        symmetric_value,
        kx_version: picked,
        ..Default::default()
    });
    timeout(CONNECT_TIMEOUT, conn.send(&msg)).await??;
    conn.set_negotiated_key(
        key,
        true,
        &hbb_common::tcp::KxTranscript {
            initiator_pk: &asymmetric_value,
            responder_pk: &ephemeral,
            advertised,
            picked,
        },
    )?;
    if !conn.is_secured() {
        bail!("Encrypted direct handshake failed");
    }
    Ok(signing_key.0.to_vec())
}

#[cfg(test)]
mod direct_transport_tests {
    use super::*;
    use crate::message_proto::{SignedId, TestDelay};
    use hbb_common::{rendezvous_proto::IdPk, tokio};
    use hbb_common::{
        sodiumoxide::crypto::box_,
        tcp::{Encrypt, FramedStream, KxTranscript, KX_VERSION_LATEST},
    };

    #[test]
    fn endpoints_accept_ip_hostname_and_explicit_port_only() {
        for (input, expected) in [
            ("127.0.0.1", "127.0.0.1:21118"),
            ("[::1]:1234", "[::1]:1234"),
            ("::1", "[::1]:21118"),
            ("Desktop.local", "desktop.local:21118"),
            ("desktop:1234", "desktop:1234"),
        ] {
            assert_eq!(normalize_direct_endpoint(input).unwrap(), expected);
        }
        for invalid in [
            "123456789",
            "123/r",
            "id@server",
            "wss://host",
            "host:0",
            "host:65536",
            "host/path",
            "",
            "-host",
        ] {
            assert!(normalize_direct_endpoint(invalid).is_err(), "{}", invalid);
        }
    }

    async fn direct_peer(
        advertised: u32,
        tamper: bool,
    ) -> (String, Vec<u8>, tokio::task::JoinHandle<bool>) {
        let (pk, sk) = sign::gen_keypair();
        let listener = hbb_common::tcp::new_listener("127.0.0.1:0", false)
            .await
            .unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let host = tokio::spawn(async move {
            let (socket, addr) = listener.accept().await.unwrap();
            let mut stream = Stream::Tcp(FramedStream::from(socket, addr));
            let (epk, esk) = box_::gen_keypair();
            let data = IdPk {
                id: "local-device-id".into(),
                pk: epk.0.to_vec().into(),
                kx_version: advertised,
                ..Default::default()
            }
            .write_to_bytes()
            .unwrap();
            let mut signed = sign::sign(&data, &sk);
            if tamper {
                signed[0] ^= 1;
            }
            let mut msg = Message::new();
            msg.set_signed_id(SignedId {
                id: signed.into(),
                signing_key: pk.0.to_vec().into(),
                ..Default::default()
            });
            stream.send(&msg).await.unwrap();
            let Some(Ok(bytes)) = stream.next().await else {
                return false;
            };
            let Some(message::Union::PublicKey(key)) =
                Message::parse_from_bytes(&bytes).unwrap().union
            else {
                panic!("application data sent before encryption");
            };
            assert_eq!(key.kx_version, KX_VERSION_LATEST);
            let secret =
                Encrypt::decode(&key.symmetric_value, &key.asymmetric_value, &esk).unwrap();
            stream
                .set_negotiated_key(
                    secret,
                    false,
                    &KxTranscript {
                        initiator_pk: &key.asymmetric_value,
                        responder_pk: &epk.0,
                        advertised,
                        picked: key.kx_version,
                    },
                )
                .unwrap();
            assert!(stream.is_secured());
            let Some(Ok(bytes)) = stream.next().await else {
                return false;
            };
            matches!(
                Message::parse_from_bytes(&bytes).unwrap().union,
                Some(message::Union::TestDelay(_))
            )
        });
        (endpoint, pk.0.to_vec(), host)
    }

    #[tokio::test]
    async fn direct_handshake_encrypts_application_data_and_binds_pinned_identity() {
        let (endpoint, identity, host) = direct_peer(KX_VERSION_LATEST, false).await;
        let mut stream = Stream::Tcp(
            FramedStream::new(endpoint.as_str(), None, 3000)
                .await
                .unwrap(),
        );
        let result = connect_handshake(&mut stream, &identity).await.unwrap();
        assert_eq!(result, identity);
        assert!(stream.is_secured());
        let mut msg = Message::new();
        msg.set_test_delay(TestDelay::new());
        stream.send(&msg).await.unwrap();
        assert!(host.await.unwrap());
    }

    #[tokio::test]
    async fn direct_handshake_rejects_changed_identity_invalid_signature_and_legacy_kx_without_plaintext(
    ) {
        for (version, tamper, pin) in [
            (KX_VERSION_LATEST, false, b"different-key".as_slice()),
            (KX_VERSION_LATEST, true, b"".as_slice()),
            (0, false, b"".as_slice()),
        ] {
            let (endpoint, _, host) = direct_peer(version, tamper).await;
            let mut stream = Stream::Tcp(
                FramedStream::new(endpoint.as_str(), None, 3000)
                    .await
                    .unwrap(),
            );
            assert!(connect_handshake(&mut stream, pin).await.is_err());
            drop(stream);
            assert!(!host.await.unwrap());
        }
    }
    #[test]
    fn identity_persistence_refuses_overwrite_corruption_and_read_failures() {
        let dir = std::env::temp_dir().join(format!(
            "rustdesk-direct-pin-{}",
            hbb_common::uuid::Uuid::new_v4()
        ));
        let path = dir.join("identity");
        let a = vec![1; sign::PUBLICKEYBYTES];
        let b = vec![2; sign::PUBLICKEYBYTES];
        assert_eq!(load_direct_identity(&path).unwrap(), None);
        let other = path.clone();
        let first = a.clone();
        let t = std::thread::spawn(move || persist_direct_identity(&other, &first).is_ok());
        let second_won = persist_direct_identity(&path, &b).is_ok();
        let first_won = t.join().unwrap();
        assert_ne!(first_won, second_won);
        let expected = if first_won { &a } else { &b };
        assert_eq!(
            load_direct_identity(&path).unwrap().as_ref(),
            Some(expected)
        );
        assert!(persist_direct_identity(&path, expected).is_ok());
        std::fs::write(&path, []).unwrap();
        assert!(load_direct_identity(&path).is_err());
        assert!(persist_direct_identity(&path, &a).is_err());
        assert!(load_direct_identity(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
