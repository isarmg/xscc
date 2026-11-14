//! Routes a stage's captured touch sequence without mixing mouse controls into video gestures.
use crate::{
    pointer_input::{PointerSnapshot, TrackpadInput},
    touch_input::{Geometry, Output, Phase, Point, TouchInput},
};

mod floating_controls;
pub use floating_controls::Control;
use floating_controls::{Gesture, Mode, Placements};
use std::time::Instant;

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
    pub middle: Option<Bounds>,
    pub wheel: Option<Bounds>,
    pub reserved: Option<Bounds>,
    pub canvas: Option<Bounds>,
}

#[derive(Clone, Copy)]
enum Route {
    Video,
    Control(Gesture),
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
    placements: Placements,
    contacts: Vec<Contact>,
}

impl SessionInput {
    pub fn active(&self) -> bool {
        !self.contacts.is_empty()
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
        if self.mode == InputMode::Pointer
            && self.controls == controls
            && self.pointer.translate_viewport(geometry)
        {
            self.geometry = Some(geometry);
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

    pub fn click_control(&mut self, control: Control) -> Vec<Output> {
        if self.mode != InputMode::Pointer || self.active() {
            return Vec::new();
        }
        let Some(button) = control.button() else {
            return Vec::new();
        };
        let mut output = self.pointer.set_button(button, true);
        output.extend(self.pointer.set_button(button, false));
        output
    }

    pub fn wheel_step(&mut self, delta: i32) -> Vec<Output> {
        if self.mode != InputMode::Pointer || self.active() {
            return Vec::new();
        }
        self.pointer.scroll_vertical(delta as f32);
        self.pointer.flush()
    }

    pub fn placed_controls(&self, controls: ControlRegions, bounds: Bounds) -> ControlRegions {
        self.placements
            .regions(controls, controls.canvas.unwrap_or(bounds))
    }

    pub fn moving_control(&self) -> Option<Control> {
        self.contacts
            .iter()
            .find_map(|contact| match contact.route {
                Route::Control(gesture) if gesture.mode == Mode::Moving => Some(gesture.control),
                _ => None,
            })
    }

    pub fn wheel_position(&self) -> f32 {
        self.contacts
            .iter()
            .find_map(|contact| match contact.route {
                Route::Control(gesture)
                    if gesture.control == Control::Wheel && gesture.mode == Mode::Scrolling =>
                {
                    Some(
                        ((gesture.point.1 - gesture.bounds.1 - 16.)
                            / (gesture.bounds.3 - 32.).max(1.))
                        .clamp(0., 1.),
                    )
                }
                _ => None,
            })
            .unwrap_or(0.5)
    }

    pub fn control_pressed(&self, control: Control) -> bool {
        self.contacts.iter().any(|contact| {
            matches!(contact.route, Route::Control(gesture)
            if gesture.control == control && matches!(gesture.mode, Mode::Waiting | Mode::Held))
        })
    }

    pub fn pending(&self) -> bool {
        self.contacts.iter().any(|contact| {
            matches!(contact.route,
            Route::Control(gesture) if gesture.mode == Mode::Waiting)
        }) || match self.mode {
            InputMode::Direct => self.direct.pending(),
            InputMode::Pointer => self.pointer.pending(),
        }
    }

    pub fn flush(&mut self) -> Vec<Output> {
        for contact in &mut self.contacts {
            if let Route::Control(gesture) = &mut contact.route {
                gesture.advance(Instant::now());
            }
        }
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
        self.event_at(id, phase, point, geometry, controls, Instant::now())
    }

    fn event_at(
        &mut self,
        id: u64,
        phase: Phase,
        point: Point,
        geometry: Geometry,
        controls: ControlRegions,
        now: Instant,
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
            let mut route = self.route(point, geometry, now);
            if matches!(route, Route::Video) {
                for contact in &mut self.contacts {
                    if let Route::Control(gesture) = &mut contact.route {
                        gesture.advance(now);
                        if gesture.mode == Mode::Waiting
                            && let Some(button) = gesture.control.button()
                        {
                            gesture.mode = Mode::Held;
                            output.extend(self.pointer.set_button(button, true));
                        }
                    }
                }
            } else if let Route::Control(gesture) = &mut route {
                if self.contacts.iter().any(|contact| matches!(contact.route, Route::Control(other) if other.control == gesture.control)) {
                    route = Route::Ignored;
                } else if self.contacts.iter().any(|contact| matches!(contact.route, Route::Video))
                    && let Some(button) = gesture.control.button()
                {
                    gesture.mode = Mode::Held;
                    output.extend(self.pointer.set_button(button, true));
                }
            }
            self.contacts.push(Contact { id, route });
            if matches!(route, Route::Video) {
                output.extend(self.video_event(id, phase, point, geometry));
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
                Route::Control(mut gesture) => {
                    let scroll = gesture.update(point, now);
                    if gesture.mode == Mode::Moving {
                        self.placements
                            .move_control(gesture, controls.canvas.unwrap_or(geometry.bounds));
                    } else if scroll != 0. {
                        self.pointer.scroll_vertical(scroll);
                    }
                    if phase == Phase::Ended
                        && let Some(button) = gesture.control.button()
                    {
                        if gesture.mode == Mode::Waiting {
                            output.extend(self.pointer.set_button(button, true));
                            output.extend(self.pointer.set_button(button, false));
                        } else if gesture.mode == Mode::Held {
                            output.extend(self.pointer.set_button(button, false));
                        }
                    }
                    self.contacts[index].route = Route::Control(gesture);
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

    fn route(&self, point: Point, geometry: Geometry, now: Instant) -> Route {
        if self
            .contacts
            .iter()
            .any(|contact| matches!(contact.route, Route::Retired))
        {
            return Route::Retired;
        }
        if self.mode == InputMode::Pointer {
            let controls = self.placed_controls(self.controls, geometry.bounds);
            // Match the paint order when the user deliberately overlaps controls.
            for control in [
                Control::Wheel,
                Control::Middle,
                Control::Right,
                Control::Left,
            ] {
                if let Some(bounds) = control.bounds(controls)
                    && contains(bounds, point)
                {
                    return Route::Control(Gesture::new(control, point, bounds, now));
                }
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
                middle: Some((140., 100., 60., 50.)),
                wheel: None,
                reserved: None,
                canvas: None,
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
    fn floating_controls_use_the_full_canvas_even_when_video_has_black_bars() {
        let geometry = Geometry {
            bounds: (100., 0., 400., 300.),
            extent: (800, 600),
            origin: (0, 0),
        };
        let controls = ControlRegions {
            left: Some((180., 100., 48., 48.)),
            canvas: Some((0., 0., 600., 300.)),
            ..Default::default()
        };
        let mut input = SessionInput::default();
        input.set_mode(InputMode::Pointer);
        let now = Instant::now();
        assert!(
            input
                .event_at(
                    1,
                    Phase::Started,
                    Point(204., 124.),
                    geometry,
                    controls,
                    now
                )
                .is_empty()
        );
        assert!(
            input
                .event_at(
                    1,
                    Phase::Moved,
                    Point(-100., -100.),
                    geometry,
                    controls,
                    now + std::time::Duration::from_millis(501)
                )
                .is_empty()
        );
        input.event_at(
            1,
            Phase::Ended,
            Point(-100., -100.),
            geometry,
            controls,
            now + std::time::Duration::from_millis(510),
        );
        assert_eq!(
            input.placed_controls(controls, geometry.bounds).left,
            Some((0., 0., 48., 48.))
        );
        input.event_at(
            2,
            Phase::Started,
            Point(24., 24.),
            geometry,
            controls,
            now + std::time::Duration::from_secs(1),
        );
        assert!(
            !input
                .event_at(
                    2,
                    Phase::Ended,
                    Point(24., 24.),
                    geometry,
                    controls,
                    now + std::time::Duration::from_millis(1100)
                )
                .is_empty(),
            "painted control in a black bar must still receive the click"
        );
    }

    #[test]
    fn keyboard_camera_translation_keeps_a_live_pointer_gesture() {
        let mut input = SessionInput::default();
        input.set_mode(InputMode::Pointer);
        let mut geometry = Geometry {
            bounds: (0., 0., 600., 300.),
            extent: (1200, 600),
            origin: (0, 0),
        };
        let controls = ControlRegions::default();
        input.event(1, Phase::Started, Point(200., 100.), geometry, controls);
        geometry.bounds.1 = -80.;
        assert!(input.configure(geometry, controls).is_empty());
        assert!(input.active());
        input.event(1, Phase::Moved, Point(240., 140.), geometry, controls);
        assert!(!input.flush().is_empty());
    }

    #[test]
    fn short_taps_click_but_long_press_moves_each_control_without_remote_input() {
        let (mut g, _) = layout();
        g.bounds = (0., 0., 320., 240.);
        let controls = ControlRegions {
            left: Some((20., 180., 48., 48.)),
            middle: Some((80., 180., 48., 48.)),
            right: Some((140., 180., 48., 48.)),
            wheel: Some((200., 100., 48., 112.)),
            reserved: Some((268., 0., 44., 44.)),
            canvas: None,
        };
        for control in [
            Control::Left,
            Control::Middle,
            Control::Right,
            Control::Wheel,
        ] {
            let mut input = SessionInput::default();
            input.configure(g, controls);
            input.set_mode(InputMode::Pointer);
            let bounds = control.bounds(controls).unwrap();
            let point = Point(bounds.0 + 24., bounds.1 + 24.);
            let now = Instant::now();
            assert!(
                input
                    .event_at(1, Phase::Started, point, g, controls, now)
                    .is_empty()
            );
            assert!(input.pending());
            assert!(input.control_pressed(control));
            let tap = input
                .event_at(1, Phase::Ended, point, g, controls, now)
                .into_iter()
                .filter(|event| !matches!(event, Output::Move(_, _)))
                .collect::<Vec<_>>();
            match control {
                Control::Left => assert!(matches!(
                    tap.as_slice(),
                    [Output::Down(_, _), Output::Up(_, _)]
                )),
                Control::Middle => assert!(matches!(
                    tap.as_slice(),
                    [Output::MiddleDown(_, _), Output::MiddleUp(_, _)]
                )),
                Control::Right => assert!(matches!(
                    tap.as_slice(),
                    [Output::RightDown(_, _), Output::RightUp(_, _)]
                )),
                Control::Wheel => assert!(tap.is_empty()),
            }
            input.event_at(2, Phase::Started, point, g, controls, now);
            let later = now + floating_controls::HOLD_TO_MOVE;
            assert!(
                input
                    .event_at(2, Phase::Moved, Point(-500., -500.), g, controls, later)
                    .is_empty()
            );
            assert_eq!(input.moving_control(), Some(control));
            let moved = input.placed_controls(controls, g.bounds);
            assert_eq!(control.bounds(moved).map(|b| (b.0, b.1)), Some((0., 0.)));
            assert!(
                input
                    .event_at(2, Phase::Ended, Point(-500., -500.), g, controls, later)
                    .is_empty()
            );
            assert!(input.flush().is_empty());
            assert!(!input.active() && input.moving_control().is_none());
            // The saved placement, not the original button rectangle, receives the next tap.
            assert!(
                input
                    .event_at(3, Phase::Started, Point(24., 24.), g, controls, later)
                    .is_empty()
            );
            let moved_tap = input.event_at(3, Phase::Ended, Point(24., 24.), g, controls, later);
            assert_eq!(
                moved_tap.len(),
                if control == Control::Wheel { 0 } else { 3 }
            );
            input.event_at(4, Phase::Started, Point(24., 24.), g, controls, later);
            assert!(
                input
                    .event_at(
                        4,
                        Phase::Moved,
                        Point(900., -900.),
                        g,
                        controls,
                        later + floating_controls::HOLD_TO_MOVE
                    )
                    .is_empty()
            );
            let placed = control
                .bounds(input.placed_controls(controls, g.bounds))
                .unwrap();
            assert!(
                placed.0 >= 0.
                    && placed.1 >= 0.
                    && placed.0 + placed.2 <= 320.
                    && placed.1 + placed.3 <= 240.
            );
            assert!(
                placed.1 >= 44. || placed.0 + placed.2 <= 268.,
                "the toolbar must not cover a moved control"
            );
            assert!(input.cancel_event(4, Phase::Cancelled).is_empty());
            assert!(
                input
                    .event_at(
                        4,
                        Phase::Ended,
                        Point(900., -900.),
                        g,
                        controls,
                        later + floating_controls::HOLD_TO_MOVE
                    )
                    .is_empty()
            );
            assert!(!input.active() && !input.pending());
        }
    }

    #[test]
    fn wheel_sliding_and_button_plus_video_drag_remain_independent_and_balanced() {
        let (g, mut controls) = layout();
        controls.wheel = Some((160., 0., 40., 80.));
        let mut input = SessionInput::default();
        input.configure(g, controls);
        input.set_mode(InputMode::Pointer);
        let now = Instant::now();
        input.event_at(1, Phase::Started, Point(180., 20.), g, controls, now);
        input.event_at(1, Phase::Moved, Point(180., 40.), g, controls, now);
        assert_eq!(
            input.flush(),
            vec![Output::Move(-200, 150), Output::Scroll(0, 20)]
        );
        assert!(
            input
                .event_at(
                    1,
                    Phase::Ended,
                    Point(180., 40.),
                    g,
                    controls,
                    now + floating_controls::HOLD_TO_MOVE
                )
                .is_empty()
        );
        assert_eq!(input.placed_controls(controls, g.bounds), controls);
        assert_eq!(input.wheel_position(), 0.5);
        for (id, point, down, up) in [
            (
                2,
                Point(20., 120.),
                Output::Down(-200, 150),
                Output::Up(-160, 150),
            ),
            (
                4,
                Point(90., 120.),
                Output::RightDown(-160, 150),
                Output::RightUp(-120, 150),
            ),
            (
                6,
                Point(170., 120.),
                Output::MiddleDown(-120, 150),
                Output::MiddleUp(-80, 150),
            ),
        ] {
            assert!(
                input
                    .event_at(id, Phase::Started, point, g, controls, now)
                    .is_empty()
            );
            assert!(
                input
                    .event_at(id + 1, Phase::Started, Point(60., 30.), g, controls, now)
                    .contains(&down)
            );
            input.event_at(id + 1, Phase::Moved, Point(80., 30.), g, controls, now);
            input.flush();
            assert_eq!(
                input.event_at(id, Phase::Ended, point, g, controls, now),
                vec![up]
            );
            assert!(
                input
                    .event_at(id + 1, Phase::Ended, Point(80., 30.), g, controls, now)
                    .is_empty()
            );
        }
        assert!(matches!(
            input.click_control(Control::Middle).as_slice(),
            [
                Output::Move(_, _),
                Output::MiddleDown(_, _),
                Output::MiddleUp(_, _)
            ]
        ));
        assert!(input.wheel_step(-40).contains(&Output::Scroll(0, -40)));
        // Fast movement before the hold is neither a click nor a reposition.
        input.event_at(10, Phase::Started, Point(20., 120.), g, controls, now);
        assert!(
            input
                .event_at(10, Phase::Moved, Point(40., 120.), g, controls, now)
                .is_empty()
        );
        assert!(
            input
                .event_at(10, Phase::Ended, Point(40., 120.), g, controls, now)
                .is_empty()
        );
        assert_eq!(input.placed_controls(controls, g.bounds), controls);
    }

    #[test]
    fn mode_geometry_and_cancel_release_buttons_and_retire_old_contacts() {
        let (g, controls) = layout();
        let mut input = SessionInput::default();
        input.configure(g, controls);
        input.set_mode(InputMode::Pointer);
        assert!(input.set_position((-350, 70)));
        assert!(!input.set_position((50, 70)));
        input.event(100 + 1, Phase::Started, Point(60., 30.), g, controls);
        input.event(1, Phase::Started, Point(20., 120.), g, controls);
        input.event(100 + 1, Phase::Ended, Point(60., 30.), g, controls);
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
        input.event(100 + 3, Phase::Started, Point(60., 30.), rotated, controls);
        input.event(3, Phase::Started, Point(90., 120.), rotated, controls);
        input.event(100 + 3, Phase::Ended, Point(60., 30.), rotated, controls);
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
        input.event(100 + 11, Phase::Started, Point(60., 30.), g, controls);
        input.event(11, Phase::Started, Point(20., 120.), g, controls);
        input.event(100 + 11, Phase::Ended, Point(60., 30.), g, controls);
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

        input.event(
            100 + 31,
            Phase::Started,
            Point(60., 30.),
            other_display,
            controls,
        );
        input.event(
            31,
            Phase::Started,
            Point(170., 120.),
            other_display,
            controls,
        );
        input.event(
            100 + 31,
            Phase::Ended,
            Point(60., 30.),
            other_display,
            controls,
        );
        assert!(input.snapshot().middle_down);
        assert!(matches!(
            input.cancel_event(31, Phase::Moved).as_slice(),
            [Output::MiddleUp(_, _)]
        ));
        assert!(!input.snapshot().middle_down);
        assert!(
            input
                .event(31, Phase::Ended, Point(170., 120.), g, controls)
                .is_empty()
        );

        input.event(
            100 + 32,
            Phase::Started,
            Point(60., 30.),
            other_display,
            controls,
        );
        input.event(
            32,
            Phase::Started,
            Point(170., 120.),
            other_display,
            controls,
        );
        input.event(
            100 + 32,
            Phase::Ended,
            Point(60., 30.),
            other_display,
            controls,
        );
        assert!(matches!(
            input.set_mode(InputMode::Direct).as_slice(),
            [Output::MiddleUp(_, _)]
        ));
        input.event(32, Phase::Ended, Point(170., 120.), g, controls);
        input.set_mode(InputMode::Pointer);
        input.event(
            100 + 33,
            Phase::Started,
            Point(60., 30.),
            other_display,
            controls,
        );
        input.event(
            33,
            Phase::Started,
            Point(170., 120.),
            other_display,
            controls,
        );
        input.event(
            100 + 33,
            Phase::Ended,
            Point(60., 30.),
            other_display,
            controls,
        );
        assert!(matches!(
            input.configure(rotated, controls).as_slice(),
            [Output::MiddleUp(_, _)]
        ));
        input.event(33, Phase::Ended, Point(170., 120.), other_display, controls);

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
