use hbb_common::{
    rand::{rngs::OsRng, RngCore},
    thiserror,
};
use reqwest::{
    header::{HeaderValue, AUTHORIZATION},
    redirect::Policy,
    Client, Response, StatusCode,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::time::Duration;
use url::Url;
use uuid::Uuid;
use xscc::protocol::{self, Binding};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EnrollmentError {
    #[error("invalid management server or enrollment input")]
    Configuration,
    #[error("protected management storage unavailable")]
    Storage,
    #[error("a binding already exists for this server; recover it first")]
    ExistingBinding,
    #[error("no pending binding exists for this server")]
    MissingBinding,
    #[error("management server unavailable")]
    Unavailable,
    #[error("management request rejected")]
    Rejected,
    #[error("management credential rejected")]
    Unauthorized,
    #[error("management response does not match the saved binding")]
    BindingMismatch,
    #[error("invalid management response")]
    Protocol,
}

type Result<T> = std::result::Result<T, EnrollmentError>;

/// A single HTTPS origin is also the protected-store namespace. Paths and URL
/// credentials are rejected instead of silently changing the server selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerOrigin(Url);

impl ServerOrigin {
    pub fn parse(value: &str) -> Result<Self> {
        let url = Url::parse(value).map_err(|_| EnrollmentError::Configuration)?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(EnrollmentError::Configuration);
        }
        Ok(Self(url))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    fn endpoint(&self, path: &str) -> Url {
        let mut url = self.0.clone();
        url.set_path(path);
        url
    }

    pub fn connection_url(&self) -> Result<Url> {
        let mut url = self.endpoint(protocol::CONNECT_PATH);
        url.set_scheme("wss")
            .map_err(|_| EnrollmentError::Configuration)?;
        Ok(url)
    }
}

/// No Debug implementation: the independent credential belongs only in protected
/// service storage. The one-time authorization code is deliberately absent.
pub struct SavedBinding {
    server: ServerOrigin,
    binding: Binding,
    credential: Zeroizing<String>,
}

impl SavedBinding {
    pub fn restore(
        server: ServerOrigin,
        binding: Binding,
        credential: Zeroizing<String>,
    ) -> Result<Self> {
        if binding.manager_id.is_nil()
            || binding.device_id.is_nil()
            || binding.installation_id.is_nil()
            || credential.len() != 64
            || !credential
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(EnrollmentError::Configuration);
        }
        Ok(Self {
            server,
            binding,
            credential,
        })
    }

    pub fn server(&self) -> &ServerOrigin {
        &self.server
    }
    pub fn binding(&self) -> &Binding {
        &self.binding
    }
    /// Only platform protected-storage implementations should persist this value.
    pub fn credential(&self) -> &str {
        self.credential.as_str()
    }

    pub fn connection(&self) -> Result<xscc::transport::ManagerConnection> {
        xscc::transport::ManagerConnection::new(
            self.server.connection_url()?.as_str(),
            Zeroizing::new(self.credential.to_string()),
        )
        .map_err(|_| EnrollmentError::Configuration)
    }
}

/// Implement in the actual host service context, with an exclusive installation
/// namespace. create must atomically fail if present and durably commit before
/// returning; mark_enrolled must durably retain the same identity and credential.
/// Never implement this using GUI preferences or world-readable config files.
#[async_trait::async_trait]
pub trait BindingStore: Send + Sync {
    async fn load(&self, server: &ServerOrigin) -> Result<Option<SavedBinding>>;
    async fn create(&self, pending: &SavedBinding) -> Result<()>;
    async fn mark_enrolled(&self, saved: &SavedBinding) -> Result<()>;
}

pub struct EnrollmentClient {
    http: Client,
    server: ServerOrigin,
}

impl EnrollmentClient {
    pub fn new(server: ServerOrigin) -> Result<Self> {
        let http = Client::builder()
            .https_only(true)
            .use_rustls_tls()
            .redirect(Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|_| EnrollmentError::Configuration)?;
        Ok(Self { http, server })
    }

