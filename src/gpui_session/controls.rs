use super::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BitrateMode {
    #[default]
    Auto,
    LimitKbps(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolutionMode {
    pub width: i32,
    pub height: i32,
}

impl ResolutionMode {
    fn from_wire(value: &Resolution) -> Option<Self> {
        (value.width > 0 && value.height > 0 && value.width <= 16384 && value.height <= 16384)
            .then_some(Self {
                width: value.width,
                height: value.height,
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VirtualDisplayRequest {
    Add,
    Remove(u32),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaControls {
    pub bitrate_supported: bool,
    pub bitrate_mode: BitrateMode,
    pub can_change_resolution: bool,
    pub current_resolution: Option<ResolutionMode>,
    pub resolutions: Vec<ResolutionMode>,
    pub virtual_display_supported: bool,
    pub can_add_virtual_display: bool,
    pub virtual_display_remove_latest_only: bool,
    /// Amyuni exposes only the latest count token, never a physical monitor index.
    pub virtual_display_ids: Vec<u32>,
}

#[derive(Default)]
pub(super) struct Controls {
    pub(super) state: MediaControls,
    idd_impl: String,
    installed: bool,
    display_controls_supported: bool,
    windows: bool,
    retina: bool,
    display_scales: Vec<f64>,
}

impl Controls {
    pub(super) fn peer(&mut self, peer: &PeerInfo) {
        self.state.bitrate_supported = peer.features.as_ref().is_some_and(|v| v.bitrate_limit);
        self.display_controls_supported =
            peer.features.as_ref().is_some_and(|v| v.display_controls);
        self.windows = peer.platform == "Windows";
        self.retina = peer.platform == "Mac OS";
        self.displays(&peer.displays, peer.current_display.max(0) as usize);
        self.resolutions(&peer.resolutions);
    }

    pub(super) fn resolutions(&mut self, resolutions: &SupportedResolutions) {
        self.state.resolutions = resolutions
            .resolutions
            .iter()
            .filter_map(ResolutionMode::from_wire)
            .collect();
        self.state.resolutions.sort_by_key(|v| (v.width, v.height));
        self.state.resolutions.dedup();
    }

    pub(super) fn displays(&mut self, displays: &[DisplayInfo], current: usize) {
        self.display_scales = displays
            .iter()
            .map(|display| {
                if self.retina && display.scale.is_finite() && display.scale >= 1.0 {
                    display.scale
                } else {
                    1.0
                }
            })
            .collect();
        if let Some(display) = displays.get(current) {
            self.display(display.width, display.height, current);
        }
    }

    pub(super) fn display(&mut self, width: i32, height: i32, index: usize) {
        let scale = self.display_scales.get(index).copied().unwrap_or(1.0);
        self.state.current_resolution = ResolutionMode::from_wire(&Resolution {
            width: (width as f64 / scale).round() as i32,
            height: (height as f64 / scale).round() as i32,
            ..Default::default()
        });
    }

    pub(super) fn additions(&mut self, text: &str) -> ResultType<()> {
        if text.is_empty() {
            return Ok(());
        }
        let value: serde_json::Value = serde_json::from_str(text)?;
        let object = value
            .as_object()
            .ok_or_else(|| hbb_common::anyhow::anyhow!("Invalid remote display capabilities"))?;
        if let Some(installed) = object.get("is_installed") {
            self.installed = installed.as_bool().ok_or_else(|| {
                hbb_common::anyhow::anyhow!("Invalid remote installation capability")
            })?;
        }
        self.idd_impl.clear();
        if let Some(implementation) = object.get("idd_impl") {
            self.idd_impl = implementation
                .as_str()
                .ok_or_else(|| {
                    hbb_common::anyhow::anyhow!("Invalid remote display implementation")
                })?
                .into();
        }
        self.state.virtual_display_ids.clear();
        let supported = self.windows && self.installed && self.display_controls_supported;
        self.state.virtual_display_remove_latest_only = self.idd_impl == "amyuni_idd";
        match self.idd_impl.as_str() {
            "amyuni_idd" if supported => {
                let count = object
                    .get("amyuni_virtual_displays")
                    .map(|v| {
                        v.as_u64().filter(|n| *n <= 4).ok_or_else(|| {
                            hbb_common::anyhow::anyhow!("Invalid remote virtual display count")
                        })
                    })
                    .transpose()?
                    .unwrap_or(0) as u32;
                if count > 0 {
                    self.state.virtual_display_ids.push(count);
                }
                self.state.virtual_display_supported = true;
                self.state.can_add_virtual_display = count < 4;
            }
            "rustdesk_idd" if supported => {
                if let Some(ids) = object.get("rustdesk_virtual_displays") {
                    let ids = ids.as_array().ok_or_else(|| {
                        hbb_common::anyhow::anyhow!("Invalid remote virtual displays")
                    })?;
                    for id in ids {
                        let id = id
                            .as_u64()
                            .filter(|id| (1..5).contains(id))
                            .ok_or_else(|| {
                                hbb_common::anyhow::anyhow!("Invalid remote virtual display index")
                            })?;
                        self.state.virtual_display_ids.push(id as u32);
                    }
                    self.state.virtual_display_ids.sort_unstable();
                    self.state.virtual_display_ids.dedup();
                }
                self.state.virtual_display_supported = true;
                self.state.can_add_virtual_display = self.state.virtual_display_ids.len() < 4;
            }
            _ => {
                self.state.virtual_display_supported = false;
                self.state.can_add_virtual_display = false;
            }
        }
        Ok(())
    }

    fn next_virtual_id(&self) -> Option<u32> {
        match self.idd_impl.as_str() {
            "amyuni_idd" => self.state.can_add_virtual_display.then_some(0),
            "rustdesk_idd" => (1..5).find(|id| !self.state.virtual_display_ids.contains(id)),
            _ => None,
        }
    }
}

impl Handler {
    pub(super) fn cancel_resolution_request(&self) -> bool {
        let mut monitors = self.0.monitors.write().unwrap();
        if monitors.pending == Some(monitors.current) {
            monitors.pending = None;
            true
        } else {
            false
        }
    }

    pub(super) fn publish_media_controls(&self) {
        self.event(SessionEvent::MediaControls(self.media_controls()));
    }

    pub(super) fn media_controls(&self) -> MediaControls {
        let mailbox = &self.0;
        let connected =
            mailbox.connected.load(Ordering::Acquire) && !mailbox.closed.load(Ordering::Acquire);
        let pending = mailbox.monitors.read().unwrap().pending.is_some();
        let keyboard = mailbox.keyboard.load(Ordering::Acquire);
        let controls = mailbox.controls.read().unwrap();
        let display_controls_supported = controls.display_controls_supported;
        let mut state = controls.state.clone();
        state.bitrate_supported &= connected;
        state.can_change_resolution = display_controls_supported
            && connected
            && keyboard
            && !pending
            && !state.resolutions.is_empty()
            && state.current_resolution.is_some();
        state.virtual_display_supported &= connected && keyboard && !pending;
        state.can_add_virtual_display &= state.virtual_display_supported;
        state
    }
}

impl SessionHandle {
    pub fn media_controls(&self) -> MediaControls {
        self.session.ui_handler.media_controls()
    }

    pub fn set_bitrate_mode(&self, mode: BitrateMode) -> ResultType<()> {
        let value = match mode {
            BitrateMode::Auto => 0,
            BitrateMode::LimitKbps(value) => {
                base::session_protocol::validate_bitrate_limit(value)?;
                value
            }
        };
        if !self.media_controls().bitrate_supported {
            bail!("The remote host does not support bitrate limits");
        }
        let mut option = OptionMessage::new();
        option.bitrate_limit_kbps = Some(value);
        let mut misc = Misc::new();
        misc.set_option(option);
        self.send_media_request(misc)
    }

    pub fn set_resolution(&self, mode: ResolutionMode) -> ResultType<()> {
        let mailbox = &self.session.ui_handler.0;
        let state = self.media_controls();
        if !state.can_change_resolution {
            bail!("Display controls are unavailable or input is disabled");
        }
        if state.current_resolution == Some(mode) {
            let index = mailbox.monitors.read().unwrap().current;
            self.session.refresh_video(index as i32);
            self.session.ui_handler.publish_media_controls();
            return Ok(());
        }
        if !state.resolutions.contains(&mode) {
            bail!("The requested display resolution is unavailable");
        }
        let mut monitors = mailbox.monitors.write().unwrap();
        if monitors.pending.is_some() {
            bail!("Display switch is pending");
        }
        let index = monitors.current;
        let mut misc = Misc::new();
        let resolution = Resolution {
            width: mode.width,
            height: mode.height,
            ..Default::default()
        };
        if crate::common::is_support_multi_ui_session_num(self.session.lc.read().unwrap().version) {
            misc.set_change_display_resolution(DisplayResolution {
                display: index as i32,
                resolution: Some(resolution).into(),
                ..Default::default()
            });
        } else {
            misc.set_change_resolution(resolution);
        }
        self.send_media_request(misc)?;
        monitors.pending = Some(index);
        mailbox.frame.pop();
        drop(monitors);
        self.session.refresh_video(index as i32);
        Ok(())
    }

    pub fn request_virtual_display(&self, request: VirtualDisplayRequest) -> ResultType<()> {
        let state = self.media_controls();
        if !state.virtual_display_supported {
            bail!("Virtual displays are unavailable or input is disabled");
        }
        let (index, on) = match request {
            VirtualDisplayRequest::Add => {
                if !state.can_add_virtual_display {
                    bail!("No additional virtual display is available");
                }
                let index = self
                    .session
                    .ui_handler
                    .0
                    .controls
                    .read()
                    .unwrap()
                    .next_virtual_id()
                    .ok_or_else(|| {
                        hbb_common::anyhow::anyhow!("No virtual display slot is available")
                    })?;
                (index, true)
            }
            VirtualDisplayRequest::Remove(index) => {
                if !state.virtual_display_ids.contains(&index) {
                    bail!("The virtual display is no longer available");
                }
                (index, false)
            }
        };
        let mut misc = Misc::new();
        misc.set_toggle_virtual_display(ToggleVirtualDisplay {
            display: index as i32,
            on,
            ..Default::default()
        });
        self.send_media_request(misc)
    }

    fn send_media_request(&self, misc: Misc) -> ResultType<()> {
        let mailbox = &self.session.ui_handler.0;
        if !mailbox.connected.load(Ordering::Acquire) || mailbox.closed.load(Ordering::Acquire) {
            bail!("Session is not connected");
        }
        let mut message = Message::new();
        message.set_misc(misc);
        self.session
            .sender
            .read()
            .unwrap()
            .as_ref()
            .ok_or_else(|| hbb_common::anyhow::anyhow!("Session is disconnected"))?
            .send(Data::Message(message))
            .map_err(|_| hbb_common::anyhow::anyhow!("Session is disconnected"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_controls_require_capabilities_and_confirm_remote_changes() {
        let handle = SessionHandle {
            session: Session::default(),
        };
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        *handle.session.sender.write().unwrap() = Some(sender);
        let handler = &handle.session.ui_handler;
        assert!(handle
            .set_bitrate_mode(BitrateMode::LimitKbps(4000))
            .is_err());
        let mode = ResolutionMode {
            width: 1280,
            height: 720,
        };
        let mut peer = PeerInfo {
            platform: "Windows".into(),
            features: Some(Features {
                bitrate_limit: true,
                display_controls: true,
                ..Default::default()
            })
            .into(),
            platform_additions:
                r#"{"is_installed":true,"idd_impl":"amyuni_idd","amyuni_virtual_displays":2}"#
                    .into(),
            resolutions: Some(SupportedResolutions {
                resolutions: vec![Resolution {
                    width: mode.width,
                    height: mode.height,
                    ..Default::default()
                }],
                ..Default::default()
            })
            .into(),
            ..Default::default()
        };
        peer.displays.push(DisplayInfo {
            width: 1920,
            height: 1080,
            online: true,
            ..Default::default()
        });
        handler.set_peer_info(&peer);
        assert_eq!(handle.media_controls().virtual_display_ids, vec![2]);
        assert!(handle.media_controls().virtual_display_remove_latest_only);
        assert!(handle.set_bitrate_mode(BitrateMode::LimitKbps(0)).is_err());
        handle
            .set_bitrate_mode(BitrateMode::LimitKbps(4000))
            .unwrap();
        let Data::Message(message) = receiver.try_recv().unwrap() else {
            panic!("expected command")
        };
        assert_eq!(message.misc().option().bitrate_limit_kbps, Some(4000));
        assert_eq!(handle.media_controls().bitrate_mode, BitrateMode::Auto);
        handler.media_options(&OptionMessage {
            bitrate_limit_kbps: Some(4000),
            ..Default::default()
        });
        assert_eq!(
            handle.media_controls().bitrate_mode,
            BitrateMode::LimitKbps(4000)
        );
        handle
            .set_resolution(handle.media_controls().current_resolution.unwrap())
            .unwrap();
        assert!(handle.media_controls().can_change_resolution);
        let Data::Message(refresh) = receiver.try_recv().unwrap() else {
            panic!("expected no-op refresh")
        };
        assert!(refresh.misc().refresh_video());
        assert!(receiver.try_recv().is_err());
        assert!(handle
            .set_resolution(ResolutionMode {
                width: 1,
                height: 1
            })
            .is_err());
        handle.set_resolution(mode).unwrap();
        let Data::Message(resolution) = receiver.try_recv().unwrap() else {
            panic!("expected resolution request")
        };
        assert_eq!(resolution.misc().change_resolution().width, mode.width);
        assert_eq!(resolution.misc().change_resolution().height, mode.height);
        let Data::Message(refresh) = receiver.try_recv().unwrap() else {
            panic!("expected refresh after resolution request")
        };
        assert!(refresh.misc().refresh_video());
        assert!(receiver.try_recv().is_err());
        assert!(!handle.media_controls().can_change_resolution);
        handler.set_permission("keyboard", false);
        assert!(handle
            .request_virtual_display(VirtualDisplayRequest::Remove(2))
            .is_err());
        assert!(handle.set_resolution(mode).is_err());
        // Viewing clients may still constrain their stream bitrate.
        handle.set_bitrate_mode(BitrateMode::Auto).unwrap();
        handler.set_permission("keyboard", true);
        handler.msgbox(
            "media-control-error",
            "Display",
            "OS rejected mode",
            "",
            false,
        );
        assert!(handle.media_controls().can_change_resolution);
        assert!(handle
            .request_virtual_display(VirtualDisplayRequest::Remove(1))
            .is_err());
        handle
            .request_virtual_display(VirtualDisplayRequest::Remove(2))
            .unwrap();
        peer.platform = "Mac OS".into();
        peer.displays[0].width = 3840;
        peer.displays[0].height = 2160;
        peer.displays[0].scale = 2.0;
        handler.set_peer_info(&peer);
        assert_eq!(
            handle.media_controls().current_resolution,
            Some(ResolutionMode {
                width: 1920,
                height: 1080
            })
        );
    }
}
