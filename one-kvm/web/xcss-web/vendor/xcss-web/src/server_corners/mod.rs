//! Explicit server decoration. Public product pages remain opt-in.
//!
//! G4 profile, geometry and geometry regressions are imported from the local ZXS
//! repository at `f009be2abbdcc56e5b7e8344461c97885a2e3f13`, from
//! `src/interface/src/contours/{profile,geometry}.rs` and
//! `test/interface/unit/contours/geometry/tests.rs`. ZXS declares Apache-2.0 at
//! the project root and Apache-2.0 AND MIT for its integrated interface crate;
//! the retained Apache-2.0 license text is in this crate's `LICENSE`.
//! Local adaptations use the existing serde dependency, inline the upstream
//! tests, and retain the XSCS paint/controller API and CSS attribute names.
//! The analytic curve is G4; the generated SVG remains an error-bounded polyline.
#[cfg(target_arch = "wasm32")]
mod controller;
pub mod geometry;
#[cfg(any(target_arch = "wasm32", test))]
mod paint;
#[expect(
    clippy::doc_lazy_continuation,
    reason = "Retain the imported G4 mathematical documentation"
)]
mod profile;
#[cfg(target_arch = "wasm32")]
pub use controller::{ServerCornersController, start};
