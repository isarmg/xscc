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
