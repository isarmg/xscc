pub(super) fn install_flags(hardware_id: &str) -> u32 {
    const INSTALLFLAG_FORCE: u32 = 0x00000001;
    const INSTALLFLAG_NONINTERACTIVE: u32 = 0x00000004;

    if hardware_id.eq_ignore_ascii_case(r"Root\MttVDD") {
        INSTALLFLAG_FORCE | INSTALLFLAG_NONINTERACTIVE
    } else {
        INSTALLFLAG_FORCE
    }
}

pub(super) fn hardware_id_multi_sz(hardware_id: &str) -> Vec<u16> {
    // SPDRP_HARDWAREID requires REG_MULTI_SZ, including the empty final string.
    hardware_id.encode_utf16().chain([0, 0]).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_mtt_installation_rejects_interactive_prompts() {
        for hardware_id in [r"Root\MttVDD", r"root\mttvdd", r"ROOT\MTTVDD"] {
            assert_eq!(super::install_flags(hardware_id), 0x00000005);
        }
        for hardware_id in ["usbmmidd", r"Root\OtherVDD", r"Root\MttVDD2"] {
            assert_eq!(super::install_flags(hardware_id), 0x00000001);
        }
    }

    #[test]
    fn mtt_hardware_id_has_a_terminated_multi_string() {
        let value = super::hardware_id_multi_sz(r"Root\MttVDD");
        assert_eq!(String::from_utf16(&value).unwrap(), "Root\\MttVDD\0\0");
    }
}
