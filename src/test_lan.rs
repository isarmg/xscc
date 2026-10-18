//! Temporary, explicitly enabled LAN testing only. Remove after the testing phase.
mod address;
#[cfg(target_os = "ios")]
mod ios;

use hbb_common::{
    anyhow::Context,
    bail,
    config::{self, CONNECT_TIMEOUT},
    tcp::FramedStream,
    ResultType, Stream,
};
#[cfg(all(
    feature = "gpui-host",
    not(any(target_os = "android", target_os = "ios"))
))]
use hbb_common::{config::Config, log, tokio};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

pub(crate) use address::PREFIX;

pub(crate) fn is_peer(peer: &str) -> bool {
    peer.strip_prefix(PREFIX)
        .is_some_and(|value| address::endpoint(value).is_ok())
}

#[cfg(not(target_os = "ios"))]
fn local_network(local: Ipv4Addr, peer: Ipv4Addr) -> bool {
    default_net::get_interfaces().into_iter().any(|interface| {
        interface.ipv4.iter().any(|network| {
            network.addr == local && address::same_subnet(local, network.prefix_len, peer)
        })
    })
}

#[cfg(not(target_os = "ios"))]
pub(crate) fn target(input: &str) -> ResultType<SocketAddrV4> {
    let target = address::endpoint(input).map_err(hbb_common::anyhow::Error::msg)?;
    if !default_net::get_interfaces().into_iter().any(|interface| {
        interface
            .ipv4
            .iter()
            .any(|network| address::same_subnet(network.addr, network.prefix_len, *target.ip()))
    }) {
        bail!("Test LAN target must be on a directly connected private IPv4 subnet");
    }
    Ok(target)
}

#[cfg(target_os = "ios")]
fn local_network(local: Ipv4Addr, peer: Ipv4Addr) -> bool {
    ios::ipv4_networks().is_ok_and(|networks| {
        networks
            .into_iter()
            .any(|(ip, prefix)| ip == local && address::same_subnet(local, prefix, peer))
    })
}

#[cfg(target_os = "ios")]
pub(crate) fn target(input: &str) -> ResultType<SocketAddrV4> {
    let target = address::endpoint(input).map_err(hbb_common::anyhow::Error::msg)?;
    if !ios::ipv4_networks()?
        .into_iter()
        .any(|(ip, prefix)| address::same_subnet(ip, prefix, *target.ip()))
    {
        bail!("Test LAN target must be on a directly connected private IPv4 subnet");
    }
    Ok(target)
}

pub(crate) async fn connect(peer: &str) -> ResultType<(Stream, Vec<u8>, &'static str)> {
    if config::is_incoming_only() {
        bail!("Incoming-only mode cannot initiate test LAN connections");
    }
    let endpoint = target(
        peer.strip_prefix(PREFIX)
            .context("Invalid test LAN target")?,
    )?;
    let socket = FramedStream::new(SocketAddr::V4(endpoint), None, CONNECT_TIMEOUT).await?;
    let SocketAddr::V4(local) = socket.local_addr() else {
        bail!("Test LAN transport requires IPv4");
    };
    if !local_network(*local.ip(), *endpoint.ip()) {
        bail!("Test LAN route must use the directly connected private subnet");
    }
    let mut stream = Stream::Tcp(socket);
    stream.set_send_timeout(CONNECT_TIMEOUT);
    stream.set_max_packet_length(4096);
    // Deliberately no TOFU storage or managed-identity claim in this test-only path.
    let key = base::direct_transport::client::connect_handshake(&mut stream, &[]).await?;
    Ok((stream, key, "TCP"))
}

#[cfg(all(
    feature = "gpui-host",
    not(any(target_os = "android", target_os = "ios"))
))]
pub(crate) fn configured_bind() -> ResultType<SocketAddrV4> {
    let input = std::env::var("XSCC_TEST_LAN_BIND")
        .context("Set XSCC_TEST_LAN_BIND to this host's private IPv4 address before starting --test-lan-host")?;
    let address = address::endpoint(&input).map_err(hbb_common::anyhow::Error::msg)?;
    if !local_network(*address.ip(), *address.ip()) {
        bail!("XSCC_TEST_LAN_BIND must name a private IPv4 address on a local interface");
    }
    Ok(address)
}

// This is a dedicated process entry point, called before the UI runtime starts.
#[cfg(all(
    feature = "gpui-host",
    not(any(target_os = "android", target_os = "ios"))
))]
#[tokio::main]
pub(crate) async fn run_host(address: SocketAddrV4) -> ResultType<()> {
    if config::is_outgoing_only()
        || config::is_disable_tcp_listen()
        || config::option2bool("stop-service", &Config::get_option("stop-service"))
    {
        bail!("Host listening is disabled");
    }
    if cfg!(all(target_os = "linux", feature = "drm")) {
        bail!("Foreground LAN testing requires a build without the drm feature; run it from a logged-in Wayland desktop using portal capture");
    }
    #[cfg(feature = "hwcodec")]
    tokio::task::spawn_blocking(crate::client::get_hwcodec_config).await?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    crate::server::check_zombie();
    #[cfg(target_os = "linux")]
    eprintln!("Linux foreground LAN testing uses portal capture/input; audio is disabled to avoid sharing the installed host's audio IPC.");
    log::warn!("TEST ONLY: unauthenticated LAN control listening on {address}; any same-subnet peer can control this desktop");
    eprintln!("TEST ONLY: listening on {address}. Any same-subnet peer can control this desktop. Stop this process to disable it.");
    let server = crate::server::new();
    crate::server::input_service::fix_key_down_timeout_loop();
    loop {
        let (socket, peer) = listener.accept().await?;
        let SocketAddr::V4(peer) = peer else {
            continue;
        };
        if !local_network(*address.ip(), *peer.ip()) {
            continue;
        }
        socket.set_nodelay(true)?;
        let local = socket.local_addr()?;
        let server = server.clone();
        tokio::spawn(async move {
            let meta = crate::server::ConnectionMeta {
                test_lan: true,
                ..Default::default()
            };
            if let Err(error) = crate::server::create_tcp_connection(
                server,
                Stream::from(socket, local),
                SocketAddr::V4(peer),
                meta,
            )
            .await
            {
                log::trace!("Test LAN connection closed: {error}");
            }
        });
    }
}
