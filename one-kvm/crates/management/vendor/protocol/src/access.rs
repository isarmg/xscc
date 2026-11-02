//! Directed instance permissions and host-reported direct connection addresses.
use crate::{Binding, InvalidTask, MAX_MESSAGE_BYTES};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, net::IpAddr};

pub const MAX_ACCESS_PEERS: usize = 32;
pub const MAX_HOST_ADDRESSES: usize = 4;
pub const ACCESS_LEASE_MS: u64 = 30_000;

/// Pair-specific proof key, delivered only on the authenticated management channel.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct SessionKey(String);
impl SessionKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(hex::encode(bytes))
    }
    pub fn as_hex(&self) -> &str {
        &self.0
    }
    pub fn bytes(&self) -> Result<[u8; 32], InvalidTask> {
        crate::validate_revision(&self.0)?;
        let mut bytes = [0; 32];
        hex::decode_to_slice(&self.0, &mut bytes).map_err(|_| InvalidTask)?;
        Ok(bytes)
    }
}
impl std::fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionKey(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstanceRole {
    Controller,
    Controlled,
}
impl InstanceRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Controller => "controller",
            Self::Controlled => "controlled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostAddress {
    /// Canonical IP literal (IPv6 without brackets) or lowercase ASCII DNS name.
    pub host: String,
    pub port: u16,
}
impl HostAddress {
    pub fn validate(&self) -> Result<(), InvalidTask> {
        if self.port == 0 || self.host.len() > 253 || self.host.is_empty() {
            return Err(InvalidTask);
        }
        if let Ok(ip) = self.host.parse::<IpAddr>() {
            if ip.to_string() != self.host || ip.is_unspecified() || ip.is_multicast() {
                return Err(InvalidTask);
            }
            return Ok(());
        }
        // Numeric-looking malformed IPs must not be accepted as DNS names.
        if self.host.bytes().all(|b| b.is_ascii_digit() || b == b'.')
            || !self.host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && label.as_bytes()[0].is_ascii_alphanumeric()
                    && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
        {
            return Err(InvalidTask);
        }
        Ok(())
    }
}
pub fn validate_addresses(addresses: &[HostAddress]) -> Result<(), InvalidTask> {
    let mut seen = HashSet::new();
    if addresses.len() > MAX_HOST_ADDRESSES {
        return Err(InvalidTask);
    }
    for address in addresses {
        address.validate()?;
        if !seen.insert(address) {
            return Err(InvalidTask);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccessPeer {
    pub binding: Binding,
    pub name: String,
    pub role: InstanceRole,
    pub online: bool,
    /// Only controlled instances expose addresses; stale observations are omitted.
    pub addresses: Vec<HostAddress>,
    pub session_key: SessionKey,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccessPolicy {
    pub binding: Binding,
    pub role: InstanceRole,
    /// Refreshed by xscs every five seconds. Host must fail closed after this lease.
    pub valid_for_ms: u64,
    pub peers: Vec<AccessPeer>,
}
impl AccessPolicy {
    pub fn validate(&self, binding: &Binding) -> Result<(), InvalidTask> {
        if &self.binding != binding
            || self.valid_for_ms != ACCESS_LEASE_MS
            || self.peers.len() > MAX_ACCESS_PEERS
            || binding.manager_id.is_nil()
            || binding.device_id.is_nil()
            || binding.installation_id.is_nil()
        {
            return Err(InvalidTask);
        }
        let mut seen = HashSet::new();
        for peer in &self.peers {
            if peer.role == self.role
                || peer.binding.manager_id != binding.manager_id
                || peer.binding.device_id.is_nil()
                || peer.binding.installation_id.is_nil()
                || peer.binding.device_id == binding.device_id
                || peer.binding.installation_id == binding.installation_id
                || !seen.insert(peer.binding.device_id)
                || peer.name.chars().count() > 32
                || (peer.role == InstanceRole::Controller && !peer.addresses.is_empty())
            {
                return Err(InvalidTask);
            }
            super::validate_text(&peer.name, 128)?;
            validate_addresses(&peer.addresses)?;
            peer.session_key.bytes()?;
        }
        if serde_json::to_vec(self).map_err(|_| InvalidTask)?.len() > MAX_MESSAGE_BYTES - 1024 {
            return Err(InvalidTask);
        }
        Ok(())
    }
}
