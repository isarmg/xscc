use crate::message_proto::{message, Message, SignedId};
use hbb_common::{
    anyhow::Context,
    bail,
    bytes::Bytes,
    config::CONNECT_TIMEOUT,
    protobuf::Message as _,
    rendezvous_proto::IdPk,
    sodiumoxide::crypto::{box_, sign},
    tcp, timeout, ResultType, Stream,
};

/// Authenticate the host identity and establish encryption before accepting login messages.
pub async fn accept_handshake(
    stream: &mut Stream,
    signing_secret: &[u8],
    signing_public: &[u8],
    device_id: &str,
) -> ResultType<()> {
    let sk = sign::SecretKey::from_slice(signing_secret)
        .context("Invalid host signing secret key")?;
    if signing_public.len() != sign::PUBLICKEYBYTES {
        bail!("Invalid host signing public key");
    }
    let (our_pk_b, our_sk_b) = box_::gen_keypair();
    let advertised = tcp::KX_VERSION_LATEST;
    let mut msg_out = Message::new();
    msg_out.set_signed_id(SignedId {
        id: sign::sign(
            &IdPk {
                id: device_id.to_owned(),
                pk: Bytes::from(our_pk_b.0.to_vec()),
                kx_version: advertised,
                ..Default::default()
            }
            .write_to_bytes()?,
            &sk,
        )
        .into(),
        signing_key: Bytes::copy_from_slice(signing_public),
        ..Default::default()
    });
    timeout(CONNECT_TIMEOUT, stream.send(&msg_out)).await??;
    let bytes = timeout(CONNECT_TIMEOUT, stream.next())
        .await?
        .context("Failed to receive public key")??;
    let msg_in = Message::parse_from_bytes(&bytes)?;
    let Some(message::Union::PublicKey(pk)) = msg_in.union else {
        bail!("Handshake failed: expected an encrypted key exchange");
    };
    if pk.asymmetric_value.len() != box_::PUBLICKEYBYTES {
        bail!("Handshake failed: invalid public key length from peer");
    }
    if pk.kx_version != advertised {
        bail!("Handshake failed: current key exchange version is required");
    }
    let key = tcp::Encrypt::decode(&pk.symmetric_value, &pk.asymmetric_value, &our_sk_b)?;
    stream.set_negotiated_key(
        key,
        false,
        &tcp::KxTranscript {
            initiator_pk: &pk.asymmetric_value,
            responder_pk: &our_pk_b.0,
            advertised,
            picked: pk.kx_version,
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        direct_transport::{create_symmetric_key_msg, decode_peer_identity},
        message_proto::{LoginRequest, PublicKey, TestDelay},
    };
    use hbb_common::tokio;

    async fn connected_pair() -> (Stream, Stream) {
        let listener = tcp::new_listener("127.0.0.1:0", false).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (client, accepted) = tokio::join!(
            tcp::FramedStream::new(addr, None, 3000),
            listener.accept()
        );
        let (accepted, _) = accepted.unwrap();
        (Stream::Tcp(client.unwrap()), Stream::from(accepted, addr))
    }

    async fn receive_identity(client: &mut Stream) -> SignedId {
        let bytes = client.next_timeout(3000).await.unwrap().unwrap();
        let msg = Message::parse_from_bytes(&bytes).unwrap();
        let Some(message::Union::SignedId(identity)) = msg.union else {
            panic!("host did not send its signed identity");
        };
        identity
    }

    #[tokio::test]
    async fn direct_handshake_encrypts_both_directions() {
        let (mut client, mut host) = connected_pair().await;
        let (host_public, host_secret) = sign::gen_keypair();
        let expected_public = host_public.0;
        let handshake = tokio::spawn(async move {
            accept_handshake(&mut host, &host_secret.0, &host_public.0, "host")
                .await
                .unwrap();
            assert!(host.is_secured());
            let mut message = Message::new();
            message.set_test_delay(TestDelay {
                time: 42,
                ..Default::default()
            });
            host.send(&message).await.unwrap();
            let echoed = host.next_timeout(3000).await.unwrap().unwrap();
            assert_eq!(Message::parse_from_bytes(&echoed).unwrap(), message);
        });
        let identity = crate::direct_transport::client::connect_handshake(
            &mut client,
            &expected_public,
        )
        .await
        .unwrap();
        assert_eq!(identity, expected_public);
        assert!(client.is_secured());
        let encrypted = client.next_timeout(3000).await.unwrap().unwrap();
        let message = Message::parse_from_bytes(&encrypted).unwrap();
        assert_eq!(message.test_delay().time, 42);
        client.send(&message).await.unwrap();
        timeout(3000, handshake).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn direct_handshake_rejects_plaintext_and_invalid_keys() {
        let mut empty_key = Message::new();
        empty_key.set_public_key(PublicKey::new());
        let mut malformed_key = Message::new();
        malformed_key.set_public_key(PublicKey {
            asymmetric_value: vec![1; box_::PUBLICKEYBYTES].into(),
            kx_version: tcp::KX_VERSION_LATEST,
            ..Default::default()
        });
        let mut legacy_key = malformed_key.clone();
        legacy_key.mut_public_key().kx_version = 0;
        let mut login = Message::new();
        login.set_login_request(LoginRequest::new());
        let mut responses: Vec<Vec<u8>> =
            [Message::new(), empty_key, malformed_key, legacy_key, login]
                .iter()
                .map(|message| message.write_to_bytes().unwrap())
                .collect();
        responses.push(vec![0xff]);
        for response in responses {
            let (mut client, mut host) = connected_pair().await;
            let (host_public, host_secret) = sign::gen_keypair();
            let handshake = tokio::spawn(async move {
                let result = accept_handshake(
                    &mut host, &host_secret.0, &host_public.0, "host",
                ).await;
                assert!(!host.is_secured());
                result
            });
            receive_identity(&mut client).await;
            client.send_bytes(response.into()).await.unwrap();
            assert!(timeout(3000, handshake).await.unwrap().unwrap().is_err());
            assert!(matches!(client.next_timeout(3000).await, None));
        }
    }

    #[tokio::test]
    async fn direct_handshake_rejects_a_replayed_session_key() {
        let (mut first_client, mut first_host) = connected_pair().await;
        let (host_public, host_secret) = sign::gen_keypair();
        let first_secret = host_secret.clone();
        let first_public = host_public.clone();
        let first_handshake = tokio::spawn(async move {
            accept_handshake(&mut first_host, &first_secret.0, &first_public.0, "host").await
        });
        let first_identity = receive_identity(&mut first_client).await;
        let signing_key = sign::PublicKey::from_slice(&first_identity.signing_key).unwrap();
        let (_, first_ephemeral, advertised) =
            decode_peer_identity(&first_identity.id, &signing_key).unwrap();
        let (asymmetric_value, symmetric_value, _) =
            create_symmetric_key_msg(first_ephemeral);
        let mut response = Message::new();
        response.set_public_key(PublicKey {
            asymmetric_value,
            symmetric_value,
            kx_version: advertised,
            ..Default::default()
        });
        first_client.send(&response).await.unwrap();
        timeout(3000, first_handshake).await.unwrap().unwrap().unwrap();

        let (mut second_client, mut second_host) = connected_pair().await;
        let second_handshake = tokio::spawn(async move {
            let result = accept_handshake(
                &mut second_host, &host_secret.0, &host_public.0, "host",
            ).await;
            assert!(!second_host.is_secured());
            result
        });
        let second_identity = receive_identity(&mut second_client).await;
        assert_eq!(second_identity.signing_key, first_identity.signing_key);
        let (_, second_ephemeral, _) =
            decode_peer_identity(&second_identity.id, &signing_key).unwrap();
        assert_ne!(first_ephemeral, second_ephemeral);
        second_client.send(&response).await.unwrap();
        assert!(timeout(3000, second_handshake).await.unwrap().unwrap().is_err());
        assert!(matches!(second_client.next_timeout(3000).await, None));
    }

}
