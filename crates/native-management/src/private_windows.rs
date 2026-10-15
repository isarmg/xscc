use super::*;
use std::os::windows::{
    ffi::OsStrExt,
    fs::{MetadataExt, OpenOptionsExt},
    io::AsRawHandle,
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree},
        Security::{
            ACCESS_ALLOWED_ACE, ACL,
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                GetNamedSecurityInfoW, GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
            },
            DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl, GetTokenInformation,
            OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
            SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, FILE_FLAG_OPEN_REPARSE_POINT,
            GetFileInformationByHandle, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
            MoveFileExW,
        },
        System::{
            Com::CoTaskMemFree,
            SystemServices::ACCESS_ALLOWED_ACE_TYPE,
            Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath},
    },
    core::{PCWSTR, PWSTR},
};

struct LocalMemory(*mut std::ffi::c_void);
impl Drop for LocalMemory {
    fn drop(&mut self) {
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.0)));
        }
    }
}
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
fn error(e: windows::core::Error) -> io::Error {
    io::Error::from_raw_os_error(e.code().0 & 0xffff)
}
fn sid_string(sid: PSID) -> io::Result<String> {
    if sid.0.is_null() {
        return Err(invalid());
    }
    let mut text = PWSTR::null();
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(error)?;
    let _guard = LocalMemory(text.0.cast());
    unsafe { text.to_string() }.map_err(io::Error::other)
}
fn current_sid() -> io::Result<String> {
    process_sid(unsafe { GetCurrentProcess() })
}
fn process_sid(process: HANDLE) -> io::Result<String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.map_err(error)?;
    let result = (|| {
        let mut length = 0;
        unsafe {
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut length);
        }
        if length == 0 {
            return Err(invalid());
        }
        let mut bytes = vec![0u8; length as usize];
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(bytes.as_mut_ptr().cast()),
                length,
                &mut length,
            )
        }
        .map_err(error)?;
        let user = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<TOKEN_USER>()) };
        sid_string(user.User.Sid)
    })();
    unsafe {
        let _ = CloseHandle(token);
    }
    result
}
fn check_acl(
    owner: PSID,
    acl: *mut ACL,
    descriptor: PSECURITY_DESCRIPTOR,
    protected: bool,
) -> io::Result<()> {
    let user = current_sid()?;
    let owner = sid_string(owner)?;
    if (owner != user && owner != "S-1-5-18" && owner != "S-1-5-32-544") || acl.is_null() {
        return Err(invalid());
    }
    if protected {
        let mut control = 0u16;
        let mut revision = 0;
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }
            .map_err(error)?;
        if control & SE_DACL_PROTECTED.0 == 0 {
            return Err(invalid());
        }
    }
    let count = unsafe { (*acl).AceCount };
    if count == 0 {
        return Err(invalid());
    }
    for index in 0..u32::from(count) {
        let mut raw = std::ptr::null_mut();
        unsafe { GetAce(acl, index, &mut raw) }.map_err(error)?;
        let ace = raw.cast::<ACCESS_ALLOWED_ACE>();
        if unsafe { (*ace).Header.AceType } != ACCESS_ALLOWED_ACE_TYPE as u8 {
            return Err(invalid());
        }
        let sid = unsafe { PSID(std::ptr::addr_of!((*ace).SidStart).cast_mut().cast()) };
        let value = sid_string(sid)?;
        if value != user && value != "S-1-5-18" && value != "S-1-5-32-544" {
            return Err(invalid());
        }
    }
    Ok(())
}
pub fn check_parent(path: &Path) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_dir() || m.file_attributes() & 0x400 != 0 {
        return Err(invalid());
    }
    Ok(())
}
pub fn check_directory(path: &Path) -> io::Result<()> {
    check_parent(path)?;
    let mut owner = PSID::default();
    let mut acl = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let name = wide(path);
    let status = unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(name.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut acl),
            None,
            &mut descriptor,
        )
    };
    if status.0 != 0 {
        return Err(io::Error::from_raw_os_error(status.0 as i32));
    }
    let _guard = LocalMemory(descriptor.0);
    check_acl(owner, acl, descriptor, true)
}
pub fn create_directory(path: &Path) -> io::Result<()> {
    let user = current_sid()?;
    let text = format!("O:{user}D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FA;;;{user})");
    let text: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(text.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .map_err(error)?;
    let _guard = LocalMemory(descriptor.0);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let name = wide(path);
    unsafe { CreateDirectoryW(PCWSTR(name.as_ptr()), Some(&attributes)) }.map_err(error)
}
pub fn private_options(options: &mut OpenOptions) {
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0);
}
pub fn check_file(file: &File) -> io::Result<()> {
    let m = file.metadata()?;
    if !m.is_file() || m.file_attributes() & 0x400 != 0 {
        return Err(invalid());
    }
    let handle = HANDLE(file.as_raw_handle());
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(handle, &mut info) }.map_err(error)?;
    if info.nNumberOfLinks != 1 {
        return Err(invalid());
    }
    let mut owner = PSID::default();
    let mut acl = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut acl),
            None,
            Some(&mut descriptor),
        )
    };
    if status.0 != 0 {
        return Err(io::Error::from_raw_os_error(status.0 as i32));
    }
    let _guard = LocalMemory(descriptor.0);
    check_acl(owner, acl, descriptor, false)
}
pub fn publish(from: &Path, to: &Path, create: bool) -> io::Result<()> {
    let from = wide(from);
    let to = wide(to);
    let flags = if create {
        MOVEFILE_WRITE_THROUGH
    } else {
        MOVEFILE_WRITE_THROUGH | MOVEFILE_REPLACE_EXISTING
    };
    unsafe { MoveFileExW(PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr()), flags) }.map_err(error)
}
pub fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}
pub fn local_data_directory() -> io::Result<PathBuf> {
    let path = unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None) }
        .map_err(error)?;
    let value = unsafe { path.to_string() }.map_err(io::Error::other);
    unsafe {
        CoTaskMemFree(Some(path.0.cast()));
    }
    Ok(PathBuf::from(value?))
}

pub fn verify_process_owner(pid: u32) -> io::Result<()> {
    let process =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.map_err(error)?;
    let peer = process_sid(process);
    unsafe {
        let _ = CloseHandle(process);
    }
    if peer? != current_sid()? {
        return Err(invalid());
    }
    Ok(())
}
