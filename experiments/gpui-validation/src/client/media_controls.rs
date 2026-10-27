use super::ManagementPanel;
use crate::{dashboard as ui, remote_video::MediaRequest, theme};
use gpui::{Context, div, prelude::*, px, rgb};
use librustdesk::gpui_session::BitrateMode;

impl ManagementPanel {
    pub(super) fn render_media_controls(
        &self,
        palette: theme::Palette,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let state = &self.video_state;
        let controls = &state.media_controls;
        let busy = state.switching_monitor || state.pending_media.is_some();
        let idle = state.connected && !busy;
        let max_bitrate = if controls.max_bitrate_kbps >= 1000
            && controls.max_bitrate_kbps.is_multiple_of(1000)
        {
            format!("{} Mbps", controls.max_bitrate_kbps / 1000)
        } else {
            format!("{} kbps", controls.max_bitrate_kbps)
        };
        let bitrate = match controls.bitrate_mode {
            BitrateMode::Auto => "自动".into(),
            BitrateMode::LimitKbps(kbps) => format!("{kbps} kbps"),
        };
        ui::card(palette)
            .gap_3()
            .child(ui::heading(
                "画质与显示",
                "设置由主机确认后生效；实际性能取决于主机和网络。",
                palette,
            ))
            .when_some(state.pending_media, |card, request| {
                card.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(palette.accent))
                        .child(format!("等待主机确认：{}…", request.label())),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(format!("码率上限 · 当前{bitrate}")),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.muted))
                            .child(if controls.bitrate_supported {
                                format!("主机允许最高 {max_bitrate}；保留自适应：网络拥堵时会继续降低码率。")
                            } else {
                                "主机不支持码率上限调节，请升级被控端。".into()
                            }),
                    )
                    .child(
                        div().flex().flex_wrap().gap_2().children(
                            [
                                None,
                                Some(500),
                                Some(1000),
                                Some(2000),
                                Some(4000),
                                Some(8000),
                                Some(16000),
                                Some(32000),
                                Some(64000),
                                Some(100000),
                                Some(200000),
                                Some(500000),
                                Some(1000000),
                            ]
                            .into_iter()
                            .enumerate()
                            .map(|(index, kbps)| {
                                let mode = kbps
                                    .map(BitrateMode::LimitKbps)
                                    .unwrap_or(BitrateMode::Auto);
                                let selected = controls.bitrate_mode == mode;
                                let enabled = idle
                                    && controls.bitrate_supported
                                    && !selected
                                    && kbps.is_none_or(|value| value <= controls.max_bitrate_kbps);
                                let label = kbps
                                    .map(|value| {
                                        if value < 1000 {
                                            format!("{value} kbps")
                                        } else {
                                            format!("{} Mbps", value / 1000)
                                        }
                                    })
                                    .unwrap_or("自动".into());
                                ui::button(
                                    ("session-bitrate", index),
                                    &label,
                                    selected,
                                    enabled,
                                    palette,
                                )
                                .on_click(cx.listener(
                                    move |view, _, _, cx| {
                                        if enabled {
                                            view.video.update(cx, |video, cx| {
                                                video.request_media(MediaRequest::Bitrate(mode), cx)
                                            });
                                        }
                                    },
                                ))
                            }),
                        ),
                    ),
            )
            .child(div().h(px(1.)).bg(rgb(palette.border)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("分辨率与刷新率"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.muted))
                            .child(
                                controls
                                    .current_resolution
                                    .map(|mode| {
                                        if mode.refresh_rate == 0 {
                                            format!("当前 {}×{}", mode.width, mode.height)
                                        } else {
                                            format!(
                                                "当前 {}×{} @{} Hz",
                                                mode.width, mode.height, mode.refresh_rate
                                            )
                                        }
                                    })
                                    .unwrap_or("等待主机上报当前分辨率…".into()),
                            ),
                    )
                    .when(!controls.can_change_resolution && !busy, |section| {
                        section.child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(palette.muted))
                                .child("主机未授权更改分辨率、没有可用模式，或被控端需要升级。"),
                        )
                    })
                    .child(
                        div()
                            .id("session-resolution-modes")
                            .max_h(px(192.))
                            .overflow_y_scroll()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .children(controls.resolutions.iter().enumerate().map(
                                |(index, &mode)| {
                                    let selected = controls.current_resolution == Some(mode);
                                    let enabled =
                                        idle && controls.can_change_resolution && !selected;
                                    ui::button(
                                        ("session-resolution", index),
                                        &if mode.refresh_rate == 0 {
                                            format!("{}×{}", mode.width, mode.height)
                                        } else {
                                            format!(
                                                "{}×{} @{} Hz",
                                                mode.width, mode.height, mode.refresh_rate
                                            )
                                        },
                                        selected,
                                        enabled,
                                        palette,
                                    )
                                    .on_click(cx.listener(
                                        move |view, _, _, cx| {
                                            if enabled {
                                                view.video.update(cx, |video, cx| {
                                                    video.request_media(
                                                        MediaRequest::Resolution(mode),
                                                        cx,
                                                    )
                                                });
                                            }
                                        },
                                    ))
                                },
                            )),
                    ),
            )
            .child(div().h(px(1.)).bg(rgb(palette.border)))
            .child(
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
                            .child(if busy {
                                "等待当前调节完成后，可继续管理虚拟显示器。"
                            } else if !controls.virtual_display_supported {
                                "主机未启用虚拟显示器支持、权限受限，或被控端需要升级。"
                            } else if !controls.can_add_virtual_display {
                                "已达到主机允许的虚拟显示器数量。可移除不再使用的虚拟屏。"
                            } else {
                                "在主机上添加虚拟屏；创建后可从顶部屏幕列表切换。"
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.muted))
                            .child(if controls.virtual_display_driver == "amyuni_idd" {
                                "首次添加会在被控端启用随包驱动；Amyuni 许可含广告页面条款。"
                            } else if controls.virtual_display_driver == "mtt_vdd" {
                                "使用正式签名驱动；分辨率、刷新率和数量由主机驱动与 Windows 确认。"
                            } else {
                                "首次添加会在被控端启用虚拟显示器驱动。"
                            }),
                    )
                    .child(
                        ui::button(
                            "session-add-virtual",
                            "添加虚拟显示器",
                            false,
                            idle && controls.virtual_display_supported
                                && controls.can_add_virtual_display,
                            palette,
                        )
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.video.update(cx, |video, cx| {
                                video.request_media(MediaRequest::AddVirtual, cx)
                            });
                        })),
                    )
                    .child(div().flex().flex_wrap().gap_2().children(
                        controls.virtual_display_ids.iter().map(|&id| {
                            let enabled = idle && controls.virtual_display_supported;
                            ui::button(
                                ("session-remove-virtual", id as usize),
                                &if controls.virtual_display_remove_latest_only {
                                    "移除最近添加的虚拟屏".into()
                                } else {
                                    format!("移除虚拟屏 {id}")
                                },
                                false,
                                enabled,
                                palette,
                            )
                            .text_color(rgb(palette.danger))
                            .on_click(cx.listener(
                                move |view, _, _, cx| {
                                    if enabled {
                                        view.video.update(cx, |video, cx| {
                                            video.request_media(MediaRequest::RemoveVirtual(id), cx)
                                        });
                                    }
                                },
                            ))
                        }),
                    )),
            )
    }
}
