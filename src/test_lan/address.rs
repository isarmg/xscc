use std::net::{Ipv4Addr, SocketAddrV4};

pub const DEFAULT_PORT: u16 = 21119;
pub const PREFIX: &str = "test-lan:";

pub fn endpoint(input: &str) -> Result<SocketAddrV4, &'static str> {
    let input = input.trim();
    let address = input
        .parse::<SocketAddrV4>()
        .or_else(|_| {
            input
                .parse::<Ipv4Addr>()
                .map(|ip| SocketAddrV4::new(ip, DEFAULT_PORT))
        })
        .map_err(|_| "Enter a private IPv4 address, optionally followed by a port")?;
    if !address.ip().is_private() || address.port() == 0 {
        return Err("Test LAN connections require a private IPv4 address and a nonzero port");
    }
    Ok(address)
}

pub fn same_subnet(local: Ipv4Addr, prefix: u8, peer: Ipv4Addr) -> bool {
    if !local.is_private() || !peer.is_private() || !(1..=30).contains(&prefix) {
        return false;
    }
    let mask = u32::MAX << (32 - prefix);
    let network = u32::from(local) & mask;
    let host = u32::from(peer);
    host & mask == network && host != network && host != (network | !mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_private_ipv4_and_valid_ports() {
        assert_eq!(endpoint("192.168.1.2").unwrap().port(), DEFAULT_PORT);
        assert_eq!(endpoint("10.0.0.2:3456").unwrap().port(), 3456);
        for value in [
            "example.com",
            "8.8.8.8",
            "127.0.0.1",
            "0.0.0.0",
            "::1",
            "192.168.1.2:0",
            "192.168.1.2:65536",
            "100.64.0.1",
        ] {
            assert!(endpoint(value).is_err(), "{value}");
        }
    }

    #[test]
    fn subnet_excludes_network_broadcast_and_other_networks() {
        let local = Ipv4Addr::new(192, 168, 1, 2);
        assert!(same_subnet(local, 24, Ipv4Addr::new(192, 168, 1, 3)));
        for peer in [
            Ipv4Addr::new(192, 168, 2, 3),
            Ipv4Addr::new(192, 168, 1, 0),
            Ipv4Addr::new(192, 168, 1, 255),
        ] {
            assert!(!same_subnet(local, 24, peer));
        }
        assert!(!same_subnet(local, 0, local));
    }
}
