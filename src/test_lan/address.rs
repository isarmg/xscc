use std::net::{IpAddr, SocketAddr};

pub const DEFAULT_PORT: u16 = 21119;
pub const PREFIX: &str = "test-lan:";

pub fn endpoint(input: &str) -> Result<SocketAddr, &'static str> {
    let input = input.trim();
    input
        .parse::<SocketAddr>()
        .or_else(|_| {
            input
                .parse::<IpAddr>()
                .map(|ip| SocketAddr::new(ip, DEFAULT_PORT))
        })
        .map_err(|_| "Enter an IPv4 or IPv6 address, optionally with a port; use [IPv6]:port")
}

pub fn connect_endpoint(input: &str) -> Result<SocketAddr, &'static str> {
    let address = endpoint(input)?;
    if address.port() == 0 {
        return Err("Enter the host's listening port (1–65535); remote port 0 is not connectable");
    }
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ipv4_and_ipv6_without_network_scope_restrictions() {
        for ip in [
            "192.168.1.2",
            "8.8.8.8",
            "100.64.0.1",
            "127.0.0.1",
            "2001:db8::1",
            "::1",
        ] {
            let endpoint = connect_endpoint(ip).unwrap();
            assert_eq!(endpoint.ip(), ip.parse::<IpAddr>().unwrap());
            assert_eq!(endpoint.port(), DEFAULT_PORT);
        }
        assert_eq!(
            endpoint(" 8.8.8.8:3456 ").unwrap().to_string(),
            "8.8.8.8:3456"
        );
        assert_eq!(
            endpoint("[2001:db8::1]:65535").unwrap().to_string(),
            "[2001:db8::1]:65535"
        );
    }

    #[test]
    fn accepts_wildcard_listeners_and_os_assigned_ports() {
        assert_eq!(endpoint("0.0.0.0:0").unwrap().to_string(), "0.0.0.0:0");
        assert_eq!(endpoint("[::]:0").unwrap().to_string(), "[::]:0");
        assert_eq!(endpoint("::").unwrap().to_string(), "[::]:21119");
        assert!(connect_endpoint("8.8.8.8:0").is_err());
        assert!(connect_endpoint("[2001:db8::1]:0").is_err());
    }

    #[test]
    fn rejects_non_ip_and_malformed_endpoints() {
        for value in [
            "",
            "example.com",
            "https://8.8.8.8",
            "1.2.3.256",
            "8.8.8.8:65536",
            "8.8.8.8:-1",
            "[::1]:65536",
            "[::1]:abc",
        ] {
            assert!(endpoint(value).is_err(), "{value}");
        }
    }
}
