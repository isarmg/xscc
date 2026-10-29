// Contour adapted from XCSS; dashboard palettes are maintained locally.
// https://github.com/isarmg/xcss/tree/d0a41861d949f2b0626249b25a35f562fb59b592/web/admin-ui
use gpui::WindowAppearance;
#[cfg(any(feature = "live-session", feature = "mobile-management"))]
use gpui::{Bounds, IntoElement, PathBuilder, Pixels, Rgba, canvas, fill, point, prelude::*, px};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub page: u32,
    pub panel: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub danger: u32,
    pub border: u32,
    pub accent_soft: u32,
    pub on_accent: u32,
    pub success: u32,
}

// Shared desktop and mobile palettes; session controls always use DARK.
pub const LIGHT: Palette = Palette {
    page: 0xf7f6f3,
    panel: 0xffffff,
    text: 0x1f2328,
    muted: 0x626d77,
    accent: 0x0f766e,
    danger: 0xb42318,
    border: 0xe7e5e0,
    accent_soft: 0xe4f1ef,
    on_accent: 0xffffff,
    success: 0x1a7f37,
};
pub const DARK: Palette = Palette {
    page: 0x131518,
    panel: 0x1c1f24,
    text: 0xe9ecef,
    muted: 0x9aa3ad,
    accent: 0x5ac8bc,
    danger: 0xf2868e,
    border: 0x2b3036,
    accent_soft: 0x1d2f2c,
    on_accent: 0x10201d,
    success: 0x6fd39f,
};

pub fn palette(appearance: WindowAppearance) -> Palette {
    match appearance {
        WindowAppearance::Light | WindowAppearance::VibrantLight => LIGHT,
        WindowAppearance::Dark | WindowAppearance::VibrantDark => DARK,
    }
}

#[cfg(any(feature = "live-session", feature = "mobile-management"))]
pub const CONTROL_EXTENT: f32 = 10.;
pub const PANEL_EXTENT: f32 = 18.;
#[cfg(any(feature = "live-session", feature = "mobile-management"))]
pub const CONTROL_HEIGHT: f32 = 44.;
#[cfg(any(feature = "live-session", feature = "mobile-management"))]
pub const BODY_SIZE: f32 = 14.;
#[cfg(any(feature = "live-session", feature = "mobile-management"))]
pub const LINE_HEIGHT: f32 = 22.;

// Evaluate the source degree-seven curve once at compile time. A 24-segment
// polygon bounds chord error below .115px for extents up to 42px; no per-frame
// curve evaluation or size-dependent cache is needed.
const CONTROLS: [(f64, f64); 8] = [
    (0., 0.),
    (0.3633595684909532, 0.),
    (0.5804546942007682, 0.),
    (0.7281206143651912, 0.),
    (1., 0.27187938563480885),
    (1., 0.41954530579923177),
    (1., 0.6366404315090468),
    (1., 1.),
];

const fn profile() -> [(f32, f32); 25] {
    let mut points = [(0., 0.); 25];
    let mut step = 0;
    while step <= 24 {
        let t = step as f64 / 24.;
        let mut row = CONTROLS;
        let mut n = 7;
        while n > 0 {
            let mut i = 0;
            while i < n {
                row[i] = (
                    row[i].0 * (1. - t) + row[i + 1].0 * t,
                    row[i].1 * (1. - t) + row[i + 1].1 * t,
                );
                i += 1;
            }
            n -= 1;
        }
        points[step] = (row[0].0 as f32, row[0].1 as f32);
        step += 1;
    }
    points
}
const PROFILE: [(f32, f32); 25] = profile();

#[cfg(any(feature = "live-session", feature = "mobile-management"))]
fn outline(bounds: Bounds<Pixels>, extent: f32, stroke: bool) -> PathBuilder {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let e = extent.min(w / 2.).min(h / 2.).max(0.);
    let mut path = if stroke {
        PathBuilder::stroke(px(1.))
    } else {
        PathBuilder::fill()
    };
    path.move_to(bounds.origin + point(px(e), px(0.)));
    for corner in 0..4 {
        for (x, y) in PROFILE {
            let (x, y) = (x * e, y * e);
            let (x, y) = match corner {
                0 => (w - e + x, y),
                1 => (w - y, h - e + x),
                2 => (e - x, h - y),
                _ => (y, e - x),
            };
            path.line_to(bounds.origin + point(px(x), px(y)));
        }
    }
    path.close();
    path
}

#[cfg(any(feature = "live-session", feature = "mobile-management"))]
pub fn surface(color: Rgba, border: Option<Rgba>, extent: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            match outline(bounds, extent, false).build() {
                Ok(path) => window.paint_path(path, color),
                Err(_) => window.paint_quad(fill(bounds, color)),
            }
            if let Some(border) = border {
                // Inset the centreline so focus strokes stay inside the control.
                let inset = bounds.inset(px(0.5));
                match outline(inset, (extent - 0.5).max(0.), true).build() {
                    Ok(path) => window.paint_path(path, border),
                    Err(_) => {
                        window.paint_quad(gpui::outline(bounds, border, gpui::BorderStyle::Solid))
                    }
                }
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_follows_system_appearance() {
        assert_eq!(palette(WindowAppearance::Light), LIGHT);
        assert_eq!(palette(WindowAppearance::VibrantLight), LIGHT);
        assert_eq!(palette(WindowAppearance::Dark), DARK);
        assert_eq!(palette(WindowAppearance::VibrantDark), DARK);
    }
    #[test]
    fn polygon_error_stays_below_quarter_pixel_at_largest_extent() {
        let max_second_derivative = CONTROLS
            .windows(3)
            .map(|p| {
                let x = 42. * (p[2].0 - 2. * p[1].0 + p[0].0);
                let y = 42. * (p[2].1 - 2. * p[1].1 + p[0].1);
                x.hypot(y)
            })
            .fold(0_f64, f64::max);
        let segments = (PROFILE.len() - 1) as f64;
        // Bézier derivative convex hull bounds chord interpolation error.
        assert!(max_second_derivative * PANEL_EXTENT as f64 / (8. * segments * segments) < 0.115);
    }
    #[test]
    fn mac_mini_profile_preserves_endpoints_symmetry_and_monotonicity() {
        assert_eq!(PROFILE[0], (0., 0.));
        assert_eq!(PROFILE[24], (1., 1.));
        for (i, &(x, y)) in PROFILE.iter().enumerate() {
            let reverse = PROFILE[24 - i];
            assert!((x + reverse.1 - 1.).abs() < 1e-6);
            assert!((y + reverse.0 - 1.).abs() < 1e-6);
        }
        assert!(
            PROFILE
                .windows(2)
                .all(|p| p[0].0 <= p[1].0 && p[0].1 <= p[1].1)
        );
        // This is deliberately not a circular rounded rectangle.
        assert!((PROFILE[12].0 - 0.81419754).abs() < 0.0001);
    }
}
