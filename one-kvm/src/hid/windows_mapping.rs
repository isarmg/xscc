use super::keyboard::CanonicalKey;

/// Set-1 scan codes; bit 8 denotes the E0 prefix. Pause uses VK_PAUSE instead.
pub(super) fn scan_code(key: CanonicalKey) -> Option<u16> {
    Some(match key.to_hid_usage() {
        0x04..=0x1d => [
            0x1e, 0x30, 0x2e, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31,
            0x18, 0x19, 0x10, 0x13, 0x1f, 0x14, 0x16, 0x2f, 0x11, 0x2d, 0x15, 0x2c,
        ][(key.to_hid_usage() - 0x04) as usize],
        usage @ 0x1e..=0x27 => u16::from(usage - 0x1e + 2),
        0x28 => 0x1c,
        0x29 => 0x01,
        0x2a => 0x0e,
        0x2b => 0x0f,
        0x2c => 0x39,
        0x2d => 0x0c,
        0x2e => 0x0d,
        0x2f => 0x1a,
        0x30 => 0x1b,
        0x31 => 0x2b,
        0x33 => 0x27,
        0x34 => 0x28,
        0x35 => 0x29,
        0x36 => 0x33,
        0x37 => 0x34,
        0x38 => 0x35,
        0x39 => 0x3a,
        usage @ 0x3a..=0x43 => u16::from(usage - 0x3a + 0x3b),
        0x44 => 0x57,
        0x45 => 0x58,
        0x46 => 0x137,
        0x47 => 0x46,
        0x48 => return None,
        0x49 => 0x152,
        0x4a => 0x147,
        0x4b => 0x149,
        0x4c => 0x153,
        0x4d => 0x14f,
        0x4e => 0x151,
        0x4f => 0x14d,
        0x50 => 0x14b,
        0x51 => 0x150,
        0x52 => 0x148,
        0x53 => 0x45,
        0x54 => 0x135,
        0x55 => 0x37,
        0x56 => 0x4a,
        0x57 => 0x4e,
        0x58 => 0x11c,
        0x59 => 0x4f,
        0x5a => 0x50,
        0x5b => 0x51,
        0x5c => 0x4b,
        0x5d => 0x4c,
        0x5e => 0x4d,
        0x5f => 0x47,
        0x60 => 0x48,
        0x61 => 0x49,
        0x62 => 0x52,
        0x63 => 0x53,
        0x64 => 0x56,
        0x65 => 0x15d,
        usage @ 0x68..=0x72 => u16::from(usage - 0x68 + 0x64),
        0x73 => 0x76,
        0xe0 => 0x1d,
        0xe1 => 0x2a,
        0xe2 => 0x38,
        0xe3 => 0x15b,
        0xe4 => 0x11d,
        0xe5 => 0x36,
        0xe6 => 0x138,
        0xe7 => 0x15c,
        _ => return None,
    })
}

pub(super) fn absolute_coordinate(
    input: i32,
    monitor_origin: i32,
    monitor_extent: u32,
    desktop_origin: i32,
    desktop_extent: i32,
) -> i32 {
    let pixel = i64::from(monitor_origin)
        + i64::from(input.clamp(0, 32767)) * i64::from(monitor_extent.saturating_sub(1)) / 32767;
    ((pixel - i64::from(desktop_origin)) * 65535 / i64::from((desktop_extent - 1).max(1)))
        .clamp(0, 65535) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_input_stays_on_selected_monitor_with_negative_desktop_origin() {
        assert_eq!(absolute_coordinate(0, -1920, 1920, -1920, 3840), 0);
        assert_eq!(absolute_coordinate(32767, -1920, 1920, -1920, 3840), 32758);
        assert_eq!(absolute_coordinate(0, 0, 1920, -1920, 3840), 32776);
        assert_eq!(absolute_coordinate(32767, 0, 1920, -1920, 3840), 65535);
        assert_eq!(absolute_coordinate(-100, 0, 1920, -1920, 3840), 32776);
        assert_eq!(absolute_coordinate(50000, 0, 1920, -1920, 3840), 65535);
    }

    #[test]
    fn physical_keys_distinguish_extended_modifiers_and_keypad() {
        assert_eq!(scan_code(CanonicalKey::KeyA), Some(0x1e));
        assert_eq!(scan_code(CanonicalKey::ControlRight), Some(0x11d));
        assert_eq!(scan_code(CanonicalKey::NumpadEnter), Some(0x11c));
        assert_eq!(scan_code(CanonicalKey::Enter), Some(0x1c));
        assert_eq!(scan_code(CanonicalKey::Pause), None);
        for usage in 0..=u8::MAX {
            if let Some(key) = CanonicalKey::from_hid_usage(usage) {
                assert!(scan_code(key).is_some() || key == CanonicalKey::Pause);
            }
        }
    }
}
