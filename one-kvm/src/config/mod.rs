mod resources;
mod schema;
mod store;

/// Configuration change event
#[derive(Debug, Clone)]
pub struct ConfigChange {
    pub key: String,
}

pub use resources::{ResourceRegistry, ResourceReservation};
pub use schema::*;
pub use store::ConfigStore;
