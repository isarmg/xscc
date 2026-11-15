use super::{Bounds, ControlRegions};
use crate::{pointer_input::PointerButton, touch_input::Point};
use std::time::{Duration, Instant};

pub(super) const HOLD_TO_MOVE: Duration = Duration::from_millis(500);
const SLOP: f32 = 8.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Left,
    Middle,
    Right,
    Wheel,
}

impl Control {
    pub(super) fn button(self) -> Option<PointerButton> {
        match self {
            Self::Left => Some(PointerButton::Left),
            Self::Middle => Some(PointerButton::Middle),
            Self::Right => Some(PointerButton::Right),
            Self::Wheel => None,
        }
    }

    pub(super) fn bounds(self, regions: ControlRegions) -> Option<Bounds> {
        match self {
            Self::Left => regions.left,
            Self::Middle => regions.middle,
            Self::Right => regions.right,
            Self::Wheel => regions.wheel,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Waiting,
    Held,
    Moving,
    Scrolling,
    Cancelled,
}

#[derive(Clone, Copy)]
pub(super) struct Gesture {
    pub control: Control,
    pub mode: Mode,
    pub start: Point,
    pub point: Point,
    pub bounds: Bounds,
    started: Instant,
    scrolled: Instant,
}

impl Gesture {
    pub fn new(control: Control, point: Point, bounds: Bounds, now: Instant) -> Self {
        Self {
            control,
            mode: Mode::Waiting,
            start: point,
            point,
            bounds,
            started: now,
            scrolled: now,
        }
    }

    pub fn advance(&mut self, now: Instant) {
        if self.mode == Mode::Waiting && now.duration_since(self.started) >= HOLD_TO_MOVE {
            self.mode = Mode::Moving;
        }
    }

    pub fn update(&mut self, point: Point, now: Instant) {
        self.advance(now);
        self.point = point;
        if self.mode == Mode::Waiting
            && (point.0 - self.start.0).hypot(point.1 - self.start.1) >= SLOP
        {
            self.mode = if self.control == Control::Wheel {
                Mode::Scrolling
            } else {
                Mode::Cancelled
            };
            self.scrolled = now;
        }
    }

    pub fn wheel_position(&self) -> f32 {
        (0.5 + (self.point.1 - self.start.1) / (self.bounds.3 - 44.).max(16.)).clamp(0., 1.)
    }

    pub fn scroll_rate(&self) -> f32 {
        if self.mode != Mode::Scrolling {
            return 0.;
        }
        let displacement = self.point.1 - self.start.1;
        if displacement.abs() <= SLOP {
            return 0.;
        }
        let radius = ((self.bounds.3 - 44.).max(16.) / 2. - SLOP).max(1.);
        let pressure = ((displacement.abs() - SLOP) / radius).clamp(0., 1.);
        -displacement.signum() * (240. + 2160. * pressure * pressure)
    }

    pub fn scroll_delta(&mut self, now: Instant) -> f32 {
        let elapsed = now
            .saturating_duration_since(self.scrolled)
            .min(Duration::from_millis(50));
        self.scrolled = now;
        self.scroll_rate() * elapsed.as_secs_f32()
    }
}

#[derive(Default)]
pub(super) struct Placements([Option<Point>; 4]);

impl Placements {
    pub fn regions(&self, mut controls: ControlRegions, stage: Bounds) -> ControlRegions {
        let reserved = controls.reserved;
        for (index, bounds) in [
            &mut controls.left,
            &mut controls.middle,
            &mut controls.right,
            &mut controls.wheel,
        ]
        .into_iter()
        .enumerate()
        {
            if let (Some(point), Some(rect)) = (self.0[index], bounds.as_mut()) {
                rect.0 = stage.0 + point.0 * (stage.2 - rect.2).max(0.);
                rect.1 = stage.1 + point.1 * (stage.3 - rect.3).max(0.);
            }
            if let (Some(rect), Some(avoid)) = (bounds.as_mut(), reserved)
                && rect.0 < avoid.0 + avoid.2
                && rect.0 + rect.2 > avoid.0
                && rect.1 < avoid.1 + avoid.3
                && rect.1 + rect.3 > avoid.1
            {
                let below = avoid.1 + avoid.3;
                let left = avoid.0 - rect.2;
                if below + rect.3 <= stage.1 + stage.3
                    && (left < stage.0 || below - rect.1 <= rect.0 - left)
                {
                    rect.1 = below;
                } else if left < stage.0 && avoid.1 - rect.3 >= stage.1 {
                    rect.1 = avoid.1 - rect.3;
                } else {
                    rect.0 = left.max(stage.0);
                }
            }
        }
        controls
    }

    pub fn move_control(&mut self, gesture: Gesture, stage: Bounds) {
        let width = (stage.2 - gesture.bounds.2).max(0.);
        let height = (stage.3 - gesture.bounds.3).max(0.);
        self.0[gesture.control as usize] = Some(Point(
            if width > 0. {
                ((gesture.bounds.0 + gesture.point.0 - gesture.start.0 - stage.0) / width)
                    .clamp(0., 1.)
            } else {
                0.
            },
            if height > 0. {
                ((gesture.bounds.1 + gesture.point.1 - gesture.start.1 - stage.1) / height)
                    .clamp(0., 1.)
            } else {
                0.
            },
        ));
    }
}
