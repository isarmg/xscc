//! Reliable session messages use TCP; negotiated media uses authenticated UDP.
//! This module does not grant application login, input or clipboard permission.
#[cfg(feature = "udp-media")]
mod udp;
#[cfg(feature = "udp-media")]
pub use udp::SessionTransport;
#[cfg(not(feature = "udp-media"))]
pub type SessionTransport = hbb_common::Stream;
