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
                    paint_arrow(placement.point, window);
                },
            );
        }
    }

    fn paint_arrow(origin: Point<Pixels>, window: &mut Window) {
        for stroke in [false, true] {
            let mut path = if stroke {
                PathBuilder::stroke(px(1.5))
            } else {
                PathBuilder::fill()
            };
            path.move_to(origin);
            for (x, y) in [
                (0.0, 20.0),
                (5.0, 15.0),
                (9.0, 24.0),
                (13.0, 22.0),
                (9.0, 13.0),
                (17.0, 13.0),
            ] {
                path.line_to(origin + point(px(x), px(y)));
            }
            path.close();
            match path.build() {
                Ok(path) => window.paint_path(path, rgb(if stroke { 0x101010 } else { 0xffffff })),
                Err(error) => {
                    gpui_session::log_cursor_render_failure(&error);
                    window.paint_quad(fill(
                        Bounds::new(origin, size(px(3.0), px(3.0))),
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
