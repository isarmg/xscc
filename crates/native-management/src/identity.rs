//! Immutable product identity, chosen by the embedding application.
use crate::enrollment::EnrollmentError;
use xscc::protocol::{Backend, InstanceRole, PairingResponse};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplicationIdentity {
    pub backend: Backend,
    pub role: InstanceRole,
}

impl ApplicationIdentity {
    pub fn verify(self, pairing: &PairingResponse) -> Result<(), EnrollmentError> {
        pairing.validate().map_err(|_| EnrollmentError::Protocol)?;
        if (pairing.backend, pairing.role) != (self.backend, self.role) {
            return Err(EnrollmentError::InstanceMismatch);
        }
        Ok(())
    }
}
