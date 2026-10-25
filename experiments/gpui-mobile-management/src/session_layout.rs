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
    pub dock: Rect,
    pub left: Rect,
    pub right: Rect,
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
        64_f32.min(width * 0.2)
    } else {
        44_f32.min(width)
    };
    let available_width = (width - rail_width).max(0.);
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
        requested.min(height * 0.58).min((height - 44.).max(0.))
    } else if options && side_width == 0. {
        320_f32.min(height * 0.48).min((height - 56.).max(0.))
    } else {
        0.
    };
    let stage_width = (available_width - side_width).max(0.);
    let stage_height = (height - bottom_height).max(0.);
    let dock_height = if pointer_buttons && panel != Panel::Keyboard && stage_width >= 160. {
        64_f32.min((stage_height - 44.).max(0.))
    } else {
        0.
    };
    let stage = Rect {
        x: 0.,
        y: 0.,
        width: stage_width,
        height: stage_height,
    };
    let video = Rect {
        height: (stage_height - dock_height).max(0.),
        ..stage
    };
    let rail = Rect {
        x: available_width,
        y: 0.,
        width: rail_width,
        height,
    };
    let control_panel = if options {
        Some(if side_width > 0. {
            Rect {
                x: stage_width,
                y: 0.,
                width: side_width,
                height,
            }
        } else {
            Rect {
                x: 0.,
                y: stage_height,
                width: available_width,
                height: bottom_height,
            }
        })
    } else {
        None
    };
    let keyboard = (panel == Panel::Keyboard).then_some(Rect {
        x: 0.,
        y: stage_height,
        width: available_width,
        height: bottom_height,
    });
    let mouse = (dock_height >= 44.).then(|| {
        let key_size = 48_f32.min(dock_height - 8.);
        let key_y = video.height + (dock_height - key_size) / 2.;
        let first = (stage_width - key_size * 3. - 16.) / 2.;
        let left = Rect {
            x: first,
            y: key_y,
            width: key_size,
            height: key_size,
        };
        let right = Rect {
            x: first + key_size + 8.,
            ..left
        };
        let wheel = Rect {
            x: first + 2. * (key_size + 8.),
            ..left
        };
        MouseControls {
            dock: Rect {
                x: 0.,
                y: video.height,
                width: stage_width,
                height: dock_height,
            },
            left,
            right,
            wheel,
        }
    });
    SessionLayout {
        viewport: Rect {
            x: 0.,
            y: 0.,
            width,
            height,
        },
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
    fn controls_and_keyboard_never_cover_the_remote_image() {
        for (width, height) in [(1280., 576.), (800., 360.), (320., 640.), (960., 154.)] {
            for panel in [
                Panel::None,
                Panel::Operations,
                Panel::Display,
                Panel::Keyboard,
            ] {
                for expanded in [true, false] {
                    let layout = layout(width, height, expanded, panel, true, 176.);
                    for control in [
                        Some(layout.rail),
                        layout.panel,
                        layout.keyboard,
                        layout.mouse.map(|m| m.dock),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        assert!(!overlaps(layout.video, control), "{layout:?}");
                    }
                    assert!(layout.video.width > 0. && layout.video.height >= 44.);
                    assert!(layout.stage.x + layout.stage.width <= layout.rail.x + 0.01);
                }
            }
        }
    }

    #[test]
    fn collapse_restores_image_space_and_pointer_controls_stay_in_their_dock() {
        let expanded = layout(1280., 576., true, Panel::None, false, 88.);
        let collapsed = layout(1280., 576., false, Panel::None, false, 88.);
        assert!(collapsed.video.width > expanded.video.width);
        assert_eq!(collapsed.video.height, 576.);
        let pointer = layout(1280., 576., false, Panel::None, true, 88.)
            .mouse
            .unwrap();
        for key in [pointer.left, pointer.right, pointer.wheel] {
            assert!(
                key.y >= pointer.dock.y
                    && key.y + key.height <= pointer.dock.y + pointer.dock.height
            );
            assert!(key.x >= 0. && key.x + key.width <= pointer.dock.width);
        }
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
