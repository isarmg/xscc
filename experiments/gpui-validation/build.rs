fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_NATIVE_HOST");
    #[cfg(windows)]
    if std::env::var_os("CARGO_FEATURE_NATIVE_HOST").is_some() {
        let mut resource = winres::WindowsResource::new();
        resource.set_manifest(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges><requestedExecutionLevel level="requireAdministrator" uiAccess="false"/></requestedPrivileges></security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application><supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/></application>
  </compatibility>
</assembly>"#);
        if let Err(error) = resource.compile() {
            panic!("Failed to compile XSCC Host administrator manifest: {error}");
        }
    }
}
