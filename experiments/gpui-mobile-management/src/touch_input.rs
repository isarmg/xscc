//! Pure touch interpretation. Only edge events are immediate; motion is frame-coalesced.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point(pub f32, pub f32);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub bounds: (f32, f32, f32, f32),
    pub extent: (u32, u32),
    pub origin: (i32, i32),
}
impl Geometry {
    pub fn map(self, p: Point, captured: bool) -> Option<(i32, i32)> {
        let (left, top, width, height) = self.bounds;
        if ![left, top, width, height, p.0, p.1]
            .into_iter()
            .all(f32::is_finite)
            || width <= 0.
            || height <= 0.
            || self.extent.0 == 0
            || self.extent.1 == 0
        {
            return None;
        }
        let scale = (width / self.extent.0 as f32).min(height / self.extent.1 as f32);
        let x = (p.0 - left - (width - self.extent.0 as f32 * scale) / 2.) / scale;
        let y = (p.1 - top - (height - self.extent.1 as f32 * scale) / 2.) / scale;
        if !captured && (x < 0. || y < 0. || x >= self.extent.0 as f32 || y >= self.extent.1 as f32)
        {
            return None;
        }
        Some((
            self.origin
                .0
                .saturating_add(x.clamp(0., (self.extent.0 - 1) as f32) as i32),
            self.origin
                .1
                .saturating_add(y.clamp(0., (self.extent.1 - 1) as f32) as i32),
        ))
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Started,
    Moved,
    Ended,
    Cancelled,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Output {
    Move(i32, i32),
    Down(i32, i32),
    Up(i32, i32),
    Scroll(i32, i32),
    RightDown(i32, i32),
    RightUp(i32, i32),
    MiddleDown(i32, i32),
    MiddleUp(i32, i32),
}
#[derive(Default)]
pub struct TouchInput {
    contacts: Vec<(u64, Point)>,
    start: Option<Point>,
    geometry: Option<Geometry>,
    pressed: Option<(i32, i32)>,
    pending_move: Option<(i32, i32)>,
    scroll: Point,
    blocked: bool,
    right_click: bool,
}
impl TouchInput {
    pub fn arm_right_click(&mut self) -> bool {
        if !self.contacts.is_empty() {
            return false;
        }
        self.right_click = true;
        true
    }
    pub fn right_click_armed(&self) -> bool {
        self.right_click
    }

    pub fn pending(&self) -> bool {
        self.pending_move.is_some() || self.scroll.0.abs() >= 1. || self.scroll.1.abs() >= 1.
    }
    pub fn reset(&mut self) -> Vec<Output> {
        let release = self.pressed.take().map(|(x, y)| Output::Up(x, y));
        *self = Self::default();
        release.into_iter().collect()
    }
    pub fn flush(&mut self) -> Vec<Output> {
        let mut output = Vec::new();
        if let Some((x, y)) = self.pending_move.take() {
            self.pressed = Some((x, y));
            output.push(Output::Move(x, y));
        }
        let x = self.scroll.0.trunc().clamp(-120., 120.) as i32;
        let y = self.scroll.1.trunc().clamp(-120., 120.) as i32;
        self.scroll.0 -= x as f32;
        self.scroll.1 -= y as f32;
        if x != 0 || y != 0 {
            output.push(Output::Scroll(x, y));
        }
        output
    }
    pub fn cancel(&mut self) -> Vec<Output> {
        self.right_click = false;
        self.blocked = true;
        self.pending_move = None;
        self.scroll = Point::default();
        self.pressed
            .take()
            .map(|(x, y)| Output::Up(x, y))
            .into_iter()
            .collect()
    }
    pub fn event(
        &mut self,
        id: u64,
        phase: Phase,
        point: Point,
        geometry: Geometry,
    ) -> Vec<Output> {
        let mut output = Vec::new();
        if self.geometry.is_some_and(|old| old != geometry) {
            output.extend(self.cancel());
        }
        match phase {
            Phase::Started => {
                if self.contacts.iter().any(|(old, _)| *old == id) {
                    return output;
                }
                if self.contacts.is_empty() {
                    self.geometry = Some(geometry);
                    self.start = Some(point);
                    self.blocked = geometry.map(point, false).is_none();
                }
                self.contacts.push((id, point));
                if geometry.map(point, false).is_none() {
                    output.extend(self.cancel());
                }
                if self.right_click && self.contacts.len() > 1 {
                    output.extend(self.cancel());
                }
                if self.contacts.len() == 2 {
                    output.extend(self.flush());
                    output.extend(self.pressed.take().map(|(x, y)| Output::Up(x, y)));
                } else if self.contacts.len() > 2 {
                    output.extend(self.cancel());
                }
            }
            Phase::Moved => {
                let Some(index) = self.contacts.iter().position(|(old, _)| *old == id) else {
                    return output;
                };
                let previous = self.contacts[index].1;
                self.contacts[index].1 = point;
                if self.blocked {
                    return output;
                }
                if !point.0.is_finite() || !point.1.is_finite() {
                    return self.cancel();
                }
                if self.right_click {
                    if geometry.map(point, false).is_none()
                        || self
                            .start
                            .is_none_or(|start| (point.0 - start.0).hypot(point.1 - start.1) >= 8.)
                    {
                        output.extend(self.cancel());
                    }
                    return output;
                }
                if self.contacts.len() == 2 {
                    // Each finger contributes half of the centroid delta. Bound queued work.
                    self.scroll.0 =
                        (self.scroll.0 + (point.0 - previous.0) / 2.).clamp(-240., 240.);
                    self.scroll.1 =
                        (self.scroll.1 + (point.1 - previous.1) / 2.).clamp(-240., 240.);
                } else if let Some(start) = self.start {
                    if self.pressed.is_none() && (point.0 - start.0).hypot(point.1 - start.1) >= 8.
                    {
                        if let Some((x, y)) = geometry.map(start, false) {
                            output.extend([Output::Move(x, y), Output::Down(x, y)]);
                            self.pressed = Some((x, y));
                        }
                    }
                    if self.pressed.is_some() {
                        self.pending_move = geometry.map(point, true);
                    }
                }
            }
            Phase::Ended | Phase::Cancelled => {
                let Some(index) = self.contacts.iter().position(|(old, _)| *old == id) else {
                    return output;
                };
                if phase == Phase::Cancelled {
                    output.extend(self.cancel());
                } else if !self.blocked {
                    if self.contacts.len() == 1 {
                        if self.pressed.is_some() {
                            self.pending_move = geometry.map(point, true).or(self.pressed);
                            output.extend(self.flush());
                            output.extend(self.pressed.take().map(|(x, y)| Output::Up(x, y)));
                        } else if self
                            .start
                            .is_some_and(|start| (point.0 - start.0).hypot(point.1 - start.1) < 8.)
                            && let Some((x, y)) = geometry.map(point, false)
                        {
                            output.push(Output::Move(x, y));
                            if self.right_click {
                                output.extend([Output::RightDown(x, y), Output::RightUp(x, y)]);
                            } else {
                                output.extend([Output::Down(x, y), Output::Up(x, y)]);
                            }
                        }
                    } else {
                        output.extend(self.flush());
                        self.blocked = true;
                    }
                }
                self.contacts.remove(index);
                if self.contacts.is_empty() {
                    output.extend(self.reset());
                }
            }
        }
        output
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn geometry() -> Geometry {
        Geometry {
            bounds: (0., 0., 200., 200.),
            extent: (200, 100),
            origin: (-200, 10),
        }
    }
    #[test]
    fn contain_rejects_bars_and_clamps_captured_release() {
        let g = geometry();
        assert_eq!(g.map(Point(50., 20.), false), None);
        assert_eq!(g.map(Point(50., 50.), false), Some((-150, 10)));
        assert_eq!(g.map(Point(250., 250.), true), Some((-1, 109)));
        let mut input = TouchInput::default();
        assert!(
            input
                .event(1, Phase::Started, Point(50., 20.), g)
                .is_empty()
        );
        assert!(input.event(1, Phase::Ended, Point(50., 80.), g).is_empty());
    }
    #[test]
    fn tap_drag_scroll_and_cancellation_have_balanced_edges() {
        let g = geometry();
        let mut i = TouchInput::default();
        i.event(1, Phase::Started, Point(50., 80.), g);
        assert_eq!(
            i.event(1, Phase::Ended, Point(50., 80.), g),
            vec![
                Output::Move(-150, 40),
                Output::Down(-150, 40),
                Output::Up(-150, 40)
            ]
        );
        i.event(2, Phase::Started, Point(50., 80.), g);
        assert_eq!(
            i.event(2, Phase::Moved, Point(70., 80.), g),
            vec![Output::Move(-150, 40), Output::Down(-150, 40)]
        );
        assert_eq!(
            i.event(3, Phase::Started, Point(100., 80.), g),
            vec![Output::Move(-130, 40), Output::Up(-130, 40)]
        );
        i.event(3, Phase::Moved, Point(100., 100.), g);
        assert_eq!(i.flush(), vec![Output::Scroll(0, 10)]);
        assert!(i.event(4, Phase::Started, Point(110., 100.), g).is_empty());
        assert!(i.event(2, Phase::Cancelled, Point(70., 80.), g).is_empty());
        assert!(i.event(3, Phase::Ended, Point(100., 100.), g).is_empty());
        assert!(i.event(4, Phase::Ended, Point(110., 100.), g).is_empty());
    }
    #[test]
    fn motion_is_coalesced_and_reset_rejects_late_events() {
        let g = geometry();
        let mut i = TouchInput::default();
        i.event(1, Phase::Started, Point(50., 80.), g);
        i.event(1, Phase::Moved, Point(70., 80.), g);
        assert!(i.event(1, Phase::Moved, Point(80., 80.), g).is_empty());
        assert_eq!(i.flush(), vec![Output::Move(-120, 40)]);
        assert_eq!(
            i.event(1, Phase::Ended, Point(300., 300.), g),
            vec![Output::Move(-1, 109), Output::Up(-1, 109)]
        );
        i.event(2, Phase::Started, Point(50., 80.), g);
        i.event(2, Phase::Moved, Point(70., 80.), g);
        assert_eq!(i.reset(), vec![Output::Up(-150, 40)]);
        assert!(i.event(2, Phase::Moved, Point(90., 80.), g).is_empty());
        assert!(i.event(2, Phase::Ended, Point(90., 80.), g).is_empty());
        assert!(i.flush().is_empty());
        i.event(3, Phase::Started, Point(50., 80.), g);
        i.event(3, Phase::Moved, Point(70., 80.), g);
        assert_eq!(
            i.event(3, Phase::Cancelled, Point(70., 80.), g),
            vec![Output::Up(-150, 40)]
        );
        assert!(i.flush().is_empty());
        i.event(4, Phase::Started, Point(50., 80.), g);
        i.event(4, Phase::Moved, Point(70., 80.), g);
        assert_eq!(i.cancel(), vec![Output::Up(-150, 40)]);
        assert!(i.event(5, Phase::Started, Point(50., 80.), g).is_empty());
        assert!(i.event(5, Phase::Ended, Point(50., 80.), g).is_empty());
        assert!(i.event(4, Phase::Ended, Point(70., 80.), g).is_empty());
    }
    #[test]
    fn one_shot_right_click_is_balanced_and_next_tap_is_left() {
        let mut input = TouchInput::default();
        let g = geometry();
        assert!(input.arm_right_click());
        assert!(input.right_click_armed());
        assert!(
            input
                .event(1, Phase::Started, Point(50., 80.), g)
                .is_empty()
        );
        assert!(!input.arm_right_click());
        assert_eq!(
            input.event(1, Phase::Ended, Point(50., 80.), g),
            vec![
                Output::Move(-150, 40),
                Output::RightDown(-150, 40),
                Output::RightUp(-150, 40)
            ]
        );
        assert!(!input.right_click_armed());
        input.event(2, Phase::Started, Point(50., 80.), g);
        assert_eq!(
            input.event(2, Phase::Ended, Point(50., 80.), g),
            vec![
                Output::Move(-150, 40),
                Output::Down(-150, 40),
                Output::Up(-150, 40)
            ]
        );
    }

    #[test]
    fn right_click_rejects_bars_drag_multitouch_cancel_and_reset() {
        let g = geometry();
        for scenario in 0..6 {
            let mut input = TouchInput::default();
            assert!(input.arm_right_click());
            let start = if scenario == 0 {
                Point(50., 20.)
            } else {
                Point(50., 80.)
            };
            assert!(input.event(1, Phase::Started, start, g).is_empty());
            let out = match scenario {
                0 => vec![],
                1 => input.event(1, Phase::Moved, Point(70., 80.), g),
                2 => input.event(2, Phase::Started, Point(60., 80.), g),
                3 => input.event(1, Phase::Cancelled, Point(50., 80.), g),
                4 => input.cancel(),
                _ => input.reset(),
            };
            assert!(out.is_empty());
            assert!(!input.right_click_armed());
            assert!(input.event(1, Phase::Ended, Point(50., 80.), g).is_empty());
            assert!(input.event(2, Phase::Ended, Point(60., 80.), g).is_empty());
            assert!(input.flush().is_empty());
        }
    }
}
