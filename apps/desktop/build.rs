fn main() {
    // gpui_platform owns MANIFEST#1. The Windows release workflow updates only
    // the host's execution level after linking, before validation and packaging.
    println!("cargo:rerun-if-changed=build.rs");
}
