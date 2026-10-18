use hbb_common::libc;
use std::{io, net::Ipv4Addr, ptr};

pub(super) fn ipv4_networks() -> io::Result<Vec<(Ipv4Addr, u8)>> {
    let mut head = ptr::null_mut();
    // getifaddrs owns all linked entries until freeifaddrs; copy only IPv4 values.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Err(io::Error::last_os_error());
    }
    struct Interfaces(*mut libc::ifaddrs);
    impl Drop for Interfaces {
        fn drop(&mut self) {
            unsafe { libc::freeifaddrs(self.0) };
        }
    }
    let interfaces = Interfaces(head);
    let mut entry = interfaces.0;
    let mut networks = Vec::new();
    while !entry.is_null() {
        // The list and address pointers remain valid while `interfaces` is alive.
        unsafe {
            let interface = &*entry;
            if !interface.ifa_addr.is_null()
                && !interface.ifa_netmask.is_null()
                && (*interface.ifa_addr).sa_family as i32 == libc::AF_INET
                && (*interface.ifa_netmask).sa_family as i32 == libc::AF_INET
            {
                let address = &*interface.ifa_addr.cast::<libc::sockaddr_in>();
                let mask = &*interface.ifa_netmask.cast::<libc::sockaddr_in>();
                if let Some(prefix) =
                    super::address::mask_prefix(u32::from_be(mask.sin_addr.s_addr))
                {
                    networks.push((
                        Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes()),
                        prefix,
                    ));
                }
            }
            entry = interface.ifa_next;
        }
    }
    Ok(networks)
}
