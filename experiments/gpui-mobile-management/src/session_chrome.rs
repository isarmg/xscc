use crate::{
    session_input::Control,
    session_layout::{Panel, Rect, SessionLayout},
};
use gpui::{
    Context, Div, EventEmitter, IntoElement, PathBuilder, Render, Role, Stateful, Window, canvas,
    div, point, prelude::*, px, rgb,
};

mod bitrate;
mod mouse_controls;
mod resolution;
pub(crate) mod virtual_displays;

const PAGE: u32 = 0x000000;
const SURFACE: u32 = crate::theme::LIGHT.panel;
const RAISED: u32 = crate::theme::LIGHT.accent_soft;
const BORDER: u32 = crate::theme::LIGHT.border;
const TEXT: u32 = crate::theme::LIGHT.text;
const MUTED: u32 = crate::theme::LIGHT.muted;
const ACCENT: u32 = crate::theme::LIGHT.accent;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChromeMonitor {
    pub index: usize,
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub available: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChromeModel {
    pub layout: SessionLayout,
    pub connecting: bool,
    pub connected: bool,
    pub has_frame: bool,
    pub notice: String,
    pub pointer_mode: bool,
    pub view_only: bool,
    pub can_control: bool,
    pub can_request_control: bool,
    pub mouse_buttons: bool,
    pub rail_expanded: bool,
    pub panel: Panel,
    pub audio_on: bool,
    pub audio_error: Option<String>,
    pub bitrate_supported: bool,
    pub max_bitrate_kbps: u32,
    pub bitrate_limit_kbps: Option<u32>,
    pub can_change_resolution: bool,
    pub current_resolution: Option<(i32, i32, u32)>,
    pub resolutions: Vec<(i32, i32, u32)>,
    pub virtual_display_configuration: Option<virtual_displays::form::State>,
    pub virtual_display_pending: bool,
    pub virtual_display_supported: bool,
    pub virtual_display_driver: String,
    pub virtual_display_remove_latest_only: bool,
    pub can_add_virtual_display: bool,
    pub virtual_display_ids: Vec<u32>,
    pub current_monitor: Option<usize>,
    pub pending_monitor: Option<usize>,
    pub monitors: Vec<ChromeMonitor>,
    pub quality: String,
    pub left_pressed: bool,
    pub right_pressed: bool,
    pub middle_pressed: bool,
    pub moving_control: Option<Control>,
    pub wheel_position: f32,
    pub right_click_armed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromeAction {
    Exit,
    ToggleRail,
    OpenPanel(Panel),
    ClosePanel,
    SetPointer(bool),
    ToggleMouseButtons,
    MouseButton(Control),
    MouseWheel(i32),
    ToggleViewOnly,
    ToggleAudio,
    Bitrate(Option<u32>),
    Resolution(i32, i32, u32),
    QueryVirtualDisplays,
    ConfigureVirtualDisplays(virtual_displays::form::Configuration),
    AddVirtualDisplay,
    RemoveVirtualDisplay(u32),
    Monitor(usize),
    ArmRightClick,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MediaMenu {
    Displays,
    RefreshRate,
    Resolution,
    Bitrate,
    VirtualDisplays,
}

pub struct SessionChrome {
    model: ChromeModel,
    bitrate: bitrate::BitrateEditor,
    resolution: resolution::ResolutionEditor,
    media_menu: Option<MediaMenu>,
}

impl EventEmitter<ChromeAction> for SessionChrome {}

impl SessionChrome {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            model: ChromeModel::default(),
            bitrate: bitrate::BitrateEditor::new(cx),
            resolution: resolution::ResolutionEditor::new(cx),
            media_menu: None,
        }
    }

    pub fn model(&self) -> &ChromeModel {
        &self.model
    }

    pub fn set_model(&mut self, model: ChromeModel, cx: &mut Context<Self>) -> bool {
        if self.model == model {
            return false;
        }
        if model.panel != Panel::Display
            || !model.connected
            || !model.bitrate_supported
            || model.pending_monitor.is_some()
            || self.model.max_bitrate_kbps != model.max_bitrate_kbps
            || self.model.layout != model.layout
        {
            self.bitrate.cancel();
        }
        if model.panel != Panel::Display || !model.connected {
            self.media_menu = None;
        }
        self.model = model;
        cx.notify();
        true
    }

    fn button(
        &self,
        cx: &mut Context<Self>,
        id: impl Into<gpui::ElementId>,
        label: &str,
        selected: bool,
        enabled: bool,
        action: ChromeAction,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .min_h(px(44.))
            .min_w(px(44.))
            .px_3()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(14.))
            .bg(rgb(if selected { ACCENT } else { RAISED }))
            .text_color(rgb(if selected {
                crate::theme::LIGHT.on_accent
            } else {
                TEXT
            }))
            .text_size(px(13.))
            .opacity(if enabled { 1. } else { 0.42 })
            .child(label.to_owned())
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.85))
                    .active(|style| style.opacity(0.65))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if let ChromeAction::Resolution(width, height, rate) = action {
                            if !view.resolution_enabled()
                                || !refresh_modes(
                                    view.model.current_resolution,
                                    &view.model.resolutions,
                                )
                                .contains(&(width, height, rate))
                            {
                                return;
                            }
                            view.media_menu = None;
                            cx.notify();
                        }
                        if let ChromeAction::Monitor(index) = action {
                            if !view.model.connected
                                || view.model.pending_monitor.is_some()
                                || view.model.virtual_display_pending
                                || !view
                                    .model
                                    .monitors
                                    .iter()
                                    .any(|monitor| monitor.index == index && monitor.available)
                            {
                                return;
                            }
                            view.media_menu = None;
                            cx.notify();
                        }
                        cx.emit(action);
                        cx.stop_propagation();
                    }))
            })
    }

    fn toggle(
        &self,
        cx: &mut Context<Self>,
        id: &'static str,
        label: &str,
        selected: bool,
        enabled: bool,
        action: ChromeAction,
    ) -> Stateful<Div> {
        self.button(cx, id, "", false, enabled, action)
            .w_full()
            .justify_between()
            .gap_2()
            .child(label.to_owned())
            .child(
                div()
                    .relative()
                    .w(px(38.))
                    .h(px(22.))
                    .rounded_full()
                    .bg(rgb(if selected { ACCENT } else { BORDER }))
                    .child(
                        div()
                            .absolute()
                            .top(px(3.))
                            .left(px(if selected { 19. } else { 3. }))
                            .size(px(16.))
                            .rounded_full()
                            .bg(rgb(if selected {
                                crate::theme::LIGHT.on_accent
                            } else {
                                TEXT
                            })),
                    ),
            )
    }

    fn rail(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let model = &self.model;
        if !model.rail_expanded {
            return at(model.layout.rail)
                .id("session-rail-expand")
                .role(Role::Button)
                .aria_label("展开操控栏")
                .aria_expanded(false)
                .rounded(px(12.))
                .bg(rgb(PAGE))
                .border_1()
                .border_color(rgb(0xffffff))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .occlude()
                .cursor_pointer()
                .child(glyph(Glyph::Expand, 0xffffff))
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.emit(ChromeAction::ToggleRail);
                    cx.stop_propagation();
                }));
        }
        let mut rail = at(model.layout.rail)
            .id("session-control-rail")
            .rounded(px(18.))
            .border_1()
            .border_color(rgb(0xffffff))
            .bg(rgb(PAGE))
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .py_1()
            .overflow_y_scroll()
            .occlude();
        for (id, label, icon, panel, enabled) in [
            (
                "session-operations",
                "操作",
                Glyph::Operations,
                Panel::Operations,
                true,
            ),
            (
                "session-keyboard",
                "键盘",
                Glyph::Keyboard,
                Panel::Keyboard,
                model.connected
                    && model.can_control
                    && !model.view_only
                    && model.pending_monitor.is_none(),
            ),
            (
                "session-displays",
                "显示",
                Glyph::Display,
                Panel::Display,
                model.connected,
            ),
        ] {
            let selected = model.panel == panel;
            let action = if selected {
                ChromeAction::ClosePanel
            } else {
                ChromeAction::OpenPanel(panel)
            };
            rail = rail.child(self.rail_button(cx, id, label, icon, (selected, enabled), action));
        }
        rail.child(self.rail_button(
            cx,
            "session-exit",
            "退出",
            Glyph::Exit,
            (false, true),
            ChromeAction::Exit,
        ))
        .child(self.rail_button(
            cx,
            "session-rail-collapse",
            "收起",
            Glyph::Collapse,
            (false, true),
            ChromeAction::ToggleRail,
        ))
    }

    fn rail_button(
        &self,
        cx: &mut Context<Self>,
        id: &'static str,
        label: &str,
        icon: Glyph,
        state: (bool, bool),
        action: ChromeAction,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(label.to_owned())
            .aria_selected(state.0)
            .w(px(44.))
            .h(px(44.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .opacity(if state.1 { 1. } else { 0.42 })
            .child(glyph(icon, 0xffffff))
            .when(state.1, |button| {
                button
                    .cursor_pointer()
                    .hover(|style| style.opacity(0.8))
                    .active(|style| style.opacity(0.6))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(action);
                        cx.stop_propagation();
                    }))
            })
    }

    fn screen_list(&self, cx: &mut Context<Self>) -> Div {
        let mut list = div().flex().flex_col().gap_2();
        if self.model.monitors.is_empty() {
            return list.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child("正在读取远程屏幕…"),
            );
        }
        for monitor in &self.model.monitors {
            let name = if monitor.name.is_empty() {
                format!("屏幕 {}", monitor.index + 1)
            } else {
                monitor.name.clone()
            };
            let state = if self.model.pending_monitor == Some(monitor.index) {
                " · 切换中"
            } else if !monitor.available {
                " · 不可用"
            } else {
                ""
            };
            let label = format!("{} · {}×{}{}", name, monitor.width, monitor.height, state);
            list = list.child(
                self.button(
                    cx,
                    format!("session-monitor-{}", monitor.index),
                    &label,
                    self.model.current_monitor == Some(monitor.index),
                    self.model.connected
                        && self.model.pending_monitor.is_none()
                        && !self.model.virtual_display_pending
                        && monitor.available,
                    ChromeAction::Monitor(monitor.index),
                )
                .w_full()
                .justify_start(),
            );
        }
        list
    }

    fn operations(&self, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        let input_ready = model.connected
            && model.has_frame
            && model.can_control
            && model.pending_monitor.is_none();
        let mode = div()
            .flex()
            .gap_2()
            .child(
                self.button(
                    cx,
                    "session-direct-touch",
                    "屏幕触控",
                    !model.pointer_mode,
                    input_ready,
                    ChromeAction::SetPointer(false),
                )
                .flex_1(),
            )
            .child(
                self.button(
                    cx,
                    "session-trackpad",
                    "鼠标指针",
                    model.pointer_mode,
                    input_ready,
                    ChromeAction::SetPointer(true),
                )
                .flex_1(),
            );
        let mut body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(mode)
            .child(self.toggle(
                cx,
                "session-audio",
                "声音",
                model.audio_on,
                model.connected,
                ChromeAction::ToggleAudio,
            ))
            .child(self.toggle(
                cx,
                "session-view-only",
                "仅查看",
                model.view_only || !model.can_control,
                model.connected && (model.can_control || model.can_request_control),
                ChromeAction::ToggleViewOnly,
            ))
            .when(model.can_request_control && !model.can_control, |body| {
                body.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .child("启用控制会重新连接。"),
                )
            })
            .child(self.toggle(
                cx,
                "session-mouse-buttons",
                "鼠标按键",
                model.mouse_buttons,
                input_ready && model.pointer_mode && !model.view_only,
                ChromeAction::ToggleMouseButtons,
            ));
        if !model.pointer_mode {
            body = body.child(
                self.button(
                    cx,
                    "session-right-click",
                    if model.right_click_armed {
                        "取消下一次右键"
                    } else {
                        "下一次触控为右键"
                    },
                    model.right_click_armed,
                    input_ready && !model.view_only,
                    ChromeAction::ArmRightClick,
                )
                .w_full(),
            );
        }
        if let Some(error) = &model.audio_error {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(crate::theme::LIGHT.danger))
                    .child(error.clone()),
            );
        }
        body
    }

    fn media_menu(
        &self,
        menu: MediaMenu,
        label: &str,
        enabled: bool,
        content: Div,
        cx: &mut Context<Self>,
    ) -> Div {
        let open = self.media_menu == Some(menu);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .id(match menu {
                        MediaMenu::Displays => "session-display-menu",
                        MediaMenu::RefreshRate => "session-refresh-rate-menu",
                        MediaMenu::Resolution => "session-resolution-menu",
                        MediaMenu::Bitrate => "session-bitrate-menu",
                        MediaMenu::VirtualDisplays => "session-virtual-displays-menu",
                    })
                    .min_h(px(44.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded(px(14.))
                    .bg(rgb(if open { ACCENT } else { RAISED }))
                    .text_color(rgb(if open {
                        crate::theme::LIGHT.on_accent
                    } else {
                        TEXT
                    }))
                    .opacity(if enabled { 1. } else { 0.42 })
                    .child(label.to_owned())
                    .child(if open { "▴" } else { "▾" })
                    .when(enabled, |button| {
                        button
                            .cursor_pointer()
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.bitrate.cancel();
                                view.media_menu = (view.media_menu != Some(menu)).then_some(menu);
                                if view.media_menu == Some(MediaMenu::VirtualDisplays)
                                    && view
                                        .model
                                        .virtual_display_configuration
                                        .as_ref()
                                        .is_some_and(|state| state.protocol_version >= 1)
                                {
                                    cx.emit(ChromeAction::QueryVirtualDisplays);
                                }
                                window.dismiss_virtual_keyboard();
                                cx.stop_propagation();
                                cx.notify();
                            }))
                    }),
            )
            .when(open, |menu| menu.child(content))
    }

    fn media_settings(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        let ready = model.connected && model.pending_monitor.is_none();
        let modes = refresh_modes(model.current_resolution, &model.resolutions);
        let refresh_enabled = self.resolution_enabled() && !modes.is_empty();
        let mut rates = div().flex().flex_col().gap_1();
        for mode in modes {
            rates = rates.child(
                self.button(
                    cx,
                    format!("session-refresh-rate-{}", mode.2),
                    &format!("{} Hz", mode.2),
                    model.current_resolution == Some(mode),
                    refresh_enabled,
                    ChromeAction::Resolution(mode.0, mode.1, mode.2),
                )
                .w_full(),
            );
        }
        let mut body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                self.media_menu(
                    MediaMenu::RefreshRate,
                    &model
                        .current_resolution
                        .filter(|mode| mode.2 > 0)
                        .map(|mode| format!("刷新率 · {} Hz", mode.2))
                        .unwrap_or_else(|| "刷新率 · 未知".into()),
                    refresh_enabled,
                    rates,
                    cx,
                ),
            )
            .child(self.media_menu(
                MediaMenu::Resolution,
                "分辨率",
                self.resolution_enabled(),
                self.resolution_controls(window, cx),
                cx,
            ))
            .child(self.media_menu(
                MediaMenu::Bitrate,
                &self.bitrate_label(),
                ready && model.bitrate_supported,
                self.bitrate_controls(cx),
                cx,
            ));
        let virtual_displays = if self.media_menu == Some(MediaMenu::VirtualDisplays) {
            self.virtual_display_controls(cx)
        } else {
            div()
        };
        body = body.child(self.media_menu(
            MediaMenu::VirtualDisplays,
            "虚拟显示器",
            model.connected,
            virtual_displays,
            cx,
        ));
        body
    }

    fn panel(&self, area: Rect, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let title = match self.model.panel {
            Panel::Operations => "操作",
            Panel::Display => "显示",
            Panel::Keyboard => "键盘",
            Panel::None => "",
        };
        let body = if self.model.panel == Panel::Operations {
            self.operations(cx)
        } else {
            let mut body = div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    self.media_menu(
                        MediaMenu::Displays,
                        &self
                            .model
                            .monitors
                            .iter()
                            .find(|monitor| Some(monitor.index) == self.model.current_monitor)
                            .map_or_else(
                                || "显示器".into(),
                                |monitor| {
                                    format!(
                                        "显示器 · 屏幕 {} · {}×{}",
                                        monitor.index + 1,
                                        monitor.width,
                                        monitor.height
                                    )
                                },
                            ),
                        self.model.connected
                            && self.model.pending_monitor.is_none()
                            && !self.model.virtual_display_pending
                            && self.model.monitors.iter().any(|monitor| monitor.available),
                        self.screen_list(cx),
                        cx,
                    ),
                )
                .child(self.media_settings(window, cx));
            if !self.model.quality.is_empty() {
                body = body.child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(19.))
                        .text_color(rgb(MUTED))
                        .child(self.model.quality.clone()),
                );
            }
            body
        };
        at(area)
            .id("session-options-panel")
            .rounded(px(14.))
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgb(BORDER))
            .overflow_hidden()
            .occlude()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(48.))
                    .flex_shrink_0()
                    .px_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(px(16.)).child(title))
                    .child(
                        self.button(
                            cx,
                            "session-panel-close",
                            "",
                            false,
                            true,
                            ChromeAction::ClosePanel,
                        )
                        .role(Role::Button)
                        .aria_label("关闭面板")
                        .w(px(44.))
                        .h(px(44.))
                        .px_0()
                        .child(glyph(Glyph::Close, TEXT)),
                    ),
            )
            .child(
                div()
                    .id("session-options-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(body)
                    .when(!self.model.notice.is_empty(), |body| {
                        body.child(
                            div()
                                .text_size(px(12.))
                                .line_height(px(19.))
                                .text_color(rgb(MUTED))
                                .child(self.model.notice.clone()),
                        )
                    }),
            )
    }
}

