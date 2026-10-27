use roxmltree::{Document, Node};
use std::{collections::BTreeSet, error::Error, io, ops::Range};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh_units: u32,
}

impl Mode {
    pub fn new(width: u32, height: u32, refresh_units: u32) -> Result<Self> {
        if width == 0
            || height == 0
            || width > i32::MAX as u32
            || height > i32::MAX as u32
            || refresh_units == 0
            || refresh_units > i32::MAX as u32
        {
            return Err(invalid("Invalid VDD mode dimensions or refresh rate").into());
        }
        Ok(Self {
            width,
            height,
            refresh_units,
        })
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn child<'a, 'input>(parent: Node<'a, 'input>, name: &str) -> Result<Node<'a, 'input>> {
    let mut children = parent.children().filter(|node| node.has_tag_name(name));
    let value = children
        .next()
        .ok_or_else(|| invalid("Required VDD configuration field is missing"))?;
    if children.next().is_some() {
        return Err(invalid("Duplicate VDD configuration field").into());
    }
    Ok(value)
}

fn number(node: Node<'_, '_>) -> Result<u32> {
    Ok(node
        .text()
        .ok_or_else(|| invalid("Empty VDD numeric field"))?
        .trim()
        .parse()?)
}

fn rate(text: &str) -> Result<u32> {
    let value: f64 = text.trim().parse()?;
    let units = (value * 10000.).round();
    if !value.is_finite() || units < 1. || units > i32::MAX as f64 {
        return Err(invalid("Invalid VDD refresh rate").into());
    }
    Ok(units as u32)
}

pub(super) struct Profile {
    xml: String,
    count: Range<usize>,
    resolutions: Range<usize>,
    global_rates: Vec<Range<usize>>,
    explicit_modes: BTreeSet<Mode>,
    pub modes: BTreeSet<Mode>,
}

impl Profile {
    pub fn parse(xml: String) -> Result<Self> {
        if xml.len() > 1024 * 1024 {
            return Err(invalid("VDD configuration exceeds 1 MiB").into());
        }
        let document = Document::parse(&xml)?;
        let root = document.root_element();
        if !root.has_tag_name("vdd_settings") {
            return Err(invalid("Invalid VDD configuration root").into());
        }
        let count = child(child(root, "monitors")?, "count")?;
        if number(count)? == 0 {
            return Err(invalid("VDD XML requires a positive monitor count").into());
        }
        let resolutions = child(root, "resolutions")?;
        let mut modes = BTreeSet::new();
        let mut dimensions = BTreeSet::new();
        for resolution in resolutions
            .children()
            .filter(|node| node.has_tag_name("resolution"))
        {
            let width = number(child(resolution, "width")?)?;
            let height = number(child(resolution, "height")?)?;
            if width == 0 || height == 0 {
                return Err(invalid("Invalid VDD dimensions").into());
            }
            dimensions.insert((width, height));
            for refresh in resolution
                .children()
                .filter(|node| node.has_tag_name("refresh_rate"))
            {
                let refresh = rate(
                    refresh
                        .text()
                        .ok_or_else(|| invalid("Empty VDD refresh rate"))?,
                )?;
                modes.insert(Mode::new(width, height, refresh)?);
            }
        }
        let explicit_modes = modes.clone();
        let mut global_rates = Vec::new();
        let globals: Vec<_> = root
            .children()
            .filter(|node| node.has_tag_name("global"))
            .collect();
        if globals.len() > 1 {
            return Err(invalid("Duplicate VDD global settings").into());
        }
        if let Some(global) = globals.first() {
            for refresh in global
                .children()
                .filter(|node| node.has_tag_name("g_refresh_rate"))
            {
                global_rates.push(refresh.range());
                let refresh = rate(
                    refresh
                        .text()
                        .ok_or_else(|| invalid("Empty global VDD rate"))?,
                )?;
                for &(width, height) in &dimensions {
                    modes.insert(Mode::new(width, height, refresh)?);
                }
            }
        }
        let count = count.range();
        let resolutions = resolutions.range();
        drop(document);
        Ok(Self {
            xml,
            count,
            resolutions,
            global_rates,
            explicit_modes,
            modes,
        })
    }

    pub fn render(&self, count: u32, additional: impl IntoIterator<Item = Mode>) -> Result<String> {
        if count == 0 {
            return Err(invalid("Disable the VDD device to remove its last monitor").into());
        }
        let mut modes = self.modes.clone();
        modes.extend(additional);
        if modes.is_empty() {
            return Err(invalid("No declared VDD modes are available").into());
        }
        let mut additions = String::new();
        for mode in modes.difference(&self.explicit_modes) {
            Mode::new(mode.width, mode.height, mode.refresh_units)?;
            let refresh = format!(
                "{}.{:04}",
                mode.refresh_units / 10000,
                mode.refresh_units % 10000
            );
            additions.push_str(&format!("<resolution><width>{}</width><height>{}</height><refresh_rate>{refresh}</refresh_rate></resolution>\n", mode.width, mode.height));
        }
        let mut resolutions = self.xml[self.resolutions.clone()].to_owned();
        if resolutions.ends_with("/>") {
            resolutions.truncate(resolutions.len() - 2);
            resolutions.push('>');
            resolutions.push_str(&additions);
            resolutions.push_str("</resolutions>");
        } else {
            let closing = resolutions
                .rfind("</")
                .ok_or_else(|| invalid("Invalid VDD resolutions closing tag"))?;
            resolutions.insert_str(closing, &additions);
        }
        let mut changes = vec![
            (self.count.clone(), format!("<count>{count}</count>")),
            (self.resolutions.clone(), resolutions),
        ];
        for rate in &self.global_rates {
            changes.push((rate.clone(), String::new()));
        }
        changes.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        let mut xml = self.xml.clone();
        for (range, value) in changes {
            xml.replace_range(range, &value);
        }
        if xml.len() > 1024 * 1024 {
            return Err(invalid("Complete VDD mode profile exceeds 1 MiB").into());
        }
        Ok(xml)
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, Profile};

    const XML: &str = r#"<vdd_settings><monitors><count>1</count></monitors><global><unknown>preserve</unknown><g_refresh_rate>60</g_refresh_rate><g_refresh_rate>120</g_refresh_rate></global><resolutions><resolution custom="preserve"><unknown>keep</unknown><width>2560</width><height>1440</height><refresh_rate>59.9401</refresh_rate></resolution></resolutions><options><HDRPlus>false</HDRPlus><custom>keep-me</custom></options></vdd_settings>"#;

    #[test]
    fn merges_all_hardware_and_declared_modes_without_spreading_old_global_rates() {
        let profile = Profile::parse(XML.into()).unwrap();
        let xml = profile
            .render(8, [Mode::new(7680, 4320, 1440000).unwrap()])
            .unwrap();
        let result = Profile::parse(xml.clone()).unwrap();
        for rate in [599401, 600000, 1200000] {
            assert!(result.modes.contains(&Mode::new(2560, 1440, rate).unwrap()));
        }
        assert!(result
            .modes
            .contains(&Mode::new(7680, 4320, 1440000).unwrap()));
        assert!(!result
            .modes
            .contains(&Mode::new(7680, 4320, 1200000).unwrap()));
        assert!(xml.contains("<count>8</count>") && xml.contains("<custom>keep-me</custom>"));
        assert!(
            xml.contains("<unknown>preserve</unknown>")
                && xml.contains("<resolution custom=\"preserve\"><unknown>keep</unknown>")
        );
    }

    #[test]
    fn retains_more_than_ten_modes_and_reports_invalid_configuration() {
        let profile = Profile::parse(XML.into()).unwrap();
        let modes = (1..=64).map(|n| Mode::new(640 + n * 16, 480 + n * 8, 2400000).unwrap());
        let result = Profile::parse(profile.render(12, modes).unwrap()).unwrap();
        assert_eq!(result.modes.len(), 67);
        assert!(Profile::parse(XML.replace("<width>2560</width>", "<width>-1</width>")).is_err());
        assert!(Profile::parse(XML.replace("59.9401", "NaN")).is_err());
        assert!(profile.render(0, []).is_err());
    }

    #[test]
    fn does_not_overwrite_duplicate_or_unparseable_settings() {
        assert!(Profile::parse(
            XML.replace("<count>1</count>", "<count>1</count><count>2</count>")
        )
        .is_err());
        assert!(Profile::parse("<vdd_settings>".into()).is_err());
    }
}
