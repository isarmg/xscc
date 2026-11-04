#[path = "../../../shared/virtual_display_controls.rs"]
pub(super) mod form;

use super::{ManagementPanel, Reply, toolbar_controls::ToolbarMenu};
use crate::theme;
use gpui::{Context, Window, div};
use librustdesk::{
    gpui_management,
    gpui_session::{VirtualDisplayConfiguration, VirtualDisplayState},
};

pub(super) fn project(state: &VirtualDisplayState) -> form::State {
    form::State {
        protocol_version: state.protocol_version,
        status: state.status.clone(),
        message: state.message.clone(),
        configured_count: state.configured_count,
        persistent: state.persistent,
        maximum_count: state.maximum_count,
    }
}

enum LocalRequest {
    Query,
    Configure(VirtualDisplayConfiguration),
}

impl ManagementPanel {
    fn virtual_display_state(&self) -> Option<form::State> {
        if self.host {
            self.local_virtual_display.clone()
        } else {
            self.video_state
                .media_controls
                .virtual_display_configuration
                .as_ref()
                .map(project)
        }
    }

    fn virtual_display_idle(&self) -> bool {
        if self.host {
            self.initialized && !self.local_virtual_pending && !self.action_pending
        } else {
            self.video_state.connected
                && !self.video_state.switching_monitor
                && self.video_state.pending_media.is_none()
                && !self.video_state.virtual_display_pending
        }
    }

    fn request_local_virtual_displays(&mut self, request: LocalRequest) {
        if !self.host || !self.virtual_display_idle() {
            return;
        }
        self.local_virtual_pending = true;
        let sender = self.sender.clone();
        let wake = self.wake.clone();
        self.runtime.spawn(async move {
            let operation = async move {
                match request {
                    LocalRequest::Query => gpui_management::virtual_display_status().await,
                    LocalRequest::Configure(configuration) => {
                        gpui_management::configure_virtual_displays(configuration).await
                    }
                }
            };
            let result = match tokio::time::timeout(std::time::Duration::from_secs(25), operation)
                .await
            {
                Ok(result) => result
                    .map(|state| project(&state))
                    .map_err(|error| error.to_string()),
                Err(_) => Err(
                    "尚未确认 Windows 操作结果，请重新打开菜单查询；操作可能仍在主机执行。".into(),
                ),
            };
            let _ = sender.send(Reply::VirtualDisplay(result));
            wake.notify_one();
        });
    }

    pub(super) fn query_local_virtual_displays(&mut self) {
        self.request_local_virtual_displays(LocalRequest::Query);
    }

    pub(super) fn query_virtual_displays(&mut self, cx: &mut Context<Self>) {
        if !self.virtual_display_idle() {
            return;
        }
        if self.host {
            self.query_local_virtual_displays();
        } else if self
            .virtual_display_state()
            .is_some_and(|state| state.protocol_version >= 1)
        {
            self.video
                .update(cx, |video, cx| video.query_virtual_displays(cx));
        }
    }

    fn step_virtual_displays(&mut self, increase: bool, cx: &mut Context<Self>) {
        if !self.virtual_display_idle() {
            return;
        }
        let Some(configuration) = self
            .virtual_display_state()
            .and_then(|state| state.configuration_step(increase, self.host))
        else {
            return;
        };
        let configuration = VirtualDisplayConfiguration {
            count: configuration.count,
            persistent: configuration.persistent,
            modes: Vec::new(),
        };
        if self.host {
            self.request_local_virtual_displays(LocalRequest::Configure(configuration));
        } else {
            self.video.update(cx, |video, cx| {
                video.configure_virtual_displays(configuration, cx)
            });
        }
        cx.notify();
    }

    pub(super) fn render_virtual_display_dropdown(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        if self.toolbar_menu != Some(ToolbarMenu::VirtualDisplays) {
            return self.render_toolbar_dropdown(ToolbarMenu::VirtualDisplays, div(), window, cx);
        }
        let control_height = if window.is_fullscreen() {
            theme::CONTROL_HEIGHT
        } else {
            28.
        };
        let state = self
            .virtual_display_state()
            .filter(|state| state.protocol_version >= 1);
        let body = if let Some(state) = state {
            let idle = self.virtual_display_idle();
            form::count_controls(
                Some(state.configured_count),
                idle && state.configuration_step(false, self.host).is_some(),
                idle && state.configuration_step(true, self.host).is_some(),
                control_height,
                theme::palette(window.appearance()),
                cx.listener(|view, increase: &bool, _, cx| {
                    view.step_virtual_displays(*increase, cx);
                    cx.stop_propagation();
                }),
            )
        } else if self.host {
            form::count_controls(
                None,
                false,
                false,
                control_height,
                theme::palette(window.appearance()),
                |_, _, _| {},
            )
        } else {
            self.render_legacy_virtual_display_controls(window, cx)
        };
        self.render_toolbar_dropdown(ToolbarMenu::VirtualDisplays, body, window, cx)
    }
}
