use base::message_proto::CursorData;
use std::{collections::VecDeque, sync::Arc, time::Duration};

pub fn log_cursor_render_failure(error: &dyn std::fmt::Display) {
    hbb_common::throttled_log!(
        Duration::from_secs(5),
        warn,
        "Remote cursor rendering failed: {error}"
    );
}

const MAX_SHAPES: usize = 128;
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct CursorShape {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub hotx: i32,
    pub hoty: i32,
    pub bgra: Arc<[u8]>,
}

#[derive(Clone, Debug, Default)]
pub struct CursorSnapshot {
    pub shape: Option<Arc<CursorShape>>,
    pub position: Option<(i32, i32)>,
}

#[derive(Default)]
pub(super) struct Cursors {
    shapes: VecDeque<Arc<CursorShape>>,
    bytes: usize,
    selected: Option<u64>,
    position: Option<(i32, i32)>,
}

impl Cursors {
    pub fn insert(&mut self, data: CursorData) {
        self.selected = Some(data.id);
        if !(1..=512).contains(&data.width)
            || !(1..=512).contains(&data.height)
            || !(0..data.width).contains(&data.hotx)
            || !(0..data.height).contains(&data.hoty)
            || data.colors.len() != data.width as usize * data.height as usize * 4
        {
            self.selected = None;
            return;
        }
        let previous = self.shapes.iter().position(|shape| shape.id == data.id);
        if let Some(previous) = previous.and_then(|index| self.shapes.remove(index)) {
            self.bytes -= previous.bgra.len();
        }
        let mut bgra = data.colors.to_vec();
        for pixel in bgra.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        while self.shapes.len() >= MAX_SHAPES || self.bytes + bgra.len() > MAX_BYTES {
            if let Some(previous) = self.shapes.pop_front() {
                self.bytes -= previous.bgra.len();
            } else {
                break;
            }
        }
        self.bytes += bgra.len();
        self.shapes.push_back(Arc::new(CursorShape {
            id: data.id,
            width: data.width as u32,
            height: data.height as u32,
            hotx: data.hotx,
            hoty: data.hoty,
            bgra: bgra.into(),
        }));
    }

    pub fn select(&mut self, id: Option<u64>) -> bool {
        let changed = self.selected != id;
        self.selected = id;
        let selected = self.shapes.iter().position(|shape| Some(shape.id) == id);
        if let Some(shape) = selected.and_then(|index| self.shapes.remove(index)) {
            self.shapes.push_back(shape);
        }
        changed
    }

    pub fn position(&mut self, position: (i32, i32)) -> bool {
        let changed = self.position != Some(position);
        self.position = Some(position);
        changed
    }

    pub fn snapshot(&self) -> CursorSnapshot {
        CursorSnapshot {
            shape: self
                .shapes
                .iter()
                .find(|shape| Some(shape.id) == self.selected)
                .cloned(),
            position: self.position,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(id: u64, size: i32) -> CursorData {
        CursorData {
            id,
            width: size,
            height: size,
            colors: [10, 20, 30, 255]
                .repeat(size as usize * size as usize)
                .into(),
            ..Default::default()
        }
    }

    #[test]
    fn cursor_selection_reuses_shape_and_unknown_id_cannot_show_old_cursor() {
        let mut cursors = Cursors::default();
        cursors.insert(shape(u64::MAX, 1));
        cursors.position((-1920, 4));
        let snapshot = cursors.snapshot();
        let pixels = snapshot.shape.unwrap();
        assert_eq!(&*pixels.bgra, &[30, 20, 10, 255]);
        cursors.select(Some(42));
        assert!(cursors.snapshot().shape.is_none());
        cursors.select(Some(u64::MAX));
        assert!(Arc::ptr_eq(&pixels, &cursors.snapshot().shape.unwrap()));
        assert_eq!(cursors.snapshot().position, Some((-1920, 4)));
    }

    #[test]
    fn cursor_cache_bounds_memory_and_keeps_recently_selected_shapes() {
        let mut cursors = Cursors::default();
        for id in 0..MAX_SHAPES as u64 {
            cursors.insert(shape(id, 1));
        }
        cursors.select(Some(0));
        cursors.insert(shape(MAX_SHAPES as u64, 1));
        cursors.select(Some(1));
        assert!(cursors.snapshot().shape.is_none());
        cursors.select(Some(0));
        assert!(cursors.snapshot().shape.is_some());
        for id in 1000..1020 {
            cursors.insert(shape(id, 512));
        }
        assert!(cursors.shapes.len() <= MAX_SHAPES && cursors.bytes <= MAX_BYTES);
        assert!(cursors.snapshot().shape.is_some());
        cursors.select(Some(1000));
        assert!(cursors.snapshot().shape.is_none());
    }

    #[test]
    fn malformed_cursor_pixels_never_reach_presentation() {
        let mut cursors = Cursors::default();
        let mut cursor = shape(7, 1);
        cursor.hotx = 1;
        cursors.insert(cursor);
        assert!(cursors.snapshot().shape.is_none());
        let mut cursor = shape(8, 1);
        cursor.colors = vec![0; 3].into();
        cursors.insert(cursor);
        assert!(cursors.snapshot().shape.is_none());
    }
}
