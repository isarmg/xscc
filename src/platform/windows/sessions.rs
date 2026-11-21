use hbb_common::{anyhow::Context, bail, ResultType};
use std::{
    ffi::c_void,
    io, mem,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr,
};
use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::HANDLE,
        Security::{
            DuplicateTokenEx, SecurityImpersonation, SetTokenInformation, TokenPrimary,
            TokenSessionId, TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY,
            TOKEN_DUPLICATE, TOKEN_QUERY,
        },
        System::{
            Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock},
            RemoteDesktop::{
                WTSActive, WTSClientProtocolType, WTSEnumerateSessionsW, WTSFreeMemory,
                WTSGetActiveConsoleSessionId, WTSQuerySessionInformationW, WTSQueryUserToken,
                WTSSessionInfoEx, WTSUserName, WTSINFOEXW, WTS_INFO_CLASS,
            },
            Threading::{
                CreateProcessAsUserW, GetCurrentProcess, OpenProcessToken,
                CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS, PROCESS_INFORMATION,
                STARTF_USESHOWWINDOW, STARTUPINFOW,
            },
        },
        UI::WindowsAndMessaging::SW_SHOW,
    },
};

struct SessionMemory(*mut c_void);

impl Drop for SessionMemory {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WTSFreeMemory(self.0) };
        }
    }
}

fn query(session_id: u32, class: WTS_INFO_CLASS) -> Option<(SessionMemory, usize)> {
    if session_id == u32::MAX {
        return None;
    }
    let mut buffer = PWSTR::null();
    let mut size = 0;
    unsafe { WTSQuerySessionInformationW(None, session_id, class, &mut buffer, &mut size) }.ok()?;
    (!buffer.is_null()).then_some((SessionMemory(buffer.0.cast()), size as usize))
}

pub(super) fn console_session() -> u32 {
    unsafe { WTSGetActiveConsoleSessionId() }
}

pub(super) fn current_session(include_rdp: bool) -> u32 {
    let console = console_session();
    if !include_rdp {
        return console;
    }
    let sessions = active_sessions();
    sessions
        .iter()
        .find(|(_, station)| *station == "Console")
        .or_else(|| sessions.last())
        .map_or(console, |(id, _)| *id)
}

pub(super) fn active_sessions() -> Vec<(u32, &'static str)> {
    let (mut sessions, mut count) = (ptr::null_mut(), 0);
    if unsafe { WTSEnumerateSessionsW(None, 0, 1, &mut sessions, &mut count) }.is_err()
        || sessions.is_null()
    {
        return Vec::new();
    }
    let _memory = SessionMemory(sessions.cast());
    unsafe { std::slice::from_raw_parts(sessions, count as usize) }
        .iter()
        .filter_map(|session| {
            if session.State != WTSActive || session.pWinStationName.is_null() {
                return None;
            }
            let name = unsafe { session.pWinStationName.to_string() }.ok()?;
            let name = name.to_ascii_lowercase();
            let station = if name == "console" {
                "Console"
            } else if name.starts_with("ica") {
                "ICA"
            } else if name.starts_with("rdp") || is_rdp(session.SessionId) {
                "RDP"
            } else {
                return None;
            };
            Some((session.SessionId, station))
        })
        .collect()
}

fn is_rdp(session_id: u32) -> bool {
    const RDP_PROTOCOL: u16 = 2;
    query(session_id, WTSClientProtocolType).is_some_and(|(buffer, size)| {
        size >= mem::size_of::<u16>()
            && unsafe { buffer.0.cast::<u16>().read_unaligned() } == RDP_PROTOCOL
    })
}

pub(super) fn username(session_id: u32) -> Option<String> {
    let (buffer, size) = query(session_id, WTSUserName)?;
    if size % mem::size_of::<u16>() != 0 {
        return None;
    }
    let wide = unsafe { std::slice::from_raw_parts(buffer.0.cast::<u16>(), size / 2) };
    let end = wide
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(wide.len());
    Some(String::from_utf16_lossy(&wide[..end]))
}

