use std::path::PathBuf;

pub struct Options {
    pub diagnostic: bool,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub seconds: Option<u64>,
    pub ivf: Option<PathBuf>,
    pub report: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
}
impl Options {
    pub fn parse() -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_args(std::env::args().skip(1))
    }

    fn from_args(
        arguments: impl IntoIterator<Item = String>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut result = Self::default();
        let mut arguments = arguments.into_iter();
        while let Some(key) = arguments.next() {
            if key == "--help" {
                println!(
                    "xscc-gpui-validation [--diagnostic] [--width N] [--height N] [--fps N] [--seconds 1..600] [--ivf VP9.ivf] [--state-dir /absolute/private/path] [--report new.json]"
                );
                std::process::exit(0);
            }
            if key == "--diagnostic" {
                result.diagnostic = true;
                continue;
            }
            let value = arguments.next().ok_or("each option requires a value")?;
            match key.as_str() {
                "--width" => result.width = value.parse()?,
                "--height" => result.height = value.parse()?,
                "--fps" => result.fps = value.parse()?,
                "--seconds" => result.seconds = Some(value.parse()?),
                "--ivf" => result.ivf = Some(value.into()),
                "--report" => result.report = Some(value.into()),
                "--state-dir" => result.state_dir = Some(value.into()),
                _ => return Err(format!("unknown option: {key}").into()),
            }
        }
        if result.width == 0
            || result.width > 3840
            || result.height == 0
            || result.height > 2160
            || !(1..=120).contains(&result.fps)
            || result
                .seconds
                .is_some_and(|seconds| !(1..=600).contains(&seconds))
        {
            return Err("invalid dimensions, FPS, or duration".into());
        }
        result.diagnostic |=
            result.seconds.is_some() || result.ivf.is_some() || result.report.is_some();
        if result.diagnostic && result.seconds.is_none() {
            result.seconds = Some(15);
        }
        Ok(result)
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            diagnostic: false,
            width: 1280,
            height: 720,
            fps: 60,
            seconds: None,
            ivf: None,
            report: None,
            state_dir: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, Box<dyn std::error::Error>> {
        Options::from_args(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn default_launch_is_idle_and_persistent() {
        let options = parse(&[]).unwrap();
        assert!(!options.diagnostic);
        assert_eq!(options.seconds, None);
    }

    #[test]
    fn explicit_diagnostics_keep_bounded_ci_runs() {
        let options = parse(&["--diagnostic"]).unwrap();
        assert!(options.diagnostic);
        assert_eq!(options.seconds, Some(15));
        let options = parse(&["--seconds", "30"]).unwrap();
        assert!(options.diagnostic);
        assert_eq!(options.seconds, Some(30));
        assert!(parse(&["--seconds", "0"]).is_err());
        assert!(parse(&["--seconds", "601"]).is_err());
    }
}
