pub(super) fn units(data: &[u8]) -> Option<impl Iterator<Item = &[u8]>> {
    let prefix = start_code(data)?;
    if data[..prefix].iter().any(|&byte| byte != 0) {
        return None;
    }
    let mut remaining = &data[prefix + 3..];
    Some(std::iter::from_fn(move || loop {
        if remaining.is_empty() {
            return None;
        }
        let unit = match start_code(remaining) {
            Some(next) => {
                let unit = &remaining[..next];
                remaining = &remaining[next + 3..];
                unit
            }
            None => std::mem::take(&mut remaining),
        };
        if let Some(end) = unit.iter().rposition(|&byte| byte != 0) {
            return Some(&unit[..=end]);
        }
    }))
}

fn start_code(data: &[u8]) -> Option<usize> {
    data.windows(3).position(|prefix| prefix == [0, 0, 1])
}

#[cfg(test)]
mod tests {
    #[test]
    fn annex_b_splits_both_start_codes_without_removing_emulation_prevention() {
        let bytes = [
            0, 0, 0, 1, 0x67, 0, 0, 3, 1, 0, 0, 1, 0x68, 0, 0, 0, 1, 0x65, 7, 0,
        ];
        let units = super::units(&bytes).unwrap().collect::<Vec<_>>();
        assert_eq!(units, [&[0x67, 0, 0, 3, 1][..], &[0x68], &[0x65, 7]]);
        assert!(super::units(&[5, 0, 0, 1, 0x65]).is_none());
        assert!(super::units(&[0, 0, 1]).unwrap().next().is_none());
    }
}
