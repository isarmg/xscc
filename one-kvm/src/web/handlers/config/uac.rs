use axum::{extract::State, Json};
use serde::Deserialize;

use crate::config::UacConfig;
use crate::error::Result;
use crate::web::state::UsbApiState;

use super::usb_update::update_usb_config;

pub async fn get_uac_config(State(state): State<UsbApiState>) -> Json<UacConfig> {
    Json(state.config.get().uac.clone())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UacConfigUpdate {
    enabled: Option<bool>,
    sample_rate: Option<u32>,
    channels: Option<u8>,
}

impl UacConfigUpdate {
    fn apply_to(self, config: &mut UacConfig) -> Result<()> {
        if let Some(enabled) = self.enabled {
            config.enabled = enabled;
        }
        if let Some(sample_rate) = self.sample_rate {
            config.sample_rate = sample_rate;
        }
        if let Some(channels) = self.channels {
            config.channels = channels;
        }
        config.validate()
    }
}

pub async fn update_uac_config(
    State(state): State<UsbApiState>,
    axum::Extension(grant): axum::Extension<crate::management::AccessGrant>,
    Json(request): Json<UacConfigUpdate>,
) -> Result<Json<UacConfig>> {
    let config = update_usb_config(&state, &grant, move |staged| {
        request.apply_to(&mut staged.uac)?;
        Ok(None)
    })
    .await?;
    Ok(Json(config.uac))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn partial_uac_update_preserves_enabled_and_omitted_fields() {
        let mut config = UacConfig {
            enabled: true,
            ..Default::default()
        };
        let patch: UacConfigUpdate = serde_json::from_value(json!({"sample_rate": 48000})).unwrap();
        patch.apply_to(&mut config).unwrap();
        assert!(config.enabled);
        assert_eq!(config.sample_rate, 48000);
        assert_eq!(config.channels, 2);
    }

    #[test]
    fn partial_uac_update_rejects_unsupported_sample_rate_on_enabled_device() {
        let mut config = UacConfig {
            enabled: true,
            ..Default::default()
        };
        let patch: UacConfigUpdate = serde_json::from_value(json!({"sample_rate": 44100})).unwrap();
        assert!(patch.apply_to(&mut config).is_err());
        assert!(config.enabled);
        assert!(serde_json::from_value::<UacConfigUpdate>(json!({"unexpected": true})).is_err());
    }
}
