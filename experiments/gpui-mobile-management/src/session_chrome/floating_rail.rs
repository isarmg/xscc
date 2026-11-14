use super::{ChromeAction, SessionChrome};
use crate::session_layout::{Panel, Rect};
use gpui::{
    Bounds, Context, HitboxBehavior, IntoElement, Pixels, TouchEvent, TouchId, TouchPhase, canvas,
    prelude::*,
};
use std::time::{Duration, Instant};

pub(super) struct Gesture {
    id: TouchId,
    pub(super) started: Instant,
    start: (f32, f32),
    rail: Rect,
    action: Option<ChromeAction>,
    moving: bool,
}

impl SessionChrome {
    pub(super) fn rail_touch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = self.rail_touch_owner.entity_id();
        let touch = cx.listener(
            move |view, (event, bounds): &(TouchEvent, Bounds<Pixels>), window, cx| {
                let visible = window.fully_visible_bounds();
                let (_, width) = crate::session_layout::centered_horizontal_bounds(
                    window.viewport_size().width.into(),
                    visible.origin.x.into(),
                    visible.size.width.into(),
                );
                view.move_rail(
                    event,
                    *bounds,
                    (width, visible.size.height.into()),
                    Instant::now(),
                    cx,
                );
            },
        );
        canvas(
            |bounds, window, _| (bounds, window.insert_hitbox(bounds, HitboxBehavior::Normal)),
            move |_, (bounds, hitbox), window, _| {
                window.on_touch_event(owner, &hitbox, move |event, window, cx| {
                    touch(&(event.clone(), bounds), window, cx);
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    fn move_rail(
        &mut self,
        event: &TouchEvent,
        bounds: Bounds<Pixels>,
        extent: (f32, f32),
        now: Instant,
        cx: &mut Context<Self>,
    ) {
        let position = (f32::from(event.position.x), f32::from(event.position.y));
        if event.phase == TouchPhase::Started {
            let model = &self.model;
            let action = if !model.rail_expanded {
                Some(ChromeAction::ToggleRail)
            } else {
                let offset = if bounds.size.width > bounds.size.height {
                    f32::from(event.position.x - bounds.origin.x)
                } else {
                    f32::from(event.position.y - bounds.origin.y)
                } - 4.;
                let index = (offset / 48.).floor() as i32;
                let in_button = offset >= 0. && offset % 48. < 44.;
                match (in_button, index) {
                    (true, 0..=2) => {
                        let panel =
                            [Panel::Operations, Panel::Keyboard, Panel::Display][index as usize];
                        let enabled = match panel {
                            Panel::Keyboard => {
                                model.connected
                                    && model.can_control
                                    && !model.view_only
                                    && model.pending_monitor.is_none()
                            }
                            Panel::Display => model.connected,
                            _ => true,
                        };
                        enabled.then_some(if panel == model.panel {
                            ChromeAction::ClosePanel
                        } else {
                            ChromeAction::OpenPanel(panel)
                        })
                    }
                    (true, 3) => Some(ChromeAction::RotateScreen),
                    (true, 4) => Some(ChromeAction::Exit),
                    (true, 5) => Some(ChromeAction::ToggleRail),
                    _ => None,
                }
            };
            self.rail_gesture = Some(Gesture {
                id: event.id,
                started: now,
                start: position,
                rail: self.model.layout.rail,
                action,
                moving: false,
            });
            return;
        }
        let Some(gesture) = self
            .rail_gesture
            .as_mut()
            .filter(|gesture| gesture.id == event.id)
        else {
            return;
        };
        let held = now.saturating_duration_since(gesture.started) >= Duration::from_millis(500);
        let dx = position.0 - gesture.start.0;
        let dy = position.1 - gesture.start.1;
        if event.phase == TouchPhase::Moved {
            if held {
                gesture.moving = true;
                let normalize = |value: f32, extent: f32| {
                    if extent > 0. {
                        (value / extent * 10_000.).clamp(0., 10_000.).round() as u16
                    } else {
                        0
                    }
                };
                cx.emit(ChromeAction::MoveRail(
                    normalize(gesture.rail.x + dx, extent.0 - gesture.rail.width),
                    normalize(gesture.rail.y + dy, extent.1 - gesture.rail.height),
                ));
            } else if !held && dx.hypot(dy) >= 8. {
                gesture.action = None;
            }
        } else if matches!(event.phase, TouchPhase::Ended | TouchPhase::Cancelled) {
            let action = (event.phase == TouchPhase::Ended && !held && !gesture.moving)
                .then_some(gesture.action)
                .flatten();
            self.rail_gesture = None;
            if let Some(action) = action {
                cx.emit(action);
            }
        }
    }
}
