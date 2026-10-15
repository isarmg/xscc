use std::convert::TryInto;

use hbb_common::{
    anyhow::anyhow,
    bytes::Bytes,
    protobuf::Message as _,
    rendezvous_proto::IdPk,
    sodiumoxide::crypto::{box_, secretbox, sign},
    ResultType,
};

pub mod client;
pub mod host;

pub fn decode_peer_identity(
    signed: &[u8],
    key: &sign::PublicKey,
) -> ResultType<(String, [u8; 32], u32)> {
    let identity = IdPk::parse_from_bytes(
        &sign::verify(signed, key).map_err(|_| anyhow!("Signature mismatch"))?,
    )?;
    let public_key: [u8; 32] = identity
        .pk
        .as_ref()
        .try_into()
        .map_err(|_| anyhow!("Invalid peer public key length"))?;
    Ok((identity.id, public_key, identity.kx_version))
}

pub fn create_symmetric_key_msg(their_pk_b: [u8; 32]) -> (Bytes, Bytes, secretbox::Key) {
    let their_pk_b = box_::PublicKey(their_pk_b);
    let (our_pk_b, out_sk_b) = box_::gen_keypair();
    let key = secretbox::gen_key();
    let nonce = box_::Nonce([0u8; box_::NONCEBYTES]);
    let sealed_key = box_::seal(&key.0, &nonce, &their_pk_b, &out_sk_b);
    (Vec::from(our_pk_b.0).into(), sealed_key.into(), key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_identity_signature_covers_ephemeral_key_and_version() {
        let (pk, sk) = sign::gen_keypair();
        let identity = IdPk {
            id: "host".to_owned(),
            pk: Bytes::from(vec![7u8; 32]),
            kx_version: hbb_common::tcp::KX_VERSION_LATEST,
            ..Default::default()
        };
        let signed = sign::sign(&identity.write_to_bytes().unwrap(), &sk);
        assert_eq!(
            decode_peer_identity(&signed, &pk).unwrap(),
            ("host".to_owned(), [7u8; 32], hbb_common::tcp::KX_VERSION_LATEST)
        );
        let (other_pk, _) = sign::gen_keypair();
        assert!(decode_peer_identity(&signed, &other_pk).is_err());
        let mut tampered = signed;
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert!(decode_peer_identity(&tampered, &pk).is_err());
        let invalid = IdPk {
            pk: Bytes::from(vec![7u8; 31]),
            ..identity
        };
        assert!(decode_peer_identity(
            &sign::sign(&invalid.write_to_bytes().unwrap(), &sk),
            &pk
        )
        .is_err());
    }
}
