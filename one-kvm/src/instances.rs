//! A host contains independent, locally enrolled KVM instances.
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    str::FromStr,
};

pub const DEFAULT_INSTANCE: &str = "default";
pub const MAX_INSTANCES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InstanceId(String);

impl InstanceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn web_path(&self) -> String {
        format!("/instances/{}/", self.0)
    }
    pub fn data_dir(&self, root: &Path) -> PathBuf {
        if self.0 == DEFAULT_INSTANCE {
            root.to_owned()
        } else {
            root.join("instances").join(&self.0)
        }
    }
}

impl Default for InstanceId {
    fn default() -> Self {
        Self(DEFAULT_INSTANCE.into())
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for InstanceId {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty()
            || value.len() > 48
            || !value.as_bytes()[0].is_ascii_alphanumeric()
            || !value
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_')
        {
            return Err("Instance ID must contain 1–48 lowercase ASCII letters, digits, '-' or '_', starting with a letter or digit");
        }
        Ok(Self(value.into()))
    }
}

#[derive(Debug, Clone)]
pub struct InstanceSpec {
    pub id: InstanceId,
    pub data_dir: PathBuf,
}

/// Prevent local CLI changes and another host process racing the live catalog.
pub fn lock_host(root: &Path) -> io::Result<fs::File> {
    fs::create_dir_all(root)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("one-kvm.lock"))?;
    file.try_lock().map_err(io::Error::other)?;
    Ok(file)
}

/// Preserve the original root as `default`; named instances never alias it.
pub fn discover(root: &Path) -> io::Result<Vec<InstanceSpec>> {
    let mut instances = vec![InstanceSpec {
        id: InstanceId::default(),
        data_dir: root.to_owned(),
    }];
    let directory = root.join("instances");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(instances),
        Err(error) => return Err(error),
    };
    if fs::symlink_metadata(&directory)?.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Instances directory cannot be a symbolic link",
        ));
    }
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Instance entries must be real directories",
            ));
        }
        let name = entry.file_name().into_string().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid instance directory name",
            )
        })?;
        let id: InstanceId = name
            .parse()
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
        if id == InstanceId::default() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "The default instance uses the original root directory",
            ));
        }
        instances.push(InstanceSpec {
            id,
            data_dir: entry.path(),
        });
        if instances.len() > MAX_INSTANCES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Too many KVM instances",
            ));
        }
    }
    instances.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(instances)
}

#[cfg(feature = "desktop")]
mod host {
    use super::*;
    use crate::{
        config::{AppConfig, ConfigStore},
        db::open_database_pool,
        runtime::{ApplicationRuntime, RuntimeBuilder, WebConfigOverrides},
    };
    use axum::{
        extract::{OriginalUri, Request, State},
        http::{header, HeaderMap, Method, StatusCode},
        middleware::{self, Next},
        response::{Html, IntoResponse, Redirect, Response},
        routing::get,
        Extension, Json, Router,
    };
    use axum_extra::extract::CookieJar;
    use serde::Serialize;
    use std::{
        collections::{BTreeSet, HashSet},
        sync::Arc,
    };

    #[derive(Clone)]
    pub struct InstanceScope(pub InstanceId);

    #[derive(Clone, Serialize)]
    pub struct InstanceSummary {
        pub id: String,
        pub path: String,
        pub native_port: u16,
    }

    pub struct InstanceHost {
        runtimes: Vec<(InstanceSpec, ApplicationRuntime)>,
        summaries: Arc<Vec<InstanceSummary>>,
        controllers: Arc<one_kvm_management::BrowserControllerPool>,
        _listener_resource: crate::config::ResourceReservation,
        _directory_lock: fs::File,
    }

    pub async fn load_config(data_dir: &Path) -> anyhow::Result<AppConfig> {
        let database = open_database_pool(data_dir).await?;
        let store = ConfigStore::new(database.clone_pool());
        store.load().await?;
        Ok(store.get().as_ref().clone())
    }

