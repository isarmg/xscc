//! Explicitly configured experimental listener. TLS and the manager permit do
//! not grant desktop login; the existing Connection owns that decision.
use base::direct_transport::quic::{
    server_config, CertificateDer, Incoming, Listener, PrivatePkcs8KeyDer,
};
use hbb_common::{
    anyhow::Context,
    bail,
    config::{self, Config},
    log,
    sodiumoxide::crypto::sign,
    tokio::{
        self,
        time::{timeout, Instant},
    },
    ResultType,
};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use crate::{
    server::{
        admit_unauthorized, Connection, ConnectionMeta, ServerPtr, UnauthorizedID,
        MAX_UNAUTHORIZED_MESSAGE,
    },
    session_transport::SessionTransport,
};

const HANDSHAKE_LIMIT: Duration = Duration::from_secs(10);

struct Settings {
    address: SocketAddr,
    certificate: PathBuf,
    key: PathBuf,
}
impl Settings {
    fn parse(
        listen: Option<String>,
        certificate: Option<String>,
        key: Option<String>,
    ) -> ResultType<Option<Self>> {
        if listen.is_none() && certificate.is_none() && key.is_none() {
            return Ok(None);
        }
        let address: SocketAddr = listen
            .context("XSCC_EXPERIMENTAL_QUIC_LISTEN is required")?
            .parse()
            .context("QUIC listen address must be an IP address and port")?;
        if address.port() == 0 {
            bail!("QUIC listen port must be nonzero");
        }
        let certificate = certificate
            .filter(|value| !value.is_empty())
            .context("XSCC_EXPERIMENTAL_QUIC_CERT_DER is required")?
            .into();
        let key = key
            .filter(|value| !value.is_empty())
            .context("XSCC_EXPERIMENTAL_QUIC_KEY_DER is required")?
            .into();
        Ok(Some(Self {
            address,
            certificate,
            key,
        }))
    }
    fn environment() -> ResultType<Option<Self>> {
        fn value(name: &str) -> ResultType<Option<String>> {
            match std::env::var(name) {
                Ok(value) => Ok(Some(value)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(error) => Err(error.into()),
            }
        }
        Self::parse(
            value("XSCC_EXPERIMENTAL_QUIC_LISTEN")?,
            value("XSCC_EXPERIMENTAL_QUIC_CERT_DER")?,
            value("XSCC_EXPERIMENTAL_QUIC_KEY_DER")?,
        )
    }
}

fn disabled() -> bool {
    config::is_outgoing_only()
        || config::is_disable_tcp_listen()
        || Config::get_bool_option("stop-service")
        || crate::platform::installing_service()
}

pub(crate) async fn run(server: ServerPtr) {
    if let Err(error) = listen(server).await {
        log::error!("Experimental QUIC listener stopped: {}", error);
    }
}

async fn listen(server: ServerPtr) -> ResultType<()> {
    let Some(settings) = Settings::environment()? else {
        return Ok(());
    };
    let certificate = tokio::fs::read(&settings.certificate)
        .await
        .context("Read QUIC DER certificate")?;
    let key = tokio::fs::read(&settings.key)
        .await
        .context("Read QUIC PKCS8 DER private key")?;
    if certificate.is_empty() || key.is_empty() {
        bail!("QUIC certificate and private key must not be empty");
    }
    server_config(
        vec![CertificateDer::from(certificate.clone())],
        PrivatePkcs8KeyDer::from(key.clone()).into(),
    )
    .context("Invalid QUIC certificate or private key")?;
    let (secret, _) =
        Config::get_existing_key_pair().context("QUIC requires an existing host identity")?;
    let identity =
        sign::SecretKey::from_slice(&secret).context("Invalid existing host identity")?;
    let mut listener: Option<Listener> = None;
    loop {
        if disabled() {
            if let Some(bound) = listener.take() {
                bound.close();
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }
        if listener.is_none() {
            match Listener::bind(
                settings.address,
                vec![CertificateDer::from(certificate.clone())],
                PrivatePkcs8KeyDer::from(key.clone()).into(),
                identity.clone(),
            ) {
                Ok(bound) => listener = Some(bound),
                Err(error) => {
                    hbb_common::throttled_log!(
                        Duration::from_secs(60),
                        error,
                        "Cannot bind experimental QUIC listener on {}: {}",
                        settings.address,
                        error
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            }
            log::info!("Experimental QUIC listening on {}", settings.address);
        }
        let Some(bound) = listener.as_ref() else {
            continue;
        };
        let incoming = match timeout(Duration::from_secs(1), bound.incoming()).await {
            Ok(Some(incoming)) => incoming,
            Ok(None) => bail!("QUIC listener closed"),
            Err(_) => continue,
        };
        let accepted_at = Instant::now();
        let addr = hbb_common::try_into_v4(incoming.remote_address());
        let id = server.write().unwrap().get_new_id();
        let Some(unauthorized) = admit_unauthorized(id, addr.ip()) else {
            incoming.refuse();
            continue;
        };
        let server = server.clone();
        tokio::spawn(async move {
            if let Err(error) = serve(server, incoming, id, unauthorized, accepted_at).await {
                hbb_common::throttled_log!(
                    Duration::from_secs(60),
                    warn,
                    "Experimental QUIC connection failed: {}",
                    error
                );
            }
        });
    }
}

async fn serve(
    server: ServerPtr,
    incoming: Incoming,
    id: i32,
    unauthorized: UnauthorizedID,
    accepted_at: Instant,
) -> ResultType<()> {
    let addr = hbb_common::try_into_v4(incoming.remote_address());
    let (mut transport, permit) = tokio::select! {
        result = tokio::time::timeout_at(accepted_at + HANDSHAKE_LIMIT, async {
            let pending = incoming.handshake().await?;
            let (session, permit) = crate::management::quic::accept_owned(pending).await?;
            Ok::<_, hbb_common::anyhow::Error>((SessionTransport::from_quic(session).await?, permit))
        }) => result.context("QUIC handshake deadline exceeded")??,
        _ = unauthorized.evicted() => bail!("Unauthenticated QUIC connection evicted"),
    };
    if disabled() {
        bail!("Host service is disabled");
    }
    transport.set_max_packet_length(MAX_UNAUTHORIZED_MESSAGE);
    let meta = ConnectionMeta {
        managed_permit: Some(permit),
        quic_accepted_at: Some(accepted_at),
        ..Default::default()
    };
    Connection::start(
        addr,
        transport,
        id,
        Arc::downgrade(&server),
        meta,
        unauthorized,
    )
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_requires_explicit_complete_configuration() {
        assert!(Settings::parse(None, None, None).unwrap().is_none());
        assert!(Settings::parse(Some("127.0.0.1:21119".into()), None, None).is_err());
        assert!(Settings::parse(None, Some("host.der".into()), Some("key.der".into())).is_err());
        assert!(Settings::parse(
            Some("127.0.0.1:0".into()),
            Some("host.der".into()),
            Some("key.der".into())
        )
        .is_err());
        let settings = Settings::parse(
            Some("127.0.0.1:21119".into()),
            Some("host.der".into()),
            Some("key.der".into()),
        )
        .unwrap()
        .unwrap();
        assert_eq!(settings.address, "127.0.0.1:21119".parse().unwrap());
    }
}
