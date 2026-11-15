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

impl SessionLayout {
    pub fn control_canvas(&self) -> Rect {
        Rect {
            height: self.stage.height.max(self.rail.y + self.rail.height),
            ..self.stage
        }
    }
}

pub fn place_rail(
    layout: &mut SessionLayout,
    position: Option<(u16, u16)>,
    width: f32,
    height: f32,
) {
    if let Some((x, y)) = position {
        layout.rail.x = (width - layout.rail.width).max(0.) * f32::from(x) / 10_000.;
        layout.rail.y = (height - layout.rail.height).max(0.) * f32::from(y) / 10_000.;
    }
}

pub fn keyboard_overlay(layout: &mut SessionLayout, height: f32, ime: f32) {
    let bottom = (height - ime).max(0.);
    if let Some(keyboard) = &mut layout.keyboard {
        keyboard.height = keyboard.height.min(bottom);
        keyboard.y = bottom - keyboard.height;
    }
    layout.stage.height = height;
    layout.viewport.height = height;
    layout.video.height = if layout.rail.width > layout.rail.height {
        (height - layout.rail.height).max(0.)
    } else {
        height
    };
}

pub fn pan_for_keyboard(
    layout: &mut SessionLayout,
    extent: (u32, u32),
    pointer_y: i32,
    offset: &mut f32,
) {
    let Some(keyboard) = layout.keyboard else {
        return;
    };
    if extent.0 == 0 || extent.1 == 0 {
        return;
    }
    let scale = (layout.video.width / extent.0 as f32).min(layout.video.height / extent.1 as f32);
    let image_top = (layout.video.height - extent.1 as f32 * scale) / 2.;
    let image_bottom = image_top + extent.1 as f32 * scale;
    let edge = 24_f32.min(keyboard.y / 4.);
    let low = (keyboard.y - edge).max(0.);
    let high = (image_top + edge).min((low - edge).max(0.));
    let pointer =
        image_top + pointer_y.clamp(0, extent.1.saturating_sub(1) as i32) as f32 * scale + *offset;
    if pointer > low {
        *offset -= pointer - low;
    } else if pointer < high {
        *offset += high - pointer;
    }
    *offset = offset.clamp((low - image_bottom).min(0.), 0.);
    layout.video.y = *offset;
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
    let portrait = height > width;
    let rail_width = if rail_expanded && !portrait {
        56_f32.min(width * 0.2)
    } else {
        0.
    };
    let available_width = (width - rail_width).max(0.);
    let options = matches!(panel, Panel::Operations | Panel::Display);
    let side_width = if options && !portrait {
        320_f32
            .min(available_width * 0.42)
            .min((available_width - 160.).max(0.))
    } else {
        0.
    };
    let bottom_height = if panel == Panel::Keyboard {
        let requested = if keyboard_height.is_finite() {
            keyboard_height.max(0.)
        } else {
            88.
        };
        requested.min((height - 44.).max(0.))
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
        height: if panel == Panel::Keyboard {
            control_height
        } else {
            height
        },
    };
    let video = if rail_expanded {
        Rect {
            x: rail_width,
            width: (width - 2. * rail_width).max(0.),
            ..stage
        }
    } else {
        stage
    };
    let rail = if rail_expanded {
        Rect {
            x: available_width,
            y: 0.,
            width: rail_width,
            height,
        }
    } else {
        let size = 44_f32.min(width).min(height);
        let right = 16_f32.min((width - size).max(0.));
        let top = 16_f32.min((height - size).max(0.));
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
            let rail = if rail_expanded {
                let rail_height = 56_f32.min(height);
                Rect {
                    x: 0.,
                    y: height - rail_height,
                    width,
                    height: rail_height,
                }
            } else {
                let size = 44_f32.min(width).min(height);
                let margin_x = 16_f32.min((width - size).max(0.));
                let margin_y = 16_f32.min((height - size).max(0.));
                Rect {
                    x: width - size - margin_x,
                    y: height - size - margin_y,
                    width: size,
                    height: size,
                }
            };
            let requested_height = if panel == Panel::Keyboard {
                if keyboard_height.is_finite() {
                    keyboard_height.max(0.)
                } else {
                    88.
                }
            } else {
                320_f32.min(height * 0.48)
            };
            let panel_bottom = if rail_expanded {
                rail.y
            } else {
                (rail.y - 8.).max(0.)
            };
            let panel_height = requested_height.min((panel_bottom - 44.).max(0.));
            let bottom = Rect {
                x: 0.,
                y: panel_bottom - panel_height,
                width: available_width,
                height: panel_height,
            };
            let stage = Rect {
                height: if panel == Panel::Keyboard {
                    bottom.y
                } else if rail_expanded {
                    rail.y
                } else {
                    height
                },
                ..stage
            };
            (
                rail,
                options.then_some(bottom),
                (panel == Panel::Keyboard).then_some(bottom),
                stage,
                Rect {
                    height: stage.height,
                    ..video
                },
                available_width,
                if options {
                    bottom.y
                } else {
                    stage.height.min(panel_bottom)
                },
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
        && control_height >= 124.)
        .then(|| {
            let key_size = 48.;
            let first = (control_width - key_size * 2. - 8.) / 2.;
            let left = Rect {
                x: first,
                y: control_height - key_size - 12.,
                width: key_size,
                height: key_size,
            };
            let right = Rect {
                x: first + key_size + 8.,
                ..left
            };
            let stack_height = if options { control_height } else { height };
            let wheel_height = (stack_height - key_size - 32.).clamp(44., 112.);
            let group_height = key_size + 8. + wheel_height;
            let middle = Rect {
                x: control_width - key_size - 12.,
                y: (stack_height - group_height) / 2.,
                width: key_size,
                height: key_size,
            };
            let wheel = Rect {
                y: middle.y + key_size + 8.,
                height: wheel_height,
                ..middle
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
                        assert_eq!(layout.video.height, layout.stage.height);
                        assert_eq!(layout.video.x + layout.video.width / 2., width / 2.);
                    } else if panel == Panel::Keyboard {
                        assert_eq!(layout.stage.width, baseline.stage.width);
                        assert_eq!(
                            layout.stage.height + layout.keyboard.unwrap().height,
                            height
                        );
                        assert_eq!(layout.video.height, layout.stage.height);
                        assert_eq!(layout.video.x + layout.video.width / 2., width / 2.);
                    } else {
                        assert_eq!(layout.video.height, baseline.video.height);
                        assert_eq!(layout.video.x + layout.video.width / 2., width / 2.);
                        assert_eq!(layout.stage, baseline.stage);
                    }
                    assert_eq!(layout.video.height, layout.viewport.height);
                    assert_eq!(layout.video.x + layout.video.width / 2., width / 2.);
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
                    x: width - 60.,
                    y: if height > width { height - 60. } else { 16. },
                    width: 44.,
                    height: 44.,
                }
            );
        }
    }

    #[test]
    fn portrait_dock_and_bottom_panels_keep_the_image_centered_and_uncovered() {
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
                assert_eq!(layout.video.x + layout.video.width / 2., 393. / 2.);
                if expanded {
                    assert_eq!(layout.rail.x, 0.);
                    assert_eq!(layout.rail.y + layout.rail.height, 873.);
                    assert_eq!(layout.rail.width, 393.);
                    assert!(!overlaps(layout.video, layout.rail));
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
                    if panel == Panel::Keyboard {
                        assert!(!overlaps(bottom, layout.video));
                        assert_eq!(layout.video.y + layout.video.height, bottom.y);
                    }
                    assert!(bottom.y + bottom.height <= 873.);
                } else {
                    assert_eq!(
                        layout.video.height,
                        if expanded {
                            layout.rail.y
                        } else {
                            baseline.video.height
                        }
                    );
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
        let docked = layout(1280., 576., true, Panel::None, true, 88.);
        assert_eq!(full.video.height, docked.video.height);
        assert_eq!(
            full.video.x + full.video.width / 2.,
            docked.video.x + docked.video.width / 2.
        );
        assert!(!overlaps(docked.video, docked.rail));
    }

    #[test]
    fn keyboard_overlays_full_size_video_and_pans_in_both_directions() {
        for (width, height, ime) in [(800., 360., 180.), (393., 873., 330.)] {
            let before = layout(width, height, true, Panel::None, false, 0.).video;
            let mut layout = layout(width, height, true, Panel::Keyboard, false, 36.);
            keyboard_overlay(&mut layout, height, ime);
            let keyboard = layout.keyboard.unwrap();
            assert_eq!(keyboard.y + keyboard.height, height - ime);
            let size = (layout.video.width, layout.video.height);
            assert_eq!(layout.stage.height, height);
            assert_eq!(
                (layout.video.width, layout.video.height),
                (before.width, before.height)
            );
            let mut offset = 0.;
            pan_for_keyboard(&mut layout, (1920, 1080), 1079, &mut offset);
            assert!(offset < 0., "bottom pointer must clear the keyboard");
            let bottom = offset;
            pan_for_keyboard(&mut layout, (1920, 1080), 0, &mut offset);
            assert!(
                offset > bottom,
                "top pointer must move the image down again"
            );
            assert_eq!((layout.video.width, layout.video.height), size);
        }
    }

    #[test]
    fn middle_and_wheel_default_to_a_centered_right_hand_stack() {
        let layout = layout(800., 360., false, Panel::None, true, 0.);
        let mouse = layout.mouse.unwrap();
        assert_eq!(mouse.middle.x, mouse.wheel.x);
        assert_eq!(
            (mouse.middle.y + mouse.wheel.y + mouse.wheel.height) / 2.,
            180.
        );
        assert_eq!(mouse.wheel.x + mouse.wheel.width, 788.);
        let portrait = super::layout(393., 873., true, Panel::None, true, 0.);
        assert_eq!(portrait.control_canvas().height, 873.);
        let portrait_mouse = portrait.mouse.unwrap();
        assert_eq!(
            (portrait_mouse.middle.y + portrait_mouse.wheel.y + portrait_mouse.wheel.height) / 2.,
            873. / 2.
        );
        let mut moved = layout;
        place_rail(&mut moved, Some((0, 0)), 800., 360.);
        assert_eq!((moved.rail.x, moved.rail.y), (0., 0.));
        place_rail(&mut moved, Some((10_000, 10_000)), 800., 360.);
        assert_eq!(
            (
                moved.rail.x + moved.rail.width,
                moved.rail.y + moved.rail.height
            ),
            (800., 360.)
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
