//! Temporary, explicitly enabled IP direct testing only. Remove after the testing phase.
mod address;

use hbb_common::{
    anyhow::Context,
    bail,
    config::{self, CONNECT_TIMEOUT},
    tcp::FramedStream,
    tokio, ResultType, Stream,
};
#[cfg(all(
    feature = "gpui-host",
    not(any(target_os = "android", target_os = "ios"))
))]
use hbb_common::{config::Config, log};
use std::net::SocketAddr;

pub(crate) use address::PREFIX;

pub(crate) fn is_peer(peer: &str) -> bool {
    peer.strip_prefix(PREFIX)
        .is_some_and(|value| address::connect_endpoint(value).is_ok())
}

pub(crate) fn target(input: &str) -> ResultType<SocketAddr> {
    address::connect_endpoint(input).map_err(hbb_common::anyhow::Error::msg)
}

pub(crate) async fn connect(peer: &str) -> ResultType<(Stream, Vec<u8>, &'static str)> {
    if config::is_incoming_only() {
        bail!("Incoming-only mode cannot initiate test LAN connections");
    }
    let endpoint = target(
        peer.strip_prefix(PREFIX)
            .context("Invalid test LAN target")?,
    )?;
    let local = config::Config::get_any_listen_addr(endpoint.is_ipv4());
    let socket = if endpoint.is_ipv4() {
        tokio::net::TcpSocket::new_v4()
    } else {
        tokio::net::TcpSocket::new_v6()
    }
    .context("TEST IP: TCP socket creation failed")?;
    // Match FramedStream::new's socket options while retaining each failure's cause.
    #[cfg(all(unix, not(target_os = "illumos")))]
    socket.set_reuseport(true).ok();
    socket.set_reuseaddr(true).ok();
    socket
        .bind(local)
        .context("TEST IP: TCP local bind failed")?;
    let socket = hbb_common::timeout(CONNECT_TIMEOUT, socket.connect(endpoint))
        .await
        .with_context(|| format!("TEST IP: TCP connection to {endpoint} timed out"))?
        .with_context(|| format!("TEST IP: TCP connection to {endpoint} failed"))?;
    socket.set_nodelay(true).ok();
    let local = socket
        .local_addr()
        .context("TEST IP: TCP local address unavailable")?;
    let socket = FramedStream::from(socket, local);
    let mut stream = Stream::Tcp(socket);
    stream.set_send_timeout(CONNECT_TIMEOUT);
    stream.set_max_packet_length(4096);
    // Deliberately no TOFU storage or managed-identity claim in this test-only path.
    let key = base::direct_transport::client::connect_handshake(&mut stream, &[])
        .await
        .context("TEST IP: encrypted handshake failed")?;
    Ok((stream, key, "TCP"))
}

#[cfg(all(
    feature = "gpui-host",
    not(any(target_os = "android", target_os = "ios"))
))]
pub(crate) fn configured_bind() -> ResultType<SocketAddr> {
    let input = std::env::var("XSCC_TEST_LAN_BIND")
        .context("Set XSCC_TEST_LAN_BIND to a listening IPv4 or IPv6 address before starting --test-lan-host")?;
    address::endpoint(&input).map_err(hbb_common::anyhow::Error::msg)
}

// This is a dedicated process entry point, called before the UI runtime starts.
#[cfg(all(
    feature = "gpui-host",
    not(any(target_os = "android", target_os = "ios"))
))]
#[tokio::main]
pub(crate) async fn run_host(address: SocketAddr) -> ResultType<()> {
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
    let address = listener.local_addr()?;
    crate::server::check_zombie();
    #[cfg(target_os = "linux")]
    eprintln!("Linux foreground LAN testing uses portal capture/input; audio is disabled to avoid sharing the installed host's audio IPC.");
    log::warn!("TEST ONLY: unauthenticated IP direct control listening on {address}; any reachable peer can control this desktop");
    eprintln!("TEST ONLY: listening on {address}. Any reachable peer can control this desktop. Stop this process to disable it.");
    let server = crate::server::new();
    crate::server::input_service::fix_key_down_timeout_loop();
    eprintln!("XSCC_TEST_DIRECT_READY={address}");
    loop {
        let (socket, peer) = listener.accept().await?;
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
                peer,
                meta,
            )
            .await
            {
                log::trace!("Test LAN connection closed: {error}");
            }
        });
    }
}

#[cfg(test)]
mod connection_tests {
    use super::{connect, PREFIX};
    use hbb_common::tokio;

    #[tokio::test]
    async fn refused_tcp_connection_preserves_stage_and_os_cause() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        drop(listener);
        let error = connect(&format!("{PREFIX}{endpoint}")).await.err().unwrap();
        assert!(format!("{error:#}").contains("TCP connection"));
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::ConnectionRefused
        );
    }

    #[tokio::test]
    async fn silent_peer_preserves_handshake_timeout_cause() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let error = connect(&format!("{PREFIX}{endpoint}")).await.err().unwrap();
        peer.abort();
        assert!(format!("{error:#}").contains("encrypted handshake failed"));
        assert!(error
            .downcast_ref::<tokio::time::error::Elapsed>()
            .is_some());
    }
}
