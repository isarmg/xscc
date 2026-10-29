use crate::session_layout::{Panel, Rect, SessionLayout};
use gpui::{
    Context, Div, EventEmitter, IntoElement, PathBuilder, Render, Stateful, Window, canvas, div,
    point, prelude::*, px, rgb,
};

mod bitrate;

// The always-dark session chrome derives from the shared dark palette, so the
// session matches the rest of the application. RAISED and BORDER are two local
// neutral steps above `panel` (keys, outlines) that have no palette slot.
const PAGE: u32 = crate::theme::DARK.page;
const SURFACE: u32 = crate::theme::DARK.panel;
const RAISED: u32 = 0x262b31;
const BORDER: u32 = 0x363c44;
const TEXT: u32 = crate::theme::DARK.text;
const MUTED: u32 = crate::theme::DARK.muted;
const ACCENT: u32 = crate::theme::DARK.accent;

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
    pub target_fps: Option<i32>,
    pub bitrate_supported: bool,
    pub max_bitrate_kbps: u32,
    pub bitrate_limit_kbps: Option<u32>,
    pub can_change_resolution: bool,
    pub current_resolution: Option<(i32, i32, u32)>,
    pub resolutions: Vec<(i32, i32, u32)>,
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
    Fps(i32),
    Bitrate(Option<u32>),
    Resolution(i32, i32, u32),
    AddVirtualDisplay,
    RemoveVirtualDisplay(u32),
    Monitor(usize),
    ArmRightClick,
}

pub struct SessionChrome {
    model: ChromeModel,
    bitrate: bitrate::BitrateEditor,
}

impl EventEmitter<ChromeAction> for SessionChrome {}

impl SessionChrome {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            model: ChromeModel::default(),
            bitrate: bitrate::BitrateEditor::new(cx),
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
                crate::theme::DARK.on_accent
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
                    .on_click(cx.listener(move |_, _, _, cx| {
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
                                crate::theme::DARK.on_accent
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
        let stream_ready = model.connected && model.pending_monitor.is_none();
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
        let mut rates = div().flex().flex_wrap().gap_2();
        for fps in [30, 60, 90, 120] {
            rates = rates.child(
                self.button(
                    cx,
                    format!("session-fps-{fps}"),
                    &fps.to_string(),
                    model.target_fps == Some(fps),
                    stream_ready,
                    ChromeAction::Fps(fps),
                )
                .flex_1(),
            );
        }
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
                    .text_color(rgb(crate::theme::DARK.danger))
                    .child(error.clone()),
            );
        }
        body.child(section("目标帧率"))
            .child(rates)
            .child(section("屏幕"))
            .child(self.screen_list(cx))
    }

    fn media_settings(&self, cx: &mut Context<Self>) -> Div {
        let model = &self.model;
        let ready = model.connected && model.pending_monitor.is_none();
        let mut body = div().flex().flex_col().gap_3().child(section("码率上限"));
        body = body.child(self.bitrate_controls(cx));
        body = body.child(section("分辨率与刷新率"));
        if let Some((width, height, refresh_rate)) = model.current_resolution {
            body = body.child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(
                if refresh_rate == 0 {
                    format!("当前 {width}×{height}")
                } else {
                    format!("当前 {width}×{height} @{refresh_rate} Hz")
                },
            ));
        }
        if model.resolutions.is_empty() {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child("远端未提供可选分辨率。"),
            );
        } else {
            let mut modes = div().flex().flex_wrap().gap_2();
            for &(width, height, refresh_rate) in &model.resolutions {
                modes = modes.child(
                    self.button(
                        cx,
                        format!("session-resolution-{width}-{height}-{refresh_rate}"),
                        &if refresh_rate == 0 {
                            format!("{width}×{height}")
                        } else {
                            format!("{width}×{height} @{refresh_rate} Hz")
                        },
                        model.current_resolution == Some((width, height, refresh_rate)),
                        ready && model.can_change_resolution,
                        ChromeAction::Resolution(width, height, refresh_rate),
                    )
                    .flex_1(),
                );
            }
            body = body.child(modes);
        }
        body = body.child(section("虚拟显示器"));
        if model.virtual_display_supported {
            body = body
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
                body = body.child(
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
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child("远端未提供虚拟显示器控制。"),
            );
        }
        body
    }

    fn panel(&self, area: Rect, cx: &mut Context<Self>) -> Stateful<Div> {
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
                .child(section("远程屏幕"))
                .child(self.screen_list(cx))
                .child(self.media_settings(cx));
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
                        crate::theme::DARK.on_accent
                    } else {
                        TEXT
                    }))
                    .child(glyph(
                        icon,
                        if pressed {
                            crate::theme::DARK.on_accent
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut chrome = div()
            .relative()
            .size_full()
            .text_color(rgb(TEXT))
            .on_key_down(cx.listener(Self::bitrate_key_down))
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
            chrome = chrome.child(self.panel(panel, cx));
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
