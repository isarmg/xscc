//! Local Host management uses the same authenticated owner as the native UI.
use base::session_protocol::virtual_display::{MAX_JSON_BYTES, VirtualDisplayConfiguration};

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Status,
    Configure(VirtualDisplayConfiguration),
}

impl Command {
    pub fn parse(arguments: &[String]) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        let Some(first) = arguments.first() else {
            return Ok(None);
        };
        if !first.starts_with("--virtual-display-") {
            return Ok(None);
        }
        match (first.as_str(), &arguments[1..]) {
            ("--virtual-display-status", []) => Ok(Some(Self::Status)),
            ("--virtual-display-configure", [json]) => {
                if json.len() > MAX_JSON_BYTES {
                    return Err("Virtual display configuration exceeds the message bound".into());
                }
                let configuration: VirtualDisplayConfiguration = serde_json::from_str(json)?;
                configuration.validate()?;
                Ok(Some(Self::Configure(configuration)))
            }
            _ => {
                Err("Use only --virtual-display-status or --virtual-display-configure JSON".into())
            }
        }
    }

    #[cfg(all(target_os = "windows", feature = "native-host"))]
    pub async fn execute(
        &self,
    ) -> Result<librustdesk::gpui_management::VirtualDisplayState, Box<dyn std::error::Error>> {
        let state = match self {
            Self::Status => librustdesk::gpui_management::virtual_display_status().await?,
            Self::Configure(configuration) => {
                librustdesk::gpui_management::configure_virtual_displays(configuration.clone())
                    .await?
            }
        };
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Option<Command>, Box<dyn std::error::Error>> {
        Command::parse(
            &arguments
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn exact_commands_reject_mixed_missing_and_unknown_arguments() {
        assert_eq!(
            parse(&["--virtual-display-status"]).unwrap(),
            Some(Command::Status)
        );
        assert_eq!(parse(&["--diagnostic"]).unwrap(), None);
        for arguments in [
            vec!["--virtual-display-status", "--diagnostic"],
            vec!["--virtual-display-configure"],
            vec!["--virtual-display-unknown"],
        ] {
            assert!(parse(&arguments).is_err());
        }
    }

    #[test]
    fn configuration_keeps_high_refresh_precision_and_explicit_removal() {
        for (count, units) in [(1, 2400000), (0, 599401)] {
            let configuration = VirtualDisplayConfiguration {
                count,
                persistent: false,
                modes: vec![
                    base::session_protocol::virtual_display::VirtualDisplayMode {
                        width: 1920,
                        height: 1080,
                        refresh_rate_units: units,
                    },
                ],
            };
            assert_eq!(
                parse(&[
                    "--virtual-display-configure",
                    &serde_json::to_string(&configuration).unwrap()
                ])
                .unwrap(),
                Some(Command::Configure(configuration))
            );
        }
    }

    #[test]
    fn malformed_oversized_or_out_of_range_configuration_never_reaches_the_owner() {
        for json in [
            "not-json".to_owned(),
            r#"{"count":4294967295,"persistent":false,"modes":[]}"#.to_owned(),
            r#"{"count":1,"persistent":false,"modes":[{"width":0,"height":1080,"refresh_rate_units":1200000}]}"#.to_owned(),
            r#"{"count":0,"persistent":false,"modes":[],"unrecognized":true}"#.to_owned(),
            " ".repeat(MAX_JSON_BYTES + 1),
        ] {
            assert!(parse(&["--virtual-display-configure", &json]).is_err());
        }
    }
}
