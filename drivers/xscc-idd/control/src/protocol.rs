// SPDX-License-Identifier: MIT
use crate::{Error, Result};

const VERSION: u32 = 1;
pub(crate) const CONNECTORS: u32 = 32;
const MAX_MODES: usize = 4096;
pub(crate) const NO_CONNECTOR: u32 = u32::MAX;
pub(crate) const QUERY: u32 = 0x8000_e000;
pub(crate) const LIST: u32 = 0x8000_e004;
pub(crate) const ADD: u32 = 0x8000_e008;
pub(crate) const REMOVE: u32 = 0x8000_e00c;
pub(crate) const OWNED_LIST: u32 = 0x8000_e010;
pub(crate) const MAX_LIST_BYTES: usize = 16 + CONNECTORS as usize * 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectorId(u32);

impl ConnectorId {
    pub fn new(value: u32) -> Result<Self> {
        if value >= CONNECTORS {
            return Err(Error::protocol("connector", "outside driver capacity"));
        }
        Ok(Self(value))
    }
    pub fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    /// Units of 1/10000 Hz. The driver additionally validates timing arithmetic.
    pub refresh_rate_units: u32,
}

impl Mode {
    fn validate(self) -> Result<()> {
        if self.width == 0 || self.height == 0 || self.refresh_rate_units == 0 {
            return Err(Error::protocol("mode", "zero dimension or refresh rate"));
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct Capabilities {
    /// Protocol capacity; not a measured GPU/Windows monitor limit.
    pub connector_capacity: u32,
    /// All owners' completed arrivals, not CCD-active desktops.
    pub connected_count: u32,
    pub max_modes_per_monitor: u32,
    pub gpu_monitor_limit_unknown: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Monitor {
    pub connector: ConnectorId,
    pub mode_count: u32,
    /// First advertised mode, not the current CCD mode.
    pub preferred_mode: Mode,
}

pub(crate) fn request(connector: Option<ConnectorId>, modes: &[Mode]) -> Result<Vec<u8>> {
    if modes.len() > MAX_MODES {
        return Err(Error::protocol("request", "too many modes"));
    }
    let mut bytes = Vec::with_capacity(16 + modes.len() * 12);
    for value in [
        VERSION,
        (16 + modes.len() * 12) as u32,
        connector.map_or(NO_CONNECTOR, ConnectorId::get),
        modes.len() as u32,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for &mode in modes {
        mode.validate()?;
        for value in [mode.width, mode.height, mode.refresh_rate_units] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(bytes)
}

fn word(bytes: &[u8], index: usize) -> u32 {
    let offset = index * 4;
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn response(bytes: &[u8], size: usize, operation: &'static str) -> Result<()> {
    if bytes.len() != size || word(bytes, 0) != VERSION || word(bytes, 1) as usize != size {
        return Err(Error::protocol(
            operation,
            "invalid response version or length",
        ));
    }
    Ok(())
}

pub(crate) fn capabilities(bytes: &[u8]) -> Result<Capabilities> {
    response(bytes, 24, "query response")?;
    let capacity = word(bytes, 2);
    let count = word(bytes, 3);
    let modes = word(bytes, 4);
    let flags = word(bytes, 5);
    if capacity == 0
        || capacity > CONNECTORS
        || count > capacity
        || modes == 0
        || modes > MAX_MODES as u32
        || flags & !1 != 0
    {
        return Err(Error::protocol(
            "query response",
            "invalid capability values",
        ));
    }
    Ok(Capabilities {
        connector_capacity: capacity,
        connected_count: count,
        max_modes_per_monitor: modes,
        gpu_monitor_limit_unknown: flags & 1 != 0,
    })
}

pub(crate) fn monitors(bytes: &[u8]) -> Result<Vec<Monitor>> {
    if bytes.len() < 16 || bytes.len() > MAX_LIST_BYTES {
        return Err(Error::protocol("list response", "invalid response length"));
    }
    let count = word(bytes, 2) as usize;
    if count > CONNECTORS as usize || word(bytes, 3) != 20 {
        return Err(Error::protocol(
            "list response",
            "invalid monitor count or record size",
        ));
    }
    response(bytes, 16 + count * 20, "list response")?;
    let mut result = Vec::with_capacity(count);
    let mut seen = 0u32;
    for record in bytes[16..].as_chunks::<20>().0 {
        let connector = ConnectorId::new(word(record, 0))?;
        let bit = 1 << connector.get();
        let mode_count = word(record, 1);
        let preferred_mode = Mode {
            width: word(record, 2),
            height: word(record, 3),
            refresh_rate_units: word(record, 4),
        };
        if seen & bit != 0 || mode_count == 0 || mode_count > MAX_MODES as u32 {
            return Err(Error::protocol(
                "list response",
                "duplicate connector or invalid mode count",
            ));
        }
        preferred_mode.validate()?;
        seen |= bit;
        result.push(Monitor {
            connector,
            mode_count,
            preferred_mode,
        });
    }
    Ok(result)
}

pub(crate) fn completed(bytes: &[u8], expected: Option<ConnectorId>) -> Result<ConnectorId> {
    response(bytes, 12, "operation response")?;
    let connector = ConnectorId::new(word(bytes, 2))?;
    if expected.is_some_and(|value| value != connector) {
        return Err(Error::protocol(
            "operation response",
            "connector does not match request",
        ));
    }
    Ok(connector)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(values: &[u32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    #[test]
    fn requests_match_the_driver_contract_without_limiting_high_refresh() {
        assert_eq!(
            [QUERY, LIST, ADD, REMOVE, OWNED_LIST],
            [
                0x8000_e000,
                0x8000_e004,
                0x8000_e008,
                0x8000_e00c,
                0x8000_e010
            ]
        );
        assert_eq!(request(None, &[]).unwrap(), wire(&[1, 16, u32::MAX, 0]));
        let mode = Mode {
            width: 3840,
            height: 2160,
            refresh_rate_units: 2_400_000,
        };
        assert_eq!(
            request(None, &[mode]).unwrap(),
            wire(&[1, 28, u32::MAX, 1, 3840, 2160, 2_400_000])
        );
        assert_eq!(
            request(Some(ConnectorId::new(31).unwrap()), &[]).unwrap(),
            wire(&[1, 16, 31, 0])
        );
        assert!(ConnectorId::new(32).is_err());
        assert!(request(None, &[mode; MAX_MODES + 1]).is_err());
        assert!(request(None, &[Mode { width: 0, ..mode }]).is_err());
    }

    #[test]
    fn inventory_rejects_truncated_overlong_duplicate_and_unsupported_responses() {
        let query = wire(&[1, 24, 32, 2, 4096, 1]);
        assert_eq!(capabilities(&query).unwrap().connected_count, 2);
        for length in 0..query.len() {
            assert!(capabilities(&query[..length]).is_err());
        }
        assert!(capabilities(&wire(&[2, 24, 32, 2, 4096, 1])).is_err());
        assert!(capabilities(&wire(&[1, 24, 32, 33, 4096, 1])).is_err());
        assert!(capabilities(&wire(&[1, 24, 32, 2, 4096, 2])).is_err());
        assert!(monitors(&wire(&[1, 16, 0, 20])).unwrap().is_empty());
        let list = wire(&[1, 36, 1, 20, 0, 1, 1920, 1080, 1_200_000]);
        assert_eq!(
            monitors(&list).unwrap()[0]
                .preferred_mode
                .refresh_rate_units,
            1_200_000
        );
        for length in 0..list.len() {
            assert!(monitors(&list[..length]).is_err());
        }
        assert!(monitors(&wire(&[
            1, 56, 2, 20, 0, 1, 1920, 1080, 600_000, 0, 1, 1920, 1080, 600_000
        ]))
        .is_err());
        assert!(monitors(&wire(&[1, 16, u32::MAX, 20])).is_err());
        assert!(monitors(&wire(&[1, 20, 0, 20, 0])).is_err());
    }

    #[test]
    fn operation_acknowledgement_must_name_the_requested_connector() {
        let connector = ConnectorId::new(7).unwrap();
        assert_eq!(
            completed(&wire(&[1, 12, 7]), Some(connector)).unwrap(),
            connector
        );
        assert!(completed(&wire(&[1, 12, 8]), Some(connector)).is_err());
        assert!(completed(&wire(&[1, 12, 32]), None).is_err());
        assert!(completed(&wire(&[1, 12, 7, 0]), None).is_err());
    }
}
