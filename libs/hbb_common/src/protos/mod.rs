include!(concat!(env!("OUT_DIR"), "/protos/mod.rs"));

#[cfg(test)]
mod tests {
    use super::rendezvous::{rendezvous_message, PeerDiscovery, RendezvousMessage};
    use protobuf::Message;

    #[test]
    fn lan_discovery_keeps_its_wire_tag_and_retired_messages_are_not_dispatched() {
        let mut message = RendezvousMessage::new();
        message.set_peer_discovery(PeerDiscovery {
            cmd: "ping".to_owned(),
            ..Default::default()
        });
        let wire = message.write_to_bytes().unwrap();
        assert_eq!(&wire[..2], &[0xb2, 0x01]);
        let decoded = RendezvousMessage::parse_from_bytes(&wire).unwrap();
        assert!(matches!(
            decoded.union,
            Some(rendezvous_message::Union::PeerDiscovery(p)) if p.cmd == "ping"
        ));
        for retired in [&[0x32, 0x00][..], &[0x92, 0x01, 0x00][..]] {
            assert!(RendezvousMessage::parse_from_bytes(retired)
                .unwrap()
                .union
                .is_none());
        }
    }
}
