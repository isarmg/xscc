use super::{ACCENT, BORDER, ChromeAction, ChromeModel, RAISED, SessionChrome, TEXT, at};
use crate::session_input::Control;
use gpui::{Context, Div, IntoElement, PathBuilder, canvas, div, point, prelude::*, px, rgb, rgba};

pub(super) fn render(model: &ChromeModel, cx: &mut Context<SessionChrome>) -> Div {
    let Some(mouse) = model.layout.mouse else {
        return div();
    };
    let mut dock = div().absolute().top_0().left_0().size_full();
    for (control, rect, id, label, pressed) in [
        (
            Control::Left,
            mouse.left,
            "session-mouse-left",
            "左键",
            model.left_pressed,
        ),
        (
            Control::Right,
            mouse.right,
            "session-mouse-right",
            "右键",
            model.right_pressed,
        ),
        (
            Control::Middle,
            mouse.middle,
            "session-mouse-middle",
            "中键",
            model.middle_pressed,
        ),
    ] {
        let moving = model.moving_control == Some(control);
        let color = if pressed {
            crate::theme::LIGHT.on_accent
        } else {
            TEXT
        };
        dock = dock.child(
            at(rect)
                .id(id)
                .role(gpui::Role::Button)
                .aria_label(label)
                .aria_description("轻点点击鼠标按键；单指长按半秒后拖动可移动按钮位置")
                .rounded_full()
                .bg(rgba(if pressed {
                    (ACCENT << 8) | 0xee
                } else {
                    (RAISED << 8) | 0xe0
                }))
                .border_2()
                .border_color(rgb(if moving || pressed { ACCENT } else { BORDER }))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .text_color(rgb(color))
                .child(mouse_icon(control, color))
                .child(div().text_size(px(10.)).line_height(px(12.)).child(label))
                .on_click(
                    cx.listener(move |_, _, _, cx| cx.emit(ChromeAction::MouseButton(control))),
                ),
        );
    }
    let moving = model.moving_control == Some(Control::Wheel);
    let travel = (mouse.wheel.height - 44.).max(0.);
    dock.child(
        at(mouse.wheel)
            .id("session-mouse-wheel")
            .aria_label("鼠标滚轮滑块")
            .aria_description("上下滑动滚动；单指长按半秒后拖动可移动滑块位置")
            .rounded_full()
            .bg(rgba((RAISED << 8) | 0xe0))
            .border_2()
            .border_color(rgb(if moving { ACCENT } else { BORDER }))
            .child(
                div()
                    .absolute()
                    .left(px(21.))
                    .top(px(14.))
                    .w(px(2.))
                    .h(px((mouse.wheel.height - 28.).max(0.)))
                    .rounded_full()
                    .bg(rgb(BORDER)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(10.))
                    .top(px(10. + model.wheel_position * travel))
                    .w(px(24.))
                    .h(px(20.))
                    .rounded_full()
                    .bg(rgb(ACCENT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(crate::theme::LIGHT.on_accent))
                    .text_size(px(12.))
                    .child("≡"),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .h(px(mouse.wheel.height / 2.))
                    .id("session-wheel-up")
                    .role(gpui::Role::Button)
                    .aria_label("滚轮向上滚动")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ChromeAction::MouseWheel(40)))),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .w_full()
                    .h(px(mouse.wheel.height / 2.))
                    .id("session-wheel-down")
                    .role(gpui::Role::Button)
                    .aria_label("滚轮向下滚动")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ChromeAction::MouseWheel(-40)))),
            ),
    )
}

