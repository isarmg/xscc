//! Browser authorization and lifecycle adapters for the shared XSCC KVM owner.
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::extract::{
    cookie::{Cookie, SameSite},
    CookieJar,
};
use futures::StreamExt;
use serde::Deserialize;
use std::{future::Future, sync::Arc, time::Duration};

use crate::{auth::SESSION_COOKIE, state::AppState};
pub use one_kvm_management::AccessGrant;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRequest {
    pub authorization_code: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddressRequest {
    pub addresses: Vec<one_kvm_management::HostAddress>,
}

fn failure(status: StatusCode, message: &'static str) -> Response {
    (
        status,
        Json(serde_json::json!({"success":false,"message":message})),
    )
        .into_response()
}

fn same_origin(headers: &HeaderMap, https: bool, configured: Option<&str>) -> bool {
    let single = |name: header::HeaderName| {
        let mut values = headers.get_all(name).iter();
        let value = values.next()?.to_str().ok()?;
        values.next().is_none().then_some(value)
    };
    let Some(origin) = single(header::ORIGIN).and_then(|value| url::Url::parse(value).ok()) else {
        return false;
    };
    if let Some(expected) = configured {
        return origin.as_str() == expected;
    }
    let Some(host) = single(header::HOST) else {
        return false;
    };
    let scheme = if https { "https" } else { "http" };
    let Ok(expected) = url::Url::parse(&format!("{scheme}://{host}")) else {
        return false;
    };
    origin.scheme() == scheme
        && origin.host_str() == expected.host_str()
        && origin.port_or_known_default() == expected.port_or_known_default()
        && origin.username().is_empty()
        && origin.password().is_none()
        && origin.path() == "/"
        && origin.query().is_none()
        && origin.fragment().is_none()
}

pub async fn middleware(
    State(state): State<Arc<AppState>>,
    cookies: CookieJar,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request
        .uri()
        .path()
        .strip_prefix("/api")
        .unwrap_or(request.uri().path());
    let logout = path == "/auth/logout";
    let public = matches!(path, "/health" | "/management/status" | "/management/login");
    let changes_state = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    let upgrade = request.headers().contains_key(header::UPGRADE);
    if (changes_state || upgrade)
        && !same_origin(
            request.headers(),
            state.browser_https,
            state
                .browser_origin
                .as_ref()
                .map(one_kvm_management::ServerOrigin::as_str),
        )
    {
        return failure(
            StatusCode::FORBIDDEN,
            "Same-origin management request required",
        );
    }
    if public {
        return next.run(request).await;
    }
    let Some(cookie) = cookies.get(SESSION_COOKIE) else {
        return failure(
            StatusCode::UNAUTHORIZED,
            "XSCS controller authorization required",
        );
    };
    let Ok(grant) = state.managed.authorize(cookie.value()) else {
        return failure(
            StatusCode::UNAUTHORIZED,
            "XSCS authorization expired or revoked",
        );
    };
    if changes_state
        && !request
            .headers()
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| grant.csrf_matches(value))
    {
        return failure(StatusCode::FORBIDDEN, "Valid CSRF token required");
    }
    request.extensions_mut().insert(grant.clone());
    let response = if logout {
        next.run(request).await
    } else {
        tokio::select! {
            biased;
            _ = grant.cancelled() => return failure(StatusCode::UNAUTHORIZED, "XSCS authorization expired or revoked"),
            response = next.run(request) => response,
        }
    };
    let current = grant.clone();
    guard_response(
        response,
        logout,
        async move { grant.cancelled().await },
        move || current.is_valid(),
    )
}

fn guard_response(
    response: Response,
    logout: bool,
    cancelled: impl Future<Output = ()> + Send + 'static,
    is_valid: impl Fn() -> bool + Send + 'static,
) -> Response {
    // A requested Upgrade is not necessarily accepted by the route. Ordinary
    // HTTP streams still need revocation checks when the peer sends that header.
    if response.status() == StatusCode::SWITCHING_PROTOCOLS || logout {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    parts.headers.insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    let mut input = body.into_data_stream();
    let stream = async_stream::stream! {
        tokio::pin!(cancelled);
        loop {
            tokio::select! {
                biased;
                _ = &mut cancelled => break,
                chunk = input.next() => match chunk {
                    Some(chunk) if is_valid() => yield chunk,
                    _ => break,
                },
            }
        }
    };
    Response::from_parts(parts, Body::from_stream(stream))
}

pub async fn public_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(state.managed.public_status()),
    )
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    cookies: CookieJar,
    Json(request): Json<LoginRequest>,
) -> Response {
    let Ok(_permit) = state.managed_logins.try_acquire() else {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            "Controller binding is busy; retry shortly",
        );
    };
    let result = match state.managed.login(&request.authorization_code).await {
        Ok(result) => result,
        Err(error) => {
            use one_kvm_management::Error;
            let (status, message) = match &error {
                Error::Invalid => (
                    StatusCode::BAD_REQUEST,
                    "Invalid controller authorization code",
                ),
                Error::Unconfigured => (
                    StatusCode::CONFLICT,
                    "Bind this KVM locally before controller login",
                ),
                Error::Capacity => (
                    StatusCode::TOO_MANY_REQUESTS,
                    "Controller capacity exhausted",
                ),
                Error::Denied => (
                    StatusCode::FORBIDDEN,
                    "Controller authorization rejected; verify its XSCS binding and access rules",
                ),
                Error::Unavailable => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "XSCS authorization is unavailable",
                ),
                Error::Storage(_)
                | Error::Task(_)
                | Error::Management(_)
                | Error::Enrollment(_) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Controller binding is unavailable",
                ),
            };
            if status.is_server_error() {
                tracing::warn!(%error, "XSCS controller binding failed");
            } else {
                tracing::trace!(%error, "XSCS controller login rejected");
            }
            return failure(status, message);
        }
    };
    let cookie = Cookie::build((SESSION_COOKIE, result.cookie_token.clone()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(
            state
                .browser_origin
                .as_ref()
                .map_or(state.browser_https, |origin| {
                    origin.as_str().starts_with("https://")
                }),
        )
        .max_age(time::Duration::seconds(3600))
        .build();
    (
        cookies.add(cookie),
        [(header::CACHE_CONTROL, "no-store")],
        Json(result),
    )
        .into_response()
}

pub async fn check(State(state): State<Arc<AppState>>, cookies: CookieJar) -> Response {
    let Some(cookie) = cookies.get(SESSION_COOKIE) else {
        return failure(StatusCode::UNAUTHORIZED, "Controller session required");
    };
    match state.managed.browser_status(cookie.value()) {
        Ok(status) => ([(header::CACHE_CONTROL, "no-store")], Json(status)).into_response(),
        Err(_) => failure(
            StatusCode::UNAUTHORIZED,
            "XSCS authorization expired or revoked",
        ),
    }
}

pub async fn device_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.managed.device_status())
}