pub(super) fn is_locked(session_id: u32) -> Option<bool> {
    let (buffer, size) = query(session_id, WTSSessionInfoEx)?;
    if size < mem::size_of::<WTSINFOEXW>() {
        return None;
    }
    let info = unsafe { buffer.0.cast::<WTSINFOEXW>().read_unaligned() };
    if info.Level != 1 {
        return None;
    }
    match unsafe { info.Data.WTSInfoExLevel1.SessionFlags } {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_invalid() {
        return Err(io::Error::other("Windows returned an invalid handle"));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}

fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}

pub(super) fn token(session_id: u32, as_user: bool) -> ResultType<OwnedHandle> {
    if session_id == u32::MAX {
        bail!("No Windows desktop session is available");
    }
    if as_user {
        let mut token = HANDLE::default();
        match unsafe { WTSQueryUserToken(session_id, &mut token) } {
            Ok(()) => return Ok(owned(token)?),
            Err(error) => {
                let is_current_user = !super::is_root()
                    && super::get_current_process_session_id() == Some(session_id)
                    && username(session_id).is_some_and(|name| {
                        !name.is_empty() && name.eq_ignore_ascii_case(&crate::username())
                    });
                if !is_current_user {
                    return Err(error).context("Could not open the Windows session user token");
                }
            }
        }
    } else if !super::is_root() {
        bail!("Launching a privileged session process requires the LocalSystem service");
    }

    let mut process_token = HANDLE::default();
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY,
            &mut process_token,
        )
        .context("Could not open the current Windows process token")?;
    }
    let process_token = owned(process_token)?;
    let mut token = HANDLE::default();
    let mut desired = TOKEN_ASSIGN_PRIMARY | TOKEN_DUPLICATE | TOKEN_QUERY;
    if !as_user {
        // SetTokenInformation(TokenSessionId) checks both access rights on the duplicate.
        desired |= TOKEN_ADJUST_DEFAULT | TOKEN_ADJUST_SESSIONID;
    }
    unsafe {
        DuplicateTokenEx(
            raw(&process_token),
            desired,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut token,
        )
        .context("Could not duplicate the Windows primary token")?;
    }
    let token = owned(token)?;
    if !as_user && super::get_current_process_session_id() != Some(session_id) {
        unsafe {
            SetTokenInformation(
                raw(&token),
                TokenSessionId,
                (&session_id as *const u32).cast(),
                mem::size_of_val(&session_id) as u32,
            )
            .with_context(|| {
                format!("Could not assign the Windows token to session {session_id}")
            })?;
        }
    }
    Ok(token)
}

struct Environment(*mut c_void);

impl Environment {
    fn for_user(token: &OwnedHandle) -> ResultType<Self> {
        let mut block = ptr::null_mut();
        unsafe { CreateEnvironmentBlock(&mut block, Some(raw(token)), true) }
            .context("Could not prepare the Windows session user's environment")?;
        Ok(Self(block))
    }
}

impl Drop for Environment {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyEnvironmentBlock(self.0);
        }
    }
}

pub(super) fn launch(
    session_id: u32,
    command: &str,
    as_user: bool,
    show: bool,
) -> ResultType<OwnedHandle> {
    if command.is_empty() || command.contains('\0') {
        bail!("Invalid Windows session command");
    }
    let token = token(session_id, as_user)?;
    let environment = as_user.then(|| Environment::for_user(&token)).transpose()?;
    let mut command: Vec<_> = command.encode_utf16().chain([0]).collect();
    let mut desktop: Vec<_> = "winsta0\\default".encode_utf16().chain([0]).collect();
    let mut startup = STARTUPINFOW {
        cb: mem::size_of::<STARTUPINFOW>() as u32,
        dwFlags: STARTF_USESHOWWINDOW,
        ..Default::default()
    };
    if show {
        startup.lpDesktop = PWSTR(desktop.as_mut_ptr());
        startup.wShowWindow = SW_SHOW.0 as u16;
    }

    let flags = if environment.is_some() {
        DETACHED_PROCESS | CREATE_UNICODE_ENVIRONMENT
    } else {
        DETACHED_PROCESS
    };
    let mut process = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessAsUserW(
            Some(raw(&token)),
            PCWSTR::null(),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            flags,
            environment.as_ref().map(|value| value.0.cast_const()),
            PCWSTR::null(),
            &startup,
            &mut process,
        )
        .with_context(|| format!("Could not create a process in Windows session {session_id}"))?;
    }
    let process_handle = owned(process.hProcess);
    let _thread = owned(process.hThread)?;
    Ok(process_handle?)
}
