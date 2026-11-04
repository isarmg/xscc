use super::{ManagementPanel, toolbar_controls::ToolbarMenu, virtual_display_controls::form};
use crate::{remote_video::MediaRequest, theme};
use gpui::{Context, Window};

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
        let control_height = if window.is_fullscreen() {
            theme::CONTROL_HEIGHT
        } else {
            28.
        };
        let state = &self.video_state;
        let controls = &state.media_controls;
        let idle = state.connected
            && !state.switching_monitor
            && state.pending_media.is_none()
            && !controls.virtual_display_pending
            && controls.virtual_display_supported;
        form::count_controls(
            controls.virtual_display_count,
            idle && !controls.virtual_display_ids.is_empty(),
            idle && controls.can_add_virtual_display,
            control_height,
            theme::palette(window.appearance()),
            cx.listener(|view, increase: &bool, _, cx| {
                let state = &view.video_state;
                let controls = &state.media_controls;
                if state.connected
                    && !state.switching_monitor
                    && state.pending_media.is_none()
                    && !controls.virtual_display_pending
                    && controls.virtual_display_supported
                {
                    let request = if *increase && controls.can_add_virtual_display {
                        Some(MediaRequest::AddVirtual)
                    } else if !*increase {
                        controls
                            .virtual_display_ids
                            .last()
                            .copied()
                            .map(MediaRequest::RemoveVirtual)
                    } else {
                        None
                    };
                    if let Some(request) = request {
                        view.video
                            .update(cx, |video, cx| video.request_media(request, cx));
                    }
                }
                cx.stop_propagation();
            }),
        )
    }
}
