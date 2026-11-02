//! Same-origin, bounded HTTP and serialized in-memory administrator sessions.
pub use crate::contracts::AdministratorSession;
use crate::contracts::*;
use futures::{
    FutureExt,
    future::{LocalBoxFuture, Shared},
    lock::Mutex,
};
use gloo_events::EventListener;
use gloo_timers::callback::Timeout;
use js_sys::{Reflect, Uint8Array};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    fmt,
    rc::Rc,
};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AbortController, AbortSignal, RequestCache, RequestCredentials, RequestInit, RequestRedirect,
    Response, Url,
};

pub const DEFAULT_TIMEOUT_MS: u32 = 10_000;
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
pub const AUTH_CHANGE_EVENT: &str = "xcss-auth-change";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub request_id: Option<String>,
    pub retryable: bool,
    pub retry_after_seconds: Option<u32>,
}
impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            crate::i18n::t(
                "请求未能完成，请重试。",
                "The request could not be completed. Please retry."
            )
        )?;
        if let Some(id) = &self.request_id {
            write!(f, " Request ID: {id}")?;
        }
        Ok(())
    }
}
impl std::error::Error for ApiError {}
impl ApiError {
    pub fn local(code: &str) -> Self {
        Self {
            status: 0,
            code: code.into(),
            message: code.into(),
            request_id: None,
            retryable: matches!(code, "network_error" | "request_timeout"),
            retry_after_seconds: None,
        }
    }
}

#[derive(Clone)]
pub struct RequestOptions {
    pub signal: Option<AbortSignal>,
    pub timeout_ms: u32,
    pub max_response_bytes: usize,
    pub headers: Vec<(String, String)>,
}
impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            signal: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            headers: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AuthPhase {
    Loading,
    Anonymous,
    Authenticated,
    LogoutUnconfirmed,
    Error,
}
#[derive(Clone, PartialEq)]
pub struct AuthSnapshot {
    pub session: Option<AdministratorSession>,
    pub phase: AuthPhase,
    pub error: Option<ApiError>,
    pub account_updated: bool,
}
type Restore = Shared<LocalBoxFuture<'static, Result<AdministratorSession, ApiError>>>;
struct State {
    session: Option<AdministratorSession>,
    transport: Option<AdministratorSession>,
    generation: u64,
    phase: AuthPhase,
    error: Option<ApiError>,
    account_updated: bool,
    revoking: Option<u64>,
    restore: Option<(u64, Restore)>,
}
thread_local! {
    static STATE: RefCell<State> = const { RefCell::new(State { session: None, transport: None, generation: 0, phase: AuthPhase::Loading, error: None, account_updated: false, revoking: None, restore: None }) };
    static MUTATIONS: Rc<Mutex<()>> = Rc::new(Mutex::new(()));
}
pub fn current_session() -> Option<AdministratorSession> {
    STATE.with(|s| s.borrow().session.clone())
}
pub fn snapshot() -> AuthSnapshot {
    STATE.with(|s| {
        let s = s.borrow();
        AuthSnapshot {
            session: s.session.clone(),
            phase: s.phase.clone(),
            error: s.error.clone(),
            account_updated: s.account_updated,
        }
    })
}
fn changed() {
    if let Some(window) = web_sys::window()
        && let Ok(event) = web_sys::Event::new(AUTH_CHANGE_EVENT)
    {
        let _ = window.dispatch_event(&event);
    }
}
fn invalidate(error: Option<ApiError>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.generation += 1;
        s.session = None;
        s.restore = None;
        s.phase = if error.is_some() {
            AuthPhase::Error
        } else {
            AuthPhase::Anonymous
        };
        s.error = error;
    });
    changed();
}
fn begin_mutation(revoking: bool) -> u64 {
    let generation = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.generation += 1;
        if revoking {
            s.revoking = Some(s.generation);
        }
        s.restore = None;
        s.session = None;
        // Keep the revocation warning until a successful login replaces its
        // session or the server confirms logout. A failed login is no proof.
        if s.phase != AuthPhase::LogoutUnconfirmed {
            s.phase = AuthPhase::Anonymous;
            s.error = None;
        }
        s.generation
    });
    changed();
    generation
}
fn require_generation(generation: u64) -> Result<(), ApiError> {
    if STATE.with(|s| s.borrow().generation) == generation {
        Ok(())
    } else {
        Err(ApiError::local("auth_operation_superseded"))
    }
}
fn publish(
    session: AdministratorSession,
    generation: u64,
) -> Result<AdministratorSession, ApiError> {
    STATE.with(|s| s.borrow_mut().transport = Some(session.clone()));
    require_generation(generation)?;
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.session = Some(session.clone());
        s.phase = AuthPhase::Authenticated;
        s.error = None;
        s.account_updated = false;
        s.revoking = None;
    });
    changed();
    Ok(session)
}

