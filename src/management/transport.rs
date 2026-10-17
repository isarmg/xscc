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
    stream.send_bytes(serde_json::to_vec(message)?.into()).await
}
async fn receive(stream: &mut Stream) -> ResultType<WireMessage> {
    let packet = stream
        .next_timeout(CONNECT_TIMEOUT)
        .await
        .context("Management handshake timed out")??;
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
    #[cfg(all(feature = "test-lan-direct", not(any(target_os = "android", target_os = "ios"))))]
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