    pub async fn create_instance(
        root: &Path,
        id: &InstanceId,
        port: Option<u16>,
    ) -> anyhow::Result<InstanceSummary> {
        fs::create_dir_all(root)?;
        let entries = discover(root)?;
        let existing = entries.iter().find(|entry| &entry.id == id);
        let exists = existing.is_some_and(|entry| entry.data_dir.join("one-kvm.db").is_file());
        if exists {
            anyhow::bail!("Instance {id} already exists");
        }
        if existing.is_none() && entries.len() >= MAX_INSTANCES {
            anyhow::bail!("Too many KVM instances");
        }
        let mut used = BTreeSet::new();
        for entry in entries.iter().filter(|entry| &entry.id != id) {
            let config = load_config(&entry.data_dir).await?;
            used.insert(config.rustdesk.direct_access_port);
        }
        let port = match port {
            Some(0) => anyhow::bail!("Native port cannot be zero"),
            Some(port) if used.contains(&port) => {
                anyhow::bail!("Native port {port} is already assigned to another instance")
            }
            Some(port) => port,
            None => (21118..=u16::MAX)
                .find(|port| !used.contains(port))
                .ok_or_else(|| anyhow::anyhow!("No native port available"))?,
        };
        let directory = id.data_dir(root);
        if id.as_str() != DEFAULT_INSTANCE {
            let parent = root.join("instances");
            fs::create_dir_all(&parent)?;
            if fs::symlink_metadata(&parent)?.file_type().is_symlink() {
                anyhow::bail!("Instances directory cannot be a symbolic link");
            }
            fs::create_dir(&directory)?;
        }
        let database = open_database_pool(&directory).await?;
        let store = ConfigStore::new(database.clone_pool());
        store.load().await?;
        store
            .update(|config| {
                config.rustdesk.enabled = true;
                config.rustdesk.mode = crate::rustdesk::config::RustDeskMode::DirectIp;
                config.rustdesk.direct_access_port = port;
            })
            .await?;
        Ok(InstanceSummary {
            id: id.to_string(),
            path: id.web_path(),
            native_port: port,
        })
    }

    pub async fn set_native_port(
        root: &Path,
        id: &InstanceId,
        port: u16,
    ) -> anyhow::Result<InstanceSummary> {
        if port == 0 {
            anyhow::bail!("Native port cannot be zero");
        }
        let entries = discover(root)?;
        let entry = entries
            .iter()
            .find(|entry| &entry.id == id)
            .ok_or_else(|| anyhow::anyhow!("Instance {id} does not exist"))?;
        for other in entries.iter().filter(|other| &other.id != id) {
            if load_config(&other.data_dir)
                .await?
                .rustdesk
                .direct_access_port
                == port
            {
                anyhow::bail!(
                    "Native port {port} is already assigned to instance {}",
                    other.id
                );
            }
        }
        let database = open_database_pool(&entry.data_dir).await?;
        let store = ConfigStore::new(database.clone_pool());
        store.load().await?;
        store
            .update(|config| config.rustdesk.direct_access_port = port)
            .await?;
        Ok(InstanceSummary {
            id: id.to_string(),
            path: id.web_path(),
            native_port: port,
        })
    }

    pub async fn summaries(root: &Path) -> anyhow::Result<Vec<InstanceSummary>> {
        let mut result = Vec::new();
        for entry in discover(root)? {
            let config = load_config(&entry.data_dir).await?;
            result.push(InstanceSummary {
                id: entry.id.to_string(),
                path: entry.id.web_path(),
                native_port: config.rustdesk.direct_access_port,
            });
        }
        Ok(result)
    }

    impl InstanceHost {
        pub async fn build(root: &Path, overrides: WebConfigOverrides) -> anyhow::Result<Self> {
            let directory_lock = lock_host(root).map_err(|error| {
                anyhow::anyhow!(
                    "One-KVM host directory is already in use or cannot be locked: {error}"
                )
            })?;
            let entries = discover(root)?;
            let mut host_config = load_config(root).await?;
            overrides.clone().apply(&mut host_config);
            let mut ports = HashSet::new();
            for entry in &entries {
                let config = load_config(&entry.data_dir).await?;
                let port = config.rustdesk.direct_access_port;
                if port == 0 || !ports.insert(port) {
                    anyhow::bail!("Instance {} has zero or duplicate native port {port}; assign a distinct port before startup", entry.id);
                }
            }
            let registry = Arc::new(crate::config::ResourceRegistry::default());
            let web_port = if host_config.web.https_enabled {
                host_config.web.https_port
            } else {
                host_config.web.http_port
            };
            let listener_resource = registry.reserve_listener(web_port)?;
            let controllers = one_kvm_management::BrowserControllerPool::open(root).await?;
            let mut runtimes: Vec<(InstanceSpec, ApplicationRuntime)> = Vec::new();
            let mut descriptions = Vec::new();
            for entry in entries {
                let result = RuntimeBuilder::new(entry.data_dir.clone())
                    .with_host_web_config(host_config.web.clone())
                    .with_browser_controllers(controllers.clone())
                    .with_resources(registry.clone(), entry.id.to_string())
                    .build()
                    .await;
                let runtime = match result {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        for (_, runtime) in &mut runtimes {
                            ApplicationRuntime::shutdown(runtime).await;
                        }
                        if let Err(shutdown_error) = controllers.shutdown().await {
                            tracing::warn!(%shutdown_error, "Browser controller cleanup failed after instance startup error");
                        }
                        return Err(error);
                    }
                };
                descriptions.push(InstanceSummary {
                    id: entry.id.to_string(),
                    path: entry.id.web_path(),
                    native_port: runtime.config().rustdesk.direct_access_port,
                });
                runtimes.push((entry, runtime));
            }
            Ok(Self {
                runtimes,
                summaries: Arc::new(descriptions),
                controllers,
                _listener_resource: listener_resource,
                _directory_lock: directory_lock,
            })
        }

