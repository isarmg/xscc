//! One enrollment envelope shared by the server and every application backend.
use crate::{Backend, InstanceRole, InvalidTask};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use uuid::Uuid;

// Secret-bearing requests intentionally have no Debug implementation. Clients
// borrow their protected strings; HTTP extractors deserialize owned strings.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairingRequest<'a> {
    pub authorization_code: Cow<'a, str>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollmentRequest<'a> {
    pub device_id: Uuid,
    pub installation_id: Uuid,
    pub token: Cow<'a, str>,
    pub credential: Cow<'a, str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairingResponse {
    pub manager_id: Uuid,
    pub device_id: Uuid,
    pub role: InstanceRole,
    pub backend: Backend,
}

impl PairingResponse {
    pub fn validate(&self) -> Result<(), InvalidTask> {
        if self.manager_id.is_nil() || self.device_id.is_nil() {
            return Err(InvalidTask);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;

    fn owned<T: DeserializeOwned>() {}

    #[test]
    fn borrowed_requests_roundtrip_through_owned_http_extraction() {
        owned::<PairingRequest<'static>>();
        owned::<EnrollmentRequest<'static>>();
        let code = "a".repeat(36);
        let credential = "b".repeat(64);
        let input = EnrollmentRequest {
            device_id: Uuid::from_u128(1),
            installation_id: Uuid::from_u128(2),
            token: Cow::Borrowed(&code),
            credential: Cow::Borrowed(&credential),
        };
        let bytes = serde_json::to_vec(&input).unwrap();
        let output: EnrollmentRequest<'static> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(output.device_id, input.device_id);
        assert_eq!(output.token, code);
        assert_eq!(output.credential, credential);
    }

    #[test]
    fn pairing_requires_the_current_role_and_backend_contract() {
        let response = PairingResponse {
            manager_id: Uuid::from_u128(1),
            device_id: Uuid::from_u128(2),
            role: InstanceRole::Controlled,
            backend: Backend::OneKvm,
        };
        response.validate().unwrap();
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(
            serde_json::from_value::<PairingResponse>(value.clone()).unwrap(),
            response
        );
        for field in ["role", "backend"] {
            let mut incomplete = value.clone();
            incomplete.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<PairingResponse>(incomplete).is_err());
        }
        let mut invalid = response;
        invalid.manager_id = Uuid::nil();
        assert!(invalid.validate().is_err());
    }
}
