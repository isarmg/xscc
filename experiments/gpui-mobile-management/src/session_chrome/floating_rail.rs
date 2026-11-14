use super::{ChromeAction, SessionChrome};
use crate::session_layout::Rect;
use gpui::{
    Context, HitboxBehavior, IntoElement, TouchEvent, TouchId, TouchPhase, canvas, prelude::*,
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
        let touch = cx.listener(move |view, event: &TouchEvent, window, cx| {
            let viewport = window.viewport_size();
            let height = (viewport.height - window.insets().ime.bottom).max(gpui::px(0.));
            view.move_rail(
                event,
                (viewport.width.into(), height.into()),
                Instant::now(),
                cx,
            );
        });
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                window.on_touch_event(owner, &hitbox, move |event, window, cx| {
                    touch(event, window, cx);
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
        extent: (f32, f32),
        now: Instant,
        cx: &mut Context<Self>,
    ) {
        let position = (f32::from(event.position.x), f32::from(event.position.y));
        if event.phase == TouchPhase::Started {
            let action = Some(ChromeAction::ToggleRail);
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
