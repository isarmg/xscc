use crate::timing::Mode;
use roxmltree::{Document, Node};
use std::{
    borrow::Cow,
    collections::BTreeSet,
    fs::File,
    io::{self, Read},
    path::Path,
};

const PROFILE_SIZE: usize = 1024 * 1024;
const MAX_EDID_BYTES: usize = 256 * 128;
const DEFAULT_EDID: &[u8] = include_bytes!("monitor.edid");
const DEFAULT_PROFILE: &str = include_str!("../vdd_settings.xml");

#[repr(C)]
pub(crate) struct Colour {
    pub format: u32,
    pub sdr_bits: u32,
    pub hdr_bits: u32,
}

pub(crate) struct Profile {
    pub monitor_count: u32,
    pub modes: Vec<Mode>,
    pub colour: Colour,
    pub edid: Vec<u8>,
    pub render_adapter: Option<String>,
}

impl Profile {
    pub fn load(directory: &Path) -> io::Result<Self> {
        let configuration = read_bounded(&directory.join("vdd_settings.xml"), PROFILE_SIZE);
        let (mut profile, custom_edid) = match configuration {
            Ok(bytes) => Self::parse(&decode_xml(&bytes)?)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let legacy = read_bounded(&directory.join("option.txt"), PROFILE_SIZE);
                let (mut profile, custom_edid) = Self::parse(DEFAULT_PROFILE)?;
                match legacy {
                    Ok(bytes) => profile.apply_legacy_modes(&decode_xml(&bytes)?)?,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                (profile, custom_edid)
            }
            Err(error) => return Err(error),
        };

        if custom_edid {
            match read_bounded(&directory.join("user_edid.bin"), MAX_EDID_BYTES) {
                Ok(bytes) if valid_edid(&bytes) => profile.edid = bytes,
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }

        if profile.render_adapter.is_none() {
            match read_bounded(&directory.join("adapter.txt"), 4096) {
                Ok(bytes) => {
                    profile.render_adapter = String::from_utf8_lossy(&bytes)
                        .lines()
                        .next()
                        .and_then(adapter_name);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }

        Ok(profile)
    }

    fn apply_legacy_modes(&mut self, options: &str) -> io::Result<()> {
        let mut lines = options.lines().map(str::trim);
        let Some(count) = lines.next().filter(|line| !line.is_empty()) else {
            return Ok(());
        };
        let count = parse_monitor_count(count)?;
        let mut seen = BTreeSet::new();
        let mut modes = Vec::new();
        for line in lines.filter(|line| !line.starts_with('#')) {
            let mut fields = line.split(',').map(str::trim);
            let (Some(width), Some(height), Some(refresh), None) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let mode = parse_mode(
                width.parse().map_err(invalid)?,
                height.parse().map_err(invalid)?,
                parse_rate(refresh)?,
            )?;
            if seen.insert(mode) {
                modes.push(mode);
            }
        }
        if modes.is_empty() {
            return Err(invalid(
                "Legacy virtual display configuration defines no modes",
            ));
        }
        self.monitor_count = count;
        self.modes = modes;
        Ok(())
    }

    fn parse(xml: &str) -> io::Result<(Self, bool)> {
        let document = Document::parse(xml).map_err(invalid)?;
        let root = document.root_element();
        if !root.has_tag_name("vdd_settings") {
            return Err(invalid("Invalid virtual display configuration root"));
        }

        let monitor_count = parse_monitor_count(
            required(required(root, "monitors")?, "count")?
                .text()
                .unwrap_or_default(),
        )?;

        let mut dimensions = BTreeSet::new();
        let mut modes = Vec::new();
        let mut seen_modes = BTreeSet::new();
        let mut add_mode = |width, height, refresh| -> io::Result<()> {
            let mode = parse_mode(width, height, refresh)?;
            if seen_modes.insert(mode) {
                modes.push(mode);
            }
            Ok(())
        };

        for resolution in required(root, "resolutions")?
            .children()
            .filter(|node| node.has_tag_name("resolution"))
        {
            let width = parse_field::<u32>(resolution, "width")?;
            let height = parse_field::<u32>(resolution, "height")?;
            if width == 0 || height == 0 {
                return Err(invalid("Virtual display dimensions must be positive"));
            }
            dimensions.insert((width, height));
            for rate in resolution
                .children()
                .filter(|node| node.has_tag_name("refresh_rate"))
            {
                add_mode(width, height, parse_rate(rate.text().unwrap_or_default())?)?;
            }
        }

        let mut global_rates = BTreeSet::new();
        if let Some(global) = child(root, "global")? {
            for rate in global
                .children()
                .filter(|node| node.has_tag_name("g_refresh_rate"))
            {
                global_rates.insert(parse_rate(rate.text().unwrap_or_default())?);
            }
        }
        // The XML size alone does not bound the number of modes after this product.
        if dimensions.len().saturating_mul(global_rates.len()) > PROFILE_SIZE {
            return Err(invalid("Virtual display mode expansion is too large"));
        }
        for rate in global_rates {
            for &(width, height) in &dimensions {
                add_mode(width, height, rate)?;
            }
        }
        if modes.is_empty() {
            return Err(invalid("Virtual display configuration defines no modes"));
        }

        let render_adapter = child(root, "gpu")?
            .map(|gpu| child(gpu, "friendlyname"))
            .transpose()?
            .flatten()
            .and_then(|node| node.text())
            .and_then(adapter_name);
        let colour = Colour {
            format: match option(root, "colour", "ColourFormat")? {
                Some("YCbCr444") => 1,
                Some("YCbCr422") => 2,
                Some("YCbCr420") => 3,
                _ => 0,
            },
            sdr_bits: if enabled(root, "colour", "SDR10bit")? {
                10
            } else {
                8
            },
            hdr_bits: if enabled(root, "colour", "HDRPlus")? {
                12
            } else {
                10
            },
        };
        let custom_edid = enabled(root, "edid", "CustomEdid")?;

        Ok((
            Self {
                monitor_count,
                modes,
                colour,
                edid: DEFAULT_EDID.into(),
                render_adapter,
            },
            custom_edid,
        ))
    }
}

fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

fn decode_xml(data: &[u8]) -> io::Result<Cow<'_, str>> {
    let (bytes, get_unit) = match data {
        [0xff, 0xfe, bytes @ ..] => (bytes, u16::from_le_bytes as fn([u8; 2]) -> u16),
        [0xfe, 0xff, bytes @ ..] => (bytes, u16::from_be_bytes as fn([u8; 2]) -> u16),
        bytes => {
            let text = std::str::from_utf8(bytes).map_err(invalid)?;
            return Ok(Cow::Borrowed(text.trim_start_matches('\u{feff}')));
        }
    };
    let (units, remainder) = bytes.as_chunks();
    if !remainder.is_empty() {
        return Err(invalid("Incomplete UTF-16 virtual display profile"));
    }
    let text: Result<String, _> = char::decode_utf16(units.iter().copied().map(get_unit)).collect();
    text.map(Cow::Owned).map_err(invalid)
}

fn read_bounded(path: &Path, max: usize) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    File::open(path)?
        .take(max as u64 + 1)
        .read_to_end(&mut data)?;
    if data.len() > max {
        return Err(invalid("Virtual display configuration file is too large"));
    }
    Ok(data)
}

fn child<'a, 'input>(parent: Node<'a, 'input>, name: &str) -> io::Result<Option<Node<'a, 'input>>> {
    let mut children = parent.children().filter(|node| node.has_tag_name(name));
    let first = children.next();
    if children.next().is_some() {
        return Err(invalid(format!("Duplicate virtual display setting {name}")));
    }
    Ok(first)
}

fn required<'a, 'input>(node: Node<'a, 'input>, name: &str) -> io::Result<Node<'a, 'input>> {
    child(node, name)?.ok_or_else(|| invalid(format!("Missing virtual display setting {name}")))
}

fn option<'a>(root: Node<'a, '_>, section: &str, name: &str) -> io::Result<Option<&'a str>> {
    let Some(section) = child(root, section)? else {
        return Ok(None);
    };
    Ok(child(section, name)?
        .and_then(|node| node.text())
        .map(str::trim))
}

fn enabled(root: Node<'_, '_>, section: &str, name: &str) -> io::Result<bool> {
    Ok(option(root, section, name)?.is_some_and(|value| value.eq_ignore_ascii_case("true")))
}

fn parse_field<T>(root: Node<'_, '_>, name: &str) -> io::Result<T>
where
    T: std::str::FromStr,
    T::Err: ToString,
{
    required(root, name)?
        .text()
        .unwrap_or_default()
        .trim()
        .parse()
        .map_err(invalid)
}

fn parse_monitor_count(value: &str) -> io::Result<u32> {
    let count = value.trim().parse::<u32>().map_err(invalid)?;
    if count == 0 {
        return Err(invalid("Virtual display count must be positive"));
    }
    Ok(count)
}

fn parse_mode(width: u32, height: u32, refresh: u32) -> io::Result<Mode> {
    let mode = Mode {
        width,
        height,
        refresh,
    };
    if mode.signal(false).is_none() || mode.signal(true).is_none() {
        return Err(invalid(
            "Virtual display signal is outside Windows field ranges",
        ));
    }
    Ok(mode)
}

fn parse_rate(value: &str) -> io::Result<u32> {
    let rate: f64 = value.trim().parse().map_err(invalid)?;
    let units = (rate * 10_000.).round();
    if !(1.0..=f64::from(u32::MAX)).contains(&units) {
        return Err(invalid("Invalid virtual display refresh rate"));
    }
    Ok(units as u32)
}

fn adapter_name(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("default") {
        None
    } else {
        Some(value.to_owned())
    }
}

fn valid_edid(data: &[u8]) -> bool {
    data.starts_with(&[0, 255, 255, 255, 255, 255, 255, 0])
        && data
            .get(126)
            .is_some_and(|count| (usize::from(*count) + 1) * 128 == data.len())
        && data
            .as_chunks::<128>()
            .0
            .iter()
            .all(|block| block.iter().copied().fold(0u8, u8::wrapping_add) == 0)
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_PROFILE, Mode, Profile, decode_xml, valid_edid};

