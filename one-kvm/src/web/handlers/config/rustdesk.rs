use axum::{extract::State, Json};

use crate::error::{AppError, Result};
use crate::rustdesk::config::RustDeskConfig;
use crate::web::state::RemoteAccessApiState;

use super::types::RustDeskConfigUpdate;
use crate::runtime::{try_apply_lock, ConfigApplyOptions};

fn validate_candidate(state: &RemoteAccessApiState, config: &RustDeskConfig) -> Result<()> {
    if !config.enabled
        || config.mode != crate::rustdesk::config::RustDeskMode::DirectIp
        || config.direct_access_port == 0
    {
        return Err(AppError::BadRequest(
            "XSCC KVM requires an enabled managed direct listener on a nonzero port".into(),
        ));
    }
    let mut candidate = state.config.get().as_ref().clone();
    candidate.rustdesk = config.clone();
    crate::video::codec_constraints::validate_third_party_codec_compatibility(&candidate)
}

async fn persist_and_apply(
    state: &RemoteAccessApiState,
    old_config: RustDeskConfig,
    new_config: RustDeskConfig,
) -> Result<RustDeskConfig> {
    validate_candidate(state, &new_config)?;
    let mut candidate = state.config.get().as_ref().clone();
    candidate.rustdesk = new_config.clone();
    let mut reservation = state.config.reserve_resources(&candidate)?;
    let old_runtime = state.coordinator.runtime_config().await.rustdesk;
    let result = async {
        state
            .coordinator
            .apply_rustdesk(&old_config, &new_config, ConfigApplyOptions::default())
            .await?;
        state
            .config
            .update(|config| {
                // The coordinator persists generated identity before opening a listener.
                let identity = &config.rustdesk;
                let mut stored = new_config.clone();
                stored.public_key = identity.public_key.clone();
                stored.private_key = identity.private_key.clone();
                stored.signing_public_key = identity.signing_public_key.clone();
                stored.signing_private_key = identity.signing_private_key.clone();
                stored.uuid = identity.uuid.clone();
                config.rustdesk = stored;
            })
            .await
    }
    .await;
    if let Err(error) = result {
        let rollback = async {
            let mut stopped = new_config.clone();
            stopped.enabled = false;
            let options = ConfigApplyOptions {
                force: true,
                runtime_only: true,
                ..Default::default()
            };
            state
                .coordinator
                .apply_rustdesk(&new_config, &stopped, options)
                .await?;
            state
                .coordinator
                .apply_rustdesk(&stopped, &old_runtime, options)
                .await
        }
        .await;
        if let Err(rollback) = rollback {
            let mut stopped = new_config.clone();
            stopped.enabled = false;
            if let Err(cleanup) = state
                .coordinator
                .apply_rustdesk(
                    &old_runtime,
                    &stopped,
                    ConfigApplyOptions {
                        force: true,
                        runtime_only: true,
                        ..Default::default()
                    },
                )
                .await
            {
                tracing::error!(%cleanup, "Failed to disable native listener after rollback failure");
            }
            if let Some(reservation) = reservation.as_mut() {
                reservation.quarantine();
            }
            return Err(AppError::Config(format!(
                "{error}; native listener rollback failed: {rollback}"
            )));
        }
        return Err(error);
    }
    Ok(state.config.get().rustdesk.clone())
}

fn validate_managed_update(request: &RustDeskConfigUpdate) -> Result<()> {
    if request.enabled == Some(false)
        || request
            .mode
            .is_some_and(|mode| mode != crate::rustdesk::config::RustDeskMode::DirectIp)
        || request.rendezvous_server.is_some()
        || request.relay_server.is_some()
        || request.relay_key.is_some()
        || request.device_password.is_some()
    {
        return Err(AppError::BadRequest(
            "Only the managed direct listener port and codec can be configured".into(),
        ));
    }
    Ok(())
}

async fn current_status(
    state: &RemoteAccessApiState,
    config: RustDeskConfig,
) -> RustDeskStatusResponse {
    let runtime = state.coordinator.rustdesk_status().await;

    RustDeskStatusResponse {
        config: RustDeskConfigResponse::from(&config),
        service_status: runtime.service_status,
        rendezvous_status: runtime.rendezvous_status,
        connection_count: runtime.connection_count,
        listening: runtime.listening,
        listen_port: runtime.listen_port,
    }
}

#[derive(Debug, serde::Serialize)]
pub struct RustDeskConfigResponse {
    pub enabled: bool,
    pub mode: crate::rustdesk::config::RustDeskMode,
    pub codec: crate::rustdesk::config::RustDeskCodec,
    pub direct_access_port: u16,
    pub rendezvous_server: String,
    pub relay_server: Option<String>,
    pub device_id: String,
    pub has_password: bool,
    pub relay_key: Option<String>,
}

