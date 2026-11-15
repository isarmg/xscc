use gpui::{Bounds, Pixels, Point, point, px, size};

struct Placement {
    frame: Bounds<Pixels>,
    point: Point<Pixels>,
    scale: f32,
}

fn placement(
    viewport: Bounds<Pixels>,
    extent: (u32, u32),
    origin: (i32, i32),
    local_pointer: Option<(i32, i32)>,
    remote_pointer: Option<(i32, i32)>,
) -> Option<Placement> {
    let width = f32::from(viewport.size.width);
    let height = f32::from(viewport.size.height);
    if ![
        f32::from(viewport.origin.x),
        f32::from(viewport.origin.y),
        width,
        height,
    ]
    .into_iter()
    .all(f32::is_finite)
        || width <= 0.0
        || height <= 0.0
        || extent.0 == 0
        || extent.1 == 0
    {
        return None;
    }
    let (x, y) = local_pointer.or(remote_pointer).map_or(
        (i64::from(extent.0 / 2), i64::from(extent.1 / 2)),
        |(x, y)| {
            (
                i64::from(x) - i64::from(origin.0),
                i64::from(y) - i64::from(origin.1),
            )
        },
    );
    if x < 0 || y < 0 || x >= i64::from(extent.0) || y >= i64::from(extent.1) {
        return None;
    }
    let scale = (width / extent.0 as f32).min(height / extent.1 as f32);
    let frame = Bounds::new(
        viewport.origin
            + point(
                px((width - extent.0 as f32 * scale) / 2.0),
                px((height - extent.1 as f32 * scale) / 2.0),
            ),
        size(px(extent.0 as f32 * scale), px(extent.1 as f32 * scale)),
    );
    Some(Placement {
        frame,
        point: frame.origin + point(px(x as f32 * scale), px(y as f32 * scale)),
        scale,
    })
}

fn image_bounds(placement: &Placement, extent: (u32, u32), hotspot: (i32, i32)) -> Bounds<Pixels> {
    Bounds::new(
        placement.point
            - point(
                px(hotspot.0 as f32 * placement.scale),
                px(hotspot.1 as f32 * placement.scale),
            ),
        size(
            px(extent.0 as f32 * placement.scale),
            px(extent.1 as f32 * placement.scale),
        ),
    )
}

fn arrow_points(placement: &Placement, local_pointer: bool) -> [Point<Pixels>; 7] {
    let origin = placement.point;
    let horizontal = if local_pointer && origin.x + px(17.) > placement.frame.right() {
        -1.
    } else {
        1.
    };
    let vertical = if local_pointer && origin.y + px(24.) > placement.frame.bottom() {
        -1.
    } else {
        1.
    };
    [
        (0., 0.),
        (0., 20.),
        (5., 15.),
        (9., 24.),
        (13., 22.),
        (9., 13.),
        (17., 13.),
    ]
    .map(|(x, y)| origin + point(px(x * horizontal), px(y * vertical)))
}

#[cfg(all(
    any(target_os = "android", target_os = "ios"),
    feature = "mobile-session"
))]
mod native {
    use super::*;
    use gpui::{ContentMask, PathBuilder, RenderImage, Window, fill, rgb};
    use librustdesk::gpui_session::{self, CursorShape, CursorSnapshot};
    use std::sync::Arc;

    #[derive(Default)]
    pub struct CursorPresentation {
        image: Option<(Arc<CursorShape>, Arc<RenderImage>)>,
    }

    #[derive(Clone, Default)]
    pub struct PreparedCursor {
        visible: bool,
        local_pointer: Option<(i32, i32)>,
        remote_pointer: Option<(i32, i32)>,
        image: Option<(Arc<CursorShape>, Arc<RenderImage>)>,
    }

