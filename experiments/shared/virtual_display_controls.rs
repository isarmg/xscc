use crate::theme;
use gpui::{App, Window, div, prelude::*, px, rgb};
use std::rc::Rc;

const CONTROLLER_MAX_COUNT: u32 = 8;

pub fn controller_count_step(count: u32, increase: bool, maximum: Option<u32>) -> Option<u32> {
    if increase {
        (count
            < maximum
                .unwrap_or(CONTROLLER_MAX_COUNT)
                .min(CONTROLLER_MAX_COUNT))
        .then(|| count + 1)
    } else {
        count.checked_sub(1)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub protocol_version: u32,
    pub status: String,
    pub message: String,
    pub configured_count: u32,
    pub persistent: bool,
    pub maximum_count: Option<u32>,
}

impl State {
    pub fn configurable(&self) -> bool {
        self.protocol_version >= 1
            && matches!(
                self.status.as_str(),
                "ready" | "not_installed" | "disabled" | "reboot_required" | "error"
            )
    }

    pub fn configuration_step(&self, increase: bool, host: bool) -> Option<Configuration> {
        if !self.configurable() {
            return None;
        }
        let count = if host {
            if increase {
                self.configured_count
                    .checked_add(1)
                    .filter(|count| *count <= self.maximum_count.unwrap_or(i32::MAX as u32))
            } else {
                self.configured_count.checked_sub(1)
            }
        } else {
            controller_count_step(self.configured_count, increase, self.maximum_count)
        }?;
        Some(Configuration {
            count,
            persistent: self.persistent,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Configuration {
    pub count: u32,
    pub persistent: bool,
}

pub fn count_controls(
    count: Option<u32>,
    can_remove: bool,
    can_add: bool,
    palette: theme::Palette,
    on_step: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    let on_step = Rc::new(on_step);
    let button =
        |increase| {
            let (id, label, description, enabled) = if increase {
                ("virtual-count-plus", "+", "增加虚拟屏数量", can_add)
            } else {
                ("virtual-count-minus", "−", "减少虚拟屏数量", can_remove)
            };
            let on_step = on_step.clone();
            div()
                .id(id)
                .debug_selector(move || id.into())
                .aria_label(description)
                .w(px(48.))
                .h(px(48.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(10.))
                .bg(rgb(palette.accent_soft))
                .opacity(if enabled { 1. } else { 0.42 })
                .child(label)
                .when(enabled, |button| {
                    button.cursor_pointer().focusable().tab_index(0).on_click(
                        move |_, window, cx| {
                            on_step(&increase, window, cx);
                        },
                    )
                })
        };
    div()
        .flex()
        .items_center()
        .justify_center()
        .gap_2()
        .child(button(false))
        .child(
            div()
                .min_w(px(64.))
                .text_center()
                .child(count.map_or_else(|| "—".into(), |count| count.to_string())),
        )
        .child(button(true))
}

#[cfg(test)]
mod tests {
    use super::{Configuration, State, controller_count_step};

    #[test]
    fn controller_count_buttons_allow_zero_and_stop_at_eight_or_host_limit() {
        assert_eq!(controller_count_step(0, false, None), None);
        assert_eq!(controller_count_step(1, false, None), Some(0));
        assert_eq!(controller_count_step(7, true, None), Some(8));
        assert_eq!(controller_count_step(8, true, None), None);
        assert_eq!(controller_count_step(3, true, Some(4)), Some(4));
        assert_eq!(controller_count_step(4, true, Some(4)), None);
        assert_eq!(controller_count_step(8, true, Some(32)), None);
        assert_eq!(controller_count_step(12, true, None), None);
        assert_eq!(controller_count_step(12, false, None), Some(11));
    }

    #[test]
    fn count_changes_preserve_policy_and_use_the_confirmed_host_count() {
        let mut state = State {
            protocol_version: 1,
            status: "ready".into(),
            configured_count: 12,
            persistent: true,
            ..Default::default()
        };
        assert_eq!(state.configuration_step(true, false), None);
        assert_eq!(
            state.configuration_step(false, false),
            Some(Configuration {
                count: 11,
                persistent: true
            })
        );
        assert_eq!(
            state.configuration_step(true, true),
            Some(Configuration {
                count: 13,
                persistent: true
            })
        );
        state.configured_count = 0;
        assert_eq!(state.configuration_step(false, true), None);
        state.maximum_count = Some(1);
        assert_eq!(state.configuration_step(true, true).unwrap().count, 1);
        state.status = "external_owned".into();
        assert_eq!(state.configuration_step(true, true), None);
    }
}