pub fn validate_api_path(path: &str) -> bool {
    path.starts_with("/api/v1/")
        && !path.contains('#')
        && !path.contains('\\')
        && !path.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
}
fn resolve(path: &str) -> Result<String, ApiError> {
    if !validate_api_path(path) {
        return Err(ApiError::local("invalid_api_path"));
    }
    let window = web_sys::window().ok_or_else(|| ApiError::local("browser_required"))?;
    let base = Url::new(
        &window
            .location()
            .href()
            .map_err(|_| ApiError::local("invalid_api_path"))?,
    )
    .map_err(|_| ApiError::local("invalid_api_path"))?;
    let url =
        Url::new_with_base(path, &base.href()).map_err(|_| ApiError::local("invalid_api_path"))?;
    if url.origin() != base.origin()
        || !url.pathname().starts_with("/api/v1/")
        || !url.username().is_empty()
        || !url.password().is_empty()
        || !["http:", "https:"].contains(&url.protocol().as_str())
    {
        return Err(ApiError::local("invalid_api_path"));
    }
    Ok(url.href())
}

/// Serialize a typed request without silently replacing serialization failures with null.
pub fn encode_json_body<T: Serialize + ?Sized>(body: &T) -> Result<Value, ApiError> {
    serde_json::to_value(body).map_err(|_| ApiError::local("body_serialization_failed"))
}

/// Decode a successful JSON response. Nullable or empty responses require `Option<T>` or `()`.
pub fn decode_response<T: DeserializeOwned>(value: Value) -> Result<T, ApiError> {
    serde_json::from_value(value).map_err(|_| ApiError::local("invalid_typed_response_shape"))
}

/// Keep product-specific bounds, revisions and path binding before constructing a DTO.
pub fn decode_validated_response<T: DeserializeOwned>(
    value: Value,
    validate: impl FnOnce(&Value) -> bool,
) -> Result<T, ApiError> {
    if !validate(&value) {
        return Err(ApiError::local("invalid_response_contract"));
    }
    decode_response(value)
}

pub async fn request_typed<T: DeserializeOwned>(
    path: &str,
    method: &str,
    body: Option<Value>,
) -> Result<T, ApiError> {
    request_typed_with_options(path, method, body, RequestOptions::default()).await
}

/// Same bounded transport, cancellation and session handling as `request_with_options`.
pub async fn request_typed_with_options<T: DeserializeOwned>(
    path: &str,
    method: &str,
    body: Option<Value>,
    options: RequestOptions,
) -> Result<T, ApiError> {
    decode_response(request_with_options(path, method, body, options).await?)
}

/// Deserialize only after the existing runtime response contract has accepted the wire value.
pub async fn request_typed_validated_with_options<T: DeserializeOwned>(
    path: &str,
    method: &str,
    body: Option<Value>,
    options: RequestOptions,
    validate: impl FnOnce(&Value) -> bool,
) -> Result<T, ApiError> {
    decode_validated_response(
        request_with_options(path, method, body, options).await?,
        validate,
    )
}

