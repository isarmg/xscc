//! Relative touchpad gestures; pointer motion and scrolling are display-frame coalesced.
use crate::touch_input::{Geometry, Output, Phase, Point};

const TAP_SLOP: f64 = 8.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PointerSnapshot {
    pub position: Option<(i32, i32)>,
    pub left_down: bool,
    pub right_down: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Gesture {
    #[default]
    Idle,
    SingleTap,
    PointerMove,
    TwoTap,
    TwoScroll,
    Blocked,
}

struct Contact {
    id: u64,
    start: Point,
    point: Point,
}

#[derive(Default)]
pub struct TrackpadInput {
    geometry: Option<Geometry>,
    position: Option<(f64, f64)>,
    contacts: Vec<Contact>,
    gesture: Gesture,
    pending_move: Option<(i32, i32)>,
    scroll: (f64, f64),
    tap_scroll: (f64, f64),
    left: Option<(i32, i32)>,
    right: Option<(i32, i32)>,
}

impl TrackpadInput {
    pub fn snapshot(&self) -> PointerSnapshot {
        PointerSnapshot {
            position: self.global_position(),
            left_down: self.left.is_some(),
            right_down: self.right.is_some(),
        }
    }

    pub fn configure(&mut self, geometry: Geometry) -> Vec<Output> {
        if self.geometry == Some(geometry) {
            return Vec::new();
        }
        let old = self.geometry;
        let output = self.cancel();
        if contain_scale(geometry).is_none() {
            self.geometry = None;
            self.position = None;
            return output;
        }
        self.geometry = Some(geometry);
        if self.position.is_none()
            || old.is_none_or(|old| old.extent != geometry.extent || old.origin != geometry.origin)
        {
            self.position = Some((
                f64::from(geometry.extent.0 - 1) / 2.,
                f64::from(geometry.extent.1 - 1) / 2.,
            ));
        }
        self.clamp_position();
        output
    }

    pub fn set_position(&mut self, position: (i32, i32)) -> bool {
        let Some(geometry) = self.geometry else {
            return false;
        };
        if !self.contacts.is_empty()
            || self.left.is_some()
            || self.right.is_some()
            || self.pending()
            || i64::from(position.0) < i64::from(geometry.origin.0)
            || i64::from(position.1) < i64::from(geometry.origin.1)
            || i64::from(position.0) >= i64::from(geometry.origin.0) + i64::from(geometry.extent.0)
            || i64::from(position.1) >= i64::from(geometry.origin.1) + i64::from(geometry.extent.1)
        {
            return false;
        }
        self.position = Some((
            f64::from(position.0) - f64::from(geometry.origin.0),
            f64::from(position.1) - f64::from(geometry.origin.1),
        ));
        true
    }

    pub fn scroll_by(&mut self, dx: f32, dy: f32) -> Vec<Output> {
        if !dx.is_finite() || !dy.is_finite() {
            return self.cancel();
        }
        if self.geometry.is_some() {
            self.scroll.0 = (self.scroll.0 + f64::from(dx)).clamp(-240., 240.);
            self.scroll.1 = (self.scroll.1 + f64::from(dy)).clamp(-240., 240.);
            if self.scroll.0.abs() >= 1. || self.scroll.1.abs() >= 1. {
                self.gesture = match self.gesture {
                    Gesture::SingleTap => Gesture::PointerMove,
                    Gesture::TwoTap => {
                        self.tap_scroll = (0., 0.);
                        Gesture::TwoScroll
                    }
                    gesture => gesture,
                };
            }
        }
        Vec::new()
    }

    pub fn pending(&self) -> bool {
        self.pending_move.is_some() || self.scroll.0.abs() >= 1. || self.scroll.1.abs() >= 1.
    }

    pub fn flush(&mut self) -> Vec<Output> {
        let mut output = Vec::new();
        if let Some(position) = self.pending_move.take() {
            self.emit_move(position, &mut output);
        }
        let x = self.scroll.0.trunc().clamp(-120., 120.) as i32;
        let y = self.scroll.1.trunc().clamp(-120., 120.) as i32;
        self.scroll.0 -= f64::from(x);
        self.scroll.1 -= f64::from(y);
        if (x != 0 || y != 0)
            && let Some(position) = self.global_position()
        {
            self.emit_move(position, &mut output);
            output.push(Output::Scroll(x, y));
        }
        output
    }

    pub fn set_button(&mut self, button: PointerButton, pressed: bool) -> Vec<Output> {
        let held = match button {
            PointerButton::Left => self.left.is_some(),
            PointerButton::Right => self.right.is_some(),
        };
        if held == pressed || self.global_position().is_none() {
            return Vec::new();
        }
        let mut output = self.flush();
        if pressed {
            self.gesture = match self.gesture {
                Gesture::SingleTap => Gesture::PointerMove,
                Gesture::TwoTap => {
                    self.tap_scroll = (0., 0.);
                    Gesture::TwoScroll
                }
                gesture => gesture,
            };
            if let Some(position) = self.global_position() {
                self.emit_move(position, &mut output);
                match button {
                    PointerButton::Left => {
                        self.left = Some(position);
                        output.push(Output::Down(position.0, position.1));
                    }
                    PointerButton::Right => {
                        self.right = Some(position);
                        output.push(Output::RightDown(position.0, position.1));
                    }
                }
            }
        } else {
            let release = match button {
                PointerButton::Left => self.left.take().map(|p| Output::Up(p.0, p.1)),
                PointerButton::Right => self.right.take().map(|p| Output::RightUp(p.0, p.1)),
            };
            output.extend(release);
        }
        output
    }

    pub fn cancel(&mut self) -> Vec<Output> {
        let mut output = Vec::new();
        output.extend(self.left.take().map(|p| Output::Up(p.0, p.1)));
        output.extend(self.right.take().map(|p| Output::RightUp(p.0, p.1)));
        self.pending_move = None;
        self.scroll = (0., 0.);
        self.tap_scroll = (0., 0.);
        self.gesture = Gesture::Blocked;
        output
    }

    pub fn reset(&mut self) -> Vec<Output> {
        let output = self.cancel();
        *self = Self::default();
        output
    }

    pub fn event(
        &mut self,
        id: u64,
        phase: Phase,
        point: Point,
        geometry: Geometry,
    ) -> Vec<Output> {
        let mut output = self.configure(geometry);
        match phase {
            Phase::Started => {
                if self.contacts.iter().any(|contact| contact.id == id) {
                    return output;
                }
                if self.contacts.is_empty() {
                    output.extend(self.flush());
                    self.gesture = if self.geometry.is_some() && inside_viewport(point, geometry) {
                        if self.left.is_none() && self.right.is_none() {
                            Gesture::SingleTap
                        } else {
                            Gesture::PointerMove
                        }
                    } else {
                        Gesture::Blocked
                    };
                } else if self.contacts.len() == 1 && self.gesture != Gesture::Blocked {
                    self.gesture = if self.gesture == Gesture::SingleTap {
                        Gesture::TwoTap
                    } else {
                        Gesture::TwoScroll
                    };
                }
                self.contacts.push(Contact {
                    id,
                    start: point,
                    point,
                });
                if self.contacts.len() > 2 || !inside_viewport(point, geometry) {
                    output.extend(self.cancel());
                }
            }
            Phase::Moved | Phase::Ended => {
                let Some(index) = self.contacts.iter().position(|contact| contact.id == id) else {
                    return output;
                };
                if !point.0.is_finite() || !point.1.is_finite() {
                    output.extend(self.cancel());
                } else {
                    self.update_contact(index, point);
                }
                if phase == Phase::Ended {
                    self.contacts.remove(index);
                    if self.contacts.is_empty() {
                        let tap = match self.gesture {
                            Gesture::SingleTap => Some(PointerButton::Left),
                            Gesture::TwoTap => {
                                self.tap_scroll = (0., 0.);
                                Some(PointerButton::Right)
                            }
                            _ => None,
                        };
                        if self.left.is_none() && self.right.is_none() {
                            if let Some(button) = tap {
                                output.extend(self.click(button));
                            }
                        } else if self.gesture == Gesture::TwoTap {
                            self.tap_scroll = (0., 0.);
                        }
                        self.gesture = Gesture::Idle;
                    }
                }
            }
            Phase::Cancelled => {
                if let Some(index) = self.contacts.iter().position(|contact| contact.id == id) {
                    output.extend(self.cancel());
                    self.contacts.remove(index);
                    if self.contacts.is_empty() {
                        self.gesture = Gesture::Idle;
                    }
                }
            }
        }
        output
    }

    fn update_contact(&mut self, index: usize, point: Point) {
        let previous = self.contacts[index].point;
        self.contacts[index].point = point;
        let start = self.contacts[index].start;
        let moved = (f64::from(point.0) - f64::from(start.0))
            .hypot(f64::from(point.1) - f64::from(start.1))
            >= TAP_SLOP;
        let dx = f64::from(point.0) - f64::from(previous.0);
        let dy = f64::from(point.1) - f64::from(previous.1);
        match self.gesture {
            Gesture::SingleTap | Gesture::PointerMove => {
                if moved {
                    self.gesture = Gesture::PointerMove;
                }
                if let (Some(geometry), Some(position)) = (self.geometry, self.position)
                    && let Some(scale) = contain_scale(geometry)
                {
                    let previous = self.global_position();
                    self.position = Some((position.0 + dx / scale, position.1 + dy / scale));
                    self.clamp_position();
                    if self.global_position() != previous {
                        self.pending_move = self.global_position();
                    }
                }
            }
            Gesture::TwoTap if self.contacts.len() == 2 => {
                self.tap_scroll.0 = (self.tap_scroll.0 + dx / 2.).clamp(-240., 240.);
                self.tap_scroll.1 = (self.tap_scroll.1 + dy / 2.).clamp(-240., 240.);
                if moved {
                    self.gesture = Gesture::TwoScroll;
                    self.scroll.0 = (self.scroll.0 + self.tap_scroll.0).clamp(-240., 240.);
                    self.scroll.1 = (self.scroll.1 + self.tap_scroll.1).clamp(-240., 240.);
                    self.tap_scroll = (0., 0.);
                }
            }
            Gesture::TwoScroll if self.contacts.len() == 2 => {
                self.scroll.0 = (self.scroll.0 + dx / 2.).clamp(-240., 240.);
                self.scroll.1 = (self.scroll.1 + dy / 2.).clamp(-240., 240.);
            }
            Gesture::TwoTap if moved => {
                self.gesture = Gesture::Blocked;
                self.tap_scroll = (0., 0.);
            }
            _ => {}
        }
    }

    fn click(&mut self, button: PointerButton) -> Vec<Output> {
        let mut output = self.flush();
        if let Some(position) = self.global_position() {
            self.emit_move(position, &mut output);
            match button {
                PointerButton::Left => output.extend([
                    Output::Down(position.0, position.1),
                    Output::Up(position.0, position.1),
                ]),
                PointerButton::Right => output.extend([
                    Output::RightDown(position.0, position.1),
                    Output::RightUp(position.0, position.1),
                ]),
            }
        }
        output
    }

    fn global_position(&self) -> Option<(i32, i32)> {
        let (geometry, position) = (self.geometry?, self.position?);
        Some((
            (f64::from(geometry.origin.0) + position.0.round())
                .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
            (f64::from(geometry.origin.1) + position.1.round())
                .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
        ))
    }

    fn clamp_position(&mut self) {
        if let (Some(geometry), Some(position)) = (self.geometry, self.position) {
            self.position = Some((
                position.0.clamp(0., f64::from(geometry.extent.0 - 1)),
                position.1.clamp(0., f64::from(geometry.extent.1 - 1)),
            ));
        }
    }

    fn emit_move(&mut self, position: (i32, i32), output: &mut Vec<Output>) {
        if output.last() != Some(&Output::Move(position.0, position.1)) {
            output.push(Output::Move(position.0, position.1));
        }
        if self.left.is_some() {
            self.left = Some(position);
        }
        if self.right.is_some() {
            self.right = Some(position);
        }
    }
}

fn contain_scale(geometry: Geometry) -> Option<f64> {
    let (left, top, width, height) = geometry.bounds;
    if ![left, top, width, height].into_iter().all(f32::is_finite)
        || width <= 0.
        || height <= 0.
        || geometry.extent.0 == 0
        || geometry.extent.1 == 0
    {
        return None;
    }
    Some(
        (f64::from(width) / f64::from(geometry.extent.0))
            .min(f64::from(height) / f64::from(geometry.extent.1)),
    )
}

fn inside_viewport(point: Point, geometry: Geometry) -> bool {
    let (left, top, width, height) = geometry.bounds;
    point.0.is_finite()
        && point.1.is_finite()
        && f64::from(point.0) >= f64::from(left)
        && f64::from(point.1) >= f64::from(top)
        && f64::from(point.0) < f64::from(left) + f64::from(width)
        && f64::from(point.1) < f64::from(top) + f64::from(height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry() -> Geometry {
        Geometry {
            bounds: (10., 20., 200., 200.),
            extent: (400, 200),
            origin: (-400, 50),
        }
    }

    #[test]
    fn relative_motion_is_coalesced_without_press_and_taps_are_clicks() {
        let g = geometry();
        let mut pointer = TrackpadInput::default();
        pointer.configure(g);
        assert_eq!(pointer.snapshot().position, Some((-200, 150)));
        assert!(
            pointer
                .event(1, Phase::Started, Point(50., 30.), g)
                .is_empty()
        );
        assert!(
            pointer
                .event(1, Phase::Moved, Point(60., 40.), g)
                .is_empty()
        );
        assert!(
            pointer
                .event(1, Phase::Moved, Point(65., 50.), g)
                .is_empty()
        );
        assert_eq!(pointer.flush(), vec![Output::Move(-170, 190)]);
        assert!(
            pointer
                .event(1, Phase::Ended, Point(65., 50.), g)
                .is_empty()
        );
        pointer.event(2, Phase::Started, Point(100., 100.), g);
        assert_eq!(
            pointer.event(2, Phase::Ended, Point(100., 100.), g),
            vec![
                Output::Move(-170, 190),
                Output::Down(-170, 190),
                Output::Up(-170, 190),
            ]
        );
    }

    #[test]
    fn two_finger_tap_and_scroll_do_not_become_left_clicks_or_drags() {
        let g = geometry();
        let mut pointer = TrackpadInput::default();
        pointer.configure(g);
        pointer.event(1, Phase::Started, Point(60., 70.), g);
        pointer.event(2, Phase::Started, Point(90., 70.), g);
        assert!(
            pointer
                .event(1, Phase::Ended, Point(60., 70.), g)
                .is_empty()
        );
        assert_eq!(
            pointer.event(2, Phase::Ended, Point(90., 70.), g),
            vec![
                Output::Move(-200, 150),
                Output::RightDown(-200, 150),
                Output::RightUp(-200, 150),
            ]
        );
        pointer.event(3, Phase::Started, Point(60., 70.), g);
        pointer.event(4, Phase::Started, Point(90., 70.), g);
        pointer.event(3, Phase::Moved, Point(60., 90.), g);
        pointer.event(4, Phase::Moved, Point(90., 90.), g);
        assert_eq!(
            pointer.flush(),
            vec![Output::Move(-200, 150), Output::Scroll(0, 20)]
        );
        assert!(
            pointer
                .event(3, Phase::Ended, Point(60., 90.), g)
                .is_empty()
        );
        assert!(
            pointer
                .event(4, Phase::Ended, Point(90., 90.), g)
                .is_empty()
        );
        assert!(!pointer.snapshot().left_down && !pointer.snapshot().right_down);
    }

    #[test]
    fn explicit_drag_holds_across_lift_and_all_cancellation_paths_release() {
        let g = geometry();
        let mut pointer = TrackpadInput::default();
        pointer.configure(g);
        assert_eq!(
            pointer.set_button(PointerButton::Left, true),
            vec![Output::Move(-200, 150), Output::Down(-200, 150),]
        );
        pointer.event(1, Phase::Started, Point(100., 100.), g);
        pointer.event(1, Phase::Moved, Point(5000., -5000.), g);
        assert_eq!(pointer.flush(), vec![Output::Move(-1, 50)]);
        assert!(
            pointer
                .event(1, Phase::Ended, Point(5000., -5000.), g)
                .is_empty()
        );
        assert!(pointer.snapshot().left_down);
        assert_eq!(
            pointer.set_button(PointerButton::Right, true),
            vec![Output::Move(-1, 50), Output::RightDown(-1, 50),]
        );
        pointer.event(2, Phase::Started, Point(100., 100.), g);
        pointer.event(2, Phase::Moved, Point(80., 100.), g);
        assert_eq!(
            pointer.cancel(),
            vec![Output::Up(-1, 50), Output::RightUp(-1, 50)]
        );
        assert!(!pointer.pending());
        assert!(
            pointer
                .event(2, Phase::Ended, Point(80., 100.), g)
                .is_empty()
        );
        pointer.set_button(PointerButton::Left, true);
        let rotated = Geometry {
            bounds: (0., 0., 400., 160.),
            ..g
        };
        assert!(matches!(
            pointer.configure(rotated).as_slice(),
            [Output::Up(_, _)]
        ));
        pointer.set_button(PointerButton::Right, true);
        assert!(matches!(
            pointer.reset().as_slice(),
            [Output::RightUp(_, _)]
        ));
        assert_eq!(pointer.snapshot(), PointerSnapshot::default());
    }
}
