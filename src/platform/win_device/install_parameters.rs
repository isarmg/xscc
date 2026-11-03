pub(super) fn hardware_id_multi_sz(hardware_id: &str) -> Vec<u16> {
    // SPDRP_HARDWAREID requires REG_MULTI_SZ, including the empty final string.
    hardware_id.encode_utf16().chain([0, 0]).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn mtt_hardware_id_has_a_terminated_multi_string() {
        let value = super::hardware_id_multi_sz(r"Root\MttVDD");
        assert_eq!(String::from_utf16(&value).unwrap(), "Root\\MttVDD\0\0");
    }
}