    /// installation_id must come from base::managed_identity in the service's
    /// existing shared installation directory, never a GUI/session-generated ID.
    pub async fn enroll_new<S: BindingStore>(
        &self,
        store: &S,
        installation_id: Uuid,
        authorization_code: &str,
    ) -> Result<Binding> {
        if installation_id.is_nil() || !protocol::is_valid_authorization_code(authorization_code) {
            return Err(EnrollmentError::Configuration);
        }
        if store.load(&self.server).await?.is_some() {
            return Err(EnrollmentError::ExistingBinding);
        }
        let pairing = self.pairing(authorization_code).await?;
        let mut random = Zeroizing::new([0u8; 32]);
        OsRng
            .try_fill_bytes(random.as_mut())
            .map_err(|_| EnrollmentError::Unavailable)?;
        let saved = SavedBinding::restore(
            self.server.clone(),
            Binding {
                manager_id: pairing.manager_id,
                device_id: pairing.device_id,
                installation_id,
            },
            Zeroizing::new(hex::encode(random.as_ref())),
        )?;
        store.create(&saved).await?;
        self.submit(store, &saved, authorization_code).await
    }

    /// First action after an ambiguous enrollment result, including a restart.
    /// Failure never deletes the original credential or creates another identity.
    pub async fn recover<S: BindingStore>(&self, store: &S) -> Result<Binding> {
        let saved = self.load(store).await?;
        let binding = self.identity(&saved).await?;
        verify_binding(&saved.binding, &binding)?;
        store.mark_enrolled(&saved).await?;
        Ok(binding)
    }

    /// Retry only the same saved transaction. Identity recovery always precedes
    /// reusing an authorization code; a mismatched pairing cannot replace it.
    pub async fn retry_enrollment<S: BindingStore>(
        &self,
        store: &S,
        authorization_code: &str,
    ) -> Result<Binding> {
        if !protocol::is_valid_authorization_code(authorization_code) {
            return Err(EnrollmentError::Configuration);
        }
        let saved = self.load(store).await?;
        match self.identity(&saved).await {
            Ok(binding) => {
                verify_binding(&saved.binding, &binding)?;
                store.mark_enrolled(&saved).await?;
                return Ok(binding);
            }
            Err(EnrollmentError::Unauthorized) => {}
            Err(error) => return Err(error),
        }
        let pairing = self.pairing(authorization_code).await?;
        if pairing.manager_id != saved.binding.manager_id
            || pairing.device_id != saved.binding.device_id
        {
            return Err(EnrollmentError::BindingMismatch);
        }
        self.submit(store, &saved, authorization_code).await
    }

    async fn load<S: BindingStore>(&self, store: &S) -> Result<SavedBinding> {
        let saved = store
            .load(&self.server)
            .await?
            .ok_or(EnrollmentError::MissingBinding)?;
        if saved.server != self.server {
            return Err(EnrollmentError::BindingMismatch);
        }
        Ok(saved)
    }

    async fn pairing(&self, code: &str) -> Result<Pairing> {
        let response = self
            .http
            .post(self.server.endpoint(protocol::PAIRING_PATH))
            .json(&PairingRequest {
                authorization_code: code,
            })
            .send()
            .await
            .map_err(|_| EnrollmentError::Unavailable)?;
        let pairing: Pairing = decode(response).await?;
        if pairing.manager_id.is_nil() || pairing.device_id.is_nil() {
            return Err(EnrollmentError::Protocol);
        }
        Ok(pairing)
    }

    async fn identity(&self, saved: &SavedBinding) -> Result<Binding> {
        let raw = Zeroizing::new(format!("Bearer {}", saved.credential()));
        let mut header =
            HeaderValue::from_str(raw.as_str()).map_err(|_| EnrollmentError::Configuration)?;
        header.set_sensitive(true);
        let response = self
            .http
            .get(self.server.endpoint(protocol::IDENTITY_PATH))
            .header(AUTHORIZATION, header)
            .send()
            .await
            .map_err(|_| EnrollmentError::Unavailable)?;
        decode(response).await
    }

    async fn submit<S: BindingStore>(
        &self,
        store: &S,
        saved: &SavedBinding,
        code: &str,
    ) -> Result<Binding> {
        let response = self
            .http
            .post(self.server.endpoint(protocol::ENROLL_PATH))
            .json(&EnrollmentRequest {
                device_id: saved.binding.device_id,
                installation_id: saved.binding.installation_id,
                token: code,
                credential: saved.credential(),
            })
            .send()
            .await
            .map_err(|_| EnrollmentError::Unavailable)?;
        let binding: Binding = decode(response).await?;
        verify_binding(&saved.binding, &binding)?;
        store.mark_enrolled(saved).await?;
        Ok(binding)
    }
}

