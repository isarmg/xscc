#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Panel {
    #[default]
    None,
    Operations,
    Display,
    Keyboard,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseControls {
    pub left: Rect,
    pub right: Rect,
    pub middle: Rect,
    pub wheel: Rect,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SessionLayout {
    pub viewport: Rect,
    pub stage: Rect,
    pub video: Rect,
    pub rail: Rect,
    pub panel: Option<Rect>,
    pub keyboard: Option<Rect>,
    pub mouse: Option<MouseControls>,
}

pub fn centered_horizontal_bounds(width: f32, left: f32, visible_width: f32) -> (f32, f32) {
    let margin = left.max(width - left - visible_width).min(width / 2.);
    (margin, width - margin * 2.)
}

pub fn layout(
    width: f32,
    height: f32,
    rail_expanded: bool,
    panel: Panel,
    pointer_buttons: bool,
    keyboard_height: f32,
) -> SessionLayout {
    let width = if width.is_finite() { width.max(0.) } else { 0. };
    let height = if height.is_finite() {
        height.max(0.)
    } else {
        0.
    };
    let rail_width = if rail_expanded {
        56_f32.min(width * 0.2)
    } else {
        0.
    };
    let available_width = (width - rail_width - if rail_expanded { 8. } else { 0. }).max(0.);
    let options = matches!(panel, Panel::Operations | Panel::Display);
    let side_width = if options && width >= 640. {
        320_f32
            .min(available_width * 0.42)
            .min((available_width - 160.).max(0.))
    } else {
        0.
    };
    let bottom_height = if panel == Panel::Keyboard {
        let requested = if keyboard_height.is_finite() {
            keyboard_height.max(44.)
        } else {
            88.
        };
        requested.min(height * 0.60).min((height - 44.).max(0.))
    } else if options && side_width == 0. {
        320_f32.min(height * 0.48).min((height - 56.).max(0.))
    } else {
        0.
    };
    let control_width = (available_width - side_width).max(0.);
    let control_height = (height - bottom_height).max(0.);
    let stage = Rect {
        x: 0.,
        y: 0.,
        width,
        height,
    };
    let video = stage;
    let rail = if rail_expanded {
        Rect {
            x: available_width,
            y: 8_f32.min(height),
            width: rail_width,
            height: 294_f32.min((height - 16.).max(0.)),
        }
    } else {
        let size = 44_f32.min(width).min(height);
        let right = 8_f32.min((width - size).max(0.));
        let top = 8_f32.min((height - size).max(0.));
        Rect {
            x: width - size - right,
            y: top,
            width: size,
            height: size,
        }
    };
    let control_panel = if options {
        Some(if side_width > 0. {
            Rect {
                x: control_width,
                y: 0.,
                width: side_width,
                height,
            }
        } else {
            Rect {
                x: 0.,
                y: control_height,
                width: available_width,
                height: bottom_height,
            }
        })
    } else {
        None
    };
    let keyboard = (panel == Panel::Keyboard).then_some(Rect {
        x: 0.,
        y: control_height,
        width: available_width,
        height: bottom_height,
    });
    let (rail, control_panel, keyboard, stage, video, control_width, control_height) =
        if height > width {
            let rail_height = if rail_expanded { 56_f32 } else { 44_f32 }.min(height);
            let vertical_margin = 8_f32.min((height - rail_height).max(0.) / 2.);
            let rail_width = if rail_expanded { 294_f32 } else { 44_f32 }.min(width);
            let horizontal_margin = 8_f32.min((width - rail_width).max(0.) / 2.);
            let rail = Rect {
                x: if rail_expanded {
                    (width - rail_width) / 2.
                } else {
                    width - rail_width - horizontal_margin
                },
                y: height - rail_height - vertical_margin,
                width: rail_width,
                height: rail_height,
            };
            let requested_height = if panel == Panel::Keyboard {
                if keyboard_height.is_finite() {
                    keyboard_height.max(44.)
                } else {
                    88.
                }
                .min(height * 0.60)
            } else {
                320_f32.min(height * 0.48)
            };
            let panel_bottom = (rail.y - 8.).max(0.);
            let panel_height = requested_height.min((panel_bottom - 44.).max(0.));
            let bottom = Rect {
                x: 0.,
                y: panel_bottom - panel_height,
                width,
                height: panel_height,
            };
            let stage = Rect {
                height: if panel == Panel::None {
                    height
                } else {
                    bottom.y
                },
                ..stage
            };
            (
                rail,
                options.then_some(bottom),
                (panel == Panel::Keyboard).then_some(bottom),
                stage,
                stage,
                width,
                stage.height.min(panel_bottom),
            )
        } else {
            (
                rail,
                control_panel,
                keyboard,
                stage,
                video,
                control_width,
                control_height,
            )
        };
    let mouse = (pointer_buttons
        && panel != Panel::Keyboard
        && control_width >= 224.
        && control_height >= 64.)
        .then(|| {
            let key_size = 48.;
            let first = (control_width - key_size * 4. - 24.) / 2.;
            let left = Rect {
                x: first,
                y: control_height - key_size - 12.,
                width: key_size,
                height: key_size,
            };
            let middle = Rect {
                x: first + key_size + 8.,
                ..left
            };
            let right = Rect {
                x: first + 2. * (key_size + 8.),
                ..left
            };
            let wheel_height = 112_f32.min(control_height - 24.).max(44.);
            let wheel = Rect {
                x: first + 3. * (key_size + 8.),
                y: control_height - wheel_height - 12.,
                width: key_size,
                height: wheel_height,
            };
            MouseControls {
                left,
                middle,
                right,
                wheel,
            }
        });
    SessionLayout {
        viewport: stage,
        stage,
        video,
        rail,
        panel: control_panel,
        keyboard,
        mouse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.width > 0.
            && a.height > 0.
            && b.width > 0.
            && b.height > 0.
            && a.x < b.x + b.width - 0.01
            && b.x < a.x + a.width - 0.01
            && a.y < b.y + b.height - 0.01
            && b.y < a.y + a.height - 0.01
    }

    #[test]
    fn asymmetric_safe_areas_keep_the_remote_view_centered_on_the_screen() {
        for (left, visible_width) in [(36., 764.), (0., 764.), (16., 776.), (0., 800.)] {
            let (x, width) = centered_horizontal_bounds(800., left, visible_width);
            assert_eq!(x + width / 2., 400.);
            assert!(x >= left);
            assert!(x + width <= left + visible_width);
        }
    }

    #[test]
    fn menu_and_panel_toggles_preserve_the_full_remote_viewport() {
        for (width, height) in [(1280., 576.), (800., 360.), (320., 640.), (960., 154.)] {
            let baseline = layout(width, height, false, Panel::None, false, 88.);
            for panel in [
                Panel::None,
                Panel::Operations,
                Panel::Display,
                Panel::Keyboard,
            ] {
                for expanded in [true, false] {
                    let layout = layout(width, height, expanded, panel, true, 176.);
                    if height > width {
                        assert_eq!(layout.stage.x, baseline.stage.x);
                        assert_eq!(layout.stage.width, baseline.stage.width);
                        assert!(layout.stage.height <= baseline.stage.height);
                        assert_eq!(layout.video, layout.stage);
                    } else {
                        assert_eq!(layout.video, baseline.video);
                        assert_eq!(layout.stage, baseline.stage);
                    }
                    assert_eq!(layout.video, layout.viewport);
                    for control in [Some(layout.rail), layout.panel, layout.keyboard]
                        .into_iter()
                        .flatten()
                    {
                        assert!(control.x >= 0. && control.y >= 0.);
                        assert!(control.x + control.width <= width);
                        assert!(control.y + control.height <= height);
                    }
                    if expanded {
                        if let Some(panel) = layout.panel.or(layout.keyboard) {
                            assert!(!overlaps(panel, layout.rail));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn collapsed_controls_float_at_the_outer_corner_without_reserving_a_sidebar() {
        for (width, height) in [(1280., 576.), (800., 360.), (320., 640.), (960., 154.)] {
            let layout = layout(width, height, false, Panel::None, false, 88.);
            assert_eq!(layout.video, layout.viewport);
            assert_eq!(layout.stage, layout.viewport);
            assert_eq!(
                layout.rail,
                Rect {
                    x: width - 52.,
                    y: if height > width { height - 52. } else { 8. },
                    width: 44.,
                    height: 44.,
                }
            );
        }
    }

    #[test]
    fn portrait_bottom_controls_leave_a_full_width_uncovered_remote_viewport() {
        let baseline = layout(393., 873., false, Panel::None, false, 88.);
        for panel in [
            Panel::None,
            Panel::Operations,
            Panel::Display,
            Panel::Keyboard,
        ] {
            for expanded in [false, true] {
                let layout = layout(393., 873., expanded, panel, true, 360.);
                assert_eq!(layout.stage.x, 0.);
                assert_eq!(layout.stage.width, baseline.stage.width);
                assert_eq!(layout.viewport, layout.stage);
                assert_eq!(layout.video, layout.stage);
                if expanded {
                    assert!(layout.rail.width > layout.rail.height);
                }
                if let Some(mouse) = layout.mouse {
                    for button in [mouse.left, mouse.middle, mouse.right, mouse.wheel] {
                        assert!(!overlaps(button, layout.rail));
                        if let Some(bottom) = layout.panel {
                            assert!(!overlaps(button, bottom));
                        }
                    }
                }
                if let Some(bottom) = layout.panel.or(layout.keyboard) {
                    assert!(!overlaps(bottom, layout.rail));
                    assert!(!overlaps(bottom, layout.video));
                    assert_eq!(layout.video.y + layout.video.height, bottom.y);
                    assert!(bottom.y + bottom.height <= layout.rail.y - 8.);
                } else {
                    assert_eq!(layout.video, baseline.video);
                }
            }
        }
    }

    #[test]
    fn floating_buttons_preserve_full_image_geometry_and_stay_inside_it() {
        for (width, height) in [(1280., 576.), (800., 360.), (320., 640.), (960., 154.)] {
            for panel in [
                Panel::None,
                Panel::Operations,
                Panel::Display,
                Panel::Keyboard,
            ] {
                let hidden = layout(width, height, false, panel, false, 88.);
                let shown = layout(width, height, false, panel, true, 88.);
                assert_eq!(shown.video, hidden.video);
                assert_eq!(shown.stage, hidden.stage);
                if let Some(mouse) = shown.mouse {
                    assert_ne!(panel, Panel::Keyboard);
                    for button in [mouse.left, mouse.middle, mouse.right, mouse.wheel] {
                        assert!(button.width >= 44. && button.height >= 44.);
                        assert!(button.x >= shown.video.x && button.y >= shown.video.y);
                        assert!(button.x + button.width <= shown.video.x + shown.video.width);
                        assert!(button.y + button.height <= shown.video.y + shown.video.height);
                    }
                    assert!(!overlaps(mouse.left, mouse.middle));
                    assert!(!overlaps(mouse.middle, mouse.right));
                    assert!(!overlaps(mouse.right, mouse.wheel));
                }
            }
        }
        let full = layout(1280., 576., false, Panel::None, true, 88.);
        assert_eq!(full.video.height, 576.);
        assert_eq!(
            full.video,
            layout(1280., 576., true, Panel::None, true, 88.).video
        );
    }

    #[test]
    fn unavailable_surface_sizes_do_not_produce_negative_or_invalid_geometry() {
        for (width, height) in [(0., 0.), (-1., 40.), (f32::NAN, f32::INFINITY)] {
            let layout = layout(width, height, true, Panel::Keyboard, true, 176.);
            for rect in [layout.viewport, layout.stage, layout.video, layout.rail] {
                assert!(
                    [rect.x, rect.y, rect.width, rect.height]
                        .into_iter()
                        .all(f32::is_finite)
                );
                assert!(rect.width >= 0. && rect.height >= 0.);
            }
        }
    }
}
