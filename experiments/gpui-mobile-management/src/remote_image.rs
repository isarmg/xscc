use gpui::{ImageSource, Img, ObjectFit, RenderImage, Styled, StyledImage, img};
use std::sync::Arc;

pub fn remote_image(image: Arc<RenderImage>) -> Img {
    // Absolute layout fixes both axes before Image's intrinsic aspect ratio is applied.
    // Video rendering and the sibling touch canvas therefore share the same viewport.
    img(ImageSource::Render(image))
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .object_fit(ObjectFit::Contain)
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    use super::*;
    use crate::touch_input::{Geometry, Point as TouchPoint};
    use gpui_test_support::{
        Bounds, DevicePixels, IntoElement, ParentElement, TestAppContext, canvas, div, point, px,
        size,
    };

    #[gpui_test_support::test]
    fn contained_video_paints_complete_frame_in_the_actual_touch_stage(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        for (width, height, left, top) in [(736., 360., 0., 0.), (276., 640., 13., 17.)] {
            let image =
                Arc::new(RenderImage::from_bgra(1920, 1080, vec![255; 1920 * 1080 * 4]).unwrap());
            let stage = std::rc::Rc::new(std::cell::Cell::new(None));
            let captured_stage = stage.clone();
            // Match the product's fixed absolute video parent and independent touch canvas.
            window.draw(
                point(px(0.), px(0.)),
                size(px(width + 64.), px(height + top)),
                |_, _| {
                    div()
                        .relative()
                        .w(px(width + 64.))
                        .h(px(height + top))
                        .overflow_hidden()
                        .child(
                            div()
                                .absolute()
                                .left(px(left))
                                .top(px(top))
                                .w(px(width))
                                .h(px(height))
                                .overflow_hidden()
                                .child(remote_image(image)),
                        )
                        .child(
                            canvas(
                                move |bounds, _, _| captured_stage.set(Some(bounds)),
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .left(px(left))
                            .top(px(top))
                            .w(px(width))
                            .h(px(height)),
                        )
                        .into_any_element()
                },
            );
            let (sprite, dpi) = window.update(|window, _| {
                (
                    *window
                        .painted_images()
                        .last()
                        .expect("video must paint a sprite"),
                    window.scale_factor(),
                )
            });
            let stage = stage.get().expect("the real Stage canvas must prepaint");
            let scale = (width / 1920.).min(height / 1080.);
            let expected = Bounds::new(
                point(
                    px(left + (width - 1920. * scale) / 2.),
                    px(top + (height - 1080. * scale) / 2.),
                ),
                size(px(1920. * scale), px(1080. * scale)),
            );
            eprintln!(
                "remote-image stage={stage:?}, sprite={:?}, mask={:?}, tile={:?}, dpi={dpi}",
                sprite.bounds, sprite.content_mask.bounds, sprite.tile.bounds
            );
            assert_eq!(
                stage,
                Bounds::new(point(px(left), px(top)), size(px(width), px(height)))
            );
            let expected_scaled = Bounds::new(
                expected.origin.map(|value| value.scale(dpi)),
                expected.size.map(|value| value.scale(dpi)),
            );
            for (actual, expected) in [
                (sprite.bounds.left().0, expected_scaled.left().0),
                (sprite.bounds.top().0, expected_scaled.top().0),
                (sprite.bounds.right().0, expected_scaled.right().0),
                (sprite.bounds.bottom().0, expected_scaled.bottom().0),
            ] {
                assert!(
                    (actual - expected).abs() <= 0.51,
                    "actual Image paint must fit the touch stage, allowing only half a device pixel of snapping: {actual} vs {expected}"
                );
            }
            assert_eq!(
                sprite.tile.bounds.size,
                size(DevicePixels(1920), DevicePixels(1080)),
                "all source-frame pixels must remain in the sprite UV tile"
            );
            assert_eq!(
                sprite.bounds.intersect(&sprite.content_mask.bounds),
                sprite.bounds,
                "the actual frame bottom must remain inside the parent's clip"
            );

            let geometry = Geometry {
                bounds: (
                    stage.origin.x.into(),
                    stage.origin.y.into(),
                    stage.size.width.into(),
                    stage.size.height.into(),
                ),
                extent: (1920, 1080),
                origin: (-1920, 40),
            };
            // Sample the center of the *actual emitted sprite*, not a separately built rectangle.
            let sample = TouchPoint(
                (sprite.bounds.origin.x.0 + sprite.bounds.size.width.0 / 2.) / dpi,
                (sprite.bounds.origin.y.0 + sprite.bounds.size.height.0 / 2.) / dpi,
            );
            assert_eq!(
                geometry.map(sample, false),
                Some((-960, 580)),
                "touch at the actual image center must address the source-frame center"
            );
        }
    }
}
