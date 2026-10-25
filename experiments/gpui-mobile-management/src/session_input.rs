//! Routes a stage's captured touch sequence without mixing mouse controls into video gestures.
use crate::{
    pointer_input::{PointerButton, PointerSnapshot, TrackpadInput},
    touch_input::{Geometry, Output, Phase, Point, TouchInput},
};

pub type Bounds = (f32, f32, f32, f32);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputMode {
    #[default]
    Direct,
    Pointer,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ControlRegions {
    pub left: Option<Bounds>,
    pub right: Option<Bounds>,
    pub wheel: Option<Bounds>,
}

#[derive(Clone, Copy)]
enum Route {
    Video,
    Left,
    Right,
    Wheel(Point),
    Ignored,
    Retired,
}

struct Contact {
    id: u64,
    route: Route,
}

#[derive(Default)]
pub struct SessionInput {
    mode: InputMode,
    direct: TouchInput,
    pointer: TrackpadInput,
    geometry: Option<Geometry>,
    controls: ControlRegions,
    contacts: Vec<Contact>,
}

impl SessionInput {
    pub fn active(&self) -> bool {
        !self.contacts.is_empty()
    }

    pub fn wheel_active(&self) -> bool {
        self.contacts
            .iter()
            .any(|contact| matches!(contact.route, Route::Wheel(_)))
    }

    pub fn mode(&self) -> InputMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: InputMode) -> Vec<Output> {
        if mode == self.mode {
            return Vec::new();
        }
        let mut output = self.cancel();
        self.mode = mode;
        if mode == InputMode::Pointer {
            output.extend(self.pointer.reset());
            if let Some(geometry) = self.geometry {
                output.extend(self.pointer.configure(geometry));
            }
        }
        output
    }

    pub fn configure(&mut self, geometry: Geometry, controls: ControlRegions) -> Vec<Output> {
        if self.geometry == Some(geometry) && self.controls == controls {
            return Vec::new();
        }
        let mut output = self.cancel();
        self.geometry = Some(geometry);
        self.controls = controls;
        output.extend(self.pointer.configure(geometry));
        output
    }

    pub fn snapshot(&self) -> PointerSnapshot {
        self.pointer.snapshot()
    }

    pub fn set_position(&mut self, position: (i32, i32)) -> bool {
        self.mode == InputMode::Pointer && self.pointer.set_position(position)
    }

    pub fn arm_right_click(&mut self) -> bool {
        self.mode == InputMode::Direct && self.direct.arm_right_click()
    }

    pub fn right_click_armed(&self) -> bool {
        self.mode == InputMode::Direct && self.direct.right_click_armed()
    }

    pub fn pending(&self) -> bool {
        match self.mode {
            InputMode::Direct => self.direct.pending(),
            InputMode::Pointer => self.pointer.pending(),
        }
    }

    pub fn flush(&mut self) -> Vec<Output> {
        match self.mode {
            InputMode::Direct => self.direct.flush(),
            InputMode::Pointer => self.pointer.flush(),
        }
    }

    pub fn cancel(&mut self) -> Vec<Output> {
        let position = self.pointer.snapshot().position;
        let mut output = self.direct.reset();
        output.extend(self.pointer.reset());
        if let Some(geometry) = self.geometry {
            output.extend(self.pointer.configure(geometry));
            if let Some(position) = position {
                self.pointer.set_position(position);
            }
        }
        for contact in &mut self.contacts {
            contact.route = Route::Retired;
        }
        output
    }

    pub fn cancel_event(&mut self, id: u64, phase: Phase) -> Vec<Output> {
        let output = self.cancel();
        self.retire_event(id, phase);
        output
    }

    pub fn reset(&mut self) -> Vec<Output> {
        let mut output = self.direct.reset();
        output.extend(self.pointer.reset());
        *self = Self {
            mode: self.mode,
            ..Self::default()
        };
        output
    }

    pub fn event(
        &mut self,
        id: u64,
        phase: Phase,
        point: Point,
        geometry: Geometry,
        controls: ControlRegions,
    ) -> Vec<Output> {
        if self
            .contacts
            .iter()
            .any(|contact| matches!(contact.route, Route::Retired))
        {
            self.retire_event(id, phase);
            return Vec::new();
        }
        let mut output = self.configure(geometry, controls);
        if phase == Phase::Started {
            if self.contacts.iter().any(|contact| contact.id == id) {
                return output;
            }
            let route = self.route(point, geometry);
            self.contacts.push(Contact { id, route });
            match route {
                Route::Video => output.extend(self.video_event(id, phase, point, geometry)),
                Route::Left => output.extend(self.pointer.set_button(PointerButton::Left, true)),
                Route::Right => output.extend(self.pointer.set_button(PointerButton::Right, true)),
                _ => {}
            }
            return output;
        }
        let Some(index) = self.contacts.iter().position(|contact| contact.id == id) else {
            return output;
        };
        if phase == Phase::Cancelled || !point.0.is_finite() || !point.1.is_finite() {
            output.extend(self.cancel());
        } else {
            match self.contacts[index].route {
                Route::Video => output.extend(self.video_event(id, phase, point, geometry)),
                Route::Left | Route::Right if phase == Phase::Ended => {
                    let left = matches!(self.contacts[index].route, Route::Left);
                    let still_held = self.contacts.iter().enumerate().any(|(other, contact)| {
                        other != index
                            && if left {
                                matches!(contact.route, Route::Left)
                            } else {
                                matches!(contact.route, Route::Right)
                            }
                    });
                    if !still_held {
                        output.extend(self.pointer.set_button(
                            if left {
                                PointerButton::Left
                            } else {
                                PointerButton::Right
                            },
                            false,
                        ));
                    }
                }
                Route::Wheel(previous) => {
                    output.extend(self.pointer.scroll_by(0., point.1 - previous.1));
                    self.contacts[index].route = Route::Wheel(point);
                }
                _ => {}
            }
        }
        if matches!(phase, Phase::Ended | Phase::Cancelled) {
            self.contacts.remove(index);
        }
        output
    }

    fn retire_event(&mut self, id: u64, phase: Phase) {
        if phase == Phase::Started && !self.contacts.iter().any(|contact| contact.id == id) {
            self.contacts.push(Contact {
                id,
                route: Route::Retired,
            });
        } else if matches!(phase, Phase::Ended | Phase::Cancelled)
            && let Some(index) = self.contacts.iter().position(|contact| contact.id == id)
        {
            self.contacts.remove(index);
        }
    }

    fn video_event(
        &mut self,
        id: u64,
        phase: Phase,
        point: Point,
        geometry: Geometry,
    ) -> Vec<Output> {
        match self.mode {
            InputMode::Direct => self.direct.event(id, phase, point, geometry),
            InputMode::Pointer => self.pointer.event(id, phase, point, geometry),
        }
    }

    fn route(&self, point: Point, geometry: Geometry) -> Route {
        if self
            .contacts
            .iter()
            .any(|contact| matches!(contact.route, Route::Retired))
        {
            return Route::Retired;
        }
        if self.mode == InputMode::Pointer {
            if self
                .controls
                .left
                .is_some_and(|bounds| contains(bounds, point))
            {
                return Route::Left;
            }
            if self
                .controls
                .right
                .is_some_and(|bounds| contains(bounds, point))
            {
                return Route::Right;
            }
            if self
                .controls
                .wheel
                .is_some_and(|bounds| contains(bounds, point))
            {
                return Route::Wheel(point);
            }
        }
        if contains(geometry.bounds, point) {
            Route::Video
        } else {
            Route::Ignored
        }
    }
}

