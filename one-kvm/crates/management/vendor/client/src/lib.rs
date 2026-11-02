//! Embedded RustDesk management client. The host owns Tokio, credentials, storage and shutdown.
pub mod engine;
pub mod host;
pub mod transport;
pub use xscs_protocol as protocol;
