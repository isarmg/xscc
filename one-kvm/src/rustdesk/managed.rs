//! Shared encrypted TCP handshake followed by current instance authorization.
use super::crypto::SigningKeyPair;
use anyhow::{bail, Context};
use hbb_common::{bytes::Bytes, Stream};
use one_kvm_management::{nonce, ManagedKvm, SessionPermit, Transcript, WireMessage};
use std::{sync::Arc, time::Duration};

const PROOF_LIMIT: usize = 4096;

async fn receive(stream: &mut Stream) -> anyhow::Result<WireMessage> {
    let packet = stream
        .next()
        .await
        .context("management peer closed during handshake")??;
    Ok(serde_json::from_slice(&packet)?)
}
async fn send(stream: &mut Stream, value: &WireMessage) -> anyhow::Result<()> {
    stream
        .send_bytes(Bytes::from(serde_json::to_vec(value)?))
        .await
}

pub async fn accept(
    mut stream: Stream,
    manager: &Arc<ManagedKvm>,
    signing: &SigningKeyPair,
    device_id: &str,
) -> anyhow::Result<(Stream, SessionPermit)> {
    tokio::time::timeout(Duration::from_secs(10), async {
        stream.set_max_packet_length(PROOF_LIMIT);
        base::direct_transport::host::accept_handshake(
            &mut stream,
            &signing.secret_key.0,
            &signing.public_key.0,
            device_id,
        )
        .await?;
        let WireMessage::Hello {
            permit,
            client_nonce,
        } = receive(&mut stream).await?
        else {
            bail!("controller identity required");
        };
        let key = manager.native_grant(&permit)?;
        let transcript = Transcript {
            permit,
            client_nonce,
            host_nonce: nonce()?,
            transport_host_key: signing
                .public_key_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        };
        send(
            &mut stream,
            &WireMessage::Challenge {
                host_nonce: transcript.host_nonce.clone(),
                proof: transcript.proof(&key, true)?,
            },
        )
        .await?;
        let WireMessage::Proof { proof } = receive(&mut stream).await? else {
            bail!("controller proof required");
        };
        transcript.verify(&key, false, &proof)?;
        if !manager.native_permits(&transcript.permit) {
            bail!("authorization expired");
        }
        send(&mut stream, &WireMessage::Accepted {}).await?;
        Ok((stream, transcript.permit))
    })
    .await
    .context("encrypted handshake timed out")?
}
