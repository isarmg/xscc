use super::*;

impl Connection {
    fn media_authority_valid(&self, input: bool) -> bool {
        if !self.authorized
            || self.authed_conn_type() != Some(AuthConnType::Remote)
            || input && !self.peer_keyboard_enabled()
        {
            return false;
        }
        #[cfg(feature = "managed")]
        if !self.session_authority_valid() {
            return false;
        }
        true
    }

    async fn media_control_error(&mut self, title: &str, text: String) {
        let mut message = Message::new();
        message.set_message_box(MessageBox {
            msgtype: "media-control-error".into(),
            title: title.into(),
            text,
            ..Default::default()
        });
        self.send(message).await;
    }

    pub(super) async fn update_bitrate_limit(&mut self, value: u32) {
        if !self.media_authority_valid(false) {
            self.media_control_error(
                "Video bitrate",
                "The session is no longer authorized".into(),
            )
            .await;
            return;
        }
        if value != 0 {
            if let Err(error) = base::session_protocol::validate_bitrate_limit(value) {
                self.media_control_error("Video bitrate", error.to_string())
                    .await;
                return;
            }
        }
        video_service::VIDEO_QOS
            .lock()
            .unwrap()
            .user_bitrate_limit(self.inner.id(), (value != 0).then_some(value));
        let mut misc = Misc::new();
        misc.set_option(OptionMessage {
            bitrate_limit_kbps: Some(value),
            ..Default::default()
        });
        let mut response = Message::new();
        response.set_misc(misc);
        self.send(response).await;
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub(super) async fn request_resolution(&mut self, display: Option<usize>, mode: Resolution) {
        if !self.media_authority_valid(true) {
            self.media_control_error(
                "Display resolution",
                "Display controls require current input permission".into(),
            )
            .await;
            return;
        }
        let index = display.unwrap_or(self.display_idx);
        #[cfg(feature = "managed")]
        let permit = self.managed_permit.clone();
        #[cfg(all(feature = "managed", feature = "test-lan-direct"))]
        let test_lan = self.test_lan;
        let result = tokio::task::spawn_blocking(move || -> ResultType<()> {
            #[cfg(feature = "managed")]
            {
                #[cfg(feature = "test-lan-direct")]
                let valid = test_lan || crate::management::permits_host(permit.as_ref());
                #[cfg(not(feature = "test-lan-direct"))]
                let valid = crate::management::permits_host(permit.as_ref());
                if !valid {
                    bail!("Session authorization expired");
                }
            }
            let displays = display_service::try_get_displays()?;
            let display = displays
                .get(index)
                .ok_or_else(|| hbb_common::anyhow::anyhow!("The display is no longer available"))?;
            let name = display.name();
            if !crate::platform::resolutions(&name)
                .iter()
                .any(|value| value.width == mode.width && value.height == mode.height)
            {
                bail!("The requested resolution is not supported by this display");
            }
            #[cfg(windows)]
            if let Some(applied) =
                virtual_display_manager::rustdesk_idd::change_resolution_if_is_virtual_display(
                    &name,
                    mode.width as _,
                    mode.height as _,
                )
            {
                if !applied {
                    bail!("Virtual display rejected the requested resolution");
                }
                return Ok(());
            }
            #[cfg(target_os = "macos")]
            let scale = display.scale();
            #[cfg(not(target_os = "macos"))]
            let scale = 1.0;
            let original = (
                ((display.width() as f64) / scale).round() as i32,
                ((display.height() as f64) / scale).round() as i32,
            );
            crate::platform::change_resolution(&name, mode.width as _, mode.height as _)?;
            #[cfg(windows)]
            let record = !virtual_display_manager::amyuni_idd::is_my_display(&name);
            #[cfg(not(windows))]
            let record = true;
            if record {
                display_service::set_last_changed_resolution(
                    &name,
                    original,
                    (mode.width, mode.height),
                );
            }
            Ok(())
        })
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.media_control_error("Display resolution", error.to_string())
                    .await
            }
            Err(error) => {
                self.media_control_error(
                    "Display resolution",
                    format!("Display change task failed: {error}"),
                )
                .await
            }
        }
    }

    #[cfg(windows)]
    pub(super) async fn control_virtual_display(&mut self, request: ToggleVirtualDisplay) {
        if !self.media_authority_valid(true) {
            self.media_control_error(
                "Virtual display",
                "Virtual displays require current input permission".into(),
            )
            .await;
            return;
        }
        #[cfg(feature = "managed")]
        let permit = self.managed_permit.clone();
        #[cfg(all(feature = "managed", feature = "test-lan-direct"))]
        let test_lan = self.test_lan;
        let result = tokio::task::spawn_blocking(move || -> ResultType<()> {
            #[cfg(feature = "managed")]
            {
                #[cfg(feature = "test-lan-direct")]
                let valid = test_lan || crate::management::permits_host(permit.as_ref());
                #[cfg(not(feature = "test-lan-direct"))]
                let valid = crate::management::permits_host(permit.as_ref());
                if !valid {
                    bail!("Session authorization expired");
                }
            }
            if !crate::platform::is_installed()
                || !virtual_display_manager::is_virtual_display_supported()
            {
                bail!("Virtual displays require an installed supported Windows host");
            }
            if request.on {
                if virtual_display_manager::is_amyuni_idd() {
                    if request.display != 0 {
                        bail!("Invalid virtual display request");
                    }
                } else if !(1..5).contains(&request.display) {
                    bail!("Invalid virtual display slot");
                }
                virtual_display_manager::plug_in_monitor(request.display as _, Vec::new())?;
            } else {
                if virtual_display_manager::is_amyuni_idd() {
                    let count = virtual_display_manager::amyuni_idd::get_monitor_count();
                    if count == 0 || request.display as usize != count {
                        bail!("Only the most recently added virtual display can be removed");
                    }
                } else if request.display < 1
                    || !virtual_display_manager::rustdesk_idd::get_virtual_displays()
                        .contains(&(request.display as u32))
                {
                    bail!("The virtual display is no longer available");
                }
                virtual_display_manager::plug_out_monitor(request.display, false, true)?;
            }
            Ok(())
        })
        .await;
        match result {
            Ok(Ok(())) => {
                let mut response = Message::new();
                response.set_peer_info(PeerInfo {
                    displays: display_service::get_sync_displays(),
                    platform_additions: serde_json::Value::Object(
                        virtual_display_manager::get_platform_additions(),
                    )
                    .to_string(),
                    ..Default::default()
                });
                self.send(response).await;
            }
            Ok(Err(error)) => {
                self.media_control_error("Virtual display", error.to_string())
                    .await
            }
            Err(error) => {
                self.media_control_error(
                    "Virtual display",
                    format!("Virtual display task failed: {error}"),
                )
                .await
            }
        }
    }
}
