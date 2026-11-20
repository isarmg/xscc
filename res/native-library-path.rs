use std::{env, path::PathBuf};

pub fn installed_target() -> Option<PathBuf> {
    println!("cargo:rerun-if-env-changed=VCPKG_INSTALLED_ROOT");
    println!("cargo:rerun-if-env-changed=VCPKG_ROOT");
    let installed = env::var_os("VCPKG_INSTALLED_ROOT")
        .map(PathBuf::from)
        .or_else(|| env::var_os("VCPKG_ROOT").map(|root| PathBuf::from(root).join("installed")))?;

    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo did not specify a target OS");
    let target_arch =
        env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo did not specify a target architecture");
    let arch = match target_arch.as_str() {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        arch => arch,
    };
    let os = match target_os.as_str() {
        "macos" => "osx",
        "windows" => "windows-static",
        os => os,
    };
    Some(installed.join(format!("{arch}-{os}")))
}
