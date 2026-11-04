#[path = "../../../shared/virtual_display_controls.rs"]
pub(crate) mod form;

use super::{ChromeAction, SessionChrome};
use gpui::Context;

impl SessionChrome {
    pub(super) fn virtual_display_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let model = &self.model;
        let enabled = model.connected
            && model.pending_monitor.is_none()
            && !model.virtual_display_pending
            && model.can_control
            && !model.view_only;
        let state = model
            .virtual_display_configuration
            .as_ref()
            .filter(|state| state.protocol_version >= 1);
        let (count, can_remove, can_add) = if let Some(state) = state {
            (
                Some(state.configured_count),
                enabled && state.configuration_step(false, false).is_some(),
                enabled && state.configuration_step(true, false).is_some(),
            )
        } else {
            (
                model
                    .virtual_display_supported
                    .then_some(model.virtual_display_ids.len() as u32),
                enabled && model.virtual_display_supported && !model.virtual_display_ids.is_empty(),
                enabled && model.virtual_display_supported && model.can_add_virtual_display,
            )
        };
        form::count_controls(
            count,
            can_remove,
            can_add,
            44.,
            crate::theme::LIGHT,
            cx.listener(|view, increase: &bool, window, cx| {
                let model = &view.model;
                if model.connected
                    && model.pending_monitor.is_none()
                    && !model.virtual_display_pending
                    && model.can_control
                    && !model.view_only
                {
                    let action = if let Some(state) = model
                        .virtual_display_configuration
                        .as_ref()
                        .filter(|state| state.protocol_version >= 1)
                    {
                        state
                            .configuration_step(*increase, false)
                            .map(ChromeAction::ConfigureVirtualDisplays)
                    } else if model.virtual_display_supported {
                        if *increase && model.can_add_virtual_display {
                            Some(ChromeAction::AddVirtualDisplay)
                        } else if !*increase {
                            model
                                .virtual_display_ids
                                .last()
                                .copied()
                                .map(ChromeAction::RemoveVirtualDisplay)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if let Some(action) = action {
                        window.dismiss_virtual_keyboard();
                        cx.emit(action);
                    }
                }
                cx.stop_propagation();
            }),
        )
    }
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    use super::*;
    use crate::{
        session_chrome::{ChromeModel, MediaMenu},
        session_layout::{Panel, Rect},
    };
    use gpui::px;
    use gpui_test_support::{TestAppContext, TouchEvent, TouchId, TouchPhase, size};
    use std::{cell::RefCell, rc::Rc};

    fn tap(cx: &mut gpui_test_support::VisualTestContext, selector: &'static str) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("visible virtual display control: {selector}"));
        for phase in [TouchPhase::Started, TouchPhase::Ended] {
            cx.simulate_event(TouchEvent {
                id: TouchId(1),
                phase,
                position: bounds.center(),
                force: None,
                predicted_position: None,
            });
        }
    }

    #[gpui_test_support::test]
    fn count_buttons_apply_immediately_and_disable_while_pending(cx: &mut TestAppContext) {
        let (editor, cx) = cx.add_window_view(|_, cx| {
            let mut editor = SessionChrome::new(cx);
            editor.media_menu = Some(MediaMenu::VirtualDisplays);
            editor.model = ChromeModel {
                connected: true,
                can_control: true,
                panel: Panel::Display,
                virtual_display_configuration: Some(form::State {
                    protocol_version: 1,
                    status: "ready".into(),
                    configured_count: 12,
                    persistent: true,
                    ..Default::default()
                }),
                ..Default::default()
            };
            editor.model.layout.panel = Some(Rect {
                x: 0.,
                y: 0.,
                width: 500.,
                height: 1100.,
            });
            editor
        });
        let emitted = Rc::new(RefCell::new(Vec::new()));
        let events = emitted.clone();
        let _subscription = cx.update(|window, cx| {
            window.subscribe(&editor, cx, move |_, action, _, _| {
                events.borrow_mut().push(*action)
            })
        });
        cx.simulate_resize(size(px(800.), px(1100.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        tap(cx, "virtual-count-plus");
        assert!(emitted.borrow().is_empty());
        tap(cx, "virtual-count-minus");
        assert_eq!(
            *emitted.borrow(),
            vec![ChromeAction::ConfigureVirtualDisplays(
                form::Configuration {
                    count: 11,
                    persistent: true
                }
            )]
        );
        cx.update(|_, cx| {
            editor.update(cx, |view, cx| {
                let mut model = view.model.clone();
                model
                    .virtual_display_configuration
                    .as_mut()
                    .unwrap()
                    .configured_count = 11;
                model.virtual_display_pending = true;
                view.set_model(model, cx);
            })
        });
        tap(cx, "virtual-count-minus");
        assert_eq!(
            emitted.borrow().len(),
            1,
            "pending controls must not submit twice"
        );
        cx.update(|_, cx| {
            editor.update(cx, |view, cx| {
                let mut model = view.model.clone();
                model
                    .virtual_display_configuration
                    .as_mut()
                    .unwrap()
                    .configured_count = 1;
                model.virtual_display_pending = false;
                view.set_model(model, cx);
            })
        });
        tap(cx, "virtual-count-minus");
        assert_eq!(
            emitted.borrow().last(),
            Some(&ChromeAction::ConfigureVirtualDisplays(
                form::Configuration {
                    count: 0,
                    persistent: true
                }
            ))
        );
        assert!(cx.debug_bounds("virtual-apply").is_none());
        assert!(cx.debug_bounds("virtual-edit").is_none());
    }
}