    impl CursorPresentation {
        pub fn clear(&mut self, window: &mut Window) -> Result<(), String> {
            if let Some((_, image)) = self.image.take() {
                window
                    .drop_image(image)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        }

        pub fn prepare(
            &mut self,
            snapshot: Option<&CursorSnapshot>,
            local_pointer: Option<(i32, i32)>,
            window: &mut Window,
        ) -> Result<PreparedCursor, String> {
            // A phone needs its own readable pointer even when the host hides its cursor.
            if local_pointer.is_some() {
                return Ok(PreparedCursor {
                    visible: true,
                    local_pointer,
                    ..Default::default()
                });
            }
            let Some(snapshot) = snapshot else {
                self.clear(window)?;
                return Ok(PreparedCursor::default());
            };
            let unchanged = match (&self.image, &snapshot.shape) {
                (Some((previous, _)), Some(next)) => Arc::ptr_eq(previous, next),
                (None, None) => true,
                _ => false,
            };
            if !unchanged {
                self.clear(window)?;
                if let Some(shape) = &snapshot.shape {
                    let image =
                        RenderImage::from_bgra(shape.width, shape.height, shape.bgra.clone())
                            .map_err(|error| error.to_string())?;
                    self.image = Some((shape.clone(), Arc::new(image)));
                }
            }
            Ok(PreparedCursor {
                visible: true,
                local_pointer,
                remote_pointer: snapshot.position,
                image: self.image.clone(),
            })
        }
    }

    impl PreparedCursor {
        pub fn paint(
            &self,
            bounds: Bounds<Pixels>,
            extent: (u32, u32),
            origin: (i32, i32),
            window: &mut Window,
        ) {
            if !self.visible {
                return;
            }
            let Some(placement) = placement(
                bounds,
                extent,
                origin,
                self.local_pointer,
                self.remote_pointer,
            ) else {
                return;
            };
            if let Some((shape, image)) = &self.image {
                let image_bounds = image_bounds(
                    &placement,
                    (shape.width, shape.height),
                    (shape.hotx, shape.hoty),
                );
                match window.paint_image(
                    placement.frame,
                    image_bounds,
                    Default::default(),
                    image.clone(),
                    0,
                    false,
                ) {
                    Ok(()) => return,
                    Err(error) => gpui_session::log_cursor_render_failure(&error),
                }
            }
            window.with_content_mask(
                Some(ContentMask {
                    bounds: placement.frame,
                }),
                |window| {
                    paint_arrow(&placement, self.local_pointer.is_some(), window);
                },
            );
        }
    }

