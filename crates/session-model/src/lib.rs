//! Frame ownership boundary shared by decoders and native presentation.
#![forbid(unsafe_code)]

/// A complete decoder-owned frame. Presentation retains the allocation until
/// the image is retired; the receipt records UI submission, not GPU scanout.
pub trait FrameBuffer: AsRef<[u8]> + Send + Sync + 'static {
    fn extent(&self) -> (u32, u32);
    fn stride(&self) -> usize;
    fn submission(&self) -> impl FnOnce() + 'static;
}
