use std::{error::Error, fmt};

pub const MIN_KBPS: u32 = 1_000;
pub const MAX_KBPS: u32 = 1_000_000;
const ANCHORS: [u32; 5] = [MIN_KBPS, 10_000, 100_000, 300_000, MAX_KBPS];
const STEPS: [u32; 4] = [1_000, 10_000, 30_000, 100_000];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BitrateRange {
    max_kbps: u32,
}

impl BitrateRange {
    pub fn for_host(max_kbps: u32) -> Option<Self> {
        (max_kbps >= MIN_KBPS).then_some(Self {
            max_kbps: max_kbps.min(MAX_KBPS),
        })
    }

    pub fn min_kbps(self) -> u32 {
        MIN_KBPS
    }

    pub fn max_kbps(self) -> u32 {
        self.max_kbps
    }

    // Keep the global scale fixed when a host supports only part of the track.
    pub fn at_fraction(self, fraction: f64) -> u32 {
        let fraction = if fraction.is_nan() {
            0.0
        } else {
            fraction.clamp(0.0, 1.0)
        };
        let segment = ((fraction * 4.0) as usize).min(3);
        let start = ANCHORS[segment];
        let width = ANCHORS[segment + 1] - start;
        let within = fraction * 4.0 - segment as f64;
        let kbps = start + (width as f64 * within).round() as u32;
        self.snap(kbps.min(self.max_kbps))
    }

    fn snap(self, kbps: u32) -> u32 {
        let segment = segment_for(kbps);
        let start = ANCHORS[segment];
        let step = STEPS[segment];
        let lower = start + (kbps - start) / step * step;
        let upper = (lower + step).min(ANCHORS[segment + 1]).min(self.max_kbps);
        if kbps - lower < upper - kbps {
            lower
        } else {
            upper
        }
    }

    pub fn step(self, kbps: u32, forward: bool) -> u32 {
        let kbps = kbps.clamp(MIN_KBPS, self.max_kbps);
        if forward {
            if kbps == self.max_kbps {
                return kbps;
            }
            let segment = segment_for(kbps);
            let start = ANCHORS[segment];
            let step = STEPS[segment];
            (start + ((kbps - start) / step + 1) * step)
                .min(ANCHORS[segment + 1])
                .min(self.max_kbps)
        } else {
            if kbps == MIN_KBPS {
                return kbps;
            }
            let segment = segment_for(kbps - 1);
            let start = ANCHORS[segment];
            let step = STEPS[segment];
            start + (kbps - start - 1) / step * step
        }
    }

    pub fn fraction(self, kbps: u32) -> f64 {
        let kbps = kbps.clamp(MIN_KBPS, self.max_kbps);
        let segment = segment_for(kbps);
        let start = ANCHORS[segment];
        let width = ANCHORS[segment + 1] - start;
        (segment as f64 + (kbps - start) as f64 / width as f64) / 4.0
    }

    pub fn parse_mbps(self, input: &str) -> Result<u32, BitrateInputError> {
        let input = input.trim();
        let (whole, decimal) = match input.split_once('.') {
            Some((whole, decimal)) => (whole, Some(decimal)),
            None => (input, None),
        };
        if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
            return Err(BitrateInputError::InvalidDecimal);
        }
        let fraction = match decimal {
            Some(decimal) => {
                if decimal.is_empty() || !decimal.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(BitrateInputError::InvalidDecimal);
                }
                if decimal.len() > 3 {
                    return Err(BitrateInputError::TooManyDecimals);
                }
                decimal
                    .parse::<u32>()
                    .map_err(|_| BitrateInputError::InvalidDecimal)?
                    * [100, 10, 1][decimal.len() - 1]
            }
            None => 0,
        };
        let out_of_range = BitrateInputError::OutOfRange {
            max_kbps: self.max_kbps,
        };
        let kbps = whole
            .parse::<u32>()
            .ok()
            .and_then(|whole| whole.checked_mul(1_000))
            .and_then(|whole| whole.checked_add(fraction))
            .ok_or(out_of_range)?;
        if !(MIN_KBPS..=self.max_kbps).contains(&kbps) {
            return Err(out_of_range);
        }
        Ok(kbps)
    }
}

fn segment_for(kbps: u32) -> usize {
    match kbps {
        0..=9_999 => 0,
        10_000..=99_999 => 1,
        100_000..=299_999 => 2,
        _ => 3,
    }
}

