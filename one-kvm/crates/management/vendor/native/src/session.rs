use hmac::{Hmac, KeyInit, Mac};
use rand::{TryRng, rngs::SysRng};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use xscc::protocol::{Binding, SessionKey, validate_revision};

#[derive(Debug, Clone, thiserror::Error)]
#[error("managed session authorization failed")]
pub struct SessionError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionPermit {
    pub controller: Binding,
    pub controlled: Binding,
    pub epoch: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transcript {
    pub permit: SessionPermit,
    pub client_nonce: String,
    pub host_nonce: String,
    pub transport_host_key: String,
}
impl Transcript {
    fn validate(&self) -> Result<(), SessionError> {
        let c = &self.permit.controller;
        let h = &self.permit.controlled;
        if c.manager_id != h.manager_id
            || c.device_id == h.device_id
            || c.installation_id == h.installation_id
            || [
                c.manager_id,
                c.device_id,
                c.installation_id,
                h.device_id,
                h.installation_id,
            ]
            .iter()
            .any(uuid::Uuid::is_nil)
        {
            return Err(SessionError);
        }
        for value in [
            &self.permit.epoch,
            &self.client_nonce,
            &self.host_nonce,
            &self.transport_host_key,
        ] {
            validate_revision(value).map_err(|_| SessionError)?;
        }
        Ok(())
    }
    fn mac(&self, key: &SessionKey, role: &[u8]) -> Result<Hmac<Sha256>, SessionError> {
        self.validate()?;
        if epoch(key) != self.permit.epoch {
            return Err(SessionError);
        }
        let bytes = key.bytes().map_err(|_| SessionError)?;
        let mut mac =
            <Hmac<Sha256> as KeyInit>::new_from_slice(&bytes).map_err(|_| SessionError)?;
        mac.update(b"xscc:managed-direct-session:v1\0");
        mac.update(role);
        mac.update(&serde_json::to_vec(self).map_err(|_| SessionError)?);
        Ok(mac)
    }
    pub fn proof(&self, key: &SessionKey, host: bool) -> Result<String, SessionError> {
        Ok(hex::encode(
            self.mac(key, if host { b"host\0" } else { b"controller\0" })?
                .finalize()
                .into_bytes(),
        ))
    }
    pub fn verify(&self, key: &SessionKey, host: bool, proof: &str) -> Result<(), SessionError> {
        validate_revision(proof).map_err(|_| SessionError)?;
        let bytes = hex::decode(proof).map_err(|_| SessionError)?;
        self.mac(key, if host { b"host\0" } else { b"controller\0" })?
            .verify_slice(&bytes)
            .map_err(|_| SessionError)
    }
}
pub fn epoch(key: &SessionKey) -> String {
    hex::encode(Sha256::digest(key.as_hex().as_bytes()))
}
pub fn nonce() -> Result<String, SessionError> {
    let mut bytes = [0; 32];
    SysRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| SessionError)?;
    Ok(hex::encode(bytes))
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireMessage {
    Hello {
        permit: SessionPermit,
        client_nonce: String,
    },
    Challenge {
        host_nonce: String,
        proof: String,
    },
    Proof {
        proof: String,
    },
    Accepted {},
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proofs_bind_both_installations_channel_nonces_and_direction() {
        let key = SessionKey::from_bytes([7; 32]);
        let manager = uuid::Uuid::new_v4();
        let binding = || Binding {
            manager_id: manager,
            device_id: uuid::Uuid::new_v4(),
            installation_id: uuid::Uuid::new_v4(),
        };
        let t = Transcript {
            permit: SessionPermit {
                controller: binding(),
                controlled: binding(),
                epoch: epoch(&key),
            },
            client_nonce: nonce().unwrap(),
            host_nonce: nonce().unwrap(),
            transport_host_key: "a".repeat(64),
        };
        let proof = t.proof(&key, true).unwrap();
        t.verify(&key, true, &proof).unwrap();
        assert!(t.verify(&key, false, &proof).is_err());
        let mut changed = t.clone();
        changed.host_nonce = nonce().unwrap();
        assert!(changed.verify(&key, true, &proof).is_err());
        changed = t.clone();
        changed.transport_host_key = "b".repeat(64);
        assert!(changed.verify(&key, true, &proof).is_err());
        changed = t;
        changed.permit.controlled.installation_id = uuid::Uuid::new_v4();
        assert!(changed.verify(&key, true, &proof).is_err());
    }
}
