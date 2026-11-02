use axum::{extract::State, Json};
use std::sync::Arc;

use crate::config::AudioConfig;
use crate::error::{AppError, Result};
use crate::state::AppState;

use super::apply::{apply_audio_config, try_apply_lock};
use super::types::AudioConfigUpdate;

pub async fn get_audio_config(State(state): State<Arc<AppState>>) -> Json<AudioConfig> {
    Json(state.config.get().audio.clone())
}

pub async fn update_audio_config(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AudioConfigUpdate>,
) -> Result<Json<AudioConfig>> {
    req.validate()?;
    if let Some(device) = req.device.as_deref().filter(|device| !device.is_empty()) {
        crate::web::handlers::audio_api::validate_audio_device(&state, device).await?;
    }

    let _apply_guard = try_apply_lock(&state.config_apply_locks.audio, "audio")?;
    let current = state.config.get();
    let old_audio_config = current.audio.clone();
    let mut candidate = (*current).clone();
    req.apply_to(&mut candidate.audio);
    let new_audio_config = candidate.audio.clone();
    let _reservation = state.config.reserve_resources(&candidate)?;

    let result = async {
        apply_audio_config(&state, &old_audio_config, &new_audio_config).await?;
        state
            .config
            .update(|config| config.audio = new_audio_config.clone())
            .await
    }
    .await;
    if let Err(error) = result {
        let rollback = async {
            state.audio.stop_streaming().await?;
            apply_audio_config(&state, &new_audio_config, &old_audio_config).await
        }
        .await;
        if let Err(rollback) = rollback {
            return Err(AppError::Config(format!(
                "{error}; audio rollback failed: {rollback}"
            )));
        }
        return Err(error);
    }

    Ok(Json(new_audio_config))
}