impl Render for SessionChrome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut chrome = div()
            .relative()
            .size_full()
            .text_color(rgb(TEXT))
            .on_key_down(cx.listener(Self::bitrate_key_down))
            .on_key_down(cx.listener(Self::resolution_key_down))
            .on_key_up(cx.listener(Self::bitrate_key_up));
        if !self.model.has_frame {
            let status = if self.model.connecting {
                "正在连接…"
            } else if self.model.connected {
                "等待远程画面"
            } else {
                "会话未连接"
            };
            chrome = chrome.child(
                at(self.model.layout.video)
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .child(glyph(Glyph::Display, MUTED))
                    .child(div().text_size(px(16.)).child(status))
                    .when(!self.model.notice.is_empty(), |view| {
                        view.child(
                            div()
                                .max_w_full()
                                .px_4()
                                .text_size(px(12.))
                                .line_height(px(19.))
                                .text_color(rgb(MUTED))
                                .child(self.model.notice.clone()),
                        )
                    }),
            );
        }
        chrome = chrome.child(mouse_controls::render(&self.model, cx));
        if let Some(panel) = self.model.layout.panel {
            chrome = chrome.child(self.panel(panel, window, cx));
        }
        chrome.child(self.rail(cx))
    }
}

fn at(rect: Rect) -> Div {
    div()
        .absolute()
        .left(px(rect.x))
        .top(px(rect.y))
        .w(px(rect.width))
        .h(px(rect.height))
}