pub async fn request(path: &str, method: &str, body: Option<Value>) -> Result<Value, ApiError> {
    request_with_options(path, method, body, RequestOptions::default()).await
}
pub async fn request_with_options(
    path: &str,
    method: &str,
    body: Option<Value>,
    options: RequestOptions,
) -> Result<Value, ApiError> {
    let (generation, token) = STATE.with(|s| {
        let s = s.borrow();
        (
            s.generation,
            s.session.as_ref().map(|v| v.csrf_token.clone()),
        )
    });
    send(path, method, body, options, token, generation).await
}

/// Stream the browser's Blob directly; do not copy uploads into WASM memory.
pub async fn request_blob(
    path: &str,
    method: &str,
    body: web_sys::Blob,
    options: RequestOptions,
) -> Result<Value, ApiError> {
    let (generation, token) = STATE.with(|s| {
        let s = s.borrow();
        (
            s.generation,
            s.session.as_ref().map(|v| v.csrf_token.clone()),
        )
    });
    send_body(path, method, None, Some(body), options, token, generation).await
}

async fn send(
    path: &str,
    method: &str,
    body: Option<Value>,
    options: RequestOptions,
    token: Option<String>,
    generation: u64,
) -> Result<Value, ApiError> {
    send_body(path, method, body, None, options, token, generation).await
}

