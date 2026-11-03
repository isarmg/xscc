use super::{ManagementPanel, toolbar_controls::ToolbarMenu};
use crate::{dashboard as ui, remote_video::MediaRequest, theme};
use gpui::{Context, Window, div, prelude::*, px, rgb};

impl ManagementPanel {
    pub(super) fn render_bitrate_dropdown(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let palette = theme::palette(window.appearance());
        let content = self.render_bitrate_editor(palette, cx);
        self.render_toolbar_dropdown(ToolbarMenu::Bitrate, content, window, cx)
    }

    pub(super) fn render_legacy_virtual_display_controls(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let palette = theme::palette(window.appearance());
        let state = &self.video_state;
        let controls = &state.media_controls;
        let busy = state.switching_monitor
            || state.pending_media.is_some()
            || controls.virtual_display_pending;
        let idle = state.connected && !busy;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("虚拟显示器"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.muted))
                    .child(if controls.virtual_display_pending {
                        "正在等待虚拟屏数量确认；未响应时请断开重连。"
                    } else if busy {
                        "等待当前调节完成后，可继续管理虚拟显示器。"
                    } else if controls.virtual_display_count.is_none() {
                        "虚拟屏数量尚未确认，请检查主机虚拟屏支持和控制权限，或断开重连。"
                    } else if !controls.virtual_display_supported {
                        "主机未启用虚拟显示器支持、权限受限，或被控端需要升级。"
                    } else if controls
                        .virtual_display_count
                        .is_some_and(|count| count >= 8)
                    {
                        "控制端最多添加 8 个虚拟屏，可移除不再使用的虚拟屏。"
                    } else if !controls.can_add_virtual_display {
                        "已达到主机允许的虚拟显示器数量。可移除不再使用的虚拟屏。"
                    } else {
                        "创建后可从屏幕列表切换。"
                    }),
            )
            .when(controls.virtual_display_driver == "amyuni_idd", |menu| {
                menu.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.muted))
                        .child("首次添加会在被控端启用随包驱动；Amyuni 许可含广告页面条款。"),
                )
            })
            .child(
                ui::button(
                    "session-add-virtual",
                    "添加虚拟显示器",
                    false,
                    idle && controls.virtual_display_supported && controls.can_add_virtual_display,
                    palette,
                )
                .w_full()
                .on_click(cx.listener(|view, _, _, cx| {
                    let state = &view.video_state;
                    if state.connected
                        && !state.switching_monitor
                        && state.pending_media.is_none()
                        && !state.media_controls.virtual_display_pending
                        && state.media_controls.virtual_display_supported
                        && state.media_controls.can_add_virtual_display
                    {
                        view.video.update(cx, |video, cx| {
                            video.request_media(MediaRequest::AddVirtual, cx)
                        });
                    }
                    cx.stop_propagation();
                })),
            )
            .children(controls.virtual_display_ids.iter().map(|&id| {
                ui::button(
                    ("session-remove-virtual", id as usize),
                    &if controls.virtual_display_remove_latest_only {
                        "移除最近添加的虚拟屏".into()
                    } else {
                        format!("移除虚拟屏 {id}")
                    },
                    false,
                    idle && controls.virtual_display_supported,
                    palette,
                )
                .w_full()
                .text_color(rgb(palette.danger))
                .on_click(cx.listener(move |view, _, _, cx| {
                    let state = &view.video_state;
                    if state.connected
                        && !state.switching_monitor
                        && state.pending_media.is_none()
                        && !state.media_controls.virtual_display_pending
                        && state.media_controls.virtual_display_supported
                        && state.media_controls.virtual_display_ids.contains(&id)
                    {
                        view.video.update(cx, |video, cx| {
                            video.request_media(MediaRequest::RemoveVirtual(id), cx)
                        });
                    }
                    cx.stop_propagation();
                }))
            }))
    }
}
