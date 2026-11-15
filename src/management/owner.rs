//! Typed local owner operations. Hardware commands never enter the manager client.
use base::session_protocol::virtual_display::VirtualDisplayConfiguration;
use serde::{Deserialize, Serialize};
use xscc_native_management::manager::Request;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "scope",
    content = "request",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum OwnerRequest {
    Management(Request),
    Display(DisplayRequest),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum DisplayRequest {
    Query,
    Prepare,
    Configure {
        configuration: VirtualDisplayConfiguration,
    },
}

pub(crate) async fn display(request: DisplayRequest) -> super::Reply {
    #[cfg(all(windows, feature = "gpui-host"))]
    {
        let result = hbb_common::tokio::task::spawn_blocking(
            move || -> hbb_common::ResultType<serde_json::Value> {
                let state = match request {
                    DisplayRequest::Query => crate::virtual_display_manager::configuration_state(),
                    DisplayRequest::Prepare => {
                        crate::virtual_display_manager::prepare_configuration()?
                    }
                    DisplayRequest::Configure { configuration } => {
                        configuration.validate()?;
                        crate::virtual_display_manager::configure_displays(&configuration)?
                    }
                };
                Ok(serde_json::to_value(state)?)
            },
        )
        .await;
        match result {
            Ok(Ok(value)) => super::Reply {
                value: Some(value),
                error: None,
            },
            Ok(Err(error)) => super::Reply::error(format!("{error:#}")),
            Err(error) => super::Reply::error(error),
        }
    }
    #[cfg(not(all(windows, feature = "gpui-host")))]
    {
        let _ = request;
        super::Reply::error(
            "Local virtual display configuration requires an installed Windows host",
        )
    }
}