async fn send_body(
    path: &str,
    method: &str,
    body: Option<Value>,
    raw_body: Option<web_sys::Blob>,
    options: RequestOptions,
    token: Option<String>,
    generation: u64,
) -> Result<Value, ApiError> {
    if !(1..=120_000).contains(&options.timeout_ms)
        || !(1..=MAX_RESPONSE_BYTES).contains(&options.max_response_bytes)
    {
        return Err(ApiError::local("invalid_request_budget"));
    }
    let target = resolve(path)?;
    let controller = AbortController::new().map_err(|_| ApiError::local("network_error"))?;
    let timed_out = Rc::new(Cell::new(false));
    let timeout = {
        let controller = controller.clone();
        let timed_out = timed_out.clone();
        Timeout::new(options.timeout_ms, move || {
            timed_out.set(true);
            controller.abort();
        })
    };
    let _caller = options.signal.as_ref().map(|signal| {
        if signal.aborted() {
            controller.abort();
        }
        let controller = controller.clone();
        EventListener::once(signal, "abort", move |_| controller.abort())
    });
    let init = RequestInit::new();
    let method = method.to_ascii_uppercase();
    init.set_method(&method);
    init.set_credentials(RequestCredentials::SameOrigin);
    init.set_cache(RequestCache::NoStore);
    init.set_redirect(RequestRedirect::Error);
    init.set_signal(Some(&controller.signal()));
    let headers = web_sys::Headers::new().map_err(|_| ApiError::local("invalid_headers"))?;
    headers
        .set("accept", "application/json")
        .map_err(|_| ApiError::local("invalid_headers"))?;
    for (name, value) in &options.headers {
        if name.eq_ignore_ascii_case("x-csrf-token") {
            return Err(ApiError::local("csrf_header_owned_by_session"));
        }
        headers
            .set(name, value)
            .map_err(|_| ApiError::local("invalid_headers"))?;
    }
    if !["GET", "HEAD", "OPTIONS", "TRACE"].contains(&method.as_str())
        && let Some(token) = token.as_deref()
    {
        headers
            .set("x-csrf-token", token)
            .map_err(|_| ApiError::local("invalid_headers"))?;
    }
    if let Some(body) = body {
        headers
            .set("content-type", "application/json")
            .map_err(|_| ApiError::local("invalid_headers"))?;
        init.set_body(&JsValue::from_str(&body.to_string()));
    }
    if let Some(body) = raw_body {
        if !headers
            .has("content-type")
            .map_err(|_| ApiError::local("invalid_headers"))?
        {
            let media = body.type_();
            headers
                .set(
                    "content-type",
                    if media.is_empty() {
                        "application/octet-stream"
                    } else {
                        &media
                    },
                )
                .map_err(|_| ApiError::local("invalid_headers"))?;
        }
        init.set_body(body.as_ref());
    }
    init.set_headers(&headers);
    let operation = async {
        let response: Response = JsFuture::from(
            web_sys::window()
                .unwrap()
                .fetch_with_str_and_init(&target, &init),
        )
        .await
        .map_err(|_| ApiError::local("network_error"))?
        .dyn_into()
        .map_err(|_| ApiError::local("invalid_response_shape"))?;
        let request_id = response
            .headers()
            .get("x-request-id")
            .ok()
            .flatten()
            .filter(|v| safe_request_id(v));
        let status = response.status();
        // Invalidate on headers, even if a malformed/slow 401 body cannot be read.
        if status == 401
            && path != LOGIN_PATH
            && path != LOGOUT_PATH
            && require_generation(generation).is_ok()
        {
            invalidate(None);
        }
        if status == 204
            || status == 205
            || method == "HEAD"
            || response
                .headers()
                .get("content-length")
                .ok()
                .flatten()
                .as_deref()
                == Some("0")
        {
            return if response.ok() {
                Ok(Value::Null)
            } else {
                Err(ApiError {
                    status,
                    request_id,
                    ..ApiError::local("invalid_error_response")
                })
            };
        }
        let is_json = response
            .headers()
            .get("content-type")
            .ok()
            .flatten()
            .is_some_and(|v| {
                let media = v
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase();
                media == "application/json" || media.ends_with("+json")
            });
        let retry_after_seconds = response
            .headers()
            .get("retry-after")
            .ok()
            .flatten()
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v.min(86_400) as u32);
        if !is_json {
            return Err(ApiError {
                status,
                request_id,
                retry_after_seconds,
                ..ApiError::local(if response.ok() {
                    "invalid_content_type"
                } else {
                    "invalid_error_response"
                })
            });
        }
        let limit = if response.ok() {
            options.max_response_bytes
        } else {
            64 * 1024
        };
        let bytes = read_bounded(&response, limit).await.map_err(|mut error| {
            error.status = status;
            error.request_id = request_id.clone();
            error
        })?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| ApiError {
            status,
            request_id: request_id.clone(),
            ..ApiError::local(if response.ok() {
                "invalid_json_response"
            } else {
                "invalid_error_response"
            })
        })?;
        if response.ok() {
            return Ok(value);
        }
        let code = value.get("code").and_then(Value::as_str).filter(|v| {
            (1..=128).contains(&v.len())
                && v.as_bytes()[0].is_ascii_lowercase()
                && v.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        });
        let message = value.get("message").and_then(Value::as_str);
        let retryable = value.get("retryable").and_then(Value::as_bool);
        let exact = value.as_object().is_some_and(|o| {
            o.keys().all(|k| {
                ["code", "message", "retryable", "request_id", "details"].contains(&k.as_str())
            })
        });
        let fields_valid = value
            .get("request_id")
            .is_none_or(|v| v.as_str().is_some_and(safe_request_id))
            && value.get("details").is_none_or(Value::is_object);
        if !exact || !fields_valid || code.is_none() || message.is_none() || retryable.is_none() {
            return Err(ApiError {
                status,
                request_id,
                retry_after_seconds,
                ..ApiError::local("invalid_error_response")
            });
        }
        Err(ApiError {
            status,
            code: code.unwrap().into(),
            message: message.unwrap().into(),
            request_id: value
                .get("request_id")
                .and_then(Value::as_str)
                .filter(|v| safe_request_id(v))
                .map(str::to_owned)
                .or(request_id),
            retryable: retryable.unwrap(),
            retry_after_seconds,
        })
    }
    .await;
    drop(timeout);
    if timed_out.get() {
        return Err(ApiError::local("request_timeout"));
    }
    if options.signal.is_some_and(|signal| signal.aborted()) {
        return Err(ApiError::local("request_aborted"));
    }
    operation
}