pub fn format_mbps(kbps: u32) -> String {
    let whole = kbps / 1_000;
    let decimal = kbps % 1_000;
    if decimal == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{decimal:03}")
            .trim_end_matches('0')
            .to_owned()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BitrateInputError {
    InvalidDecimal,
    TooManyDecimals,
    OutOfRange { max_kbps: u32 },
}

impl fmt::Display for BitrateInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDecimal => write!(formatter, "请输入普通十进制 Mbps 数值"),
            Self::TooManyDecimals => write!(formatter, "最多支持 3 位小数，即 1 kbps 精度"),
            Self::OutOfRange { max_kbps } => write!(
                formatter,
                "码率必须在 1–{} Mbps 之间",
                format_mbps(*max_kbps)
            ),
        }
    }
}

impl Error for BitrateInputError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_segments_preserve_anchors_and_use_the_requested_steps() {
        let range = BitrateRange::for_host(MAX_KBPS).unwrap();
        for (kbps, fraction) in [
            (1_000, 0.0),
            (10_000, 0.25),
            (100_000, 0.5),
            (300_000, 0.75),
            (1_000_000, 1.0),
        ] {
            assert_eq!(range.at_fraction(fraction), kbps);
            assert_eq!(range.fraction(kbps), fraction);
        }
        for (fraction, kbps) in [
            (0.125, 6_000),
            (0.375, 60_000),
            (0.625, 190_000),
            (0.875, 700_000),
        ] {
            assert_eq!(range.at_fraction(fraction), kbps);
        }
        for kbps in [
            1_000, 2_000, 9_000, 10_000, 20_000, 90_000, 100_000, 130_000, 160_000, 190_000,
            220_000, 250_000, 280_000, 300_000, 400_000, 1_000_000,
        ] {
            assert_eq!(range.at_fraction(range.fraction(kbps)), kbps);
        }
        for (value, previous, next) in [
            (1_000, 1_000, 2_000),
            (10_000, 9_000, 20_000),
            (100_000, 90_000, 130_000),
            (135_500, 130_000, 160_000),
            (280_000, 250_000, 300_000),
            (300_000, 280_000, 400_000),
            (1_000_000, 900_000, 1_000_000),
        ] {
            assert_eq!(range.step(value, false), previous);
            assert_eq!(range.step(value, true), next);
        }
        assert!((range.fraction(135_500) - 0.544375).abs() < 1e-12);
        assert_eq!(range.at_fraction(-1.0), MIN_KBPS);
        assert_eq!(range.at_fraction(2.0), MAX_KBPS);
        assert_eq!(range.at_fraction(f64::NAN), MIN_KBPS);
    }

    #[test]
    fn host_limit_clips_the_global_track_without_rescaling() {
        assert_eq!(BitrateRange::for_host(999), None);
        let range = BitrateRange::for_host(100_000).unwrap();
        assert_eq!(range.min_kbps(), 1_000);
        assert_eq!(range.max_kbps(), 100_000);
        assert_eq!(range.fraction(range.max_kbps()), 0.5);
        assert_eq!(range.at_fraction(0.25), 10_000);
        assert_eq!(range.at_fraction(0.9), 100_000);
        assert_eq!(
            range.parse_mbps("100.001"),
            Err(BitrateInputError::OutOfRange { max_kbps: 100_000 })
        );
        assert_eq!(
            BitrateRange::for_host(u32::MAX).unwrap().max_kbps(),
            MAX_KBPS
        );
        let partial = BitrateRange::for_host(145_000).unwrap();
        assert_eq!(partial.at_fraction(1.0), 145_000);
        assert_eq!(partial.at_fraction(partial.fraction(145_000)), 145_000);
        assert_eq!(partial.step(130_000, true), 145_000);
        assert_eq!(partial.step(145_000, false), 130_000);
    }

    #[test]
    fn decimal_megabits_parse_and_format_without_float_rounding() {
        let range = BitrateRange::for_host(MAX_KBPS).unwrap();
        for (text, kbps) in [
            ("1", 1_000),
            ("1.001", 1_001),
            ("12.345", 12_345),
            ("999.999", 999_999),
            ("1000.000", 1_000_000),
            (" 01.010 ", 1_010),
        ] {
            assert_eq!(range.parse_mbps(text), Ok(kbps));
            assert_eq!(range.parse_mbps(&format_mbps(kbps)), Ok(kbps));
        }
        assert_eq!(format_mbps(1_001), "1.001");
        assert_eq!(format_mbps(1_010), "1.01");
        assert_eq!(format_mbps(1_000_000), "1000");
        for text in [
            "",
            "0.999",
            "1000.001",
            "1e3",
            "NaN",
            "inf",
            "+1",
            "-1",
            "1.",
            "1.0000",
            "１",
            "1 0",
            "1,000",
            "18446744073709551616",
            "1.000\ntext",
        ] {
            assert!(range.parse_mbps(text).is_err(), "{text}");
        }
        assert_eq!(
            range.parse_mbps("1.0000"),
            Err(BitrateInputError::TooManyDecimals)
        );
    }
}
