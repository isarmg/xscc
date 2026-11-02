use axum::{extract::State, Json};
use std::sync::Arc;

use crate::atx::AtxDriverType;
use crate::config::{AtxConfig, HidBackend, HidConfig};
use crate::error::{AppError, Result};
use crate::state::AppState;

use super::apply::{apply_atx_config, try_apply_lock};
use super::types::AtxConfigUpdate;

pub async fn get_atx_config(State(state): State<Arc<AppState>>) -> Json<AtxConfig> {
    Json(state.config.get().atx.clone())
}

pub async fn update_atx_config(
    State(state): State<Arc<AppState>>,
    axum::Extension(grant): axum::Extension<crate::management::AccessGrant>,
    Json(req): Json<AtxConfigUpdate>,
) -> Result<Json<AtxConfig>> {
    let guard = try_apply_lock(&state.config_apply_locks.atx, "atx")?;
    super::transaction::require_current_grant(&grant)?;
    super::transaction::complete_device_update(guard, apply_atx_update(state, req)).await
}

async fn apply_atx_update(state: Arc<AppState>, req: AtxConfigUpdate) -> Result<Json<AtxConfig>> {
    let current_config = state.config.get();
    let old_atx_config = current_config.atx.clone();

    req.validate_with_current(&old_atx_config)?;

    let mut merged_atx_config = old_atx_config.clone();
    req.apply_to(&mut merged_atx_config);
    validate_windows_atx_backends(&merged_atx_config)?;
    validate_serial_device_conflict(&merged_atx_config, &current_config.hid)?;

    let mut candidate = (*current_config).clone();
    candidate.atx = merged_atx_config;
    candidate.enforce_invariants();
    let new_atx_config = candidate.atx.clone();
    let mut reservation = state.config.reserve_resources(&candidate)?;

    let result = async {
        apply_atx_config(&state, &old_atx_config, &new_atx_config).await?;
        state
            .config
            .update(|config| config.atx = new_atx_config.clone())
            .await
    }
    .await;
    if let Err(error) = result {
        let rollback = async {
            let controller = state.atx.write().await.take();
            if let Some(controller) = controller {
                controller.shutdown().await?;
            }
            apply_atx_config(&state, &new_atx_config, &old_atx_config).await
        }
        .await;
        if let Err(rollback) = rollback {
            let controller = state.atx.write().await.take();
            if let Some(controller) = controller {
                if let Err(cleanup) = controller.shutdown().await {
                    tracing::error!(%cleanup, "Failed to disable ATX after rollback failure");
                }
            }
            if let Some(reservation) = reservation.as_mut() {
                reservation.quarantine();
            }
            return Err(AppError::Config(format!(
                "{error}; ATX rollback failed: {rollback}"
            )));
        }
        return Err(error);
    }

    Ok(Json(new_atx_config))
}

fn validate_windows_atx_backends(atx: &AtxConfig) -> Result<()> {
    if !cfg!(windows) {
        return Ok(());
    }

    if !matches!(atx.driver, AtxDriverType::Serial | AtxDriverType::None) {
        return Err(AppError::BadRequest(
            "Windows ATX only supports serial relay or none".to_string(),
        ));
    }

    Ok(())
}

fn validate_serial_device_conflict(atx: &AtxConfig, hid: &HidConfig) -> Result<()> {
    if hid.backend != HidBackend::Ch9329 {
        return Ok(());
    }
    let reserved = hid.ch9329_port.trim();
    if reserved.is_empty() {
        return Ok(());
    }

    if atx.driver == AtxDriverType::Serial && atx.device.trim() == reserved {
        return Err(AppError::BadRequest(format!(
            "ATX serial device '{}' conflicts with HID CH9329 serial device",
            reserved
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_serial_device_conflict_rejects_ch9329_overlap() {
        let mut atx = AtxConfig::default();
        atx.driver = AtxDriverType::Serial;
        atx.device = "/dev/ttyUSB0".to_string();

        let mut hid = HidConfig::default();
        hid.backend = HidBackend::Ch9329;
        hid.ch9329_port = "/dev/ttyUSB0".to_string();

        assert!(validate_serial_device_conflict(&atx, &hid).is_err());
    }

    #[test]
    fn test_validate_serial_device_conflict_allows_non_ch9329_backend() {
        let mut atx = AtxConfig::default();
        atx.driver = AtxDriverType::Serial;
        atx.device = "/dev/ttyUSB0".to_string();

        let mut hid = HidConfig::default();
        hid.backend = HidBackend::None;
        hid.ch9329_port = "/dev/ttyUSB0".to_string();

        assert!(validate_serial_device_conflict(&atx, &hid).is_ok());
    }

    #[test]
    fn test_validate_windows_atx_backends_allows_serial() {
        let mut atx = AtxConfig::default();
        atx.driver = AtxDriverType::Serial;

        assert!(validate_windows_atx_backends(&atx).is_ok());
    }
}