async fn read_bounded(response: &Response, maximum: usize) -> Result<Vec<u8>, ApiError> {
    if response
        .headers()
        .get("content-length")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|n| n > maximum as u64)
    {
        if let Some(body) = response.body() {
            let _ = JsFuture::from(body.cancel()).await;
        }
        return Err(ApiError::local("response_too_large"));
    }
    let Some(body) = response.body() else {
        return Ok(Vec::new());
    };
    let reader: web_sys::ReadableStreamDefaultReader = body
        .get_reader()
        .dyn_into()
        .map_err(|_| ApiError::local("invalid_response_shape"))?;
    let result = async {
        let mut bytes = Vec::new();
        loop {
            let item = JsFuture::from(reader.read())
                .await
                .map_err(|_| ApiError::local("network_error"))?;
            if Reflect::get(&item, &"done".into())
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                break;
            }
            let value = Reflect::get(&item, &"value".into())
                .map_err(|_| ApiError::local("invalid_response_shape"))?;
            let chunk = Uint8Array::new(&value);
            if bytes.len().saturating_add(chunk.length() as usize) > maximum {
                return Err(ApiError::local("response_too_large"));
            }
            bytes.extend(chunk.to_vec());
        }
        Ok(bytes)
    }
    .await;
    if result.is_err() {
        let _ = JsFuture::from(reader.cancel()).await;
    }
    reader.release_lock();
    result
}

fn session(value: Value) -> Result<AdministratorSession, ApiError> {
    let session: AdministratorSession =
        serde_json::from_value(value).map_err(|_| ApiError::local("invalid_response_shape"))?;
    if !session.validate() {
        return Err(ApiError::local("invalid_response_shape"));
    }
    Ok(session)
}

pub async fn login(username: &str, password: &str) -> Result<AdministratorSession, ApiError> {
    if !valid_username_candidate(username) || !valid_password(password) {
        return Err(ApiError::local("invalid_credentials"));
    }
    let generation = begin_mutation(false);
    let mutex = MUTATIONS.with(Clone::clone);
    let _guard = mutex.lock().await;
    let received = send(
        LOGIN_PATH,
        "POST",
        Some(json!({"username":username,"password":password})),
        RequestOptions::default(),
        None,
        generation,
    )
    .await?;
    publish(session(received)?, generation)
}

pub async fn restore() -> Result<AdministratorSession, ApiError> {
    // A cookie may still be valid while logout is pending or failed. Restoring it
    // would undo the user's local sign-out and discard the revocation warning.
    // Only an explicit successful login or confirmed logout ends this barrier.
    if STATE.with(|s| s.borrow().revoking.is_some()) {
        return Err(ApiError::local("auth_logout_unconfirmed"));
    }
    let (generation, pending) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if let Some((generation, pending)) = &s.restore {
            return (*generation, pending.clone());
        }
        let generation = s.generation;
        let pending = async move {
            let mutex = MUTATIONS.with(Clone::clone);
            let _guard = mutex.lock().await;
            let received = send(
                SESSION_PATH,
                "GET",
                None,
                RequestOptions::default(),
                None,
                generation,
            )
            .await
            .and_then(session);
            match received {
                Ok(session) => publish(session, generation),
                Err(error) => {
                    if require_generation(generation).is_ok() {
                        if error.status == 401 {
                            invalidate(None);
                        } else {
                            invalidate(Some(error.clone()));
                        }
                    }
                    Err(error)
                }
            }
        }
        .boxed_local()
        .shared();
        s.restore = Some((generation, pending.clone()));
        (generation, pending)
    });
    let result = pending.await;
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.restore.as_ref().is_some_and(|(g, _)| *g == generation) {
            s.restore = None;
        }
    });
    result
}

