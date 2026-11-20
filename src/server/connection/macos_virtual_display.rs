use super::*;

pub(super) fn remote_is_connected(connection_id: i32) -> bool {
    AUTHED_CONNS.lock().unwrap().iter().any(|connection| {
        connection.conn_id == connection_id && connection.conn_type == AuthConnType::Remote
    })
}

impl Connection {
    pub(super) fn synchronize_macos_display_topology(
        &mut self,
        displays: &Vec<DisplayInfo>,
    ) -> Option<Message> {
        let previous = self
            .retina
            .displays
            .get(self.display_idx)
            .map(|display| display.name.clone());
        self.retina.set_displays(displays);
        if !crate::platform::macos_virtual_display::available()
            || previous.as_ref().is_some_and(|name| {
                displays
                    .get(self.display_idx)
                    .is_some_and(|display| usable(display) && &display.name == name)
            })
        {
            return None;
        }

        let preferred = previous
            .as_deref()
            .and_then(|name| find_display(displays, name))
            .or_else(|| {
                let primary = scrap::Display::primary().ok()?.name();
                find_display(displays, &primary)
            })
            .or_else(|| displays.iter().position(usable))?;
        let server = self.server.upgrade()?;
        if !self.switch_display_to(preferred, server.clone()) {
            return None;
        }
        if !self.multi_ui_session {
            server.write().unwrap().capture_displays(
                self.inner.clone(),
                self.video_source(),
                &[preferred],
                true,
                true,
            );
        }
        self.refresh_video_display(Some(preferred));
        video_service::make_display_changed_msg(
            preferred,
            displays.get(preferred).cloned(),
            self.video_source(),
        )
    }
}

fn usable(display: &DisplayInfo) -> bool {
    display.online && display.width > 0 && display.height > 0
}

fn find_display(displays: &[DisplayInfo], name: &str) -> Option<usize> {
    displays
        .iter()
        .position(|display| display.name == name && usable(display))
}
