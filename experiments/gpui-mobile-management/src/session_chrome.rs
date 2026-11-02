use crate::session_layout::{Panel, Rect, SessionLayout};
use gpui::{
    Context, Div, EventEmitter, IntoElement, PathBuilder, Render, Stateful, Window, canvas, div,
    point, prelude::*, px, rgb,
};

mod bitrate;
mod resolution;
pub(crate) mod virtual_displays;

const PAGE: u32 = crate::theme::LIGHT.page;
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
    virtual_display: virtual_displays::form::Editor,
}

impl EventEmitter<ChromeAction> for SessionChrome {}

impl SessionChrome {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            model: ChromeModel::default(),
            bitrate: bitrate::BitrateEditor::new(cx),
            resolution: resolution::ResolutionEditor::new(cx),
            media_menu: None,
            virtual_display: virtual_displays::form::Editor::new(cx),
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
            self.virtual_display.editing = false;
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
            .min_h(px(48.))
            .min_w(px(48.))
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
                .bg(rgb(PAGE))
                .flex()
                .items_center()
                .justify_center()
                .occlude()
                .cursor_pointer()
                .child(
                    div()
                        .w(px(12.))
                        .h(px(48.))
                        .rounded_full()
                        .overflow_hidden()
                        .bg(rgb(RAISED))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(glyph(Glyph::Expand, MUTED)),
                )
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.emit(ChromeAction::ToggleRail);
                    cx.stop_propagation();
                }));
        }
        let mut rail = at(model.layout.rail)
            .id("session-control-rail")
            .bg(rgb(PAGE))
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .py_2()
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
        .child(div().flex_1())
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
        self.button(cx, id, "", state.0, state.1, action)
            .w(px(48.))
            .h(px(48.))
            .px_0()
            .flex_shrink_0()
            .flex_col()
            .text_size(px(10.))
            .line_height(px(12.))
            .child(glyph(icon, TEXT))
            .child(label.to_owned())
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
        let mut body =
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(section("控制方式"))
                .child(mode)
                .child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(
                    if model.pointer_mode {
                        "滑动移动指针，轻点左键；双指轻点右键，双指滑动滚动。"
                    } else {
                        "直接触摸远程画面，拖动时保持按住。"
                    },
                ))
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
                    .min_h(px(48.))
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
        let mut virtual_displays = div().flex().flex_col().gap_2();
        if model
            .virtual_display_configuration
            .as_ref()
            .is_some_and(|state| state.protocol_version >= 1)
        {
            if self.media_menu == Some(MediaMenu::VirtualDisplays) {
                virtual_displays = self.virtual_display_controls(cx);
            }
        } else if model.virtual_display_supported {
            virtual_displays = virtual_displays
                .child(
                    self.button(
                        cx,
                        "session-virtual-add",
                        "添加虚拟屏幕",
                        false,
                        ready && model.can_add_virtual_display,
                        ChromeAction::AddVirtualDisplay,
                    )
                    .w_full(),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(19.))
                        .text_color(rgb(MUTED))
                        .child(if model.virtual_display_driver == "amyuni_idd" {
                            "首次添加会在被控端启用随包驱动；Amyuni许可含广告页面条款。添加后可在屏幕列表中选择。"
                        } else if model.virtual_display_driver == "mtt_vdd" {
                            "使用正式签名驱动；分辨率、刷新率和数量由主机驱动与 Windows 确认。"
                        } else {
                            "首次添加会在被控端启用随包驱动。添加后可在屏幕列表中选择。"
                        }),
                );
            for &id in &model.virtual_display_ids {
                virtual_displays = virtual_displays.child(
                    self.button(
                        cx,
                        format!("session-virtual-remove-{id}"),
                        &if model.virtual_display_remove_latest_only {
                            "移除最近添加的虚拟屏幕".to_owned()
                        } else {
                            format!("移除虚拟屏幕 {id}")
                        },
                        false,
                        ready && model.can_control && !model.view_only,
                        ChromeAction::RemoveVirtualDisplay(id),
                    )
                    .w_full(),
                );
            }
        } else {
            virtual_displays = virtual_displays.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child("远端未提供虚拟显示器控制。"),
            );
        }
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
                .gap_3()
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
                body = body.child(section("连接状态")).child(
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
                    .h(px(56.))
                    .flex_shrink_0()
                    .px_3()
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
                        .w(px(48.))
                        .h(px(48.))
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
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_3()
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

    fn floating_mouse_buttons(&self) -> Div {
        let Some(mouse) = self.model.layout.mouse else {
            return div();
        };
        let mut dock = div().absolute().top_0().left_0().size_full();
        for (rect, icon, label, pressed) in [
            (
                mouse.left,
                Glyph::MouseLeft,
                "左键",
                self.model.left_pressed,
            ),
            (
                mouse.right,
                Glyph::MouseRight,
                "右键",
                self.model.right_pressed,
            ),
            (
                mouse.middle,
                Glyph::MouseMiddle,
                "中键",
                self.model.middle_pressed,
            ),
        ] {
            dock = dock.child(
                at(rect)
                    .rounded_full()
                    .bg(gpui::rgba(if pressed {
                        (ACCENT << 8) | 0xe8
                    } else {
                        (RAISED << 8) | 0xb8
                    }))
                    .border_1()
                    .border_color(rgb(if pressed { ACCENT } else { BORDER }))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_col()
                    .text_color(rgb(if pressed {
                        crate::theme::LIGHT.on_accent
                    } else {
                        TEXT
                    }))
                    .child(glyph(
                        icon,
                        if pressed {
                            crate::theme::LIGHT.on_accent
                        } else {
                            TEXT
                        },
                    ))
                    .child(div().text_size(px(10.)).line_height(px(12.)).child(label)),
            );
        }
        dock
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
        chrome = chrome.child(self.floating_mouse_buttons());
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

fn section(label: &str) -> Div {
    div()
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .child(label.to_owned())
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
    MouseLeft,
    MouseRight,
    MouseMiddle,
}

fn glyph(shape: Glyph, color: u32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let scale =
                (f32::from(bounds.size.width) / 24.).min(f32::from(bounds.size.height) / 24.);
            let mut path = PathBuilder::stroke(px(1.6));
            for stroke in strokes(shape) {
                if let Some((first, rest)) = stroke.split_first() {
                    path.move_to(bounds.origin + point(px(first.0 * scale), px(first.1 * scale)));
                    for (x, y) in rest {
                        path.line_to(bounds.origin + point(px(x * scale), px(y * scale)));
                    }
                }
            }
            match path.build() {
                Ok(path) => window.paint_path(path, rgb(color)),
                Err(_) => window.paint_quad(gpui::fill(bounds.inset(px(8.)), rgb(color))),
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
            &[(3., 6.), (21., 6.), (21., 19.), (3., 19.), (3., 6.)],
            &[(6., 10.), (8., 10.)],
            &[(11., 10.), (13., 10.)],
            &[(16., 10.), (18., 10.)],
            &[(6., 14.), (8., 14.)],
            &[(11., 14.), (13., 14.)],
            &[(16., 14.), (18., 14.)],
            &[(8., 17.), (16., 17.)],
        ],
        Glyph::Display => &[
            &[(3., 4.), (21., 4.), (21., 17.), (3., 17.), (3., 4.)],
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
        Glyph::MouseLeft => &[&[
            (12., 3.),
            (6., 3.),
            (4., 6.),
            (4., 18.),
            (7., 22.),
            (17., 22.),
            (20., 18.),
            (20., 6.),
            (18., 3.),
            (12., 3.),
            (12., 12.),
            (4., 12.),
        ]],
        Glyph::MouseRight => &[&[
            (12., 3.),
            (6., 3.),
            (4., 6.),
            (4., 18.),
            (7., 22.),
            (17., 22.),
            (20., 18.),
            (20., 6.),
            (18., 3.),
            (12., 3.),
            (12., 12.),
            (20., 12.),
        ]],
        Glyph::MouseMiddle => &[
            &[
                (7., 3.),
                (17., 3.),
                (20., 6.),
                (20., 18.),
                (17., 22.),
                (7., 22.),
                (4., 18.),
                (4., 6.),
                (7., 3.),
            ],
            &[(10., 6.), (14., 6.), (14., 12.), (10., 12.), (10., 6.)],
        ],
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
