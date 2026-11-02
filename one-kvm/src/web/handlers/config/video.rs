use axum::{extract::State, Json};
use std::sync::Arc;

use crate::config::VideoConfig;
use crate::error::{AppError, Result};
use crate::state::AppState;

use super::apply::{apply_video_config, try_apply_lock, ConfigApplyOptions};
use super::types::VideoConfigUpdate;

pub async fn get_video_config(State(state): State<Arc<AppState>>) -> Json<VideoConfig> {
    Json(state.config.get().video.clone())
}

pub async fn update_video_config(
    State(state): State<Arc<AppState>>,
    Json(mut req): Json<VideoConfigUpdate>,
) -> Result<Json<VideoConfig>> {
    let selected_path = req
        .device
        .clone()
        .or_else(|| state.config.get().video.device.clone());
    if let Some(path) = selected_path {
        let source_following = state
            .stream_manager
            .list_devices()
            .await
            .ok()
            .and_then(|devices| {
                devices
                    .into_iter()
                    .find(|device| device.path.to_string_lossy() == path)
            })
            .is_some_and(|device| {
                device.control_mode == crate::video::device::VideoControlMode::SourceFollowing
            });
        if source_following {
            req.ignore_source_following_parameters();
        }
    }
    req.validate()?;

    let _apply_guard = try_apply_lock(&state.config_apply_locks.video, "video")?;
    let current = state.config.get();
    let old_video_config = current.video.clone();
    let mut candidate = (*current).clone();
    req.apply_to(&mut candidate.video);
    let new_video_config = candidate.video.clone();
    let _reservation = state.config.reserve_resources(&candidate)?;

    let result = async {
        apply_video_config(
            &state,
            &old_video_config,
            &new_video_config,
            ConfigApplyOptions::forced(),
        )
        .await?;
        state
            .config
            .update(|config| config.video = new_video_config.clone())
            .await
    }
    .await;
    if let Err(error) = result {
        let rollback = async {
            state.stream_manager.stop().await?;
            if old_video_config.device.is_some() {
                apply_video_config(
                    &state,
                    &new_video_config,
                    &old_video_config,
                    ConfigApplyOptions::forced(),
                )
                .await?;
            }
            Ok::<(), AppError>(())
        }
        .await;
        if let Err(rollback) = rollback {
            return Err(AppError::Config(format!(
                "{error}; video rollback failed: {rollback}"
            )));
        }
        return Err(error);
    }

    Ok(Json(new_video_config))
}
