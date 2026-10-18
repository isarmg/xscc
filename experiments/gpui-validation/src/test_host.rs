//! Explicit, process-local ownership of the temporary test listener.
#[path = "../../../src/test_lan/address.rs"]
mod address;

use std::{
    io::{BufRead, BufReader, Read},
    net::SocketAddr,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Output {
    ready: Option<SocketAddr>,
    last_line: String,
}

pub struct TestHost {
    child: Option<Child>,
    #[cfg(target_os = "windows")]
    job: Option<std::os::windows::io::OwnedHandle>,
    output: Arc<Mutex<Output>>,
    pub message: String,
    pub address: Option<SocketAddr>,
}

impl Default for TestHost {
    fn default() -> Self {
        Self {
            child: None,
            #[cfg(target_os = "windows")]
            job: None,
            output: Arc::new(Mutex::new(Output::default())),
            message: "未启动。手机只输入 IP 时默认连接 21119 端口。".into(),
            address: None,
        }
    }
}

impl TestHost {
    pub fn active(&self) -> bool {
        self.child.is_some()
    }

    pub fn start(&mut self, input: &str) {
        if self.active() {
            return;
        }
        let result = self.spawn(input);
        if let Err(error) = result {
            self.message = format!("启动失败：{error}");
        }
    }

    fn spawn(&mut self, input: &str) -> Result<(), String> {
        let bind = address::endpoint(if input.trim().is_empty() {
            "0.0.0.0:21119"
        } else {
            input
        })?;
        let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
        command
            .arg("--test-lan-host-ui")
            .env("XSCC_TEST_LAN_BIND", bind.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW; status stays in the owning UI.
        }
        self.child = Some(command.spawn().map_err(|e| e.to_string())?);
        #[cfg(target_os = "windows")]
        if let Some(child) = self.child.as_ref() {
            match librustdesk::gpui_host::guard_test_direct_child(child) {
                Ok(job) => self.job = Some(job),
                Err(error) => {
                    self.stop();
                    return Err(format!("无法保护测试监听进程的退出清理：{error}"));
                }
            }
        }
        let stderr = self
            .child
            .as_mut()
            .and_then(|child| child.stderr.take())
            .ok_or("无法读取测试监听进程的状态")?;
        self.output = Arc::new(Mutex::new(Output::default()));
        let output = self.output.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("test-listener-status".into())
            .spawn(move || {
                let mut reader = BufReader::new(stderr);
                loop {
                    let mut bytes = Vec::new();
                    match reader.by_ref().take(4096).read_until(b'\n', &mut bytes) {
                        Ok(0) => break,
                        Ok(_) => {
                            let line = String::from_utf8_lossy(&bytes).trim().to_owned();
                            let mut output = output.lock().unwrap();
                            if let Some(address) = ready_address(&line) {
                                output.ready = Some(address);
                            }
                            if !line.is_empty() {
                                output.last_line = line;
                            }
                        }
                        Err(error) => {
                            output.lock().unwrap().last_line = format!("读取状态失败：{error}");
                            break;
                        }
                    }
                }
            })
        {
            self.stop();
            return Err(error.to_string());
        }
        self.address = None;
        self.message = format!("正在启动测试监听：{bind}…");
        Ok(())
    }

    pub fn poll(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.child = None;
                #[cfg(target_os = "windows")]
                {
                    self.job = None;
                }
                self.address = None;
                self.message = format!(
                    "测试监听已退出（{status}）：{}",
                    self.output.lock().unwrap().last_line
                );
            }
            Ok(None) => {
                if let Some(address) = self.output.lock().unwrap().ready {
                    self.address = Some(address);
                    self.message = format!("测试监听已开启：{address}");
                }
            }
            Err(error) => self.message = format!("无法检查测试监听状态：{error}"),
        }
    }

    pub fn stop(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        let result = (|| -> std::io::Result<()> {
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            child.wait()?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.child = None;
                #[cfg(target_os = "windows")]
                {
                    self.job = None;
                }
                self.address = None;
                self.message = "测试监听已停止。".into();
            }
            Err(error) => self.message = format!("停止失败，请关闭测试进程：{error}"),
        }
    }
}

impl Drop for TestHost {
    fn drop(&mut self) {
        self.stop();
        if self.active() {
            eprintln!("{}", self.message);
        }
    }
}

fn ready_address(line: &str) -> Option<SocketAddr> {
    line.strip_prefix("XSCC_TEST_DIRECT_READY=")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_bound_address_reports_ready() {
        assert_eq!(
            ready_address("XSCC_TEST_DIRECT_READY=0.0.0.0:21119"),
            Some("0.0.0.0:21119".parse().unwrap())
        );
        assert_eq!(
            ready_address("XSCC_TEST_DIRECT_READY=[::]:12345"),
            Some("[::]:12345".parse().unwrap())
        );
        assert!(ready_address("TEST ONLY: starting").is_none());
        assert!(ready_address("XSCC_TEST_DIRECT_READY=invalid").is_none());
    }

    #[test]
    fn default_owner_never_starts_listener_and_invalid_input_fails_without_spawn() {
        let mut host = TestHost::default();
        assert!(!host.active());
        assert!(host.address.is_none());
        host.start("invalid host");
        assert!(!host.active());
        assert!(host.message.starts_with("启动失败"));
        host.stop();
    }
}