impl From<&RustDeskConfig> for RustDeskConfigResponse {
    fn from(config: &RustDeskConfig) -> Self {
        Self {
            enabled: config.enabled,
            mode: config.mode,
            codec: config.codec,
            direct_access_port: config.direct_access_port,
            rendezvous_server: config.rendezvous_server.clone(),
            relay_server: config.relay_server.clone(),
            device_id: config.device_id.clone(),
            has_password: !config.device_password.is_empty(),
            relay_key: config.relay_key.clone(),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct RustDeskStatusResponse {
    pub config: RustDeskConfigResponse,
    pub service_status: String,
    pub rendezvous_status: Option<String>,
    pub connection_count: usize,
    pub listening: bool,
    pub listen_port: Option<u16>,
}

pub async fn get_rustdesk_config(
    State(state): State<RemoteAccessApiState>,
) -> Json<RustDeskConfigResponse> {
    Json(RustDeskConfigResponse::from(&state.config.get().rustdesk))
}

pub async fn get_rustdesk_status(
    State(state): State<RemoteAccessApiState>,
) -> Json<RustDeskStatusResponse> {
    let config = state.config.get().rustdesk.clone();
    Json(current_status(&state, config).await)
}

pub async fn update_rustdesk_config(
    State(state): State<RemoteAccessApiState>,
    axum::Extension(grant): axum::Extension<crate::management::AccessGrant>,
    Json(req): Json<RustDeskConfigUpdate>,
) -> Result<Json<RustDeskConfigResponse>> {
    validate_managed_update(&req)?;
    req.validate()?;

    let guard = try_apply_lock(&state.rustdesk_apply_lock, "rustdesk")?;
    let old_config = state.config.get().rustdesk.clone();
    let mut merged_config = old_config.clone();
    req.apply_to(&mut merged_config);
    req.validate_merged(&merged_config)?;

    super::transaction::require_current_grant(&grant)?;
    let new_config = super::transaction::complete_device_update(guard, async move {
        persist_and_apply(&state, old_config, merged_config).await
    })
    .await?;

    Ok(Json(RustDeskConfigResponse::from(&new_config)))
}

pub async fn regenerate_device_id(
    State(state): State<RemoteAccessApiState>,
    axum::Extension(grant): axum::Extension<crate::management::AccessGrant>,
) -> Result<Json<RustDeskConfigResponse>> {
    let guard = try_apply_lock(&state.rustdesk_apply_lock, "rustdesk")?;
    let old_config = state.config.get().rustdesk.clone();
    let mut regenerated = old_config.clone();
    regenerated.device_id = RustDeskConfig::generate_device_id();
    super::transaction::require_current_grant(&grant)?;
    let new_config = super::transaction::complete_device_update(guard, async move {
        persist_and_apply(&state, old_config, regenerated).await
    })
    .await?;
    Ok(Json(RustDeskConfigResponse::from(&new_config)))
}

pub async fn regenerate_device_password(
    State(state): State<RemoteAccessApiState>,
    axum::Extension(grant): axum::Extension<crate::management::AccessGrant>,
) -> Result<Json<RustDeskConfigResponse>> {
    let guard = try_apply_lock(&state.rustdesk_apply_lock, "rustdesk")?;
    let old_config = state.config.get().rustdesk.clone();
    let mut regenerated = old_config.clone();
    regenerated.device_password = RustDeskConfig::generate_password();
    super::transaction::require_current_grant(&grant)?;
    let new_config = super::transaction::complete_device_update(guard, async move {
        persist_and_apply(&state, old_config, regenerated).await
    })
    .await?;
    Ok(Json(RustDeskConfigResponse::from(&new_config)))
}

pub async fn get_device_password(
    State(state): State<RemoteAccessApiState>,
) -> Json<serde_json::Value> {
    let config = state.config.get().rustdesk.clone();
    Json(serde_json::json!({
        "device_id": config.device_id,
        "device_password": config.device_password
    }))
}

pub async fn start_rustdesk_service(
    State(state): State<RemoteAccessApiState>,
) -> Result<Json<RustDeskStatusResponse>> {
    let _apply_guard = try_apply_lock(&state.rustdesk_apply_lock, "rustdesk")?;
    let stored_config = state.config.get().rustdesk.clone();
    let runtime_config = state.coordinator.runtime_config().await.rustdesk;
    let mut start_config = stored_config.clone();
    start_config.enabled = true;
    state
        .coordinator
        .apply_rustdesk(
            &runtime_config,
            &start_config,
            ConfigApplyOptions::runtime_only(),
        )
        .await?;
    let stored_config = state.config.get().rustdesk.clone();
    Ok(Json(current_status(&state, stored_config).await))
}

pub async fn stop_rustdesk_service(
    State(state): State<RemoteAccessApiState>,
) -> Result<Json<RustDeskStatusResponse>> {
    let _apply_guard = try_apply_lock(&state.rustdesk_apply_lock, "rustdesk")?;
    let stored_config = state.config.get().rustdesk.clone();
    let runtime_config = state.coordinator.runtime_config().await.rustdesk;
    let mut stop_config = stored_config.clone();
    stop_config.enabled = false;
    state
        .coordinator
        .apply_rustdesk(
            &runtime_config,
            &stop_config,
            ConfigApplyOptions::runtime_only(),
        )
        .await?;
    Ok(Json(current_status(&state, stored_config).await))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_updates_reject_disabling_and_legacy_connection_settings() {
        for request in [
            serde_json::json!({"enabled": false}),
            serde_json::json!({"mode": "id"}),
            serde_json::json!({"rendezvous_server": "server:21116"}),
            serde_json::json!({"relay_server": "server:21117"}),
            serde_json::json!({"relay_key": "key"}),
            serde_json::json!({"device_password": "password"}),
        ] {
            let request: RustDeskConfigUpdate = serde_json::from_value(request).unwrap();
            assert!(validate_managed_update(&request).is_err());
        }
        let request: RustDeskConfigUpdate = serde_json::from_value(serde_json::json!({
            "enabled": true, "mode": "direct_ip", "direct_access_port": 21119, "codec": "h265"
        }))
        .unwrap();
        validate_managed_update(&request).unwrap();
        request.validate().unwrap();
    }
}
