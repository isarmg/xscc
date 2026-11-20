#[cfg(windows)]
mod device_interface;
#[cfg(windows)]
mod driver;
#[cfg(any(windows, test))]
mod protocol;

#[cfg(windows)]
pub use device_interface::{set_amyuni_monitor, DisplayControlError};
#[cfg(windows)]
pub use driver::{install_update_driver, plug_in_monitor, plug_out_monitor, update_monitor_modes};

#[derive(Debug, Copy, Clone)]
pub struct MonitorMode {
    pub width: u32,
    pub height: u32,
    pub sync: u32,
}