#[derive(Clone, Copy)]
enum Glyph {
    Operations,
    Keyboard,
    Display,
    Exit,
    Expand,
    Collapse,
    Close,
}

fn glyph(shape: Glyph, color: u32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let scale =
                (f32::from(bounds.size.width) / 24.).min(f32::from(bounds.size.height) / 24.);
            let mut path = PathBuilder::stroke(px(1.8 * scale));
            let position = |(x, y): (f32, f32)| bounds.origin + point(px(x * scale), px(y * scale));
            for stroke in strokes(shape) {
                if let Some((first, rest)) = stroke.split_first() {
                    path.move_to(position(*first));
                    for (index, &(x, y)) in rest.iter().enumerate() {
                        if let Some(&(next_x, next_y)) = rest.get(index + 1) {
                            let (previous_x, previous_y) = stroke[index];
                            let before = (x - previous_x, y - previous_y);
                            let after = (next_x - x, next_y - y);
                            let before_length = before.0.hypot(before.1);
                            let after_length = after.0.hypot(after.1);
                            let radius = 2_f32.min(before_length / 2.).min(after_length / 2.);
                            if radius > 0. {
                                path.line_to(position((
                                    x - before.0 / before_length * radius,
                                    y - before.1 / before_length * radius,
                                )));
                                path.curve_to(
                                    position((
                                        x + after.0 / after_length * radius,
                                        y + after.1 / after_length * radius,
                                    )),
                                    position((x, y)),
                                );
                            }
                        } else {
                            path.line_to(position((x, y)));
                        }
                    }
                }
            }
            match path.build() {
                Ok(path) => window.paint_path(path, rgb(color)),
                Err(_) => window.paint_quad(gpui::fill(bounds.inset(px(8.)), rgb(color))),
            }
            for stroke in strokes(shape) {
                for endpoint in [stroke.first(), stroke.last()].into_iter().flatten() {
                    let radius = px(0.9 * scale);
                    window.paint_quad(
                        gpui::fill(
                            gpui::Bounds::new(
                                position(*endpoint) - point(radius, radius),
                                gpui::size(radius * 2., radius * 2.),
                            ),
                            rgb(color),
                        )
                        .corner_radii(radius),
                    );
                }
            }
        },
    )
    .w(px(24.))
    .h(px(24.))
    .flex_shrink_0()
}

