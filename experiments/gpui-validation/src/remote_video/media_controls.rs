use super::RemoteVideo;
use gpui::Context;
use librustdesk::gpui_session::{
    BitrateMode, MediaControls, ResolutionMode, TargetFps, VirtualDisplayConfiguration,
    VirtualDisplayRequest, VirtualDisplayResult,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaRequest {
    Bitrate(BitrateMode),
    Resolution(ResolutionMode),
    AddVirtual,
    RemoveVirtual(u32),
}

impl MediaRequest {
    pub fn label(self) -> String {
        match self {
            Self::Bitrate(BitrateMode::Auto) => "自动码率".into(),
            Self::Bitrate(BitrateMode::LimitKbps(kbps)) => format!("码率上限 {kbps} kbps"),
            Self::Resolution(mode) => {
                if mode.refresh_rate == 0 {
                    format!("分辨率 {}×{}", mode.width, mode.height)
                } else {
                    format!(
                        "分辨率 {}×{} @{} Hz",
                        mode.width, mode.height, mode.refresh_rate
                    )
                }
            }
            Self::AddVirtual => "添加虚拟显示器".into(),
            Self::RemoveVirtual(_) => "移除虚拟显示器".into(),
        }
    }

    fn available(self, controls: &MediaControls) -> bool {
        match self {
            Self::Bitrate(_) => controls.bitrate_supported,
            Self::Resolution(mode) => {
                controls.can_change_resolution && controls.resolutions.contains(&mode)
            }
            Self::AddVirtual => {
                controls.virtual_display_supported && controls.can_add_virtual_display
            }
            Self::RemoveVirtual(id) => {
                controls.virtual_display_supported && controls.virtual_display_ids.contains(&id)
            }
        }
    }
}

pub(super) struct PendingMedia {
    pub(super) request: MediaRequest,
    original_virtual_ids: Vec<u32>,
}

impl PendingMedia {
    fn new(request: MediaRequest, controls: &MediaControls) -> Self {
        Self {
            request,
            original_virtual_ids: if matches!(request, MediaRequest::AddVirtual) {
                controls.virtual_display_ids.clone()
            } else {
                Vec::new()
            },
        }
    }

    fn confirmed(&self, controls: &MediaControls) -> bool {
        match self.request {
            MediaRequest::Bitrate(mode) => controls.bitrate_mode == mode,
            MediaRequest::Resolution(mode) => controls.current_resolution == Some(mode),
            MediaRequest::AddVirtual => controls
                .virtual_display_ids
                .iter()
                .any(|id| !self.original_virtual_ids.contains(id)),
            MediaRequest::RemoveVirtual(id) => !controls.virtual_display_ids.contains(&id),
        }
    }
}

impl RemoteVideo {
    pub fn request_media(&mut self, request: MediaRequest, cx: &mut Context<Self>) {
        if !self.connected
            || self.switching_monitor
            || self.pending_media.is_some()
            || self.pending_virtual.is_some()
        {
            return;
        }
        if !request.available(&self.media_controls) {
            self.notice = "主机当前能力或权限不允许此操作。".into();
        } else {
            if !matches!(request, MediaRequest::Bitrate(_)) {
                self.release_input();
            }
            let Some(session) = &self.session else {
                return;
            };
            let result = match request {
                MediaRequest::Bitrate(mode) => session.set_bitrate_mode(mode),
                MediaRequest::Resolution(mode) => session.set_resolution(mode),
                MediaRequest::AddVirtual => {
                    session.request_virtual_display(VirtualDisplayRequest::Add)
                }
                MediaRequest::RemoveVirtual(id) => {
                    session.request_virtual_display(VirtualDisplayRequest::Remove(id))
                }
            };
            match result {
                Ok(()) => {
                    if matches!(request, MediaRequest::Resolution(_)) {
                        self.extent = (0, 0);
                        self.video.invalidate();
                    }
                    self.pending_media = Some(PendingMedia::new(request, &self.media_controls));
                    self.notice = format!("正在请求{}，等待主机确认…", request.label());
                }
                Err(error) => self.notice = format!("调节未完成：{error}"),
            }
        }
        self.publish(cx);
        cx.notify();
    }

    pub(super) fn update_media_controls(&mut self, controls: MediaControls) {
        if let Some(pending) = &self.pending_media
            && pending.confirmed(&controls)
        {
            self.notice = format!("主机已确认{}。", pending.request.label());
            self.pending_media = None;
        }
        self.media_controls = controls;
        if let Some(target) = self.media_controls.current_resolution.and_then(|mode| {
            TargetFps::new(
                mode.refresh_rate
                    .min(base::session_protocol::MAX_SESSION_FPS),
            )
        }) && target != self.target_fps
            && self.connected
            && let Some(session) = &self.session
        {
            match session.set_target_fps(target) {
                Ok(()) => self.target_fps = target,
                Err(error) => self.notice = error.to_string(),
            }
        }
    }
}

pub(super) struct PendingVirtual {
    request_id: u64,
    pub(super) configure: bool,
    invalidate_current: bool,
    original_ids: Vec<String>,
    current_name: Option<String>,
}

impl RemoteVideo {
    pub fn query_virtual_displays(&mut self, cx: &mut Context<Self>) {
        self.request_virtual_configuration(None, cx);
    }

    pub fn configure_virtual_displays(
        &mut self,
        configuration: VirtualDisplayConfiguration,
        cx: &mut Context<Self>,
    ) {
        self.request_virtual_configuration(Some(configuration), cx);
    }

    fn request_virtual_configuration(
        &mut self,
        configuration: Option<VirtualDisplayConfiguration>,
        cx: &mut Context<Self>,
    ) {
        if !self.connected
            || self.switching_monitor
            || self.pending_media.is_some()
            || self.pending_virtual.is_some()
        {
            return;
        }
        let configure = configuration.is_some();
        let invalidate_current = configuration
            .as_ref()
            .and_then(|configuration| configuration.modes.first())
            .is_some_and(|mode| {
                self.media_controls
                    .virtual_display_configuration
                    .as_ref()
                    .and_then(|state| {
                        state
                            .displays
                            .iter()
                            .find(|screen| screen.index == Some(self.current_monitor))
                    })
                    .is_some_and(|screen| {
                        (screen.width, screen.height, screen.refresh_rate_units)
                            != (mode.width, mode.height, mode.refresh_rate_units)
                    })
            });
        if configure {
            self.release_input();
        }
        let Some(session) = &self.session else {
            return;
        };
        let result = match configuration {
            Some(configuration) => session.configure_virtual_displays(configuration),
            None => session.query_virtual_displays(),
        };
        match result {
            Ok(request_id) => {
                self.pending_virtual = Some(PendingVirtual {
                    request_id,
                    configure,
                    invalidate_current,
                    original_ids: self
                        .media_controls
                        .virtual_display_configuration
                        .as_ref()
                        .map(|state| {
                            state
                                .displays
                                .iter()
                                .filter(|screen| screen.active)
                                .map(|screen| screen.id.clone())
                                .collect()
                        })
                        .unwrap_or_default(),
                    current_name: self
                        .monitors
                        .iter()
                        .find(|monitor| monitor.index == self.current_monitor)
                        .map(|monitor| monitor.name.clone()),
                });
                self.notice = if configure {
                    "正在应用虚拟显示配置，等待主机实际确认…"
                } else {
                    "正在查询虚拟显示器…"
                }
                .into();
            }
            Err(error) => self.notice = error.to_string(),
        }
        self.publish(cx);
        cx.notify();
    }

    pub(super) fn finish_virtual_display_result(&mut self, result: VirtualDisplayResult) {
        if !self
            .pending_virtual
            .as_ref()
            .is_some_and(|pending| pending.request_id == result.request_id)
        {
            return;
        }
        let Some(pending) = self.pending_virtual.take() else {
            return;
        };
        let failed = result.error.is_some();
        let applied = result.state.status == "ready"
            || (result.state.status == "disabled" && result.state.configured_count == 0);
        self.media_controls.virtual_display_configuration = Some(result.state.clone());
        self.notice = if let Some(error) = result.error {
            error
        } else if applied {
            if pending.configure {
                "虚拟显示配置已由主机实际确认。".into()
            } else {
                result.state.message.clone()
            }
        } else {
            result.state.message.clone()
        };
        if !pending.configure || failed || !applied {
            return;
        }
        let target = result
            .state
            .displays
            .iter()
            .find(|screen| screen.active && !pending.original_ids.contains(&screen.id))
            .and_then(|screen| screen.index)
            .filter(|index| {
                self.monitors
                    .iter()
                    .any(|monitor| monitor.index == *index && monitor.available)
            })
            .or_else(|| {
                pending.current_name.as_ref().and_then(|name| {
                    self.monitors
                        .iter()
                        .find(|monitor| monitor.available && &monitor.name == name)
                        .map(|monitor| monitor.index)
                })
            })
            .or_else(|| {
                result
                    .state
                    .displays
                    .iter()
                    .filter(|screen| screen.active)
                    .filter_map(|screen| screen.index)
                    .find(|index| {
                        self.monitors
                            .iter()
                            .any(|monitor| monitor.index == *index && monitor.available)
                    })
            })
            .or_else(|| {
                self.monitors
                    .iter()
                    .find(|monitor| monitor.available)
                    .map(|monitor| monitor.index)
            });
        let Some(index) = target.filter(|index| {
            self.monitors
                .iter()
                .any(|monitor| monitor.index == *index && monitor.available)
        }) else {
            self.release_input();
            self.extent = (0, 0);
            self.video.invalidate();
            self.notice = "配置已应用，当前没有可捕获的屏幕。".into();
            return;
        };
        if index == self.current_monitor
            && pending.current_name.as_ref().is_some_and(|name| {
                self.monitors
                    .iter()
                    .any(|monitor| monitor.index == index && &monitor.name == name)
            })
        {
            if pending.invalidate_current {
                self.extent = (0, 0);
                self.video.invalidate();
            }
            return;
        }
        self.release_input();
        if let Some(session) = &self.session {
            match session.switch_monitor(index) {
                Ok(()) => {
                    self.switching_monitor = true;
                    self.quality = Default::default();
                    self.extent = (0, 0);
                    self.video.invalidate();
                }
                Err(error) => self.notice = format!("配置已应用，屏幕切换未完成：{error}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MediaRequest, PendingMedia};
    use librustdesk::gpui_session::{BitrateMode, MediaControls, ResolutionMode};

    #[test]
    fn media_requests_wait_for_actual_settings_and_virtual_display_changes() {
        let mut controls = MediaControls {
            bitrate_supported: true,
            virtual_display_supported: true,
            can_add_virtual_display: true,
            virtual_display_ids: vec![1],
            ..Default::default()
        };
        let add = PendingMedia::new(MediaRequest::AddVirtual, &controls);
        assert!(!add.confirmed(&controls));
        controls.virtual_display_ids.push(2);
        assert!(add.confirmed(&controls));
        let bitrate = PendingMedia::new(
            MediaRequest::Bitrate(BitrateMode::LimitKbps(8000)),
            &controls,
        );
        assert!(!bitrate.confirmed(&controls));
        controls.bitrate_mode = BitrateMode::LimitKbps(8000);
        assert!(bitrate.confirmed(&controls));
        let mode = ResolutionMode {
            width: 1920,
            height: 1080,
            refresh_rate: 0,
        };
        let resolution = PendingMedia::new(MediaRequest::Resolution(mode), &controls);
        assert!(!resolution.confirmed(&controls));
        controls.current_resolution = Some(mode);
        assert!(resolution.confirmed(&controls));
        let high_refresh = ResolutionMode {
            refresh_rate: 120,
            ..mode
        };
        let resolution = PendingMedia::new(MediaRequest::Resolution(high_refresh), &controls);
        assert!(!resolution.confirmed(&controls));
        controls.current_resolution = Some(ResolutionMode {
            refresh_rate: 60,
            ..mode
        });
        assert!(!resolution.confirmed(&controls));
        controls.current_resolution = Some(high_refresh);
        assert!(resolution.confirmed(&controls));
        let remove = PendingMedia::new(MediaRequest::RemoveVirtual(2), &controls);
        assert!(!remove.confirmed(&controls));
        controls.virtual_display_ids.retain(|id| *id != 2);
        assert!(remove.confirmed(&controls));
    }
}