    #[test]
    fn compatibility_profile_expands_and_deduplicates_rates() {
        let xml = DEFAULT_PROFILE.replace(
            "<refresh_rate>30</refresh_rate>",
            "<refresh_rate>59.9401</refresh_rate><refresh_rate>60</refresh_rate>",
        );
        let (profile, custom_edid) = Profile::parse(&xml).unwrap();
        assert_eq!(profile.monitor_count, 1);
        assert_eq!(profile.modes.len(), 35);
        assert!(profile.modes.contains(&Mode {
            width: 1920,
            height: 1080,
            refresh: 599_401
        }));
        assert_eq!(
            (
                profile.colour.format,
                profile.colour.sdr_bits,
                profile.colour.hdr_bits
            ),
            (0, 8, 10)
        );
        assert!(profile.render_adapter.is_none());
        assert!(!custom_edid);
        assert!(valid_edid(&profile.edid));
    }

    #[test]
    fn rejects_invalid_monitor_count_and_mode() {
        assert!(Profile::parse(&DEFAULT_PROFILE.replace("<count>1", "<count>0")).is_err());
        assert!(Profile::parse(&DEFAULT_PROFILE.replace("<width>800", "<width>0")).is_err());
        assert!(
            Profile::parse(&DEFAULT_PROFILE.replace("<refresh_rate>30", "<refresh_rate>NaN"))
                .is_err()
        );
    }

    #[test]
    fn preserves_utf16_xml_and_legacy_mode_profiles() {
        let bytes: Vec<_> = [0xff, 0xfe]
            .into_iter()
            .chain(DEFAULT_PROFILE.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let (mut profile, _) = Profile::parse(&decode_xml(&bytes).unwrap()).unwrap();
        profile
            .apply_legacy_modes("2\n# mode\n1920,1080,59.9401\n")
            .unwrap();
        assert_eq!(profile.monitor_count, 2);
        assert_eq!(
            profile.modes,
            [Mode {
                width: 1920,
                height: 1080,
                refresh: 599_401
            }]
        );
    }

    #[test]
    fn uses_packaged_profile_when_installation_xml_is_absent() {
        let missing =
            std::env::temp_dir().join(format!("xscc-absent-driver-profile-{}", std::process::id()));
        let profile = Profile::load(&missing).unwrap();
        assert_eq!(profile.monitor_count, 1);
        assert_eq!(profile.modes.len(), 35);
    }
}