fn strokes(shape: Glyph) -> &'static [&'static [(f32, f32)]] {
    match shape {
        Glyph::Operations => &[
            &[(3., 6.), (21., 6.)],
            &[(3., 12.), (21., 12.)],
            &[(3., 18.), (21., 18.)],
            &[(8., 3.), (8., 9.)],
            &[(16., 9.), (16., 15.)],
            &[(10., 15.), (10., 21.)],
        ],
        Glyph::Keyboard => &[
            &[
                (12., 6.),
                (21., 6.),
                (21., 19.),
                (3., 19.),
                (3., 6.),
                (12., 6.),
            ],
            &[(6., 10.), (8., 10.)],
            &[(11., 10.), (13., 10.)],
            &[(16., 10.), (18., 10.)],
            &[(6., 14.), (8., 14.)],
            &[(11., 14.), (13., 14.)],
            &[(16., 14.), (18., 14.)],
            &[(8., 17.), (16., 17.)],
        ],
        Glyph::Display => &[
            &[
                (12., 4.),
                (21., 4.),
                (21., 17.),
                (3., 17.),
                (3., 4.),
                (12., 4.),
            ],
            &[(12., 17.), (12., 21.)],
            &[(7., 21.), (17., 21.)],
        ],
        Glyph::Exit => &[
            &[(11., 4.), (5., 4.), (5., 20.), (11., 20.)],
            &[(10., 12.), (22., 12.)],
            &[(17., 7.), (22., 12.), (17., 17.)],
        ],
        Glyph::Expand => &[&[(15., 6.), (9., 12.), (15., 18.)]],
        Glyph::Collapse => &[&[(9., 6.), (15., 12.), (9., 18.)]],
        Glyph::Close => &[&[(6., 6.), (18., 18.)], &[(18., 6.), (6., 18.)]],
    }
}