fn contains((left, top, width, height): Bounds, point: Point) -> bool {
    [left, top, width, height, point.0, point.1]
        .into_iter()
        .all(f32::is_finite)
        && width > 0.
        && height > 0.
        && f64::from(point.0) >= f64::from(left)
        && f64::from(point.1) >= f64::from(top)
        && f64::from(point.0) < f64::from(left) + f64::from(width)
        && f64::from(point.1) < f64::from(top) + f64::from(height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> (Geometry, ControlRegions) {
        (
            Geometry {
                bounds: (0., 0., 200., 100.),
                extent: (400, 200),
                origin: (-400, 50),
            },
            ControlRegions {
                left: Some((0., 100., 70., 50.)),
                right: Some((70., 100., 70., 50.)),
                wheel: Some((140., 100., 60., 50.)),
            },
        )
    }

    #[test]
    fn direct_mode_keeps_absolute_touch_and_armed_right_click() {
        let (g, controls) = layout();
        let mut input = SessionInput::default();
        assert_eq!(input.mode(), InputMode::Direct);
        input.event(1, Phase::Started, Point(50., 20.), g, controls);
        assert_eq!(
            input.event(1, Phase::Ended, Point(50., 20.), g, controls),
            vec![
                Output::Move(-300, 90),
                Output::Down(-300, 90),
                Output::Up(-300, 90),
            ]
        );
        assert!(input.arm_right_click() && input.right_click_armed());
        input.event(2, Phase::Started, Point(50., 20.), g, controls);
        assert_eq!(
            input.event(2, Phase::Ended, Point(50., 20.), g, controls),
            vec![
                Output::Move(-300, 90),
                Output::RightDown(-300, 90),
                Output::RightUp(-300, 90),
            ]
        );
        assert!(!input.right_click_armed());
        assert!(
            input
                .event(3, Phase::Started, Point(20., 120.), g, controls)
                .is_empty()
        );
        assert!(
            input
                .event(3, Phase::Ended, Point(20., 120.), g, controls)
                .is_empty()
        );
    }

    #[test]
    fn dock_hold_video_motion_and_wheel_are_independent_contacts() {
        let (g, controls) = layout();
        let mut input = SessionInput::default();
        input.configure(g, controls);
        input.set_mode(InputMode::Pointer);
        assert_eq!(
            input.event(1, Phase::Started, Point(20., 120.), g, controls),
            vec![Output::Move(-200, 150), Output::Down(-200, 150),]
        );
        input.event(2, Phase::Started, Point(60., 50.), g, controls);
        input.event(2, Phase::Moved, Point(80., 50.), g, controls);
        assert_eq!(input.flush(), vec![Output::Move(-160, 150)]);
        input.event(3, Phase::Started, Point(170., 110.), g, controls);
        assert!(input.active() && input.wheel_active());
        input.event(3, Phase::Moved, Point(10., 130.), g, controls);
        assert_eq!(
            input.flush(),
            vec![Output::Move(-160, 150), Output::Scroll(0, 20)]
        );
        assert!(
            input
                .event(2, Phase::Ended, Point(80., 50.), g, controls)
                .is_empty()
        );
        assert!(input.snapshot().left_down);
        assert!(
            input
                .event(3, Phase::Ended, Point(10., 130.), g, controls)
                .is_empty()
        );
        assert_eq!(
            input.event(1, Phase::Ended, Point(20., 120.), g, controls),
            vec![Output::Up(-160, 150)]
        );
        assert!(!input.pending());
        assert!(!input.active() && !input.wheel_active());

        input.event(4, Phase::Started, Point(60., 50.), g, controls);
        input.event(5, Phase::Started, Point(20., 120.), g, controls);
        assert!(matches!(
            input
                .event(5, Phase::Ended, Point(20., 120.), g, controls)
                .as_slice(),
            [Output::Up(_, _)]
        ));
        assert!(
            input
                .event(4, Phase::Ended, Point(60., 50.), g, controls)
                .is_empty()
        );
        input.event(6, Phase::Started, Point(60., 50.), g, controls);
        input.event(7, Phase::Started, Point(170., 110.), g, controls);
        input.event(7, Phase::Moved, Point(170., 125.), g, controls);
        assert!(
            input
                .flush()
                .iter()
                .any(|output| matches!(output, Output::Scroll(0, 15)))
        );
        assert!(
            input
                .event(7, Phase::Ended, Point(170., 125.), g, controls)
                .is_empty()
        );
        assert!(
            input
                .event(6, Phase::Ended, Point(60., 50.), g, controls)
                .is_empty(),
            "a wheel gesture must not become a resting video-finger click"
        );
    }

    #[test]
    fn mode_geometry_and_cancel_release_buttons_and_retire_old_contacts() {
        let (g, controls) = layout();
        let mut input = SessionInput::default();
        input.configure(g, controls);
        input.set_mode(InputMode::Pointer);
        assert!(input.set_position((-350, 70)));
        assert!(!input.set_position((50, 70)));
        input.event(1, Phase::Started, Point(20., 120.), g, controls);
        assert_eq!(
            input.set_mode(InputMode::Direct),
            vec![Output::Up(-350, 70)]
        );
        assert!(
            input
                .event(9, Phase::Started, Point(50., 20.), g, controls)
                .is_empty()
        );

        assert!(
            input
                .event(1, Phase::Moved, Point(50., 20.), g, controls)
                .is_empty()
        );
        assert!(
            input
                .event(1, Phase::Ended, Point(50., 20.), g, controls)
                .is_empty()
        );
        assert!(
            input
                .event(9, Phase::Moved, Point(70., 20.), g, controls)
                .is_empty()
        );
        assert!(
            input
                .event(9, Phase::Ended, Point(70., 20.), g, controls)
                .is_empty()
        );
        input.event(2, Phase::Started, Point(50., 20.), g, controls);
        let drag = input.event(2, Phase::Moved, Point(70., 20.), g, controls);
        assert!(
            drag.iter()
                .any(|output| matches!(output, Output::Down(_, _)))
        );
        let rotated = Geometry {
            bounds: (0., 0., 100., 200.),
            ..g
        };
        assert!(
            input
                .configure(rotated, controls)
                .iter()
                .any(|output| matches!(output, Output::Up(_, _)))
        );
        assert!(
            input
                .event(2, Phase::Ended, Point(70., 20.), rotated, controls)
                .is_empty()
        );
        input.set_mode(InputMode::Pointer);
        input.event(3, Phase::Started, Point(90., 120.), rotated, controls);
        assert!(
            input
                .cancel()
                .iter()
                .any(|output| matches!(output, Output::RightUp(_, _)))
        );
        assert!(
            input
                .event(3, Phase::Ended, Point(90., 120.), rotated, controls)
                .is_empty()
        );
        input.reset();
        assert_eq!(input.mode(), InputMode::Pointer);
        assert!(!input.active() && input.snapshot().position.is_none());
        input.configure(g, controls);
        input.event(11, Phase::Started, Point(20., 120.), g, controls);
        let other_display = Geometry {
            origin: (1000, 50),
            ..g
        };
        assert!(
            input
                .configure(other_display, controls)
                .iter()
                .any(|output| matches!(output, Output::Up(_, _)))
        );
        let current_display_position = input.snapshot().position;
        assert_eq!(current_display_position, Some((1200, 150)));
        assert!(
            input
                .event(11, Phase::Moved, Point(40., 120.), g, controls)
                .is_empty()
        );
        assert_eq!(
            input.snapshot().position,
            current_display_position,
            "retired callbacks must not restore old display geometry"
        );
        assert!(
            input
                .event(11, Phase::Ended, Point(40., 120.), g, controls)
                .is_empty()
        );
        assert_eq!(input.snapshot().position, current_display_position);

        let mut denied = SessionInput::default();
        denied.set_mode(InputMode::Pointer);
        assert!(denied.cancel_event(21, Phase::Started).is_empty());
        assert!(denied.cancel_event(22, Phase::Started).is_empty());
        assert!(denied.active());
        denied.configure(other_display, controls);
        let denied_position = denied.snapshot().position;
        assert!(denied.cancel_event(21, Phase::Ended).is_empty());
        assert!(denied.active());
        assert!(
            denied
                .event(22, Phase::Moved, Point(50., 20.), g, controls)
                .is_empty()
        );
        assert_eq!(denied.snapshot().position, denied_position);
        assert!(
            denied
                .event(22, Phase::Ended, Point(50., 20.), g, controls)
                .is_empty()
        );
        assert!(!denied.active());
        assert_eq!(denied.snapshot().position, denied_position);
    }
}