fn mouse_icon(control: Control, color: u32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let p = |x, y| bounds.origin + point(px(x), px(y));
            let mut outline = PathBuilder::stroke(px(1.5));
            outline.move_to(p(12., 2.));
            outline.curve_to(p(5., 9.), p(5., 2.));
            outline.line_to(p(5., 17.));
            outline.curve_to(p(12., 24.), p(5., 24.));
            outline.curve_to(p(19., 17.), p(19., 24.));
            outline.line_to(p(19., 9.));
            outline.curve_to(p(12., 2.), p(19., 2.));
            if let Ok(path) = outline.build() {
                window.paint_path(path, rgb(color));
            }
            let mut selected = PathBuilder::fill();
            match control {
                Control::Left => {
                    selected.move_to(p(10.5, 4.));
                    selected.curve_to(p(6.5, 9.), p(6.5, 4.));
                    selected.line_to(p(6.5, 11.));
                    selected.curve_to(p(10.5, 11.), p(8.5, 12.));
                    selected.line_to(p(10.5, 4.));
                }
                Control::Right => {
                    selected.move_to(p(13.5, 4.));
                    selected.curve_to(p(17.5, 9.), p(17.5, 4.));
                    selected.line_to(p(17.5, 11.));
                    selected.curve_to(p(13.5, 11.), p(15.5, 12.));
                    selected.line_to(p(13.5, 4.));
                }
                _ => {
                    selected.move_to(p(10.5, 7.));
                    selected.curve_to(p(13.5, 7.), p(12., 4.));
                    selected.line_to(p(13.5, 11.));
                    selected.curve_to(p(10.5, 11.), p(12., 14.));
                    selected.line_to(p(10.5, 7.));
                }
            }
            if let Ok(path) = selected.build() {
                window.paint_path(path, rgb(color));
            }
        },
    )
    .w(px(24.))
    .h(px(26.))
    .flex_shrink_0()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        session_input::{ControlRegions, InputMode, SessionInput},
        session_layout::{self, Panel, Rect},
        touch_input::{Geometry, Output, Phase, Point},
    };
    use gpui_test_support::{
        AppContext, Entity, HitboxBehavior, Render, TestAppContext, TouchEvent, TouchId,
        TouchPhase, Window,
    };
    use std::{cell::RefCell, rc::Rc};

    struct Stage {
        chrome: Entity<SessionChrome>,
        input: SessionInput,
        output: Rc<RefCell<Vec<Output>>>,
    }

    impl Render for Stage {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let owner = cx.entity_id();
            let layout = self.chrome.read(cx).model().layout;
            let mouse = layout.mouse.unwrap();
            let bounds = |rect: Rect| (rect.x, rect.y, rect.width, rect.height);
            let geometry = Geometry {
                bounds: bounds(layout.stage),
                extent: (640, 1280),
                origin: (0, 0),
            };
            let controls = ControlRegions {
                left: Some(bounds(mouse.left)),
                middle: Some(bounds(mouse.middle)),
                right: Some(bounds(mouse.right)),
                wheel: Some(bounds(mouse.wheel)),
                reserved: Some(bounds(layout.rail)),
            };
            let touch = cx.listener(move |view, event: &TouchEvent, _, _| {
                let phase = match event.phase {
                    TouchPhase::Started => Phase::Started,
                    TouchPhase::Moved => Phase::Moved,
                    TouchPhase::Ended => Phase::Ended,
                    TouchPhase::Cancelled => Phase::Cancelled,
                };
                let output = view.input.event(
                    event.id.0,
                    phase,
                    Point(event.position.x.into(), event.position.y.into()),
                    geometry,
                    controls,
                );
                view.output.borrow_mut().extend(output);
                view.output.borrow_mut().extend(view.input.flush());
            });
            div()
                .relative()
                .w(px(320.))
                .h(px(640.))
                .child(
                    canvas(
                        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                        move |_, hitbox, window, _| {
                            window.on_touch_event(owner, &hitbox, move |event, window, cx| {
                                touch(event, window, cx)
                            })
                        },
                    )
                    .absolute()
                    .w(px(layout.stage.width))
                    .h(px(layout.stage.height)),
                )
                .child(self.chrome.clone())
        }
    }

    #[gpui_test_support::test]
    fn controls_keep_raw_touch_capture_without_duplicate_clicks_and_allow_mouse_activation(
        cx: &mut TestAppContext,
    ) {
        for expanded in [true, false] {
            let output = Rc::new(RefCell::new(Vec::new()));
            let (stage, cx) = cx.add_window_view(|_, cx| {
                let mut input = SessionInput::default();
                input.set_mode(InputMode::Pointer);
                Stage {
                    chrome: cx.new(|cx| {
                        let mut chrome = SessionChrome::new(cx);
                        chrome.model.has_frame = true;
                        chrome.model.rail_expanded = expanded;
                        chrome.model.pointer_mode = true;
                        chrome.model.wheel_position = 0.5;
                        chrome.model.layout =
                            session_layout::layout(320., 640., expanded, Panel::None, true, 88.);
                        chrome
                    }),
                    input,
                    output: output.clone(),
                }
            });
            let chrome = cx.update(|_, cx| stage.read(cx).chrome.clone());
            let actions = Rc::new(RefCell::new(Vec::new()));
            let seen = actions.clone();
            let _subscription = cx.update(|window, cx| {
                window.subscribe(&chrome, cx, move |_, action, _, _| {
                    seen.borrow_mut().push(*action)
                })
            });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let mouse = cx.update(|_, cx| chrome.read(cx).model.layout.mouse.unwrap());
            for (rect, phases) in [
                (
                    mouse.left,
                    vec![(TouchPhase::Started, 0.), (TouchPhase::Ended, 0.)],
                ),
                (
                    mouse.middle,
                    vec![(TouchPhase::Started, 0.), (TouchPhase::Cancelled, 0.)],
                ),
                (
                    mouse.wheel,
                    vec![
                        (TouchPhase::Started, 0.),
                        (TouchPhase::Moved, 20.),
                        (TouchPhase::Ended, 20.),
                    ],
                ),
            ] {
                for (phase, dy) in phases {
                    cx.simulate_event(TouchEvent {
                        id: TouchId(1),
                        phase,
                        position: point(px(rect.x + 24.), px(rect.y + 24. + dy)),
                        ..TouchEvent::default()
                    });
                }
            }
            assert!(
                actions.borrow().is_empty(),
                "captured touch must not synthesize a second click"
            );
            let edges: Vec<_> = output
                .borrow()
                .iter()
                .filter(|event| !matches!(event, Output::Move(_, _)))
                .copied()
                .collect();
            assert!(matches!(
                edges.as_slice(),
                [Output::Down(_, _), Output::Up(_, _), Output::Scroll(0, 20)]
            ));
            cx.simulate_click(
                point(px(mouse.right.x + 24.), px(mouse.right.y + 24.)),
                Default::default(),
            );
            assert_eq!(
                *actions.borrow(),
                vec![ChromeAction::MouseButton(Control::Right)]
            );
        }
    }
}
