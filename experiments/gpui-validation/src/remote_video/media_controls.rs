use super::RemoteVideo;
use gpui::Context;
use librustdesk::gpui_session::{
    BitrateMode, MediaControls, ResolutionMode, VirtualDisplayRequest,
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
            Self::Resolution(mode) => format!("分辨率 {}×{}", mode.width, mode.height),
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
        if !self.connected || self.switching_monitor || self.pending_media.is_some() {
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
        };
        let resolution = PendingMedia::new(MediaRequest::Resolution(mode), &controls);
        assert!(!resolution.confirmed(&controls));
        controls.current_resolution = Some(mode);
        assert!(resolution.confirmed(&controls));
        let remove = PendingMedia::new(MediaRequest::RemoveVirtual(2), &controls);
        assert!(!remove.confirmed(&controls));
        controls.virtual_display_ids.retain(|id| *id != 2);
        assert!(remove.confirmed(&controls));
    }
}
