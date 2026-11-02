use hbb_common::{ResultType, bail};
use serde_derive::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_JSON_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VirtualDisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_rate_units: u32,
}

impl VirtualDisplayMode {
    pub fn validate(&self) -> ResultType<()> {
        if self.width == 0
            || self.height == 0
            || self.width > i32::MAX as u32
            || self.height > i32::MAX as u32
            || self.refresh_rate_units == 0
            || self.refresh_rate_units > i32::MAX as u32
        {
            bail!("Invalid virtual display dimensions or refresh rate");
        }
        let pixels = u64::from(self.width)
            .checked_mul(u64::from(self.height))
            .ok_or_else(|| hbb_common::anyhow::anyhow!("Virtual display pixel count overflow"))?;
        pixels
            .checked_mul(u64::from(self.refresh_rate_units))
            .ok_or_else(|| hbb_common::anyhow::anyhow!("Virtual display signal rate overflow"))?;
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VirtualDisplayConfiguration {
    pub count: u32,
    pub modes: Vec<VirtualDisplayMode>,
    pub persistent: bool,
}

impl VirtualDisplayConfiguration {
    pub fn validate(&self) -> ResultType<()> {
        if self.count > i32::MAX as u32 || self.modes.len() > 4096 {
            bail!("Virtual display configuration exceeds the driver or message bounds");
        }
        for mode in &self.modes {
            mode.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VirtualDisplayScreen {
    pub id: String,
    pub name: String,
    pub index: Option<usize>,
    pub active: bool,
    pub width: u32,
    pub height: u32,
    pub refresh_rate_units: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VirtualDisplayState {
    pub protocol_version: u32,
    pub driver: String,
    pub status: String,
    pub message: String,
    pub configured_count: u32,
    pub present_count: u32,
    pub active_count: u32,
    pub persistent: bool,
    pub maximum_count: Option<u32>,
    pub displays: Vec<VirtualDisplayScreen>,
    pub modes: Vec<VirtualDisplayMode>,
}

impl Default for VirtualDisplayState {
    fn default() -> Self {
        Self {
            protocol_version: 0,
            driver: String::new(),
            status: "unsupported".into(),
            message: "Virtual display configuration is not supported by this host".into(),
            configured_count: 0,
            present_count: 0,
            active_count: 0,
            persistent: false,
            maximum_count: None,
            displays: Vec::new(),
            modes: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VirtualDisplayResult {
    pub request_id: u64,
    pub state: VirtualDisplayState,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_configuration_preserves_precision_and_rejects_driver_overflows() {
        let config = VirtualDisplayConfiguration {
            count: 12,
            persistent: true,
            modes: vec![VirtualDisplayMode {
                width: 2560,
                height: 1440,
                refresh_rate_units: 599401,
            }],
        };
        config.validate().unwrap();
        let encoded = serde_json::to_string(&config).unwrap();
        assert_eq!(
            serde_json::from_str::<VirtualDisplayConfiguration>(&encoded).unwrap(),
            config
        );
        assert!(
            VirtualDisplayMode {
                width: 0,
                ..config.modes[0]
            }
            .validate()
            .is_err()
        );
        assert!(
            VirtualDisplayMode {
                width: i32::MAX as u32,
                height: i32::MAX as u32,
                refresh_rate_units: i32::MAX as u32
            }
            .validate()
            .is_err()
        );
        assert!(
            VirtualDisplayConfiguration {
                count: u32::MAX,
                ..config
            }
            .validate()
            .is_err()
        );
    }
}