pub async fn logout() -> Result<(), ApiError> {
    let generation = begin_mutation(true);
    let mutex = MUTATIONS.with(Clone::clone);
    let _guard = mutex.lock().await;
    let token = STATE.with(|s| s.borrow().transport.as_ref().map(|v| v.csrf_token.clone()));
    let result = send(
        LOGOUT_PATH,
        "POST",
        None,
        RequestOptions::default(),
        token,
        generation,
    )
    .await
    .and_then(|value| {
        if value.is_null() {
            Ok(())
        } else {
            Err(ApiError::local("invalid_response_shape"))
        }
    });
    let error = match result {
        Ok(()) => None,
        Err(error) if error.status == 401 && error.code == "auth.session_required" => None,
        Err(error) => Some(error),
    };
    let notify = STATE.with(|s| settle_revocation(&mut s.borrow_mut(), generation, error.as_ref()));
    if notify {
        changed();
    }
    if let Some(error) = error {
        return Err(error);
    }
    require_generation(generation)
}

// Called while holding MUTATIONS: no later login has reached the server yet.
// Its queued intent can supersede this operation's return value, but cannot
// undo the server's revocation result. A newer logout owns its own barrier.
fn settle_revocation(state: &mut State, generation: u64, error: Option<&ApiError>) -> bool {
    if error.is_none() {
        state.transport = None;
    }
    if state.revoking != Some(generation) {
        return false;
    }
    if let Some(error) = error {
        state.phase = AuthPhase::LogoutUnconfirmed;
        state.error = Some(error.clone());
    } else {
        state.revoking = None;
        state.phase = AuthPhase::Anonymous;
        state.error = None;
    }
    true
}

