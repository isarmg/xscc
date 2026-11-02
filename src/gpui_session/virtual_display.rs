use super::*;
use base::session_protocol::virtual_display::MAX_JSON_BYTES;
pub use base::session_protocol::virtual_display::{
    VirtualDisplayConfiguration, VirtualDisplayMode, VirtualDisplayResult, VirtualDisplayScreen,
    VirtualDisplayState,
};

#[derive(Default)]
pub(super) struct Requests {
    next: u64,
    pending: Option<u64>,
    pub(super) runtime: Option<tokio::runtime::Handle>,
}

impl SessionHandle {
    pub fn query_virtual_displays(&self) -> ResultType<u64> {
        self.virtual_display_request(None)
    }

    pub fn configure_virtual_displays(
        &self,
        config: VirtualDisplayConfiguration,
    ) -> ResultType<u64> {
        config.validate()?;
        if !self.session.ui_handler.0.keyboard.load(Ordering::Acquire) {
            bail!("Virtual displays require current input permission");
        }
        self.virtual_display_request(Some(config))
    }

    fn virtual_display_request(
        &self,
        config: Option<VirtualDisplayConfiguration>,
    ) -> ResultType<u64> {
        let mailbox = &self.session.ui_handler.0;
        if !self
            .media_controls()
            .virtual_display_configuration
            .as_ref()
            .is_some_and(|state| state.protocol_version == 1)
        {
            bail!("Upgrade the host to configure virtual displays");
        }
        let (id, runtime) = {
            let mut requests = mailbox.virtual_requests.lock().unwrap();
            if requests.pending.is_some() {
                bail!("A virtual display request is still pending");
            }
            let runtime = requests
                .runtime
                .clone()
                .ok_or_else(|| hbb_common::anyhow::anyhow!("Session runtime is unavailable"))?;
            requests.next = requests.next.checked_add(1).ok_or_else(|| {
                hbb_common::anyhow::anyhow!("Virtual display request identifier exhausted")
            })?;
            requests.pending = Some(requests.next);
            (requests.next, runtime)
        };
        let mut misc = Misc::new();
        misc.set_virtual_display_config(base::message_proto::VirtualDisplayConfig {
            request_id: id,
            configuration_json: config
                .map(|value| serde_json::to_string(&value))
                .transpose()?
                .unwrap_or_default(),
            ..Default::default()
        });
        if let Err(error) = self.send_media_request(misc) {
            mailbox.virtual_requests.lock().unwrap().pending = None;
            return Err(error);
        }
        let weak = Arc::downgrade(mailbox);
        runtime.spawn(async move {
            tokio::time::sleep(Duration::from_secs(25)).await;
            let Some(mailbox) = weak.upgrade() else {
                return;
            };
            let timed_out = {
                let mut requests = mailbox.virtual_requests.lock().unwrap();
                if requests.pending == Some(id) {
                    requests.pending = None;
                    true
                } else {
                    false
                }
            };
            if timed_out && !mailbox.closed.load(Ordering::Acquire) {
                let state = mailbox
                    .controls
                    .read()
                    .unwrap()
                    .state
                    .virtual_display_configuration
                    .clone()
                    .unwrap_or_default();
                Handler(mailbox).event(SessionEvent::VirtualDisplayResult(VirtualDisplayResult {
                    request_id: id,
                    state,
                    error: Some("尚未确认，请查询状态；Windows 操作可能仍在执行。".into()),
                }));
            }
        });
        Ok(id)
    }
}

impl Handler {
    pub(super) fn virtual_display_reply(&self, reply: &base::message_proto::VirtualDisplayResult) {
        let result = (|| -> ResultType<VirtualDisplayResult> {
            if reply.request_id == 0 || reply.result_json.len() > MAX_JSON_BYTES {
                bail!("Invalid virtual display response size or identifier");
            }
            let result: VirtualDisplayResult = serde_json::from_str(&reply.result_json)?;
            if result.request_id != reply.request_id
                || result.state.protocol_version != 1
                || result.state.displays.len() > 4096
                || result.state.modes.len() > 4096
            {
                bail!("Invalid virtual display response identity or inventory");
            }
            for mode in &result.state.modes {
                mode.validate()?;
            }
            Ok(result)
        })();
        match result {
            Ok(result) => {
                {
                    let mut requests = self.0.virtual_requests.lock().unwrap();
                    if requests.pending != Some(result.request_id) {
                        return;
                    }
                    requests.pending = None;
                }
                self.0
                    .controls
                    .write()
                    .unwrap()
                    .state
                    .virtual_display_configuration = Some(result.state.clone());
                self.publish_media_controls();
                self.event(SessionEvent::VirtualDisplayResult(result));
            }
            Err(error) => self.event(SessionEvent::ControlError(format!(
                "Invalid virtual display response: {error}"
            ))),
        }
    }
}
