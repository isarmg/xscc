#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
}

#[repr(C)]
pub(crate) struct Signal {
    pub pixel_rate: u64,
    pub width: u32,
    pub height: u32,
    pub total_width: u32,
    pub total_height: u32,
    pub vertical_numerator: u32,
    pub vertical_denominator: u32,
    pub horizontal_numerator: u32,
    pub horizontal_denominator: u32,
}

impl Mode {
    pub fn signal(self, target: bool) -> Option<Signal> {
        if self.width == 0 || self.height == 0 || self.refresh == 0 {
            return None;
        }

        let blanking = if target { 0 } else { 4 };
        let total_width = self.width.checked_add(blanking)?;
        let total_height = self.height.checked_add(blanking)?;
        let divisor = gcd(self.refresh.into(), 10_000);
        let vertical_numerator = u64::from(self.refresh) / divisor;
        let vertical_denominator = 10_000 / divisor;
        let horizontal = vertical_numerator.checked_mul(total_height.into())?;
        let divisor = gcd(horizontal, vertical_denominator);
        let pixel_rate = horizontal.checked_mul(total_width.into())? / vertical_denominator;

        Some(Signal {
            pixel_rate,
            width: self.width,
            height: self.height,
            total_width,
            total_height,
            vertical_numerator: vertical_numerator.try_into().ok()?,
            vertical_denominator: vertical_denominator.try_into().ok()?,
            horizontal_numerator: (horizontal / divisor).try_into().ok()?,
            horizontal_denominator: (vertical_denominator / divisor).try_into().ok()?,
        })
    }
}

fn gcd(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        (left, right) = (right, left % right);
    }
    left
}

#[cfg(test)]
mod tests {
    use super::Mode;

    #[test]
    fn preserves_fractional_refresh_and_wide_pixel_clocks() {
        let mode = Mode {
            width: 7680,
            height: 4320,
            refresh: 2_400_000,
        };
        let target = mode.signal(true).unwrap();
        assert_eq!(target.pixel_rate, 7_962_624_000);
        assert_eq!(
            (target.horizontal_numerator, target.horizontal_denominator),
            (1_036_800, 1)
        );

        let monitor = Mode {
            width: 1920,
            height: 1080,
            refresh: 599_401,
        }
        .signal(false)
        .unwrap();
        assert_eq!((monitor.total_width, monitor.total_height), (1924, 1084));
        assert_eq!(
            (monitor.vertical_numerator, monitor.vertical_denominator),
            (599_401, 10_000)
        );
        assert_eq!(
            (monitor.horizontal_numerator, monitor.horizontal_denominator),
            (162_437_671, 2500)
        );
        assert_eq!(monitor.pixel_rate, 125_012_031);
    }

    #[test]
    fn rejects_modes_that_do_not_fit_the_windows_signal_fields() {
        assert!(
            Mode {
                width: u32::MAX,
                height: 1,
                refresh: 600_000
            }
            .signal(false)
            .is_none()
        );
        assert!(
            Mode {
                width: 1,
                height: u32::MAX - 8,
                refresh: 599_401
            }
            .signal(true)
            .is_none()
        );
        assert!(
            Mode {
                width: 1920,
                height: 1080,
                refresh: 0
            }
            .signal(true)
            .is_none()
        );
    }
}
