//! XSCC encrypted direct transport v1, followed by the shared instance proof.
use anyhow::{anyhow, bail, Context};
use one_kvm_management::{nonce, ManagedKvm, SessionPermit, Transcript, WireMessage};
use protobuf::Message;
use sodiumoxide::crypto::{box_, generichash, secretbox, sign};
use std::{sync::Arc, time::Duration};
use tokio::net::TcpStream;

use super::{
    bytes_codec::{read_frame_with_limit, write_frame},
    crypto::SigningKeyPair,
    protocol::{message, HbbMessage, IdPk, SignedId},
};

const VERSION: u32 = 1;
const PROOF_LIMIT: usize = 4096;

pub struct AuthorizedTransport {
    pub send_key: secretbox::Key,
    pub receive_key: secretbox::Key,
    pub permit: SessionPermit,
}

fn subkey(
    key: &secretbox::Key,
    direction: u8,
    initiator: &[u8],
    responder: &[u8],
) -> anyhow::Result<secretbox::Key> {
    let failed = |_| anyhow!("XSCC transport key derivation failed");
    let mut state =
        generichash::State::new(Some(secretbox::KEYBYTES), Some(&key.0)).map_err(failed)?;
    for value in [
        b"rdkx-spl".as_slice(),
        &[direction],
        &VERSION.to_le_bytes(),
        &VERSION.to_le_bytes(),
        initiator,
        responder,
    ] {
        state.update(value).map_err(failed)?;
    }
    secretbox::Key::from_slice(state.finalize().map_err(failed)?.as_ref())
        .context("XSCC transport subkey has the required size")
}

fn packet_nonce(sequence: u64) -> secretbox::Nonce {
    let mut nonce = secretbox::Nonce([0; secretbox::NONCEBYTES]);
    nonce.0[..8].copy_from_slice(&sequence.to_le_bytes());
    nonce
}

async fn receive(
    stream: &mut TcpStream,
    key: &secretbox::Key,
    sequence: u64,
) -> anyhow::Result<WireMessage> {
    let packet = read_frame_with_limit(stream, PROOF_LIMIT).await?;
    let bytes = secretbox::open(&packet, &packet_nonce(sequence), key)
        .map_err(|_| anyhow!("Encrypted XSCC management proof required"))?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn send(
    stream: &mut TcpStream,
    key: &secretbox::Key,
    sequence: u64,
    value: &WireMessage,
) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    write_frame(
        stream,
        &secretbox::seal(&bytes, &packet_nonce(sequence), key),
    )
    .await?;
    Ok(())
}

pub async fn accept(
    stream: &mut TcpStream,
    manager: &Arc<ManagedKvm>,
    signing: &SigningKeyPair,
    device_id: &str,
) -> anyhow::Result<AuthorizedTransport> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (public, secret) = box_::gen_keypair();
        let identity = IdPk {
            id: device_id.into(),
            pk: public.0.to_vec().into(),
            kx_version: VERSION,
            ..Default::default()
        };
        let mut message = HbbMessage::new();
        message.set_signed_id(SignedId {
            id: sign::sign(&identity.write_to_bytes()?, &signing.secret_key).into(),
            signing_key: signing.public_key.0.to_vec().into(),
            ..Default::default()
        });
        write_frame(stream, &message.write_to_bytes()?).await?;
        let packet = read_frame_with_limit(stream, PROOF_LIMIT).await?;
        let message = HbbMessage::parse_from_bytes(&packet)?;
        let Some(message::Union::PublicKey(exchange)) = message.union else {
            bail!("XSCC encrypted key exchange required");
        };
        if exchange.kx_version != VERSION {
            bail!("Current XSCC key exchange version required");
        }
        let their_key = box_::PublicKey::from_slice(&exchange.asymmetric_value)
            .context("Invalid XSCC ephemeral key")?;
        let exchanged = box_::open(
            &exchange.symmetric_value,
            &box_::Nonce([0; box_::NONCEBYTES]),
            &their_key,
            &secret,
        )
        .map_err(|_| anyhow!("Invalid XSCC sealed session key"))?;
        let exchanged =
            secretbox::Key::from_slice(&exchanged).context("Invalid XSCC session key size")?;
        let receive_key = subkey(&exchanged, 1, &their_key.0, &public.0)?;
        let send_key = subkey(&exchanged, 2, &their_key.0, &public.0)?;
        let WireMessage::Hello {
            permit,
            client_nonce,
        } = receive(stream, &receive_key, 1).await?
        else {
            bail!("XSCC controller identity required");
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
            stream,
            &send_key,
            1,
            &WireMessage::Challenge {
                host_nonce: transcript.host_nonce.clone(),
                proof: transcript.proof(&key, true)?,
            },
        )
        .await?;
        let WireMessage::Proof { proof } = receive(stream, &receive_key, 2).await? else {
            bail!("XSCC controller proof required");
        };
        transcript.verify(&key, false, &proof)?;
        if !manager.native_permits(&transcript.permit) {
            bail!("XSCC authorization expired");
        }
        send(stream, &send_key, 2, &WireMessage::Accepted {}).await?;
        Ok(AuthorizedTransport {
            send_key,
            receive_key,
            permit: transcript.permit,
        })
    })
    .await
    .context("XSCC encrypted handshake timed out")?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_keys_match_xscc_v1_transcript_and_separate_directions() {
        let key = secretbox::Key([7; 32]);
        let initiator = [1; 32];
        let responder = [2; 32];
        let send = subkey(&key, 2, &initiator, &responder).unwrap();
        let receive = subkey(&key, 1, &initiator, &responder).unwrap();
        assert_ne!(send.0, receive.0);
        let packet = secretbox::seal(b"authorized", &packet_nonce(1), &send);
        assert_eq!(
            secretbox::open(&packet, &packet_nonce(1), &send).unwrap(),
            b"authorized"
        );
        assert!(secretbox::open(&packet, &packet_nonce(1), &receive).is_err());
        assert_ne!(send.0, subkey(&key, 2, &responder, &initiator).unwrap().0);
    }
}