    fn paint_arrow(placement: &Placement, local_pointer: bool, window: &mut Window) {
        let points = arrow_points(placement, local_pointer);
        for stroke in [false, true] {
            let mut path = if stroke {
                PathBuilder::stroke(px(1.5))
            } else {
                PathBuilder::fill()
            };
            path.move_to(points[0]);
            for &point in &points[1..] {
                path.line_to(point);
            }
            path.close();
            match path.build() {
                Ok(path) => window.paint_path(path, rgb(if stroke { 0x101010 } else { 0xffffff })),
                Err(error) => {
                    gpui_session::log_cursor_render_failure(&error);
                    window.paint_quad(fill(
                        Bounds::new(placement.point, size(px(3.0), px(3.0))),
                        rgb(0xffffff),
                    ));
                    break;
                }
            }
        }
    }
}

#[cfg(all(
    any(target_os = "android", target_os = "ios"),
    feature = "mobile-session"
))]
pub use native::{CursorPresentation, PreparedCursor};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_pointer_beats_delayed_remote_position_with_letterbox_and_negative_origin() {
        let viewport = Bounds::new(point(px(50.0), px(20.0)), size(px(1000.0), px(800.0)));
        let result = placement(
            viewport,
            (2000, 1000),
            (-2000, -100),
            Some((-1800, 100)),
            Some((-1700, 200)),
        )
        .unwrap();
        assert_eq!(
            result.frame,
            Bounds::new(point(px(50.0), px(170.0)), size(px(1000.0), px(500.0)))
        );
        assert_eq!(result.point, point(px(150.0), px(270.0)));
        assert_eq!(
            image_bounds(&result, (32, 32), (4, 8)),
            Bounds::new(point(px(148.0), px(266.0)), size(px(16.0), px(16.0)))
        );
    }

    #[test]
    fn inactive_local_pointer_uses_remote_position_and_missing_position_starts_centered() {
        let viewport = Bounds::new(point(px(0.0), px(0.0)), size(px(1000.0), px(400.0)));
        let remote = placement(
            viewport,
            (1000, 2000),
            (1920, -2000),
            None,
            Some((2420, -1900)),
        )
        .unwrap();
        assert_eq!(
            remote.frame,
            Bounds::new(point(px(400.0), px(0.0)), size(px(200.0), px(400.0)))
        );
        assert_eq!(remote.point, point(px(500.0), px(20.0)));
        let centered = placement(viewport, (1000, 2000), (1920, -2000), None, None).unwrap();
        assert_eq!(centered.point, point(px(500.0), px(200.0)));
    }

    #[test]
    fn local_arrow_tip_matches_click_after_relative_motion_without_a_remote_cursor() {
        use crate::{
            pointer_input::TrackpadInput,
            touch_input::{Geometry, Output, Phase, Point as TouchPoint},
        };

        let viewport = Bounds::new(point(px(50.), px(20.)), size(px(1000.), px(800.)));
        let geometry = Geometry {
            bounds: (50., 20., 1000., 800.),
            extent: (2000, 1000),
            origin: (-2000, -100),
        };
        let mut pointer = TrackpadInput::default();
        pointer.configure(geometry);
        pointer.event(1, Phase::Started, TouchPoint(500., 400.), geometry);
        pointer.event(1, Phase::Moved, TouchPoint(550., 375.), geometry);
        pointer.event(1, Phase::Ended, TouchPoint(550., 375.), geometry);
        pointer.event(2, Phase::Started, TouchPoint(600., 400.), geometry);
        let clicks = pointer.event(2, Phase::Ended, TouchPoint(600., 400.), geometry);
        let position = pointer.snapshot().position;
        let (x, y) = position.unwrap();
        assert!(clicks.contains(&Output::Down(x, y)));
        assert!(clicks.contains(&Output::Up(x, y)));
        let placed = placement(viewport, geometry.extent, geometry.origin, position, None).unwrap();
        let expected = point(
            px(50. + (x + 2000) as f32 * 0.5),
            px(170. + (y + 100) as f32 * 0.5),
        );
        assert_eq!(arrow_points(&placed, true)[0], expected);
    }

    #[test]
    fn local_arrow_body_remains_in_view_at_all_four_screen_corners() {
        let viewport = Bounds::new(point(px(50.), px(20.)), size(px(1000.), px(800.)));
        for position in [(-2000, -100), (-1, -100), (-2000, 899), (-1, 899)] {
            let placed =
                placement(viewport, (2000, 1000), (-2000, -100), Some(position), None).unwrap();
            let points = arrow_points(&placed, true);
            assert_eq!(points[0], placed.point);
            assert!(
                points
                    .into_iter()
                    .all(|point| placed.frame.contains(&point))
            );
        }
    }

    #[test]
    fn stale_display_position_and_invalid_viewport_never_produce_a_pointer() {
        let viewport = Bounds::new(point(px(0.0), px(0.0)), size(px(1000.0), px(400.0)));
        assert!(placement(viewport, (1000, 2000), (1920, 0), None, Some((100, 100))).is_none());
        assert!(placement(viewport, (0, 2000), (0, 0), None, None).is_none());
        let invalid = Bounds::new(point(px(f32::NAN), px(0.0)), viewport.size);
        assert!(placement(invalid, (1000, 2000), (0, 0), None, None).is_none());
        assert!(
            placement(
                viewport,
                (1000, 2000),
                (i32::MIN, i32::MIN),
                None,
                Some((i32::MAX, i32::MAX))
            )
            .is_none()
        );
    }
}
