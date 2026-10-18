use std::{
    net::{IpAddr, SocketAddr},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    process::Child,
};
use windows::Win32::{
    Foundation::HANDLE,
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    },
};

pub fn guard_test_direct_child(child: &Child) -> Result<OwnedHandle, String> {
    // The unnamed, non-inheritable job dies with the UI even after a crash or forced exit.
    unsafe {
        let handle = CreateJobObjectW(None, None).map_err(|e| e.to_string())?;
        let job = OwnedHandle::from_raw_handle(handle.0);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of_val(&limits) as u32,
        )
        .map_err(|e| e.to_string())?;
        AssignProcessToJobObject(handle, HANDLE(child.as_raw_handle()))
            .map_err(|e| e.to_string())?;
        Ok(job)
    }
}

pub fn test_direct_addresses(bind: SocketAddr) -> Vec<String> {
    if !bind.ip().is_unspecified() {
        return if matches!(bind.ip(), IpAddr::V6(ip) if ip.is_unicast_link_local()) {
            Vec::new()
        } else {
            vec![bind.to_string()]
        };
    }
    let mut addresses = Vec::new();
    for interface in default_net::get_interfaces() {
        if bind.is_ipv4() {
            for network in interface.ipv4 {
                if !network.addr.is_loopback() && !network.addr.is_unspecified() {
                    addresses.push(SocketAddr::new(network.addr.into(), bind.port()).to_string());
                }
            }
        } else {
            for network in interface.ipv6 {
                // Link-local zone IDs belong to the controller's interface, not this host.
                if !network.addr.is_loopback()
                    && !network.addr.is_unspecified()
                    && !network.addr.is_unicast_link_local()
                {
                    addresses.push(SocketAddr::new(network.addr.into(), bind.port()).to_string());
                }
            }
        }
    }
    addresses
}