pub async fn set_addresses(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AddressRequest>,
) -> Response {
    match state.managed.set_addresses(request.addresses).await {
        Ok(()) => Json(state.managed.device_status()).into_response(),
        Err(one_kvm_management::Error::Invalid) => failure(
            StatusCode::BAD_REQUEST,
            "Reachable addresses require valid canonical hosts and nonzero ports",
        ),
        Err(error) => {
            tracing::warn!(%error, "Updating KVM reachable addresses failed");
            failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "Unable to save reachable addresses",
            )
        }
    }
}

pub async fn logout(State(state): State<Arc<AppState>>, cookies: CookieJar) -> Response {
    if let Some(cookie) = cookies.get(SESSION_COOKIE) {
        if let Err(error) = state.managed.logout(cookie.value()).await {
            tracing::warn!(%error, "Controller logout retained pending central unbind");
        }
    }
    let cookie = Cookie::build((SESSION_COOKIE, ""))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .max_age(time::Duration::ZERO)
        .build();
    (
        cookies.add(cookie),
        Json(serde_json::json!({"success":true})),
    )
        .into_response()
}

pub(crate) fn start_session_reaper(state: &Arc<AppState>) -> tokio::task::JoinHandle<()> {
    let weak = Arc::downgrade(state);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let Some(state) = weak.upgrade() else { break };
            state.webrtc.cleanup().await;
            let ids = state
                .managed_rtc
                .lock()
                .await
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            for id in ids {
                if state.webrtc.get_session(&id).await.is_none() {
                    state.managed_rtc.lock().await.remove(&id);
                }
            }
        }
    })
}

pub async fn unsupported_update() -> Response {
    failure(
        StatusCode::CONFLICT,
        "Use a verified XSCC KVM product package for offline updates",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    #[test]
    fn browser_origin_matches_scheme_host_port_and_single_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "kvm.example:8443".parse().unwrap());
        headers.insert(header::ORIGIN, "https://kvm.example:8443".parse().unwrap());
        assert!(same_origin(&headers, true, None));
        assert!(!same_origin(&headers, false, None));
        headers.insert(
            header::ORIGIN,
            "https://other.example:8443".parse().unwrap(),
        );
        assert!(!same_origin(&headers, true, None));
        headers.insert(
            header::ORIGIN,
            "https://kvm.example:8443/path".parse().unwrap(),
        );
        assert!(!same_origin(&headers, true, None));
        headers.insert(header::ORIGIN, "https://kvm.example:8443".parse().unwrap());
        headers.append(header::ORIGIN, "https://other.example".parse().unwrap());
        assert!(!same_origin(&headers, true, None));
    }

    #[tokio::test]
    async fn ordinary_stream_stops_after_revocation_even_with_upgrade_headers() {
        let (chunks, input) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(2);
        let stream = futures::stream::unfold(input, |mut input| async {
            input.recv().await.map(|chunk| (chunk, input))
        });
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::UPGRADE, "websocket")
            .body(Body::from_stream(stream))
            .unwrap();
        let valid = Arc::new(AtomicBool::new(true));
        let current = valid.clone();
        let (revoke, revoked) = tokio::sync::oneshot::channel();
        let response = guard_response(
            response,
            false,
            async move {
                let _ = revoked.await;
            },
            move || current.load(Ordering::Acquire),
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let mut output = response.into_body().into_data_stream();
        chunks
            .send(Ok(bytes::Bytes::from_static(b"first frame")))
            .await
            .unwrap();
        assert_eq!(output.next().await.unwrap().unwrap(), "first frame");

        valid.store(false, Ordering::Release);
        revoke.send(()).unwrap();
        chunks
            .send(Ok(bytes::Bytes::from_static(b"revoked frame")))
            .await
            .unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(1), output.next())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn actual_upgrade_and_logout_keep_their_original_response() {
        for (status, logout) in [
            (StatusCode::SWITCHING_PROTOCOLS, false),
            (StatusCode::OK, true),
        ] {
            let response = Response::builder()
                .status(status)
                .header(header::SET_COOKIE, "session=; Max-Age=0")
                .body(Body::from("original response"))
                .unwrap();
            let response = guard_response(response, logout, async {}, || false);
            assert_eq!(response.status(), status);
            assert_eq!(
                response.headers()[header::SET_COOKIE],
                "session=; Max-Age=0"
            );
            assert!(!response.headers().contains_key(header::CACHE_CONTROL));
            assert_eq!(
                axum::body::to_bytes(response.into_body(), 128)
                    .await
                    .unwrap(),
                "original response"
            );
        }
    }
}
