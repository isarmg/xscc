//! Explicit experimental selection; a selected QUIC session never falls back to TCP.
use super::{direct_identity_path, Interface};
use crate::{management, session_transport::SessionTransport};
use base::direct_transport::{
    client::{load_direct_identity, normalize_direct_endpoint},
    quic::{CertificateDer, Client, RootCertStore},
};
use hbb_common::{
    anyhow::Context,
    bail,
    config::{self, CONNECT_TIMEOUT},
    rendezvous_proto::ConnType,
    tokio, ResultType,
};
use std::{env, net::SocketAddr, path::PathBuf, time::Duration};
use xscc::protocol::HostAddress;
use xscc_native_management::{manager::Target, session::SessionPermit};

const PREFIX: &str = "XSCC_EXPERIMENTAL_QUIC_";

struct Settings {
    endpoint: String,
    server_name: String,
    root_cert: PathBuf,
    host_key: [u8; 32],
}

impl Settings {
    fn read(
        peer: &str,
        mut value: impl FnMut(&str) -> ResultType<Option<String>>,
    ) -> ResultType<Option<Self>> {
        if value("PEER")?.as_deref() != Some(peer) {
            return Ok(None);
        }
        let mut required = |key: &str| -> ResultType<String> {
            value(key)?
                .filter(|value| !value.trim().is_empty())
                .with_context(|| format!("{PREFIX}{key} is required for the selected QUIC peer"))
        };
        let endpoint = required("ENDPOINT")?;
        // Unlike the legacy direct input, this opt-in must specify a port.
        let (_, port) = endpoint
            .rsplit_once(':')
            .context("QUIC endpoint requires an explicit port")?;
        if endpoint.parse::<std::net::IpAddr>().is_ok()
            || port.parse::<u16>().ok().filter(|port| *port != 0).is_none()
        {
            bail!("QUIC endpoint requires an explicit nonzero port");
        }
        let endpoint = normalize_direct_endpoint(&endpoint)?;
        let server_name = required("SERVER_NAME")?;
        let root_cert = required("ROOT_CERT_DER")?.into();
        let key =
            hex::decode(required("HOST_KEY")?).context("QUIC host identity must be hexadecimal")?;
        let host_key = key.try_into().map_err(|_| {
            hbb_common::anyhow::anyhow!("QUIC host identity must contain exactly 32 bytes")
        })?;
        Ok(Some(Self {
            endpoint,
            server_name,
            root_cert,
            host_key,
        }))
    }
}

fn authorized_endpoint(endpoint: &str, addresses: &[HostAddress]) -> bool {
    addresses.iter().any(|address| {
        let candidate = if address.host.contains(':') {
            format!("[{}]:{}", address.host, address.port)
        } else {
            format!("{}:{}", address.host, address.port)
        };
        normalize_direct_endpoint(&candidate).is_ok_and(|candidate| candidate == endpoint)
    })
}

pub(crate) async fn connect(
    peer: &str,
    conn_type: ConnType,
    interface: impl Interface,
) -> ResultType<(
    SessionTransport,
    Vec<u8>,
    &'static str,
    Option<SessionPermit>,
)> {
    let settings = Settings::read(peer, |key| match env::var(format!("{PREFIX}{key}")) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error.into()),
    })?;
    let Some(settings) = settings else {
        let (stream, identity, kind, permit) =
            management::connect(peer, conn_type, interface).await?;
        return Ok((stream.into(), identity, kind, permit));
    };
    if conn_type != ConnType::DEFAULT_CONN || config::is_incoming_only() {
        bail!("Unsupported managed QUIC session type");
    }
    // Always ask the authenticated manager, including when test-lan-direct is enabled.
    let device_id = peer
        .strip_prefix("managed:")
        .context("QUIC requires a host selected from the managed host list")?
        .parse()?;
    let target: Target =
        serde_json::from_value(management::rpc(management::Request::Target { device_id }).await?)?;
    if !authorized_endpoint(&settings.endpoint, &target.addresses) {
        bail!("QUIC endpoint is not in the current authorized host address list");
    }
    if let Some(saved) = load_direct_identity(&direct_identity_path(&settings.endpoint))? {
        if saved.as_slice() != settings.host_key {
            bail!("QUIC host identity differs from the saved device identity");
        }
    }
    interface.update_direct(None);
    interface.update_received(false);
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(
        std::fs::read(&settings.root_cert)
            .context("Could not read the explicitly configured QUIC DER root certificate")?,
    ))?;
    let address = tokio::time::timeout(
        Duration::from_millis(CONNECT_TIMEOUT),
        tokio::net::lookup_host(settings.endpoint.as_str()),
    )
    .await
    .context("QUIC endpoint resolution timed out")??
    .next()
    .context("QUIC endpoint resolved to no address")?;
    let bind = SocketAddr::new(
        if address.is_ipv4() {
            std::net::Ipv4Addr::UNSPECIFIED.into()
        } else {
            std::net::Ipv6Addr::UNSPECIFIED.into()
        },
        0,
    );
    let client = Client::bind(bind, roots)?;
    let pending = client
        .connect(address, &settings.server_name, settings.host_key)
        .await?;
    let (session, permit) = tokio::time::timeout(
        Duration::from_millis(CONNECT_TIMEOUT),
        management::quic::connect_controller(pending, target.permit),
    )
    .await
    .context("QUIC management authorization timed out")??;
    let mut transport = SessionTransport::from_quic(session).await?;
    transport.set_send_timeout(CONNECT_TIMEOUT);
    interface.update_direct(Some(true));
    Ok((transport, settings.host_key.to_vec(), "QUIC", Some(permit)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_is_per_peer_and_selected_missing_config_fails_closed() {
        assert!(Settings::read("managed:one", |_| Ok(None))
            .unwrap()
            .is_none());
        assert!(
            Settings::read("managed:one", |_| Ok(Some("managed:other".into())))
                .unwrap()
                .is_none()
        );
        assert!(Settings::read("managed:one", |key| Ok(
            (key == "PEER").then(|| "managed:one".into())
        ))
        .is_err());
    }

    #[test]
    fn endpoint_must_match_authorized_host_and_port() {
        let addresses = vec![HostAddress {
            host: "host.example".into(),
            port: 21118,
        }];
        assert!(authorized_endpoint("host.example:21118", &addresses));
        assert!(!authorized_endpoint("host.example:21119", &addresses));
        assert!(!authorized_endpoint("other.example:21118", &addresses));
        assert!(!authorized_endpoint("127.0.0.1:21118", &addresses));
    }

    #[test]
    fn selected_settings_require_explicit_port_and_full_identity() {
        let read = |endpoint: &str, host_key: &str| {
            Settings::read("managed:one", |key| {
                Ok(Some(
                    match key {
                        "PEER" => "managed:one",
                        "ENDPOINT" => endpoint,
                        "SERVER_NAME" => "host.example",
                        "ROOT_CERT_DER" => "/root.der",
                        "HOST_KEY" => host_key,
                        _ => unreachable!(),
                    }
                    .into(),
                ))
            })
        };
        assert!(read("host.example", &"11".repeat(32)).is_err());
        assert!(read("2001:db8::1", &"11".repeat(32)).is_err());
        assert!(read("[2001:db8::1]:21118", &"11".repeat(32))
            .unwrap()
            .is_some());
        assert!(read("host.example:21118", "11").is_err());
        assert!(read("host.example:21118", &"11".repeat(32))
            .unwrap()
            .is_some());
    }
}
