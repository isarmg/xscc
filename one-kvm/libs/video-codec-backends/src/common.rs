#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use serde_derive::{Deserialize, Serialize};
include!(concat!(env!("OUT_DIR"), "/common_ffi.rs"));

pub(crate) fn supported_gpu() -> (bool, bool, bool) {
    #[cfg(windows)]
    {
        // FFmpeg probes the available adapters when it opens an encoder.
        (true, true, true)
    }
    #[cfg(target_os = "linux")]
    {
        crate::linux::supported_gpu()
    }
}
