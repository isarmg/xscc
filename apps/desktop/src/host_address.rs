use xscc::protocol::{HostAddress, validate_addresses};

pub fn parse(host: &str, port: &str) -> Result<Vec<HostAddress>, String> {
    let host = host.trim();
    let port = port.trim();
    if host.is_empty() {
        return if port.is_empty() {
            Ok(Vec::new())
        } else {
            Err("填写端口时也需填写域名或 IP；两项留空表示自动上报。".into())
        };
    }
    let port = if port.is_empty() {
        21118
    } else {
        port.parse::<u16>()
            .map_err(|_| "端口必须是 1–65535 的整数。")?
    };
    let host = host
        .parse::<std::net::IpAddr>()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|_| host.to_ascii_lowercase());
    let addresses = vec![HostAddress { host, port }];
    validate_addresses(&addresses)
        .map_err(|_| "地址无效：请填写域名或 IP（不含协议、路径或端口），端口范围为 1–65535。")?;
    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_address_and_automatic_mode_use_existing_contract() {
        assert!(parse("", "").unwrap().is_empty());
        let addresses = parse("Host.Example.com", "32118").unwrap();
        assert_eq!(addresses[0].host, "host.example.com");
        assert_eq!(addresses[0].port, 32118);
        assert_eq!(parse("2001:DB8::1", "").unwrap()[0].host, "2001:db8::1");
        assert_eq!(parse("192.168.1.10", "").unwrap()[0].port, 21118);
    }

    #[test]
    fn invalid_endpoint_or_port_is_rejected() {
        for (host, port) in [
            ("https://example.com", "21118"),
            ("999.0.0.1", "21118"),
            ("host", "0"),
            ("host", "65536"),
            ("host", "abc"),
            ("", "21118"),
        ] {
            assert!(parse(host, port).is_err());
        }
    }
}