pub async fn update_account(input: Value) -> Result<(), ApiError> {
    let object = input
        .as_object()
        .ok_or_else(|| ApiError::local("invalid_account"))?;
    if !object
        .keys()
        .all(|k| ["username", "current_password", "new_password"].contains(&k.as_str()))
        || !input
            .get("username")
            .and_then(Value::as_str)
            .is_some_and(valid_username_candidate)
        || !input
            .get("current_password")
            .and_then(Value::as_str)
            .is_some_and(|p| !p.is_empty() && p.len() <= 1024)
        || input
            .get("new_password")
            .is_some_and(|p| !p.as_str().is_some_and(valid_password))
    {
        return Err(ApiError::local("invalid_account"));
    }
    let generation = STATE.with(|s| s.borrow().generation);
    let mutex = MUTATIONS.with(Clone::clone);
    let _guard = mutex.lock().await;
    require_generation(generation)?;
    let result = request(ACCOUNT_PATH, "POST", Some(input)).await?;
    if !result.is_null() {
        return Err(ApiError::local("invalid_response_shape"));
    }
    require_generation(generation)?;
    invalidate(None);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.transport = None;
        s.account_updated = true;
    });
    changed();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restore_does_not_touch_the_browser_during_revocation() {
        for phase in [AuthPhase::Anonymous, AuthPhase::LogoutUnconfirmed] {
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.revoking = Some(state.generation);
                state.phase = phase.clone();
            });
            let error = futures::executor::block_on(restore()).unwrap_err();
            assert_eq!(error.code, "auth_logout_unconfirmed");
            STATE.with(|state| {
                let state = state.borrow();
                assert!(state.revoking.is_some());
                assert!(state.phase == phase);
                assert!(state.restore.is_none());
            });
        }
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.revoking = None;
            state.phase = AuthPhase::Loading;
        });
    }

    fn revocation_state(latest_intent: u64, latest_logout: u64) -> State {
        State {
            session: None,
            transport: Some(AdministratorSession {
                authenticated: true,
                user_id: "admin-id".into(),
                username: "admin".into(),
                role: "admin".into(),
                csrf_token: "A".repeat(43),
            }),
            generation: latest_intent,
            phase: AuthPhase::Anonymous,
            error: None,
            account_updated: false,
            revoking: Some(latest_logout),
            restore: None,
        }
    }

    #[test]
    fn queued_login_does_not_discard_logout_failure() {
        // Logout owns generation 1; login intent 2 is waiting for its lock.
        let mut state = revocation_state(2, 1);
        let error = ApiError::local("network_error");
        assert!(settle_revocation(&mut state, 1, Some(&error)));
        // A failed login returns without publishing any state.
        assert_eq!(state.generation, 2);
        assert_eq!(state.phase, AuthPhase::LogoutUnconfirmed);
        assert_eq!(state.revoking, Some(1));
        assert_eq!(state.error, Some(error));
        assert!(state.transport.is_some());
    }

    #[test]
    fn queued_login_does_not_discard_confirmed_logout() {
        let mut state = revocation_state(2, 1);
        assert!(settle_revocation(&mut state, 1, None));
        assert_eq!(state.generation, 2);
        assert_eq!(state.phase, AuthPhase::Anonymous);
        assert_eq!(state.revoking, None);
        assert!(state.transport.is_none());
        assert!(state.error.is_none());
    }

    #[test]
    fn older_logout_cannot_settle_a_newer_logout_barrier() {
        // Logout 1, login 2, and logout 3 have all recorded their intents.
        for old_error in [None, Some(ApiError::local("network_error"))] {
            let mut state = revocation_state(3, 3);
            assert!(!settle_revocation(&mut state, 1, old_error.as_ref()));
            assert_eq!(state.generation, 3);
            assert_eq!(state.revoking, Some(3));
            assert_eq!(state.phase, AuthPhase::Anonymous);
            let error = ApiError::local("request_timeout");
            assert!(settle_revocation(&mut state, 3, Some(&error)));
            assert_eq!(state.phase, AuthPhase::LogoutUnconfirmed);
            assert_eq!(state.revoking, Some(3));
            assert!(settle_revocation(&mut state, 3, None));
            assert_eq!(state.revoking, None);
        }
    }

    #[test]
    fn typed_decode_preserves_large_integers_and_null_contracts() {
        #[derive(Deserialize, Debug, PartialEq)]
        struct Payload {
            sequence: i64,
            optional: Option<String>,
        }
        let value = json!({"sequence": 9_007_199_254_740_993i64, "optional": null});
        let decoded: Payload = decode_response(value).unwrap();
        assert_eq!(decoded.sequence, 9_007_199_254_740_993);
        assert_eq!(decoded.optional, None);
        assert_eq!(
            decode_response::<Payload>(Value::Null).unwrap_err().code,
            "invalid_typed_response_shape"
        );
        assert_eq!(decode_response::<()>(Value::Null).unwrap(), ());
        assert!(decode_validated_response::<Payload>(json!({"sequence": 1}), |_| false).is_err());
        assert!(decode_response::<Payload>(json!({"sequence":"1"})).is_err());
    }
    #[test]
    fn typed_body_serialization_errors_are_explicit() {
        struct Invalid;
        impl Serialize for Invalid {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("intentional failure"))
            }
        }
        assert_eq!(
            encode_json_body(&Invalid).unwrap_err().code,
            "body_serialization_failed"
        );
        assert_eq!(
            encode_json_body(&json!({"revision":"a"})).unwrap(),
            json!({"revision":"a"})
        );
    }
    #[test]
    fn api_boundary_rejects_cross_origin_and_fragment_paths() {
        for path in [
            "https://evil.example/api/v1/x",
            "//evil.example/api/v1/x",
            "/api/v1/x#secret",
            "/api/v1/x\n",
            "/api/v1/\\evil",
        ] {
            assert!(!validate_api_path(path), "{path:?}");
        }
        assert!(validate_api_path("/api/v1/items?cursor=a%2Fb"));
    }
}
