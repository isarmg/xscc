#[cfg(feature = "mediacodec")]
mod decoder_inventory;
#[cfg(feature = "mediacodec")]
pub use decoder_inventory::initialize_android_decoders;
pub mod ffi;

pub use ffi::*;
