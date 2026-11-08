//! UDP listeners use one dual-stack socket where IPv6 is available.
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
};
use tokio::net::UdpSocket;

/// Bind every interface. An IPv4 local address reports the IPv6-unavailable fallback.
/// Port conflicts and other operational failures are returned unchanged.
pub fn bind_any(port: u16) -> io::Result<UdpSocket> {
    match bind(port, true) {
        Ok(socket) => Ok(socket),
        Err(error) if ipv6_unavailable(&error) => bind(port, false),
        Err(error) => Err(error),
    }
}

/// Bind an ephemeral socket in the connected TCP peer's actual address family.
pub fn bind_for_peer(peer: SocketAddr) -> io::Result<UdpSocket> {
    bind(0, peer.is_ipv6())
}

fn bind(port: u16, ipv6: bool) -> io::Result<UdpSocket> {
    let socket = Socket::new(
        if ipv6 { Domain::ipv6() } else { Domain::ipv4() },
        Type::dgram(),
        Some(Protocol::udp()),
    )?;
    if ipv6 {
        socket.set_only_v6(false)?;
    }
    let address = if ipv6 {
        SocketAddr::from((Ipv6Addr::UNSPECIFIED, port))
    } else {
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))
    };
    socket.bind(&address.into())?;
    socket.set_nonblocking(true)?;
    let requested = 4 * 1024 * 1024;
    let receive_set = socket.set_recv_buffer_size(requested);
    let send_set = socket.set_send_buffer_size(requested);
    log::info!(
        "UDP socket buffers: requested={} OS-reported receive={:?} send={:?}; set receive={:?} send={:?}",
        requested, socket.recv_buffer_size(), socket.send_buffer_size(), receive_set, send_set,
    );
    UdpSocket::from_std(socket.into_udp_socket())
}

fn ipv6_unavailable(error: &io::Error) -> bool {
    if matches!(
        error.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::AddrNotAvailable
    ) {
        return true;
    }
    #[cfg(unix)]
    {
        matches!(
            error.raw_os_error(),
            Some(libc::EAFNOSUPPORT | libc::EPROTONOSUPPORT)
        )
    }
    #[cfg(windows)]
    // WSAEPROTONOSUPPORT and WSAEAFNOSUPPORT.
    {
        matches!(error.raw_os_error(), Some(10043 | 10047))
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_buffers(socket: UdpSocket, family: &str) {
        let socket = Socket::from(socket.into_std().unwrap());
        let receive = socket.recv_buffer_size().unwrap();
        let send = socket.send_buffer_size().unwrap();
        assert!(receive > 0 && send > 0);
        eprintln!("UDP_BUFFER_ACTUAL family={family} receive={receive} send={send}");
    }

    #[tokio::test]
    async fn udp_listener_receives_both_supported_address_families_and_keeps_port_conflicts() {
        let host = bind_any(0).unwrap();
        let address = host.local_addr().unwrap();
        let client =
            bind_for_peer(SocketAddr::from((Ipv4Addr::LOCALHOST, address.port()))).unwrap();
        assert!(client.local_addr().unwrap().is_ipv4());
        client
            .send_to(b"v4", (Ipv4Addr::LOCALHOST, address.port()))
            .await
            .unwrap();
        let mut buffer = [0; 16];
        let (length, _) = crate::timeout(2000, host.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buffer[..length], b"v4");
        if address.is_ipv6() {
            let client =
                bind_for_peer(SocketAddr::from((Ipv6Addr::LOCALHOST, address.port()))).unwrap();
            assert!(client.local_addr().unwrap().is_ipv6());
            client
                .send_to(b"v6", (Ipv6Addr::LOCALHOST, address.port()))
                .await
                .unwrap();
            let (length, _) = crate::timeout(2000, host.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&buffer[..length], b"v6");
            check_buffers(client, "IPv6");
        }
        check_buffers(client, "IPv4");
        assert_eq!(
            bind_any(address.port()).unwrap_err().kind(),
            io::ErrorKind::AddrInUse
        );
    }
}
