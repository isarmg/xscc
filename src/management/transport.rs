#[cfg(not(any(target_os = "android", target_os = "ios")))]
use super::{owner, MANAGER};
use super::{rpc, Request, SessionPermit};
use crate::client::{Client, Interface};
use hbb_common::{
    anyhow::Context,
    bail,
    config::{self, CONNECT_TIMEOUT},
    rendezvous_proto::ConnType,
    stream::Stream,
    tcp::FramedStream,
    ResultType,
};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use xscc::protocol::InstanceRole;
use xscc_native_management::{
    manager::Target,
    session::{nonce, Transcript, WireMessage},
};

async fn send(stream: &mut Stream, message: &WireMessage) -> ResultType<()> {
    stream.send_raw(serde_json::to_vec(message)?).await
}
async fn receive(stream: &mut Stream) -> ResultType<WireMessage> {
    let packet = hbb_common::timeout(CONNECT_TIMEOUT, stream.next())
        .await
        .context("Management handshake timed out")?
        .context("Management peer closed during handshake")??;
    if packet.len() > 4096 {
        bail!("Management handshake is too large");
    }
    Ok(serde_json::from_slice(&packet)?)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) async fn accept(
    stream: &mut Stream,
    host_key: &[u8],
) -> ResultType<Option<SessionPermit>> {
    // Startup initializes the owner before opening the native listener. Unit tests and an
    // application built without a configured binding retain the direct transport path.
    if MANAGER.get().is_none() {
        #[cfg(feature = "gpui-host")]
        bail!("Host management owner is unavailable");
        #[cfg(not(feature = "gpui-host"))]
        return Ok(None);
    }
    let manager = owner()?;
    if !manager.configured() {
        #[cfg(feature = "gpui-host")]
        bail!("Bind XSCC Host before accepting connections");
        #[cfg(not(feature = "gpui-host"))]
        return Ok(None);
    }
    stream.set_max_packet_length(4096);
    let WireMessage::Hello {
        permit,
        client_nonce,
    } = receive(stream).await?
    else {
        bail!("Managed controller identity required");
    };
    let key = manager.grant(&permit, InstanceRole::Controlled)?;
    let transcript = Transcript {
        permit,
        client_nonce,
        host_nonce: nonce()?,
        transport_host_key: hex::encode(host_key),
    };
    send(
        stream,
        &WireMessage::Challenge {
            host_nonce: transcript.host_nonce.clone(),
            proof: transcript.proof(&key, true)?,
        },
    )
    .await?;
    let WireMessage::Proof { proof } = receive(stream).await? else {
        bail!("Managed controller proof required");
    };
    transcript.verify(&key, false, &proof)?;
    if !manager.permits(&transcript.permit, InstanceRole::Controlled) {
        bail!("Management authorization expired");
    }
    send(stream, &WireMessage::Accepted {}).await?;
    Ok(Some(transcript.permit))
}

