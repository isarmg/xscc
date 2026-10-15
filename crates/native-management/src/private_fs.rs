use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct PrivateDirectory(Arc<PathBuf>);
impl PrivateDirectory {
    pub fn open(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(invalid());
        }
        platform::check_directory(path)?;
        Ok(Self(Arc::new(path.to_owned())))
    }
    pub fn create(parent: &Path, name: &str) -> io::Result<Self> {
        check_name(name)?;
        platform::check_parent(parent)?;
        let path = parent.join(name);
        match platform::create_directory(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        platform::check_directory(&path)?;
        platform::sync_directory(parent)?;
        Ok(Self(Arc::new(path)))
    }
    pub fn child(&self, name: &str) -> io::Result<Self> {
        Self::create(&self.0, name)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn lock(&self, name: &str) -> io::Result<File> {
        check_name(name)?;
        platform::check_directory(&self.0)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        platform::private_options(&mut options);
        let file = options.open(self.0.join(name))?;
        platform::check_file(&file)?;
        file.try_lock().map_err(io::Error::other)?;
        Ok(file)
    }
    pub fn read(&self, name: &str, maximum: usize) -> io::Result<Option<Zeroizing<Vec<u8>>>> {
        check_name(name)?;
        platform::check_directory(&self.0)?;
        let mut options = OpenOptions::new();
        options.read(true);
        platform::private_options(&mut options);
        let file = match options.open(self.0.join(name)) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        platform::check_file(&file)?;
        let mut bytes = Zeroizing::new(Vec::new());
        file.take((maximum + 1) as u64).read_to_end(&mut bytes)?;
        if bytes.len() > maximum {
            return Err(invalid());
        }
        Ok(Some(bytes))
    }
    pub fn write(&self, name: &str, bytes: &[u8], create: bool) -> io::Result<()> {
        check_name(name)?;
        platform::check_directory(&self.0)?;
        let temporary = self.0.join(format!(".write-{}.tmp", uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        platform::private_options(&mut options);
        let mut file = options.open(&temporary)?;
        platform::check_file(&file)?;
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            platform::publish(&temporary, &self.0.join(name), create)
        })();
        drop(file);
        match fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        platform::sync_directory(&self.0)?;
        result
    }
    pub fn remove(&self, name: &str) -> io::Result<()> {
        check_name(name)?;
        platform::check_directory(&self.0)?;
        match fs::remove_file(self.0.join(name)) {
            Ok(()) => platform::sync_directory(&self.0),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}
fn check_name(name: &str) -> io::Result<()> {
    if name.is_empty()
        || name.len() > 180
        || name == "."
        || name == ".."
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        Err(invalid())
    } else {
        Ok(())
    }
}
pub(crate) fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "protected management storage unavailable",
    )
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    fn uid() -> u32 {
        unsafe { libc::geteuid() }
    }
    pub fn check_parent(path: &Path) -> io::Result<()> {
        let m = fs::symlink_metadata(path)?;
        if !m.is_dir() || m.uid() != uid() || m.mode() & 0o022 != 0 {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn check_directory(path: &Path) -> io::Result<()> {
        let m = fs::symlink_metadata(path)?;
        if !m.is_dir() || m.uid() != uid() || m.mode() & 0o777 != 0o700 {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn create_directory(path: &Path) -> io::Result<()> {
        fs::DirBuilder::new().mode(0o700).create(path)
    }
    pub fn private_options(options: &mut OpenOptions) {
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    pub fn check_file(file: &File) -> io::Result<()> {
        let m = file.metadata()?;
        if !m.is_file() || m.uid() != uid() || m.mode() & 0o777 != 0o600 || m.nlink() != 1 {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn sync_directory(path: &Path) -> io::Result<()> {
        File::open(path)?.sync_all()
    }
    pub fn publish(from: &Path, to: &Path, create: bool) -> io::Result<()> {
        if create {
            fs::hard_link(from, to)
        } else {
            fs::rename(from, to)
        }
    }
}

#[cfg(windows)]
#[path = "private_windows.rs"]
mod platform;

#[cfg(windows)]
pub use platform::verify_process_owner;

pub fn default_directory() -> io::Result<PrivateDirectory> {
    let home = home_directory()?;
    PrivateDirectory::create(&home, "xscc-managed")
}
#[cfg(unix)]
fn home_directory() -> io::Result<PathBuf> {
    use std::{ffi::CStr, os::unix::ffi::OsStrExt};
    let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut found = std::ptr::null_mut();
    let mut buffer = vec![0u8; 65536];
    let code = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            record.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        )
    };
    if code != 0 || found.is_null() {
        return Err(invalid());
    }
    let record = unsafe { record.assume_init() };
    if record.pw_dir.is_null() {
        return Err(invalid());
    }
    let bytes = unsafe { CStr::from_ptr(record.pw_dir) }.to_bytes();
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(bytes));
    if !path.is_absolute() {
        return Err(invalid());
    }
    Ok(path)
}
#[cfg(windows)]
fn home_directory() -> io::Result<PathBuf> {
    platform::local_data_directory()
}
