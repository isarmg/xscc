//! Opt-in, in-process management authorization. No production transport selection
//! uses this adapter; desktop controllers with an IPC owner need a live lease.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) mod ipc;

use base::direct_transport::quic::{Authorization, ChannelBinding, PendingSession, Session};
use hbb_common::{bail, ResultType};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use xscc::protocol::InstanceRole;
use xscc_native_management::{
    manager::NativeManager,
    session::{nonce, SessionPermit, Transcript, WireMessage},
};

struct PermitAuthorization {
    manager: Arc<NativeManager>,
    permit: SessionPermit,
    role: InstanceRole,
    binding: ChannelBinding,
}
impl Authorization for PermitAuthorization {
    fn check(&self, channel: &ChannelBinding) -> ResultType<()> {
        if channel != &self.binding || !self.manager.permits(&self.permit, self.role) {
            bail!("QUIC management authorization expired or channel changed");
        }
        Ok(())
    }
}

fn transcript(
    permit: SessionPermit,
    binding: &ChannelBinding,
    client_nonce: &str,
    host_nonce: String,
) -> ResultType<Transcript> {
    xscc::protocol::validate_revision(client_nonce)?;
    let mut hash = Sha256::new();
    hash.update(b"xscc:quic:client-nonce:v1\0");
    hash.update(binding.exporter);
    hash.update(client_nonce.as_bytes());
    Ok(Transcript {
        permit,
        client_nonce: hex::encode(hash.finalize()),
        host_nonce,
        transport_host_key: hex::encode(binding.host_identity),
    })
}

async fn send(session: &mut PendingSession, message: &WireMessage) -> ResultType<()> {
    session.send_auth(&serde_json::to_vec(message)?).await
}
async fn receive(session: &mut PendingSession) -> ResultType<WireMessage> {
    let bytes = session.recv_auth().await?;
    if bytes.len() > 4096 {
        bail!("Management handshake is too large");
    }
    Ok(serde_json::from_slice(&bytes)?)
}

/// Only the existing in-process owner may authorize. An unconfigured manager
/// cannot fall back to unmanaged access here, even on non-host builds.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub(crate) async fn accept(
    mut session: PendingSession,
    manager: Arc<NativeManager>,
) -> ResultType<(Session, SessionPermit)> {
    let result = async {
        let WireMessage::Hello {
            permit,
            client_nonce,
        } = receive(&mut session).await?
        else {
            bail!("Managed controller identity required");
        };
        let key = manager.grant(&permit, InstanceRole::Controlled)?;
        let transcript = transcript(permit.clone(), session.binding(), &client_nonce, nonce()?)?;
        send(
            &mut session,
            &WireMessage::Challenge {
                host_nonce: transcript.host_nonce.clone(),
                proof: transcript.proof(&key, true)?,
            },
        )
        .await?;
        let WireMessage::Proof { proof } = receive(&mut session).await? else {
            bail!("Managed controller proof required");
        };
        transcript.verify(&key, false, &proof)?;
        let authorization = Arc::new(PermitAuthorization {
            manager,
            permit: permit.clone(),
            role: InstanceRole::Controlled,
            binding: session.binding().clone(),
        });
        authorization.check(session.binding())?;
        send(&mut session, &WireMessage::Accepted {}).await?;
        Ok((authorization, permit))
    }
    .await;
    match result {
        Ok((authorization, permit)) => Ok((session.authorize(authorization)?, permit)),
        Err(error) => {
            session.close();
            Err(error)
        }
    }
}