fn refresh_modes(
    current: Option<(i32, i32, u32)>,
    supported: &[(i32, i32, u32)],
) -> Vec<(i32, i32, u32)> {
    let Some((width, height, _)) = current.filter(|mode| mode.2 > 0) else {
        return Vec::new();
    };
    let mut modes: Vec<_> = supported
        .iter()
        .copied()
        .filter(|mode| mode.0 == width && mode.1 == height && mode.2 > 0)
        .collect();
    modes.sort_unstable_by_key(|mode| mode.2);
    modes.dedup();
    modes
}

#[cfg(test)]
mod refresh_tests {
    use super::*;

    #[test]
    fn refresh_menu_uses_only_reported_hz_for_the_current_size_and_requires_known_current_hz() {
        let modes = [
            (1920, 1080, 240),
            (3840, 2160, 144),
            (1920, 1080, 60),
            (1920, 1080, 75),
            (1920, 1080, 0),
            (1920, 1080, 60),
        ];
        assert_eq!(
            refresh_modes(Some((1920, 1080, 60)), &modes),
            [(1920, 1080, 60), (1920, 1080, 75), (1920, 1080, 240)]
        );
        assert!(refresh_modes(None, &modes).is_empty());
        assert!(refresh_modes(Some((1920, 1080, 0)), &modes).is_empty());
    }
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod rail_tests {
    use super::*;
    use gpui_test_support::{
        AppContext, Entity, HitboxBehavior, TestAppContext, TouchEvent, TouchId, TouchPhase,
    };
    use std::{cell::Cell, rc::Rc};

    struct Stage {
        chrome: Entity<SessionChrome>,
        remote_touches: Rc<Cell<usize>>,
    }

    impl Render for Stage {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let owner = cx.entity_id();
            let remote_touches = self.remote_touches.clone();
            let stage = self.chrome.read(cx).model.layout.stage;
            div()
                .relative()
                .w(px(320.))
                .h(px(640.))
                .child(
                    canvas(
                        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                        move |_, hitbox, window, _| {
                            window.on_touch_event(owner, &hitbox, move |_, _, _| {
                                remote_touches.set(remote_touches.get() + 1);
                            });
                        },
                    )
                    .absolute()
                    .w(px(stage.width))
                    .h(px(stage.height)),
                )
                .child(self.chrome.clone())
        }
    }