// These HTTP envelopes are server-private structs, not exported by xscc::protocol.
#[derive(Serialize)]
struct PairingRequest<'a> {
    authorization_code: &'a str,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pairing {
    manager_id: Uuid,
    device_id: Uuid,
}
#[derive(Serialize)]
struct EnrollmentRequest<'a> {
    device_id: Uuid,
    installation_id: Uuid,
    token: &'a str,
    credential: &'a str,
}

fn verify_binding(expected: &Binding, actual: &Binding) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(EnrollmentError::BindingMismatch)
    }
}

async fn decode<T: DeserializeOwned>(mut response: Response) -> Result<T> {
    if response.status() == StatusCode::UNAUTHORIZED
        && response
            .headers()
            .get("x-error-code")
            .is_some_and(|v| v.as_bytes() == b"unauthorized")
    {
        return Err(EnrollmentError::Unauthorized);
    }
    if !response.status().is_success() {
        return Err(EnrollmentError::Rejected);
    }
    // Do not expose server error bodies, URLs or request objects in errors/logs.
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| EnrollmentError::Unavailable)?
    {
        if bytes.len().saturating_add(chunk.len()) > protocol::MAX_MESSAGE_BYTES {
            return Err(EnrollmentError::Protocol);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| EnrollmentError::Protocol)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RefusingStore(bool);

    #[async_trait::async_trait]
    impl BindingStore for RefusingStore {
        async fn load(&self, server: &ServerOrigin) -> Result<Option<SavedBinding>> {
            if self.0 {
                return Err(EnrollmentError::Storage);
            }
            SavedBinding::restore(
                server.clone(),
                Binding {
                    manager_id: Uuid::new_v4(),
                    device_id: Uuid::new_v4(),
                    installation_id: Uuid::new_v4(),
                },
                Zeroizing::new("a".repeat(64)),
            )
            .map(Some)
        }
        async fn create(&self, _: &SavedBinding) -> Result<()> {
            panic!("an unreadable or existing binding must never be replaced")
        }
        async fn mark_enrolled(&self, _: &SavedBinding) -> Result<()> {
            panic!("no enrollment should have been attempted")
        }
    }

    #[tokio::test]
    async fn existing_or_unreadable_binding_prevents_network_and_identity_rotation() {
        let client =
            EnrollmentClient::new(ServerOrigin::parse("https://example.invalid").unwrap()).unwrap();
        for (store, expected) in [
            (RefusingStore(true), EnrollmentError::Storage),
            (RefusingStore(false), EnrollmentError::ExistingBinding),
        ] {
            assert_eq!(
                client
                    .enroll_new(&store, Uuid::new_v4(), &"a".repeat(36))
                    .await,
                Err(expected)
            );
        }
    }

    #[test]
    fn server_origin_requires_https_without_ambient_url_state() {
        for value in [
            "http://example.com",
            "https://user:secret@example.com",
            "https://example.com/path",
            "https://example.com/?token=x",
            "https://example.com/#fragment",
        ] {
            assert_eq!(
                ServerOrigin::parse(value),
                Err(EnrollmentError::Configuration)
            );
        }
        let server = ServerOrigin::parse("https://EXAMPLE.com:443").unwrap();
        assert_eq!(server.as_str(), "https://example.com/");
        assert_eq!(
            server.connection_url().unwrap().as_str(),
            "wss://example.com/xscc/v2/connect"
        );
    }

    #[test]
    fn saved_identity_rejects_invalid_credentials_and_cross_binding_response() {
        let binding = Binding {
            manager_id: Uuid::new_v4(),
            device_id: Uuid::new_v4(),
            installation_id: Uuid::new_v4(),
        };
        let server = ServerOrigin::parse("https://example.com").unwrap();
        for value in ["a".repeat(63), "A".repeat(64), "g".repeat(64)] {
            assert!(
                SavedBinding::restore(server.clone(), binding.clone(), Zeroizing::new(value))
                    .is_err()
            );
        }
        let saved =
            SavedBinding::restore(server, binding.clone(), Zeroizing::new("a".repeat(64))).unwrap();
        let connection = saved.connection().unwrap();
        let access = connection.access_policy();
        assert!(access.borrow().is_none());
        let mut other = binding.clone();
        other.installation_id = Uuid::new_v4();
        assert_eq!(
            verify_binding(&binding, &other),
            Err(EnrollmentError::BindingMismatch)
        );
    }
}
