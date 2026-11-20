use crate::MonitorMode;
use std::{io, mem::size_of};

pub(crate) const PLUG_IN: u32 = 0x30c004;
pub(crate) const PLUG_OUT: u32 = 0x30c008;
pub(crate) const UPDATE_MODES: u32 = 0x30c00c;

pub(crate) fn plug_in(index: u32, container_id: [u8; 16]) -> [u8; 24] {
    let mut message = [0; 24];
    message[..4].copy_from_slice(&index.to_le_bytes());
    message[8..].copy_from_slice(&container_id);
    message
}

pub(crate) fn plug_out(index: u32) -> [u8; 4] {
    index.to_le_bytes()
}

pub(crate) fn update_modes(index: u32, modes: &[MonitorMode]) -> io::Result<Vec<u8>> {
    let count = modes.len();
    let size = count
        .checked_mul(3 * size_of::<u32>())
        .and_then(|size| size.checked_add(2 * size_of::<u32>()))
        .filter(|&size| u32::try_from(size).is_ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "too many monitor modes"))?;
    let mut message = Vec::new();
    message
        .try_reserve_exact(size)
        .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
    message.extend(index.to_le_bytes());
    message.extend((count as u32).to_le_bytes());
    for mode in modes {
        message.extend(mode.width.to_le_bytes());
        message.extend(mode.height.to_le_bytes());
        message.extend(mode.sync.to_le_bytes());
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_monitor_operations_keep_driver_wire_format() {
        let mut expected_plug_in = [0xab; 24];
        expected_plug_in[..8].copy_from_slice(&[4, 3, 2, 1, 0, 0, 0, 0]);
        assert_eq!(plug_in(0x0102_0304, [0xab; 16]), expected_plug_in);
        assert_eq!(plug_out(0x0102_0304), [4, 3, 2, 1]);
        assert_eq!(
            [PLUG_IN, PLUG_OUT, UPDATE_MODES],
            [0x30c004, 0x30c008, 0x30c00c]
        );
    }

    #[test]
    fn mode_count_and_refresh_rate_use_fixed_little_endian_fields() {
        let modes = [MonitorMode {
            width: 1920,
            height: 1080,
            sync: 144,
        }];
        assert_eq!(
            update_modes(2, &modes).unwrap(),
            [2, 0, 0, 0, 1, 0, 0, 0, 0x80, 7, 0, 0, 0x38, 4, 0, 0, 144, 0, 0, 0]
        );
        assert_eq!(update_modes(2, &[]).unwrap(), [2, 0, 0, 0, 0, 0, 0, 0]);
    }
}
