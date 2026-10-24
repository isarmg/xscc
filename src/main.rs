#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

fn main() {
    eprintln!(
        "The XSCC desktop application uses GPUI. Start it with:\n  cargo run --manifest-path experiments/gpui-validation/Cargo.toml --features live-session"
    );
}
