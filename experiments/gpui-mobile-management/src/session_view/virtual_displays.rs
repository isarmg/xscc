use super::SessionView;
use crate::session_chrome::virtual_displays::form;
use gpui::{Context, Window};
use librustdesk::gpui_session::{
    VirtualDisplayConfiguration, VirtualDisplayMode, VirtualDisplayResult, VirtualDisplayState,
};

pub(super) struct PendingVirtual {
    request_id: u64,
    pub(super) configure: bool,
    invalidate_current: bool,
    original_ids: Vec<String>,
    current_name: Option<String>,
}

pub(super) fn project(state: &VirtualDisplayState) -> form::State {
    form::State {
        protocol_version: state.protocol_version,
        driver: state.driver.clone(),
        status: state.status.clone(),
        message: state.message.clone(),
        configured_count: state.configured_count,
        present_count: state.present_count,
        active_count: state.active_count,
        persistent: state.persistent,
        maximum_count: state.maximum_count,
        displays: state
            .displays
            .iter()
            .map(|screen| form::Screen {
                id: screen.id.clone(),
                name: screen.name.clone(),
                index: screen.index,
                active: screen.active,
                mode: form::Mode {
                    width: screen.width,
                    height: screen.height,
                    refresh_rate_units: screen.refresh_rate_units,
                },
            })
            .collect(),
        modes: state
            .modes
            .iter()
            .map(|mode| form::Mode {
                width: mode.width,
                height: mode.height,
                refresh_rate_units: mode.refresh_rate_units,
            })
            .collect(),
    }
}

impl SessionView {
    pub(super) fn query_virtual_displays(&mut self) {
        self.request_virtual_configuration(None);
    }

    pub(super) fn configure_virtual_displays(
        &mut self,
        configuration: form::Configuration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.view_only {
            return;
        }
        self.release_input();
        self.hide_keyboard(window, cx);
        self.request_virtual_configuration(Some(VirtualDisplayConfiguration {
            count: configuration.count,
            persistent: configuration.persistent,
            modes: configuration
                .mode
                .into_iter()
                .map(|mode| VirtualDisplayMode {
                    width: mode.width,
                    height: mode.height,
                    refresh_rate_units: mode.refresh_rate_units,
                })
                .collect(),
        }));
    }

    fn request_virtual_configuration(
        &mut self,
        configuration: Option<VirtualDisplayConfiguration>,
    ) {
        if !self.connected
            || !self.authorized()
            || self.pending_monitor.is_some()
            || self.pending_virtual.is_some()
        {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let configure = configuration.is_some();
        let controls = session.media_controls();
        let invalidate_current = configuration
            .as_ref()
            .and_then(|configuration| configuration.modes.first())
            .is_some_and(|mode| {
                controls
                    .virtual_display_configuration
                    .as_ref()
                    .and_then(|state| {
                        state
                            .displays
                            .iter()
                            .find(|screen| screen.index == self.current_monitor)
                    })
                    .is_some_and(|screen| {
                        (screen.width, screen.height, screen.refresh_rate_units)
                            != (mode.width, mode.height, mode.refresh_rate_units)
                    })
            });
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
                    original_ids: controls
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
                    current_name: self.current_monitor.and_then(|index| {
                        self.monitors
                            .iter()
                            .find(|monitor| monitor.index == index)
                            .map(|monitor| monitor.name.clone())
                    }),
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
    }

    pub(super) fn finish_virtual_display_result(
        &mut self,
        result: VirtualDisplayResult,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        self.notice = if let Some(error) = result.error {
            error
        } else if applied && pending.configure {
            "虚拟显示配置已由主机实际确认。".into()
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
            self.current_monitor = None;
            self.extent = (0, 0);
            self.presentation.invalidate();
            self.notice = "配置已应用，当前没有可捕获的屏幕。".into();
            return;
        };
        if self.current_monitor == Some(index)
            && pending.current_name.as_ref().is_some_and(|name| {
                self.monitors
                    .iter()
                    .any(|monitor| monitor.index == index && &monitor.name == name)
            })
        {
            if pending.invalidate_current {
                self.extent = (0, 0);
                self.presentation.invalidate();
            }
            return;
        }
        self.current_monitor = None;
        self.switch_monitor(index, window, cx);
    }
}
