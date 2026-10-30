#[path = "../../gpui-validation/src/theme.rs"]
mod shared;

pub use shared::*;

// Android reports a fixed dark appearance; keep mobile controls and shared inputs
// on the same light palette instead of inheriting that platform fallback.
pub fn palette(_: gpui::WindowAppearance) -> Palette {
    shared::palette(gpui::WindowAppearance::Light)
}
