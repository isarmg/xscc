//! The existing desktop IPC Check is a point-in-time observation, not a lease
//! issued by the owner. Match the TCP polling model, fail closed on stalled IPC,
//! and keep the host's live policy check authoritative for exact policy expiry.
use super::{receive, send, transcript};
use crate::management::{rpc, Request};
use base::direct_transport::quic::{Authorization, ChannelBinding, PendingSession, Session};
use hbb_common::{anyhow::Context, bail, tokio, ResultType};
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, time::Instant};
use xscc_native_management::session::{nonce, SessionPermit, WireMessage};

const POLL: Duration = Duration::from_secs(1);
const MAX_OBSERVATION_AGE: Duration = Duration::from_secs(2);

struct ObservedPermit {
    binding: ChannelBinding,
    observation: watch::Receiver<Option<Instant>>,
}
impl ObservedPermit {
    fn check_at(&self, binding: &ChannelBinding, now: Instant) -> ResultType<()> {
        if binding != &self.binding
            || self.observation.has_changed().is_err()
            || !self
                .observation
                .borrow()
                .is_some_and(|deadline| now < deadline)
        {
            bail!("QUIC management observation expired or owner disconnected");
        }
        Ok(())
    }
}
impl Authorization for ObservedPermit {
    fn check(&self, binding: &ChannelBinding) -> ResultType<()> {
        self.check_at(binding, Instant::now())
    }
}

async fn observe(permit: &SessionPermit) -> ResultType<Instant> {
    // Count request latency against the bound; a delayed true must not renew it.
    let deadline = Instant::now() + MAX_OBSERVATION_AGE;
    let value = tokio::time::timeout_at(
        deadline,
        rpc(Request::Check {
            permit: Some(permit.clone()),
        }),
    )
    .await??;
    if value != serde_json::Value::Bool(true) || Instant::now() >= deadline {
        bail!("Management authorization expired");
    }
    Ok(deadline)
}

async fn authorization(
    binding: ChannelBinding,
    permit: SessionPermit,
) -> ResultType<Arc<ObservedPermit>> {
    let deadline = observe(&permit).await?;
    let (sender, observation) = watch::channel(Some(deadline));
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = sender.closed() => return,
                _ = tokio::time::sleep(POLL) => {}
            }
            let result = tokio::select! {
                _ = sender.closed() => return,
                result = observe(&permit) => result,
            };
            match result {
                Ok(deadline) => {
                    sender.send_replace(Some(deadline));
                }
                Err(_) => {
                    sender.send_replace(None);
                    return;
                }
            }
        }
    });
    // Dropping every session/stream authorization receiver ends its sole poller.
    // If that task exits or panics, has_changed immediately rejects its last value.
    Ok(Arc::new(ObservedPermit {
        binding,
        observation,
    }))
}

pub(crate) async fn connect(
    mut session: PendingSession,
    permit: SessionPermit,
) -> ResultType<(Session, SessionPermit)> {
    let result = async {
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
        let value = rpc(Request::Prove {
            transcript,
            host_proof: proof,
        })
        .await?;
        let proof = value
            .as_str()
            .context("Invalid managed proof response")?
            .to_owned();
        send(&mut session, &WireMessage::Proof { proof }).await?;
        if !matches!(receive(&mut session).await?, WireMessage::Accepted {}) {
            bail!("Managed host acceptance required");
        }
        authorization(session.binding().clone(), permit.clone()).await
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

    #[test]
    fn observation_expires_on_time_revocation_or_poller_exit() {
        let now = Instant::now();
        let binding = ChannelBinding {
            exporter: [1; 32],
            host_identity: [2; 32],
        };
        let (sender, observation) = watch::channel(Some(now + MAX_OBSERVATION_AGE));
        let authorization = ObservedPermit {
            binding: binding.clone(),
            observation,
        };
        assert!(authorization.check_at(&binding, now).is_ok());
        assert!(authorization
            .check_at(&binding, now + MAX_OBSERVATION_AGE)
            .is_err());
        let mut other = binding.clone();
        other.exporter[0] ^= 1;
        assert!(authorization.check_at(&other, now).is_err());
        sender.send_replace(None);
        assert!(authorization.check_at(&binding, now).is_err());
        sender.send_replace(Some(now + MAX_OBSERVATION_AGE));
        drop(sender);
        assert!(authorization.check_at(&binding, now).is_err());
    }
}
