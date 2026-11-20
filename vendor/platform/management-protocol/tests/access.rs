use xscs_protocol::*;

#[test]
fn addresses_accept_only_explicit_canonical_hosts_and_ports() {
    for host in [
        "192.0.2.10",
        "2001:db8::1",
        "host.example.com",
        "xn--fiqs8s.example",
    ] {
        HostAddress {
            host: host.into(),
            port: 21118,
        }
        .validate()
        .unwrap();
    }
    for host in [
        "",
        "https://host.example",
        "host.example:21118",
        "[::1]",
        "EXAMPLE.com",
        "example.com/path",
        "a..b",
        "-host.example",
        "0.0.0.0",
        "::",
        "224.0.0.1",
        "999.2.3.4",
        "127.00.0.1",
        "foo\nbar",
        "主机.example",
    ] {
        assert!(
            HostAddress {
                host: host.into(),
                port: 21118
            }
            .validate()
            .is_err(),
            "{host}"
        );
    }
    assert!(
        HostAddress {
            host: "host.example".into(),
            port: 0
        }
        .validate()
        .is_err()
    );
    let address = HostAddress {
        host: "host.example".into(),
        port: 21118,
    };
    assert!(validate_addresses(&[address.clone(), address]).is_err());
}

#[test]
fn policy_binds_identity_direction_and_message_bounds() {
    let binding = Binding {
        manager_id: uuid::Uuid::new_v4(),
        device_id: uuid::Uuid::new_v4(),
        installation_id: uuid::Uuid::new_v4(),
    };
    let domain = format!(
        "{}.{}.{}.{}",
        "a".repeat(63),
        "b".repeat(63),
        "c".repeat(63),
        "d".repeat(61)
    );
    let mut policy = AccessPolicy {
        binding: binding.clone(),
        role: InstanceRole::Controller,
        valid_for_ms: ACCESS_LEASE_MS,
        peers: (0..MAX_ACCESS_PEERS)
            .map(|_| AccessPeer {
                binding: Binding {
                    manager_id: binding.manager_id,
                    device_id: uuid::Uuid::new_v4(),
                    installation_id: uuid::Uuid::new_v4(),
                },
                name: "🙂".repeat(32),
                role: InstanceRole::Controlled,
                session_key: SessionKey::from_bytes([7; 32]),
                online: true,
                addresses: (1..=MAX_HOST_ADDRESSES)
                    .map(|port| HostAddress {
                        host: domain.clone(),
                        port: port as u16,
                    })
                    .collect(),
            })
            .collect(),
    };
    policy.validate(&binding).unwrap();
    let frame = serde_json::to_vec(&ManagerMessage::AccessPolicy {
        policy: Box::new(policy.clone()),
    })
    .unwrap();
    assert!(frame.len() < MAX_MESSAGE_BYTES);
    policy.peers[0].role = InstanceRole::Controller;
    assert!(policy.validate(&binding).is_err());
    policy.peers.clear();
    policy.binding.installation_id = uuid::Uuid::new_v4();
    assert!(policy.validate(&binding).is_err());
}