/// For mobile or another controller that owns its NativeManager in process.
/// Do not substitute a one-time IPC Check result for this live owner.
pub(crate) async fn connect(
    mut session: PendingSession,
    manager: Arc<NativeManager>,
    permit: SessionPermit,
) -> ResultType<(Session, SessionPermit)> {
    let result = async {
        let key = manager.grant(&permit, InstanceRole::Controller)?;
        let client_nonce = nonce()?;
        send(
            &mut session,
            &WireMessage::Hello {
                permit: permit.clone(),
                client_nonce: client_nonce.clone(),
            },
        )
        .await?;
        let WireMessage::Challenge { host_nonce, proof } = receive(&mut session).await? else {
            bail!("Managed host proof required");
        };
        let transcript = transcript(permit.clone(), session.binding(), &client_nonce, host_nonce)?;
        transcript.verify(&key, true, &proof)?;
        send(
            &mut session,
            &WireMessage::Proof {
                proof: transcript.proof(&key, false)?,
            },
        )
        .await?;
        if !matches!(receive(&mut session).await?, WireMessage::Accepted {}) {
            bail!("Managed host acceptance required");
        }
        Ok(Arc::new(PermitAuthorization {
            manager,
            permit: permit.clone(),
            role: InstanceRole::Controller,
            binding: session.binding().clone(),
        }))
    }
    .await;
    match result {
        Ok(authorization) => Ok((session.authorize(authorization)?, permit)),
        Err(error) => {
            session.close();
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xscc::protocol::{Binding, SessionKey};
    use xscc_native_management::session::epoch;

    struct UnavailableHost;
    #[async_trait::async_trait]
    impl xscc::host::RustDeskHost for UnavailableHost {
        async fn diagnostics(
            &self,
        ) -> Result<xscc::protocol::DiagnosticSnapshot, xscc::host::HostError> {
            Err(xscc::host::HostError)
        }
    }
    #[async_trait::async_trait]
    impl xscc_native_management::manager::NativeHost for UnavailableHost {
        async fn addresses(
            &self,
            _: &[xscc::protocol::HostAddress],
        ) -> Vec<xscc::protocol::HostAddress> {
            Vec::new()
        }
    }

    #[test]
    fn real_owner_without_current_policy_never_authorizes_quic() {
        let path = std::env::temp_dir().join(format!("xscc-quic-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let directory =
            xscc_native_management::private_fs::PrivateDirectory::create(&path, "managed").unwrap();
        let manager = NativeManager::open(directory, Arc::new(UnavailableHost)).unwrap();
        let manager_id = uuid::Uuid::new_v4();
        let binding = || Binding {
            manager_id,
            device_id: uuid::Uuid::new_v4(),
            installation_id: uuid::Uuid::new_v4(),
        };
        let channel = ChannelBinding {
            exporter: [1; 32],
            host_identity: [2; 32],
        };
        let authorization = PermitAuthorization {
            manager,
            permit: SessionPermit {
                controller: binding(),
                controlled: binding(),
                epoch: epoch(&SessionKey::from_bytes([7; 32])),
            },
            role: InstanceRole::Controller,
            binding: channel.clone(),
        };
        assert!(authorization.check(&channel).is_err());
        let mut wrong_channel = channel;
        wrong_channel.exporter[0] ^= 1;
        assert!(authorization.check(&wrong_channel).is_err());
        drop(authorization);
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn existing_transcript_proof_binds_quic_exporter_and_host_identity() {
        let key = SessionKey::from_bytes([7; 32]);
        let manager_id = uuid::Uuid::new_v4();
        let binding = || Binding {
            manager_id,
            device_id: uuid::Uuid::new_v4(),
            installation_id: uuid::Uuid::new_v4(),
        };
        let permit = SessionPermit {
            controller: binding(),
            controlled: binding(),
            epoch: epoch(&key),
        };
        let channel = ChannelBinding {
            exporter: [1; 32],
            host_identity: [2; 32],
        };
        let client_nonce = nonce().unwrap();
        let host_nonce = nonce().unwrap();
        let t = transcript(permit.clone(), &channel, &client_nonce, host_nonce.clone()).unwrap();
        assert_eq!(t.transport_host_key, hex::encode(channel.host_identity));
        let proof = t.proof(&key, true).unwrap();
        t.verify(&key, true, &proof).unwrap();
        let mut changed = channel.clone();
        changed.exporter[0] ^= 1;
        assert!(
            transcript(permit.clone(), &changed, &client_nonce, host_nonce.clone())
                .unwrap()
                .verify(&key, true, &proof)
                .is_err()
        );
        changed = channel;
        changed.host_identity[0] ^= 1;
        assert!(transcript(permit, &changed, &client_nonce, host_nonce)
            .unwrap()
            .verify(&key, true, &proof)
            .is_err());
        assert!(t.verify(&key, false, &proof).is_err());
    }
}