    #[gpui_test_support::test]
    fn corner_button_toggles_without_sending_remote_touch_and_leaves_the_lower_edge_free(
        cx: &mut TestAppContext,
    ) {
        let remote_touches = Rc::new(Cell::new(0));
        let (stage, cx) = cx.add_window_view(|_, cx| Stage {
            chrome: cx.new(|cx| {
                let mut chrome = SessionChrome::new(cx);
                chrome.model.has_frame = true;
                chrome.model.layout =
                    crate::session_layout::layout(320., 640., false, Panel::None, false, 88.);
                chrome
            }),
            remote_touches: remote_touches.clone(),
        });
        let chrome = cx.update(|_, cx| stage.read(cx).chrome.clone());
        let toggles = Rc::new(Cell::new(0));
        let count = toggles.clone();
        let _subscription = cx.update(|window, cx| {
            window.subscribe(&chrome, cx, move |chrome, action, _, cx| {
                assert_eq!(*action, ChromeAction::ToggleRail);
                count.set(count.get() + 1);
                chrome.update(cx, |chrome, cx| {
                    let expanded = !chrome.model.rail_expanded;
                    let mut model = chrome.model.clone();
                    model.rail_expanded = expanded;
                    model.layout = crate::session_layout::layout(
                        320.,
                        640.,
                        expanded,
                        Panel::None,
                        false,
                        88.,
                    );
                    chrome.set_model(model, cx);
                });
            })
        });
        for (y, end, expected_toggles, expected_remote) in [
            (30., TouchPhase::Ended, 1, 0),
            (218., TouchPhase::Ended, 2, 0),
            (400., TouchPhase::Ended, 2, 2),
            (30., TouchPhase::Cancelled, 2, 2),
            (30., TouchPhase::Ended, 3, 2),
        ] {
            cx.update(|window, cx| window.draw(cx).clear(cx));
            for phase in [TouchPhase::Started, end] {
                cx.simulate_event(TouchEvent {
                    id: TouchId(1),
                    phase,
                    position: point(px(290.), px(y)),
                    ..TouchEvent::default()
                });
            }
            assert_eq!(toggles.get(), expected_toggles);
            assert_eq!(remote_touches.get(), expected_remote);
        }
    }
}