pub(crate) async fn connect(
    peer: &str,
    conn_type: ConnType,
    interface: impl Interface,
) -> ResultType<(Stream, Vec<u8>, &'static str, Option<SessionPermit>)> {
    #[cfg(all(feature = "test-lan-direct", any(feature = "gpui-mobile-session", not(any(target_os = "android", target_os = "ios")))))]
    if peer.starts_with(crate::test_lan::PREFIX) {
        if conn_type != ConnType::DEFAULT_CONN {
            bail!("Only desktop sessions are available in LAN test mode");
        }
        let (stream, key, kind) = crate::test_lan::connect(peer).await?;
        return Ok((stream, key, kind, None));
    }
    let status = rpc(Request::Status {}).await?;
    if status
        .get("configured")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        if peer.starts_with("managed:") {
            bail!("Managed authorization is unavailable");
        }
        let (stream, key, kind) = Client::start(peer, conn_type, interface).await?;
        return Ok((stream, key, kind, None));
    }
    if conn_type != ConnType::DEFAULT_CONN || config::is_incoming_only() {
        bail!("Unsupported managed session type");
    }
    let id = peer
        .strip_prefix("managed:")
        .context("Select a host from the managed host list")?
        .parse()?;
    let target: Target = serde_json::from_value(rpc(Request::Target { device_id: id }).await?)?;
    interface.update_direct(None);
    interface.update_received(false);
    let mut last_error = None;
    for address in &target.addresses {
        let endpoint = if address.host.contains(':') {
            format!("[{}]:{}", address.host, address.port)
        } else {
            format!("{}:{}", address.host, address.port)
        };
        let result = connect_target(&endpoint, &target).await;
        match result {
            Ok((stream, key)) => {
                interface.update_direct(Some(true));
                return Ok((stream, key, "TCP", Some(target.permit)));
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .unwrap_or_else(|| hbb_common::anyhow::anyhow!("No authorized host address is available")))
}

async fn connect_target(endpoint: &str, target: &Target) -> ResultType<(Stream, Vec<u8>)> {
    let mut stream = Stream::Tcp(FramedStream::new(endpoint, None, CONNECT_TIMEOUT).await?);
    stream.set_send_timeout(CONNECT_TIMEOUT);
    stream.set_max_packet_length(4096);
    let key = base::direct_transport::client::connect_handshake(&mut stream, &[]).await?;
    if !stream.is_secured() {
        bail!("Encrypted direct transport required");
    }
    let client_nonce = nonce()?;
    send(
        &mut stream,
        &WireMessage::Hello {
            permit: target.permit.clone(),
            client_nonce: client_nonce.clone(),
        },
    )
    .await?;
    let WireMessage::Challenge { host_nonce, proof } = receive(&mut stream).await? else {
        bail!("Managed host proof required");
    };
    let transcript = Transcript {
        permit: target.permit.clone(),
        client_nonce,
        host_nonce,
        transport_host_key: hex::encode(&key),
    };
    let value = rpc(Request::Prove {
        transcript,
        host_proof: proof,
    })
    .await?;
    let proof = value
        .as_str()
        .context("Invalid managed proof response")?
        .to_owned();
    send(&mut stream, &WireMessage::Proof { proof }).await?;
    if !matches!(receive(&mut stream).await?, WireMessage::Accepted {})
        || !check_controller(Some(&target.permit)).await
    {
        bail!("Management authorization expired");
    }
    Ok((stream, key))
}

pub(crate) async fn check_controller(permit: Option<&SessionPermit>) -> bool {
    rpc(Request::Check {
        permit: permit.cloned(),
    })
    .await
    .is_ok_and(|v| v == serde_json::Value::Bool(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hbb_common::{sodiumoxide::crypto::sign, tcp, tokio, uuid};
    use xscc::protocol::{Binding, SessionKey};

    async fn encrypted_pair() -> (Stream, Stream, Vec<u8>) {
        let listener = tcp::new_listener("127.0.0.1:0", false).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (controller, accepted) = tokio::join!(
            FramedStream::new(address, None, CONNECT_TIMEOUT),
            listener.accept()
        );
        let (socket, _) = accepted.unwrap();
        let mut controller = Stream::Tcp(controller.unwrap());
        let mut host = Stream::from(socket, address);
        let (public, secret) = sign::gen_keypair();
        let (accepted, connected) = tokio::join!(
            base::direct_transport::host::accept_handshake(
                &mut host,
                &secret.0,
                &public.0,
                "managed-test-host"
            ),
            base::direct_transport::client::connect_handshake(&mut controller, &public.0)
        );
        accepted.unwrap();
        let identity = connected.unwrap();
        assert_eq!(identity, public.0);
        assert!(controller.is_secured() && host.is_secured());
        controller.set_max_packet_length(4096);
        host.set_max_packet_length(4096);
        (controller, host, identity)
    }

    #[tokio::test]
    async fn encrypted_management_handshake_completes_both_direction_proofs() {
        let (mut controller, mut host, identity) = encrypted_pair().await;
        let manager_id = uuid::Uuid::new_v4();
        let binding = || Binding {
            manager_id,
            device_id: uuid::Uuid::new_v4(),
            installation_id: uuid::Uuid::new_v4(),
        };
        let key = SessionKey::from_bytes([7; 32]);
        let permit = SessionPermit {
            controller: binding(),
            controlled: binding(),
            epoch: xscc_native_management::session::epoch(&key),
        };
        let client_nonce = nonce().unwrap();
        send(
            &mut controller,
            &WireMessage::Hello {
                permit: permit.clone(),
                client_nonce: client_nonce.clone(),
            },
        )
        .await
        .unwrap();
        let WireMessage::Hello {
            permit: received_permit,
            client_nonce: received_nonce,
        } = receive(&mut host).await.unwrap()
        else {
            panic!("host did not receive the encrypted management Hello");
        };
        assert_eq!(received_permit, permit);
        let host_transcript = Transcript {
            permit: received_permit,
            client_nonce: received_nonce,
            host_nonce: nonce().unwrap(),
            transport_host_key: hex::encode(&identity),
        };
        send(
            &mut host,
            &WireMessage::Challenge {
                host_nonce: host_transcript.host_nonce.clone(),
                proof: host_transcript.proof(&key, true).unwrap(),
            },
        )
        .await
        .unwrap();
        let WireMessage::Challenge { host_nonce, proof } = receive(&mut controller).await.unwrap()
        else {
            panic!("controller did not receive the encrypted host proof");
        };
        let controller_transcript = Transcript {
            permit,
            client_nonce,
            host_nonce,
            transport_host_key: hex::encode(identity),
        };
        controller_transcript.verify(&key, true, &proof).unwrap();
        send(
            &mut controller,
            &WireMessage::Proof {
                proof: controller_transcript.proof(&key, false).unwrap(),
            },
        )
        .await
        .unwrap();
        let WireMessage::Proof { proof } = receive(&mut host).await.unwrap() else {
            panic!("host did not receive the encrypted controller proof");
        };
        host_transcript.verify(&key, false, &proof).unwrap();
        send(&mut host, &WireMessage::Accepted {}).await.unwrap();
        assert!(matches!(
            receive(&mut controller).await.unwrap(),
            WireMessage::Accepted {}
        ));
    }

    #[tokio::test]
    async fn plaintext_management_frames_are_rejected_after_key_exchange() {
        let (mut controller, mut host, _) = encrypted_pair().await;
        // send_bytes is intentionally unsealed; this was the broken management
        // send path. Application JSON must instead pass through send_raw.
        controller
            .send_bytes(
                serde_json::to_vec(&WireMessage::Accepted {})
                    .unwrap()
                    .into(),
            )
            .await
            .unwrap();
        assert!(receive(&mut host).await.is_err());
    }

    #[tokio::test]
    async fn management_peer_close_and_handshake_timeout_are_distinct() {
        let (controller, mut host, _) = encrypted_pair().await;
        drop(controller);
        assert_eq!(
            receive(&mut host).await.unwrap_err().to_string(),
            "Management peer closed during handshake"
        );
        let (_controller, mut waiting_host, _) = encrypted_pair().await;
        assert_eq!(
            receive(&mut waiting_host).await.unwrap_err().to_string(),
            "Management handshake timed out"
        );
    }
}
