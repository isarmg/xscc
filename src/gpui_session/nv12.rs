use super::{Frame, Mailbox};
use std::time::Instant;

pub(super) fn take_frame(
    display: usize,
    image: &mut scrap::ImageRgb,
    mailbox: &Mailbox,
) -> Option<Frame> {
    if image.w == 0 || image.h == 0 || image.w > 16384 || image.h > 16384 {
        return None;
    }
    let y_len = image.w.checked_mul(image.h)?;
    let uv_len = image
        .w
        .div_ceil(2)
        .checked_mul(2)?
        .checked_mul(image.h.div_ceil(2))?;
    if image.raw.len() != y_len.checked_add(uv_len)? {
        return None;
    }
    let mut replacement = mailbox.recycle.pop().unwrap_or_default();
    replacement.resize(image.raw.len(), 0);
    Some(Frame {
        display,
        width: image.w as u32,
        height: image.h as u32,
        stride: image.w,
        bgra: std::mem::replace(&mut image.raw, replacement),
        nv12: true,
        ready_at: Instant::now(),
        metrics_epoch: mailbox.metrics.lock().unwrap().epoch(),
        recycle: mailbox.recycle.clone(),
        metrics: mailbox.metrics.clone(),
    })
}