        pub fn default_runtime(&self) -> &ApplicationRuntime {
            &self
                .runtimes
                .iter()
                .find(|(entry, _)| entry.id.as_str() == DEFAULT_INSTANCE)
                .expect("invariant: default runtime always exists")
                .1
        }

        pub fn shutdown_receivers(
            &self,
        ) -> Vec<tokio::sync::broadcast::Receiver<crate::state::ShutdownAction>> {
            self.runtimes
                .iter()
                .map(|(_, runtime)| runtime.state().shutdown_tx.subscribe())
                .collect()
        }

        pub fn router(&self) -> Router {
            let mut router = Router::new()
                .route("/", get(index))
                .route("/instances", get(instance_list))
                .route("/health", get(|| async { StatusCode::OK }))
                .with_state(self.summaries.clone());
            for (entry, runtime) in &self.runtimes {
                router = mount_instance(router, &entry.id, runtime.router());
            }
            router
        }

        pub async fn shutdown(&mut self) {
            for (_, runtime) in self.runtimes.iter_mut().rev() {
                runtime.shutdown().await;
            }
            if let Err(error) = self.controllers.shutdown().await {
                tracing::error!(%error, "Browser controller pool shutdown failed");
            }
        }
    }

    fn mount_instance(router: Router, id: &InstanceId, routes: Router) -> Router {
        let path = id.web_path();
        let redirect = path.clone();
        let scope = InstanceScope(id.clone());
        let routes = routes
            .layer(middleware::from_fn_with_state(scope.clone(), scope_cookies))
            .layer(Extension(scope));
        router
            .route(
                path.trim_end_matches('/'),
                get(move || async move { Redirect::permanent(&redirect) }),
            )
            .nest(&path, routes)
    }

    async fn instance_list(
        State(instances): State<Arc<Vec<InstanceSummary>>>,
    ) -> impl IntoResponse {
        (
            [(header::CACHE_CONTROL, "no-store")],
            Json(instances.as_ref().clone()),
        )
    }

    async fn index(State(instances): State<Arc<Vec<InstanceSummary>>>) -> impl IntoResponse {
        let mut html = String::from("<!doctype html><html lang=\"zh-CN\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>One-KVM 实例</title><style>body{font:16px system-ui;max-width:680px;margin:4rem auto;padding:0 1.5rem;color:#20262b;background:#f4f5f5}ul{padding:0;list-style:none}li{margin:1rem 0}a{display:block;background:white;border:1px solid #d4dcdf;border-radius:10px;padding:1.2rem;color:#165361;text-decoration:none}p{line-height:1.7}</style><h1>One-KVM 实例</h1><p>选择实例，在其页面使用 XSCS 控制端授权码登录。每个实例分别保存绑定、设备配置和会话。</p><ul>");
        for instance in instances.iter() {
            html.push_str(&format!(
                "<li><a href=\"{}\">{}</a></li>",
                instance.path, instance.id
            ));
        }
        html.push_str("</ul><p>新增实例和绑定在设备本地通过 management 命令完成。</p></html>");
        ([(header::CACHE_CONTROL, "no-store")], Html(html))
    }

    fn host_maintenance_endpoint(path: &str, method: &Method) -> bool {
        path == "/api/system/restart"
            || path == "/api/update/upgrade"
            || path == "/api/devices/usb/reset"
            || path == "/api/terminal"
            || path.starts_with("/api/terminal/")
            || path == "/api/extensions"
            || path.starts_with("/api/extensions/")
            || (!matches!(*method, Method::GET | Method::HEAD)
                && matches!(path, "/api/config/web" | "/api/config/watchdog"))
    }

    fn session_cookie_name(scope: &InstanceScope) -> String {
        format!("{}_{}", crate::auth::SESSION_COOKIE, scope.0)
    }

    fn scope_request_cookie(headers: &mut HeaderMap, scope: &InstanceScope) {
        let cookies = CookieJar::from_headers(headers);
        let cookie = cookies
            .get(&session_cookie_name(scope))
            .map(|cookie| cookie.value().to_owned());
        headers.remove(header::COOKIE);
        if let Some(value) = cookie {
            if let Ok(value) = format!("{}={value}", crate::auth::SESSION_COOKIE).parse() {
                headers.insert(header::COOKIE, value);
            }
        }
    }

    async fn scope_cookies(
        State(scope): State<InstanceScope>,
        mut request: Request,
        next: Next,
    ) -> Response {
        let path = scope.0.web_path();
        if request
            .extensions()
            .get::<OriginalUri>()
            .is_some_and(|original| original.0.path() == path.trim_end_matches('/'))
        {
            return Redirect::permanent(&path).into_response();
        }
        if host_maintenance_endpoint(request.uri().path(), request.method()) {
            return (StatusCode::FORBIDDEN, Json(serde_json::json!({"success":false,"message":"宿主维护操作请在设备本地执行；实例授权不包含整机管理权限"}))).into_response();
        }
        scope_request_cookie(request.headers_mut(), &scope);
        let mut response = next.run(request).await;
        let values: Vec<_> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .cloned()
            .collect();
        if !values.is_empty() {
            response.headers_mut().remove(header::SET_COOKIE);
            for value in values {
                let Ok(value) = value.to_str() else {
                    continue;
                };
                let renamed;
                let value = if let Some(cookie) =
                    value.strip_prefix(&format!("{}=", crate::auth::SESSION_COOKIE))
                {
                    renamed = format!("{}={cookie}", session_cookie_name(&scope));
                    renamed.as_str()
                } else {
                    value
                };
                let scoped = value
                    .split(';')
                    .map(str::trim)
                    .filter(|part| !part.to_ascii_lowercase().starts_with("path="))
                    .collect::<Vec<_>>()
                    .join("; ");
                let scoped = format!("{scoped}; Path={}", scope.0.web_path());
                if let Ok(value) = scoped.parse() {
                    response.headers_mut().append(header::SET_COOKIE, value);
                }
            }
        }
        response
    }
    #[cfg(test)]
    mod tests {
        use super::*;

        #[tokio::test]
        async fn mounted_instance_pages_api_and_assets_keep_their_trailing_slash_routes() {
            use axum::body::{to_bytes, Body};
            use tower::ServiceExt;

            let mut router = Router::new();
            for name in ["default", "room-a"] {
                let child = Router::new()
                    .route(
                        "/",
                        get(move || async move { format!("instance page {name}") }),
                    )
                    .route("/api/management/status", get(|| async { "status" }))
                    .route("/assets/app.wasm", get(|| async { "wasm" }));
                router = mount_instance(router, &name.parse().unwrap(), child);
            }
            for name in ["default", "room-a"] {
                for (suffix, expected) in [
                    ("/", format!("instance page {name}")),
                    ("/api/management/status", "status".into()),
                    ("/assets/app.wasm", "wasm".into()),
                ] {
                    let request = Request::builder()
                        .uri(format!("/instances/{name}{suffix}"))
                        .body(Body::empty())
                        .unwrap();
                    let response = router.clone().oneshot(request).await.unwrap();
                    assert_eq!(response.status(), StatusCode::OK, "{name}{suffix}");
                    assert_eq!(
                        to_bytes(response.into_body(), 1024).await.unwrap().as_ref(),
                        expected.as_bytes()
                    );
                }
                let request = Request::builder()
                    .uri(format!("/instances/{name}"))
                    .body(Body::empty())
                    .unwrap();
                let response = router.clone().oneshot(request).await.unwrap();
                assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
                assert_eq!(
                    response.headers()[header::LOCATION],
                    format!("/instances/{name}/")
                );
            }
            let request = Request::builder()
                .uri("/instances/missing/")
                .body(Body::empty())
                .unwrap();
            assert_eq!(
                router.oneshot(request).await.unwrap().status(),
                StatusCode::NOT_FOUND
            );
        }

        #[tokio::test]
        async fn catalog_preserves_default_identity_and_assigns_distinct_native_ports() {
            let root = tempfile::tempdir().unwrap();
            let original = load_config(root.path()).await.unwrap();
            let a: InstanceId = "room-a".parse().unwrap();
            let b: InstanceId = "room-b".parse().unwrap();
            let first = create_instance(root.path(), &a, None).await.unwrap();
            let second = create_instance(root.path(), &b, None).await.unwrap();
            assert_ne!(first.native_port, original.rustdesk.direct_access_port);
            assert_ne!(second.native_port, first.native_port);
            assert_eq!(
                load_config(root.path()).await.unwrap().rustdesk.device_id,
                original.rustdesk.device_id
            );
            assert!(set_native_port(root.path(), &b, first.native_port)
                .await
                .is_err());
            assert_eq!(
                load_config(&b.data_dir(root.path()))
                    .await
                    .unwrap()
                    .rustdesk
                    .direct_access_port,
                second.native_port
            );
        }

        #[test]
        fn cookies_are_selected_by_instance_instead_of_accepting_another_instance_or_legacy_cookie()
        {
            let a = InstanceScope("room-a".parse().unwrap());
            let b = InstanceScope("room-b".parse().unwrap());
            let mut original = HeaderMap::new();
            original.insert(
                header::COOKIE,
                "one_kvm_session=legacy; one_kvm_session_room-a=alpha; one_kvm_session_room-b=beta"
                    .parse()
                    .unwrap(),
            );
            let mut headers = original.clone();
            scope_request_cookie(&mut headers, &a);
            assert_eq!(headers[header::COOKIE], "one_kvm_session=alpha");
            scope_request_cookie(&mut original, &b);
            assert_eq!(original[header::COOKIE], "one_kvm_session=beta");
            let mut unrelated = HeaderMap::new();
            unrelated.insert(
                header::COOKIE,
                "one_kvm_session=forged; one_kvm_session_room-a=alpha"
                    .parse()
                    .unwrap(),
            );
            scope_request_cookie(&mut unrelated, &b);
            assert!(!unrelated.contains_key(header::COOKIE));
        }

        #[test]
        fn instance_authorization_excludes_host_maintenance_but_keeps_device_configuration() {
            for path in [
                "/api/terminal/ws",
                "/api/extensions/ttyd/start",
                "/api/system/restart",
                "/api/devices/usb/reset",
                "/api/config/web",
            ] {
                assert!(host_maintenance_endpoint(path, &Method::PATCH), "{path}");
            }
            for path in [
                "/api/config/hid",
                "/api/config/video",
                "/api/config/audio",
                "/api/config/atx",
                "/api/config/msd",
            ] {
                assert!(!host_maintenance_endpoint(path, &Method::PATCH), "{path}");
            }
        }
    }
}

