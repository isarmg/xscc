//! Native process lifecycle for the separate GPUI applications.
use hbb_common::config;

pub fn is_host() -> bool {
    cfg!(feature = "gpui-host")
}

fn prepare() -> Result<(), String> {
    *config::APP_NAME.write().unwrap() = if is_host() {
        "XSCC Host"
    } else {
        "XSCC Controller"
    }
    .into();
    if !is_host() {
        config::HARD_SETTINGS
            .write()
            .unwrap()
            .insert("conn-type".into(), "outgoing".into());
    }
    if !crate::common::global_init() {
        return Err("Native initialization failed".into());
    }
    #[cfg(windows)]
    if !crate::platform::windows::bootstrap() {
        return Err("Windows 11 x64 initialization failed".into());
    }
    Ok(())
}

pub fn initialize() -> Result<(), String> {
    prepare()?;
    #[cfg(all(windows, feature = "gpui-host"))]
    {
        windows_host::installed_executable().map_err(|e| e.to_string())?;
        if !crate::platform::windows::is_elevated(None).map_err(|e| e.to_string())? {
            return Err("Open XSCC Host as administrator to configure its system service".into());
        }
        if !crate::platform::windows::is_service_running("XSCC Host") {
            return Err("XSCC Host service is not running. Start it in Windows Services or reinstall XSCC Host".into());
        }
        // Never create a second, user-owned host manager when the system owner is absent.
        return Ok(());
    }
    #[cfg(not(all(windows, feature = "gpui-host")))]
    {
        std::thread::Builder::new()
            .name("xscc-native-owner".into())
            .spawn(|| crate::start_server(false, false))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(any(feature = "hwcodec", test))]
fn is_stdout_codec_probe(arguments: &[String]) -> bool {
    arguments == ["--check-hwcodec-config", "--stdout"]
}

pub fn handle_background_command() -> bool {
    #[cfg(feature = "hwcodec")]
    {
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        if arguments.first().map(String::as_str) == Some("--check-hwcodec-config") {
            if !is_stdout_codec_probe(&arguments) {
                eprintln!("Hardware codec probe requires only --check-hwcodec-config --stdout");
                std::process::exit(2);
            }
            #[cfg(windows)]
            if !crate::platform::windows::bootstrap() {
                eprintln!("Windows codec probe initialization failed");
                std::process::exit(1);
            }
            println!(
                "{}{}",
                crate::client::hwcodec::OUTPUT_PREFIX,
                scrap::hwcodec::check_available_hwcodec()
            );
            return true;
        }
    }
    let Some(argument) = std::env::args().nth(1) else {
        return false;
    };
    if !matches!(
        argument.as_str(),
        "--server" | "--service" | "--install-service-only" | "--uninstall-service-only"
    ) {
        return false;
    }
    if let Err(error) = prepare() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    hbb_common::init_log(false, &argument[2..]);
    #[cfg(all(windows, feature = "gpui-host"))]
    {
        let result = match argument.as_str() {
            "--install-service-only" => windows_host::install(),
            "--uninstall-service-only" => windows_host::uninstall(),
            "--service" | "--server" => windows_host::run(&argument),
            _ => unreachable!(),
        };
        if let Err(error) = result {
            eprintln!("{error}");
            hbb_common::log::error!("{error}");
            std::process::exit(1);
        }
        return true;
    }
    #[cfg(not(all(windows, feature = "gpui-host")))]
    {
        if argument == "--server" {
            crate::start_server(true, false);
            true
        } else {
            eprintln!("System-service commands are only available in the Windows host package");
            std::process::exit(1);
        }
    }
}

#[cfg(all(windows, feature = "gpui-host"))]
pub(crate) mod windows_host {
    use hbb_common::{bail, ResultType};
    use std::{
        ffi::OsString,
        path::PathBuf,
        time::{Duration, Instant},
    };
    use windows::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath, KF_FLAG_DEFAULT},
    };
    use windows_service::{
        service::{
            Service, ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType,
            ServiceState, ServiceType,
        },
        service_manager::{ServiceManager, ServiceManagerAccess},
    };

    const NAME: &str = "XSCC Host";

    pub(crate) fn installed_executable() -> ResultType<PathBuf> {
        let folder =
            unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, None) }?;
        let converted = unsafe { folder.to_string() };
        unsafe { CoTaskMemFree(Some(folder.0.cast())) };
        let expected = PathBuf::from(converted?)
            .join("XSCC-Host")
            .join("xscc-host.exe");
        let current = std::fs::canonicalize(std::env::current_exe()?)?;
        let expected = std::fs::canonicalize(expected)?;
        if !current
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy())
        {
            bail!("XSCC Host must run from its protected Program Files installation");
        }
        Ok(current)
    }

    pub(crate) fn verify_owner(pid: u32) -> ResultType<()> {
        installed_executable()?;
        if !crate::platform::windows::is_process_running_as_system(pid)? {
            bail!("XSCC Host management owner is not SYSTEM");
        }
        Ok(())
    }

    pub(crate) fn verify_client(pid: u32) -> ResultType<()> {
        installed_executable()?;
        if !crate::platform::windows::is_root()
            || !crate::platform::windows::is_elevated(Some(pid))?
        {
            bail!("XSCC Host configuration requires an elevated administrator");
        }
        Ok(())
    }

    pub fn run(argument: &str) -> ResultType<()> {
        installed_executable()?;
        if !crate::platform::windows::is_root() {
            bail!("XSCC Host background processes must be started by the SYSTEM service");
        }
        if argument == "--service" {
            crate::start_os_service();
        } else {
            crate::start_server(true, false);
        }
        Ok(())
    }

    fn manager(create: bool) -> ResultType<ServiceManager> {
        installed_executable()?;
        if !crate::platform::windows::is_elevated(None)? {
            bail!("Service installation requires administrator privileges");
        }
        let access = ServiceManagerAccess::CONNECT
            | if create {
                ServiceManagerAccess::CREATE_SERVICE
            } else {
                ServiceManagerAccess::empty()
            };
        Ok(ServiceManager::local_computer(None::<&str>, access)?)
    }

    fn verify_service(service: &Service) -> ResultType<()> {
        let configuration = service.query_config()?;
        let executable = installed_executable()?;
        // Service configuration contains the complete quoted command, including arguments.
        let command = format!("\"{}\" --service", executable.display());
        let actual = configuration.executable_path.to_string_lossy();
        let command_without_prefix = command.replace(r"\\?\", "");
        if !actual.eq_ignore_ascii_case(&command)
            && !actual.eq_ignore_ascii_case(&command_without_prefix)
        {
            bail!("An XSCC Host service with a different executable already exists; refusing to modify it");
        }
        Ok(())
    }

    pub fn install() -> ResultType<()> {
        let manager = manager(true)?;
        let access =
            ServiceAccess::QUERY_CONFIG | ServiceAccess::QUERY_STATUS | ServiceAccess::START;
        let service = match manager.open_service(NAME, access) {
            Ok(service) => {
                verify_service(&service)?;
                service
            }
            Err(windows_service::Error::Winapi(error)) if error.raw_os_error() == Some(1060) => {
                manager.create_service(
                    &ServiceInfo {
                        name: NAME.into(),
                        display_name: NAME.into(),
                        service_type: ServiceType::OWN_PROCESS,
                        start_type: ServiceStartType::AutoStart,
                        error_control: ServiceErrorControl::Normal,
                        executable_path: installed_executable()?,
                        launch_arguments: vec![OsString::from("--service")],
                        dependencies: vec![],
                        account_name: None,
                        account_password: None,
                    },
                    access,
                )?
            }
            Err(error) => return Err(error.into()),
        };
        if service.query_status()?.current_state != ServiceState::Running {
            service.start::<&str>(&[])?;
        }
        wait_state(&service, ServiceState::Running)
    }

    pub fn uninstall() -> ResultType<()> {
        let manager = manager(false)?;
        let service = match manager.open_service(
            NAME,
            ServiceAccess::QUERY_CONFIG
                | ServiceAccess::QUERY_STATUS
                | ServiceAccess::STOP
                | ServiceAccess::DELETE,
        ) {
            Ok(service) => service,
            Err(windows_service::Error::Winapi(error)) if error.raw_os_error() == Some(1060) => {
                return Ok(())
            }
            Err(error) => return Err(error.into()),
        };
        verify_service(&service)?;
        if service.query_status()?.current_state != ServiceState::Stopped {
            service.stop()?;
            wait_state(&service, ServiceState::Stopped)?;
        }
        service.delete()?;
        Ok(())
    }

    fn wait_state(service: &Service, desired: ServiceState) -> ResultType<()> {
        let deadline = Instant::now() + Duration::from_secs(30);
        while service.query_status()?.current_state != desired {
            if Instant::now() >= deadline {
                bail!("Timed out waiting for XSCC Host service");
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Ok(())
    }
}

#[cfg(all(test, windows, feature = "gpui-host"))]
mod tests {
    #[test]
    fn rejects_unverifiable_service_owner() {
        assert!(super::windows_host::verify_owner(u32::MAX).is_err());
    }

    #[test]
    fn rejects_unverifiable_configuration_client() {
        assert!(super::windows_host::verify_client(u32::MAX).is_err());
    }
}

#[cfg(test)]
mod codec_probe_tests {
    use super::is_stdout_codec_probe;

    #[test]
    fn native_codec_probe_accepts_only_explicit_stdout_mode() {
        let args = |values: &[&str]| {
            values
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>()
        };
        assert!(is_stdout_codec_probe(&args(&[
            "--check-hwcodec-config",
            "--stdout"
        ])));
        assert!(!is_stdout_codec_probe(&args(&["--check-hwcodec-config"])));
        assert!(!is_stdout_codec_probe(&args(&[
            "--check-hwcodec-config",
            "--stdout",
            "--server"
        ])));
        assert!(!is_stdout_codec_probe(&args(&["--server", "--stdout"])));
    }
}
