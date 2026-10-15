use hbb_common::{
    anyhow::{anyhow, bail, Context},
    log, serde_json, ResultType,
};
use scrap::hwcodec::HwCodecConfig;
use std::{
    io::Read,
    process::{Child, Command, Stdio},
    sync::Once,
    time::{Duration, Instant},
};

pub(crate) const OUTPUT_PREFIX: &str = "RUSTDESK_HWCODEC_CONFIG=";

pub(super) fn ensure_config() {
    if !scrap::codec::enable_hwcodec_option() {
        return;
    }
    static ONCE: Once = Once::new();
    ONCE.call_once(|| match probe_config() {
        Ok(config) => HwCodecConfig::set(config),
        Err(err) => log::warn!("Local hardware codec probe failed: {err:#}"),
    });
}

fn probe_config() -> ResultType<String> {
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--check-hwcodec-config", "--stdout"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    #[cfg(windows)]
    if !scrap::hwcodec::child_exit_when_parent_exit(child.id()) {
        log::warn!("Could not tie codec probe lifetime to its parent");
    }
    let result = (|| {
        let mut stdout = child.stdout.take().context("Missing codec probe stdout")?;
        // Drain concurrently: native drivers may print enough output to fill the pipe.
        let reader = std::thread::Builder::new().spawn(move || {
            let mut output = Vec::new();
            stdout.read_to_end(&mut output).map(|_| output)
        })?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Ok(None) => break Err(anyhow!("Hardware codec probe timed out")),
                Err(err) => break Err(err.into()),
            }
        };
        if status.is_err() {
            stop_probe(&mut child);
        }
        let output = reader
            .join()
            .map_err(|_| anyhow!("Codec probe reader panicked"))??;
        if !status?.success() {
            bail!("Hardware codec probe exited unsuccessfully");
        }
        parse_output(&output)
    })();
    if result.is_err() {
        stop_probe(&mut child);
    }
    result
}

fn stop_probe(child: &mut Child) {
    if !matches!(child.try_wait(), Ok(Some(_))) {
        hbb_common::allow_err!(child.kill());
        hbb_common::allow_err!(child.wait());
    }
}

fn parse_output(output: &[u8]) -> ResultType<String> {
    let config = output
        .split(|byte| *byte == b'\n')
        .find_map(|line| line.strip_prefix(OUTPUT_PREFIX.as_bytes()))
        .context("Hardware codec probe returned no configuration")?;
    let config = std::str::from_utf8(config)?;
    // Validate before set(), whose legacy IPC path accepts malformed JSON as defaults.
    serde_json::from_str::<HwCodecConfig>(config)?;
    Ok(config.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_probe_accepts_complete_config_amid_driver_output() {
        let config = serde_json::to_string(&HwCodecConfig {
            signature: 7,
            ram_encode: vec![Default::default()],
            ram_decode: vec![Default::default()],
            ..Default::default()
        })
        .unwrap();
        let mut output = b"driver diagnostic: \xff\n".to_vec();
        output.extend_from_slice(format!("{OUTPUT_PREFIX}{config}\nmore diagnostics\n").as_bytes());
        assert_eq!(parse_output(&output).unwrap(), config);
    }

    #[test]
    fn local_probe_rejects_missing_or_invalid_config() {
        assert!(parse_output(b"driver failed\n").is_err());
        assert!(parse_output(format!("{OUTPUT_PREFIX}invalid\n").as_bytes()).is_err());
    }

    #[test]
    fn local_probe_failure_returns_without_host_ipc() {
        // The unit-test executable rejects the probe command instead of producing JSON.
        assert!(probe_config().is_err());
    }
}