#[cfg(feature = "desktop")]
pub use host::*;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn instance_ids_cannot_escape_their_storage_or_web_paths() {
        for value in [
            "", ".", "..", "../room", "room/a", "room\\a", "room%2fa", "Room", "<script>", "-room",
        ] {
            assert!(value.parse::<InstanceId>().is_err(), "{value}");
        }
        let id: InstanceId = "room-2_a".parse().unwrap();
        assert_eq!(id.web_path(), "/instances/room-2_a/");
        assert_eq!(
            id.data_dir(Path::new("root")),
            Path::new("root/instances/room-2_a")
        );
        assert_eq!(
            InstanceId::default().data_dir(Path::new("root")),
            Path::new("root")
        );
    }
    #[test]
    fn live_host_lock_blocks_local_catalog_mutations_until_shutdown() {
        let root = std::env::temp_dir().join(format!("one-kvm-host-lock-{}", std::process::id()));
        let host = lock_host(&root).unwrap();
        assert!(lock_host(&root).is_err());
        drop(host);
        let cli = lock_host(&root).unwrap();
        drop(cli);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn discovery_keeps_legacy_root_and_separates_named_instances() {
        let root = std::env::temp_dir().join(format!("one-kvm-instances-{}", std::process::id()));
        fs::create_dir_all(root.join("instances/room-a")).unwrap();
        let entries = discover(&root).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].data_dir, root);
        assert_eq!(entries[1].data_dir, root.join("instances/room-a"));
        fs::remove_dir_all(root).unwrap();
    }
}
