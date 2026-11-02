use super::*;

use crate::webrtc::signaling::{AnswerResponse, IceCandidateRequest, OfferRequest};

/// Create WebRTC session
#[derive(Serialize)]
pub struct CreateSessionResponse {
    pub session_id: String,
}

pub async fn webrtc_create_session(
    State(state): State<Arc<AppState>>,
    axum::Extension(grant): axum::Extension<crate::management::AccessGrant>,
) -> Result<Json<CreateSessionResponse>> {
    // Check if WebRTC mode is active
    if !state.stream_manager.is_webrtc_enabled().await {
        return Err(AppError::ServiceUnavailable(
            "WebRTC mode not active. Current mode is MJPEG.".to_string(),
        ));
    }

    let session_id = create_owned_session(&state, grant).await?;
    Ok(Json(CreateSessionResponse { session_id }))
}

/// Handle WebRTC offer
pub async fn webrtc_offer(
    State(state): State<Arc<AppState>>,
    axum::Extension(grant): axum::Extension<crate::management::AccessGrant>,
    Json(req): Json<OfferRequest>,
) -> Result<Json<AnswerResponse>> {
    // Check if WebRTC mode is active
    if !state.stream_manager.is_webrtc_enabled().await {
        return Err(AppError::ServiceUnavailable(
            "WebRTC mode not active. Current mode is MJPEG.".to_string(),
        ));
    }

    let webrtc = &state.webrtc;
    let session_id = match req.client_id {
        Some(id) => {
            require_owner(&state, &id, &grant).await?;
            id
        }
        None => create_owned_session(&state, grant).await?,
    };

    // Handle offer
    let offer = crate::webrtc::SdpOffer::new(req.sdp);
    let answer = webrtc.handle_offer(&session_id, offer).await?;

    Ok(Json(AnswerResponse::new(
        answer.sdp,
        session_id,
        answer.ice_candidates.unwrap_or_default(),
    )))
}

/// Add ICE candidate
pub async fn webrtc_ice_candidate(
    State(state): State<Arc<AppState>>,
    axum::Extension(grant): axum::Extension<crate::management::AccessGrant>,
    Json(req): Json<IceCandidateRequest>,
) -> Result<Json<LoginResponse>> {
    require_owner(&state, &req.session_id, &grant).await?;
    state
        .webrtc
        .add_ice_candidate(&req.session_id, req.candidate)
        .await?;

    Ok(Json(LoginResponse {
        success: true,
        message: None,
    }))
}

/// Get WebRTC session info
#[derive(Serialize)]
pub struct WebRtcSessionInfo {
    pub session_id: String,
    pub state: String,
}

#[derive(Serialize)]
pub struct WebRtcStatus {
    pub session_count: usize,
    pub sessions: Vec<WebRtcSessionInfo>,
}

pub async fn webrtc_status(
    State(state): State<Arc<AppState>>,
    axum::Extension(grant): axum::Extension<crate::management::AccessGrant>,
) -> Json<WebRtcStatus> {
    let sessions = state.webrtc.list_sessions().await;
    let owners = state.managed_rtc.lock().await;
    let sessions = sessions
        .into_iter()
        .filter(|session| {
            owners
                .get(&session.session_id)
                .is_some_and(|owner| owner.is_valid() && owner.permit() == grant.permit())
        })
        .collect::<Vec<_>>();
    Json(WebRtcStatus {
        session_count: sessions.len(),
        sessions: sessions
            .into_iter()
            .map(|s| WebRtcSessionInfo {
                session_id: s.session_id,
                state: s.state,
            })
            .collect(),
    })
}

/// Close WebRTC session
#[derive(Deserialize)]
pub struct CloseSessionRequest {
    pub session_id: String,
}

pub async fn webrtc_close_session(
    State(state): State<Arc<AppState>>,
    axum::Extension(grant): axum::Extension<crate::management::AccessGrant>,
    Json(req): Json<CloseSessionRequest>,
) -> Result<Json<LoginResponse>> {
    require_owner(&state, &req.session_id, &grant).await?;
    state.webrtc.close_session(&req.session_id).await?;
    state.managed_rtc.lock().await.remove(&req.session_id);

    Ok(Json(LoginResponse {
        success: true,
        message: Some("Session closed".to_string()),
    }))
}

/// ICE servers configuration for WebRTC
#[derive(Serialize)]
pub struct IceServersResponse {
    pub ice_servers: Vec<IceServerInfo>,
    pub mdns_mode: String,
}

#[derive(Serialize)]
pub struct IceServerInfo {
    pub urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

fn non_empty_config_value(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

/// Get ICE servers configuration for client-side WebRTC
/// Returns user-configured servers, or Google STUN as fallback if none configured
pub async fn webrtc_ice_servers(State(state): State<Arc<AppState>>) -> Json<IceServersResponse> {
    use crate::webrtc::config::public_ice;
    use crate::webrtc::mdns::{mdns_mode, mdns_mode_label};

    let config = state.config.get();
    let mut ice_servers = Vec::new();

    // Check if user has configured custom ICE servers
    let stun_server = non_empty_config_value(&config.stream.stun_server);
    let turn_server = non_empty_config_value(&config.stream.turn_server);

    if stun_server.is_some() || turn_server.is_some() {
        // Use user-configured ICE servers
        if let Some(stun) = stun_server {
            ice_servers.push(IceServerInfo {
                urls: vec![stun.to_string()],
                username: None,
                credential: None,
            });
        }

        if let Some(turn) = turn_server {
            let username = config.stream.turn_username.clone();
            let credential = config.stream.turn_password.clone();
            if username.is_some() && credential.is_some() {
                ice_servers.push(IceServerInfo {
                    urls: vec![turn.to_string()],
                    username,
                    credential,
                });
            }
        }
    } else {
        // No custom servers — baked-in public STUN
        ice_servers.push(IceServerInfo {
            urls: vec![public_ice::stun_server().to_string()],
            username: None,
            credential: None,
        });
        // Note: TURN servers are not provided - users must configure their own
    }

    let mdns_mode = mdns_mode();
    let mdns_mode = mdns_mode_label(mdns_mode).to_string();

    Json(IceServersResponse {
        ice_servers,
        mdns_mode,
    })
}

async fn create_owned_session(
    state: &AppState,
    grant: crate::management::AccessGrant,
) -> Result<String> {
    let _creating = state
        .managed_rtc_creating
        .try_acquire()
        .map_err(|_| AppError::RateLimited("KVM media session creation is busy".into()))?;
    if state.managed_rtc.lock().await.len() >= 32 {
        return Err(AppError::RateLimited(
            "KVM media session capacity exhausted".into(),
        ));
    }
    let id = state.webrtc.create_session(grant.clone()).await?;
    let registered = {
        let mut owners = state.managed_rtc.lock().await;
        if owners.len() < 32 && grant.is_valid() {
            owners.insert(id.clone(), grant);
            true
        } else {
            false
        }
    };
    if !registered {
        state.webrtc.close_session(&id).await?;
        return Err(AppError::RateLimited(
            "KVM media session is unavailable".into(),
        ));
    }
    Ok(id)
}

async fn require_owner(
    state: &AppState,
    id: &str,
    grant: &crate::management::AccessGrant,
) -> Result<()> {
    let owners = state.managed_rtc.lock().await;
    if owners
        .get(id)
        .is_some_and(|owner| owner.is_valid() && owner.permit() == grant.permit())
    {
        Ok(())
    } else {
        Err(AppError::Unauthorized)
    }
}
