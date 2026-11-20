use std::ffi::OsStr;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000_u32;

#[cfg(target_os = "windows")]
pub fn new_std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    use std::os::windows::process::CommandExt;

    let mut command = std::process::Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(not(target_os = "windows"))]
pub fn new_std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    std::process::Command::new(program)
}

#[cfg(target_os = "windows")]
pub fn get_powershell() -> Option<String> {
    use std::path::PathBuf;

    fn find_pwsh_in_programfiles(find_alternate: bool, find_preview: bool) -> Option<PathBuf> {
        #[cfg(target_pointer_width = "64")]
        let env_var = if find_alternate {
            "ProgramFiles(x86)"
        } else {
            "ProgramFiles"
        };

        #[cfg(target_pointer_width = "32")]
        let env_var = if find_alternate {
            "ProgramW6432"
        } else {
            "ProgramFiles"
        };

        let install_base_dir = PathBuf::from(std::env::var_os(env_var)?).join("PowerShell");
        install_base_dir
            .read_dir()
            .ok()?
            .filter_map(Result::ok)
            .filter(|entry| matches!(entry.file_type(), Ok(ft) if ft.is_dir()))
            .filter_map(|entry| {
                let dir_name = entry.file_name();
                let dir_name = dir_name.to_string_lossy();

                let version = if find_preview {
                    let dash_index = dir_name.find('-')?;
                    if &dir_name[dash_index + 1..] != "preview" {
                        return None;
                    };
                    dir_name[..dash_index].parse::<u32>().ok()?
                } else {
                    dir_name.parse::<u32>().ok()?
                };

                let exe_path = entry.path().join("pwsh.exe");
                if exe_path.is_file() {
                    Some((version, exe_path))
                } else {
                    None
                }
            })
            .max_by_key(|(version, _)| *version)
            .map(|(_, path)| path)
    }

    fn find_pwsh_in_msix(find_preview: bool) -> Option<PathBuf> {
        let msix_app_dir =
            PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Microsoft\\WindowsApps");
        let package_family_name = if find_preview {
            "Microsoft.PowerShellPreview_8wekyb3d8bbwe"
        } else {
            "Microsoft.PowerShell_8wekyb3d8bbwe"
        };
        let pwsh_exe = msix_app_dir.join(package_family_name).join("pwsh.exe");
        pwsh_exe.exists().then_some(pwsh_exe)
    }

    fn find_pwsh_in_scoop() -> Option<PathBuf> {
        // Scoop can be installed to a custom location; $SCOOP points at the
        // scoop root in that case and defaults to %USERPROFILE%\scoop otherwise.
        let scoop_dir = std::env::var_os("SCOOP").map(PathBuf::from).or_else(|| {
            std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("scoop"))
        })?;
        let pwsh_exe = scoop_dir.join("shims").join("pwsh.exe");
        pwsh_exe.is_file().then_some(pwsh_exe)
    }

    fn find_pwsh_in_dotnet_tools() -> Option<PathBuf> {
        let pwsh_exe =
            PathBuf::from(std::env::var_os("USERPROFILE")?).join(".dotnet\\tools\\pwsh.exe");
        pwsh_exe.is_file().then_some(pwsh_exe)
    }

    fn find_windows_powershell() -> Option<PathBuf> {
        let system_root = PathBuf::from(std::env::var_os("SystemRoot")?);
        let powershell = system_root.join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        powershell.is_file().then_some(powershell)
    }

    static POWERSHELL: std::sync::LazyLock<Option<String>> = std::sync::LazyLock::new(|| {
        let locations = [
            || find_pwsh_in_programfiles(false, false),
            || find_pwsh_in_programfiles(true, false),
            || find_pwsh_in_msix(false),
            || find_pwsh_in_programfiles(false, true),
            || find_pwsh_in_msix(true),
            || find_pwsh_in_programfiles(true, true),
            || find_pwsh_in_scoop(),
            || find_pwsh_in_dotnet_tools(),
            || which::which_global("pwsh.exe").ok(),
            || which::which_global("powershell.exe").ok(),
            || find_windows_powershell(),
        ];

        locations
            .into_iter()
            .find_map(|f| f())
            .map(|p| p.to_string_lossy().trim().to_owned())
            .inspect(|shell| log::info!("Found powershell in: {}", shell))
    });

    (*POWERSHELL).clone()
}
