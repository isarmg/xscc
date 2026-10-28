//! Experimental, opt-in transport. TLS and the pinned host identity establish a
//! channel, not management permission. No production listener uses this module.
use anyhow::{anyhow, bail, Context, Result};
use bytes::Bytes;
use hbb_common::{
    futures::{SinkExt, StreamExt},
    sodiumoxide::crypto::sign,
    tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec},
};
use quinn::{Connection, Endpoint, RecvStream, SendStream, TransportConfig, VarInt};
pub use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    RootCertStore,
};
use std::{convert::TryFrom, future::Future, net::SocketAddr, sync::Arc, time::Duration};
use tokio::time::{interval, timeout, MissedTickBehavior};

const ALPN: &[u8] = b"xscc-quic-experimental/1";
const EXPORTER: &[u8] = b"EXPORTER-xscc-managed-quic-v1";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const AUTH_POLL: Duration = Duration::from_millis(100);
const AUTH_LIMIT: usize = 16 * 1024;
pub const MAX_RELIABLE_MESSAGE: usize = 64 * 1024;

/// Bind the existing management proof to this connection (including its host
/// identity). These values are public context, never credentials or permission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelBinding {
    pub exporter: [u8; 32],
    pub host_identity: [u8; 32],
}

/// Implement in the application using its existing management permit. `check`
/// must verify this exact binding and current local permit validity/revocation.
/// It is checked before/after operations and at most every 100ms during blocked
/// IO; it must not make network requests. Do not
/// implement it as TLS success, a source-IP allowlist, or an unconditional grant.
pub trait Authorization: Send + Sync + 'static {
    fn check(&self, channel: &ChannelBinding) -> Result<()>;
}

fn transport() -> Arc<TransportConfig> {
    let mut config = TransportConfig::default();
    config.max_concurrent_bidi_streams(VarInt::from_u32(8));
    config.max_concurrent_uni_streams(VarInt::from_u32(0));
    config.stream_receive_window(VarInt::from_u32(256 * 1024));
    config.receive_window(VarInt::from_u32(2 * 1024 * 1024));
    config.datagram_receive_buffer_size(Some(256 * 1024));
    config.datagram_send_buffer_size(256 * 1024);
    config.max_idle_timeout(Some(VarInt::from_u32(15_000).into()));
    Arc::new(config)
}

/// Explicit trust roots may include a privately provisioned/self-signed host
/// certificate. The supplied server name must match its SAN. No trust-on-first-
/// use or certificate-verification bypass is provided.
pub fn client_config(roots: rustls::RootCertStore) -> Result<quinn::ClientConfig> {
    let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])?
    .with_root_certificates(roots)
    .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.enable_early_data = false;
    let mut config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(tls)?,
    ));
    config.transport_config(transport());
    Ok(config)
}

pub fn server_config(
    certificates: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<quinn::ServerConfig> {
    let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])?
    .with_no_client_auth()
    .with_single_cert(certificates, key)?;
    tls.alpn_protocols = vec![ALPN.to_vec()];
    tls.max_early_data_size = 0;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(tls)?,
    ));
    // Admission and the application IP whitelist are tied to the initial peer
    // address, as on TCP. Do not silently expand them through path migration.
    config.migration(false);
    config.transport_config(transport());
    Ok(config)
}

pub struct Client {
    endpoint: Endpoint,
}
impl Client {
    pub fn bind(address: SocketAddr, roots: rustls::RootCertStore) -> Result<Self> {
        let mut endpoint = Endpoint::client(address)?;
        endpoint.set_default_client_config(client_config(roots)?);
        Ok(Self { endpoint })
    }

    /// A nonempty pre-provisioned device identity is mandatory. Errors are not
    /// classified as permission to downgrade to another transport.
    pub async fn connect(
        &self,
        address: SocketAddr,
        server_name: &str,
        pinned_host: [u8; 32],
    ) -> Result<PendingSession> {
        timeout(HANDSHAKE_TIMEOUT, async {
            let connection = self.endpoint.connect(address, server_name)?.await?;
            let result = async {
                let exporter = exporter(&connection)?;
                let (send, recv) = connection.accept_bi().await?;
                let (send, mut recv) = framed(send, recv, AUTH_LIMIT);
                let proof = read_frame(&mut recv).await?;
                let signature = sign::Signature::from_bytes(&proof)
                    .map_err(|_| anyhow!("invalid QUIC identity signature"))?;
                if !sign::verify_detached(&signature, &exporter, &sign::PublicKey(pinned_host)) {
                    bail!("QUIC device identity mismatch");
                }
                Ok(PendingSession {
                    endpoint: self.endpoint.clone(),
                    connection: connection.clone(),
                    binding: ChannelBinding {
                        exporter,
                        host_identity: pinned_host,
                    },
                    send,
                    recv,
                })
            }
            .await;
            if result.is_err() {
                connection.close(1u32.into(), b"identity rejected");
            }
            result
        })
        .await
        .context("QUIC handshake timed out")?
    }
}

pub struct Listener {
    endpoint: Endpoint,
    identity: sign::SecretKey,
}
impl Listener {
    pub fn bind(
        address: SocketAddr,
        certificates: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
        identity: sign::SecretKey,
    ) -> Result<Self> {
        Ok(Self {
            endpoint: Endpoint::server(server_config(certificates, key)?, address)?,
            identity,
        })
    }
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.endpoint.local_addr()?)
    }
    /// Stop admission and close existing QUIC connections when the host service stops.
    pub fn close(&self) {
        self.endpoint.close(0u32.into(), b"listener stopped");
    }

    /// Obtain the source address and reserve application admission before TLS or
    /// identity work. The application owns the admission guard across handshake,
    /// management authorization and its normal login deadline.
    pub async fn incoming(&self) -> Option<Incoming> {
        self.endpoint.accept().await.map(|incoming| Incoming {
            incoming,
            endpoint: self.endpoint.clone(),
            identity: self.identity.clone(),
        })
    }

    /// Convenience for experiments/tests only: production listeners must call
    /// `incoming`, enforce admission and spawn each bounded `handshake` separately.
    pub async fn accept(&self) -> Result<PendingSession> {
        self.incoming()
            .await
            .context("QUIC listener closed")?
            .handshake()
            .await
    }
}

pub struct Incoming {
    incoming: quinn::Incoming,
    endpoint: Endpoint,
    identity: sign::SecretKey,
}
impl Incoming {
    pub fn remote_address(&self) -> SocketAddr {
        self.incoming.remote_address()
    }
    /// Refuse before allocating application state or performing the handshake.
    pub fn refuse(self) {
        self.incoming.refuse();
    }
    pub async fn handshake(self) -> Result<PendingSession> {
        timeout(HANDSHAKE_TIMEOUT, async {
            let connection = self.incoming.await?;
            let exporter = exporter(&connection)?;
            let (send, recv) = connection.open_bi().await?;
            let (mut send, recv) = framed(send, recv, AUTH_LIMIT);
            let proof = sign::sign_detached(&exporter, &self.identity);
            write_frame(&mut send, proof.as_ref(), AUTH_LIMIT).await?;
            Ok(PendingSession {
                endpoint: self.endpoint,
                connection,
                binding: ChannelBinding {
                    exporter,
                    host_identity: self.identity.public_key().0,
                },
                send,
                recv,
            })
        })
        .await
        .context("QUIC handshake timed out")?
    }
}

fn exporter(connection: &Connection) -> Result<[u8; 32]> {
    let mut bytes = [0; 32];
    connection
        .export_keying_material(&mut bytes, EXPORTER, ALPN)
        .map_err(|_| anyhow!("QUIC channel binding unavailable"))?;
    Ok(bytes)
}

/// Only bounded management handshake messages are available before permission.
/// The application must enforce its existing admission limits and login deadline.
pub struct PendingSession {
    endpoint: Endpoint,
    connection: Connection,
    binding: ChannelBinding,
    send: FramedSend,
    recv: FramedRecv,
}
impl PendingSession {
    pub fn binding(&self) -> &ChannelBinding {
        &self.binding
    }
    pub async fn send_auth(&mut self, message: &[u8]) -> Result<()> {
        let result = timeout(
            HANDSHAKE_TIMEOUT,
            write_frame(&mut self.send, message, AUTH_LIMIT),
        )
        .await
        .context("QUIC authorization write timed out")
        .and_then(|result| result);
        if result.is_err() {
            self.close();
        }
        result
    }
    pub async fn recv_auth(&mut self) -> Result<Bytes> {
        let result = timeout(HANDSHAKE_TIMEOUT, read_frame(&mut self.recv))
            .await
            .context("QUIC authorization read timed out")
            .and_then(|result| result);
        if result.is_err() {
            self.close();
        }
        result
    }
    pub fn authorize(self, authorization: Arc<dyn Authorization>) -> Result<Session> {
        if let Err(error) = authorization.check(&self.binding) {
            self.connection
                .close(2u32.into(), b"authorization rejected");
            return Err(error);
        }
        Ok(Session {
            _endpoint: self.endpoint,
            connection: self.connection,
            binding: self.binding,
            authorization,
            // The authentication stream remains distinct from application lanes.
            _auth: (self.send, self.recv),
        })
    }
    pub fn close(&self) {
        self.connection
            .close(2u32.into(), b"authorization rejected");
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Lane {
    Control = 1,
    Input = 2,
    Clipboard = 3,
    File = 4,
}
impl Lane {
    fn decode(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::Control),
            2 => Ok(Self::Input),
            3 => Ok(Self::Clipboard),
            4 => Ok(Self::File),
            _ => bail!("unknown QUIC reliable lane"),
        }
    }
}

pub struct Session {
    _endpoint: Endpoint,
    connection: Connection,
    binding: ChannelBinding,
    authorization: Arc<dyn Authorization>,
    _auth: (FramedSend, FramedRecv),
}
impl Session {
    /// Recheck the same local management authority before delivering buffered application data.
    pub fn ensure_authorized(&self) -> Result<()> {
        self.check()
    }

    fn check(&self) -> Result<()> {
        check(&self.connection, &self.binding, self.authorization.as_ref())
    }
    pub async fn open_lane(&self, lane: Lane) -> Result<ReliableLane> {
        checked_io(
            &self.connection,
            &self.binding,
            self.authorization.as_ref(),
            async {
                let (mut send, recv) = self.connection.open_bi().await?;
                send.write_all(&[lane as u8]).await?;
                Ok(self.lane(lane, send, recv))
            },
        )
        .await
    }
    pub async fn accept_lane(&self) -> Result<ReliableLane> {
        checked_io(
            &self.connection,
            &self.binding,
            self.authorization.as_ref(),
            async {
                let (send, mut recv) = self.connection.accept_bi().await?;
                let mut kind = [0];
                recv.read_exact(&mut kind).await?;
                Ok(self.lane(Lane::decode(kind[0])?, send, recv))
            },
        )
        .await
    }
    fn lane(&self, kind: Lane, send: SendStream, recv: RecvStream) -> ReliableLane {
        let (send, recv) = framed(send, recv, MAX_RELIABLE_MESSAGE);
        ReliableLane {
            kind,
            send,
            recv,
            connection: self.connection.clone(),
            binding: self.binding.clone(),
            authorization: self.authorization.clone(),
        }
    }
    pub fn max_datagram_size(&self) -> Option<usize> {
        self.connection.max_datagram_size()
    }
    pub fn send_media(&self, bytes: Bytes) -> Result<()> {
        self.check()?;
        self.connection.send_datagram(bytes)?;
        Ok(())
    }
    /// Backpressure-aware media send for a dedicated bounded writer task. Unlike
    /// `send_media`, a full local queue does not evict older queued fragments.
    pub async fn send_media_wait(&self, bytes: Bytes) -> Result<()> {
        checked_io(
            &self.connection,
            &self.binding,
            self.authorization.as_ref(),
            async { Ok(self.connection.send_datagram_wait(bytes).await?) },
        )
        .await
    }
    pub async fn recv_media(&self) -> Result<Bytes> {
        checked_io(
            &self.connection,
            &self.binding,
            self.authorization.as_ref(),
            async { Ok(self.connection.read_datagram().await?) },
        )
        .await
    }
    /// Wait for the peer/transport to close, without initiating a local close.
    pub async fn closed(&self) {
        self.connection.closed().await;
    }
    pub fn close(&self) {
        self.connection.close(0u32.into(), b"session closed");
    }
}

pub struct ReliableLane {
    kind: Lane,
    send: FramedSend,
    recv: FramedRecv,
    connection: Connection,
    binding: ChannelBinding,
    authorization: Arc<dyn Authorization>,
}
impl ReliableLane {
    pub fn kind(&self) -> Lane {
        self.kind
    }
    pub async fn send(&mut self, bytes: &[u8]) -> Result<()> {
        checked_io(
            &self.connection,
            &self.binding,
            self.authorization.as_ref(),
            write_frame(&mut self.send, bytes, MAX_RELIABLE_MESSAGE),
        )
        .await
    }
    /// End this reliable direction and wait for transport acknowledgement. This
    /// does not prove application processing; keep the connection alive until
    /// the application peer closes or a bounded shutdown deadline expires.
    pub async fn finish(&mut self) -> Result<()> {
        let send = &mut self.send;
        checked_io(
            &self.connection,
            &self.binding,
            self.authorization.as_ref(),
            async {
                SinkExt::<Bytes>::flush(send).await?;
                send.get_mut().finish()?;
                if send.get_ref().stopped().await?.is_some() {
                    bail!("QUIC reliable direction stopped before acknowledgement");
                }
                Ok(())
            },
        )
        .await
    }
    pub async fn recv(&mut self) -> Result<Bytes> {
        checked_io(
            &self.connection,
            &self.binding,
            self.authorization.as_ref(),
            read_frame(&mut self.recv),
        )
        .await
    }
}

fn check(
    connection: &Connection,
    binding: &ChannelBinding,
    auth: &dyn Authorization,
) -> Result<()> {
    if let Err(error) = auth.check(binding) {
        connection.close(2u32.into(), b"authorization expired");
        return Err(error);
    }
    Ok(())
}
// Poll only the local lease while backpressure or a silent peer stalls IO.
// Bytes already admitted to QUIC before revocation cannot be recalled.
async fn checked_io<T>(
    connection: &Connection,
    binding: &ChannelBinding,
    auth: &dyn Authorization,
    operation: impl Future<Output = Result<T>>,
) -> Result<T> {
    check(connection, binding, auth)?;
    let mut tick = interval(AUTH_POLL);
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    tokio::pin!(operation);
    loop {
        tokio::select! {
            biased;
            _ = tick.tick() => check(connection, binding, auth)?,
            result = &mut operation => {
                check(connection, binding, auth)?;
                return result;
            }
        }
    }
}

type FramedSend = FramedWrite<SendStream, LengthDelimitedCodec>;
type FramedRecv = FramedRead<RecvStream, LengthDelimitedCodec>;
fn framed(send: SendStream, recv: RecvStream, limit: usize) -> (FramedSend, FramedRecv) {
    let codec = || {
        LengthDelimitedCodec::builder()
            .max_frame_length(limit)
            .new_codec()
    };
    (
        FramedWrite::new(send, codec()),
        FramedRead::new(recv, codec()),
    )
}
async fn write_frame(send: &mut FramedSend, bytes: &[u8], limit: usize) -> Result<()> {
    if bytes.len() > limit {
        bail!("QUIC reliable message exceeds limit");
    }
    send.send(Bytes::copy_from_slice(bytes)).await?;
    Ok(())
}
async fn read_frame(recv: &mut FramedRecv) -> Result<Bytes> {
    match recv.next().await.context("QUIC reliable lane closed")? {
        Ok(bytes) => Ok(bytes.freeze()),
        Err(error) => {
            // A malformed length must not be interpreted as a subsequent frame.
            let _ = recv.get_mut().stop(3u32.into());
            Err(error.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Lease {
        binding: ChannelBinding,
        valid: AtomicBool,
    }
    impl Authorization for Lease {
        fn check(&self, binding: &ChannelBinding) -> Result<()> {
            if !self.valid.load(Ordering::Acquire) || binding != &self.binding {
                bail!("test permit denied");
            }
            Ok(())
        }
    }
    fn lease(binding: &ChannelBinding, valid: bool) -> Arc<Lease> {
        Arc::new(Lease {
            binding: binding.clone(),
            valid: AtomicBool::new(valid),
        })
    }
    fn endpoints() -> (Client, Listener, [u8; 32]) {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.cert.der().clone()).unwrap();
        let (public, secret) = sign::gen_keypair();
        let listener = Listener::bind(
            "127.0.0.1:0".parse().unwrap(),
            vec![cert.cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
            secret,
        )
        .unwrap();
        let client = Client::bind("127.0.0.1:0".parse().unwrap(), roots).unwrap();
        (client, listener, public.0)
    }
    async fn pair(
        client: &Client,
        host: &Listener,
        key: [u8; 32],
    ) -> (PendingSession, PendingSession) {
        let (client, host) = tokio::join!(
            client.connect(host.local_addr().unwrap(), "localhost", key),
            host.accept()
        );
        (client.unwrap(), host.unwrap())
    }

    #[tokio::test]
    async fn pinned_identity_and_tls_trust_are_both_required() {
        let (client, host, _) = endpoints();
        let (wrong_key, _) = sign::gen_keypair();
        let (client_result, host_result) = tokio::join!(
            client.connect(host.local_addr().unwrap(), "localhost", wrong_key.0),
            host.accept()
        );
        assert!(client_result.is_err());
        drop(host_result);
        let untrusted = Client::bind(
            "127.0.0.1:0".parse().unwrap(),
            rustls::RootCertStore::empty(),
        )
        .unwrap();
        let (client_result, host_result) = tokio::join!(
            untrusted.connect(host.local_addr().unwrap(), "localhost", wrong_key.0),
            host.accept()
        );
        assert!(client_result.is_err());
        assert!(host_result.is_err());
    }

    #[tokio::test]
    async fn management_gate_binds_permission_to_exact_connection() {
        let (client, host, key) = endpoints();
        let (mut a, mut b) = pair(&client, &host, key).await;
        assert_eq!(a.binding(), b.binding());
        a.send_auth(b"existing management proof").await.unwrap();
        assert_eq!(
            b.recv_auth().await.unwrap(),
            b"existing management proof"[..]
        );
        let permit = lease(a.binding(), true);
        let (other, _) = pair(&client, &host, key).await;
        assert_ne!(a.binding().exporter, other.binding().exporter);
        assert!(other.authorize(permit).is_err());
        let denied = lease(a.binding(), false);
        assert!(a.authorize(denied).is_err());
    }

    #[tokio::test]
    async fn reliable_lanes_datagrams_limits_and_revocation() {
        let (client, host, key) = endpoints();
        let (a, b) = pair(&client, &host, key).await;
        let client_lease = lease(a.binding(), true);
        let server_lease = lease(b.binding(), true);
        let a = a.authorize(client_lease.clone()).unwrap();
        let b = b.authorize(server_lease.clone()).unwrap();
        let mut input = a.open_lane(Lane::Input).await.unwrap();
        input.send(b"input").await.unwrap();
        let mut file = a.open_lane(Lane::File).await.unwrap();
        file.send(b"file").await.unwrap();
        let mut received_input = b.accept_lane().await.unwrap();
        let mut received_file = b.accept_lane().await.unwrap();
        assert_eq!(received_input.kind(), Lane::Input);
        assert_eq!(received_file.kind(), Lane::File);
        assert_eq!(received_file.recv().await.unwrap(), b"file"[..]);
        assert_eq!(received_input.recv().await.unwrap(), b"input"[..]);
        assert!(input
            .send(&vec![0; MAX_RELIABLE_MESSAGE + 1])
            .await
            .is_err());
        input.send(b"still framed").await.unwrap();
        assert_eq!(received_input.recv().await.unwrap(), b"still framed"[..]);
        // Cancellation after a partial header must preserve framing state.
        input.send.get_mut().write_all(&[0, 0]).await.unwrap();
        assert!(timeout(Duration::from_millis(20), received_input.recv())
            .await
            .is_err());
        input
            .send
            .get_mut()
            .write_all(&[0, 3, b'o', b'k', b'!'])
            .await
            .unwrap();
        assert_eq!(received_input.recv().await.unwrap(), b"ok!"[..]);
        a.send_media(Bytes::from_static(b"media fragment")).unwrap();
        assert_eq!(b.recv_media().await.unwrap(), b"media fragment"[..]);
        let too_large = a.max_datagram_size().unwrap() + 1;
        assert!(a.send_media(vec![0; too_large].into()).is_err());
        client_lease.valid.store(false, Ordering::Release);
        assert!(input.send(b"revoked input").await.is_err());
        assert!(a.send_media(Bytes::from_static(b"revoked media")).is_err());
        assert!(a.open_lane(Lane::Control).await.is_err());
        server_lease.valid.store(false, Ordering::Release);
        assert!(received_file.recv().await.is_err());
        assert!(b.recv_media().await.is_err());
    }
    #[tokio::test]
    async fn revocation_interrupts_blocked_reads_and_flow_controlled_writes() {
        let (client, host, key) = endpoints();
        let (a, b) = pair(&client, &host, key).await;
        let client_lease = lease(a.binding(), true);
        let server_lease = lease(b.binding(), true);
        let a = a.authorize(client_lease.clone()).unwrap();
        let b = b.authorize(server_lease.clone()).unwrap();
        let mut file = a.open_lane(Lane::File).await.unwrap();
        let _unread = b.accept_lane().await.unwrap();
        let writer = tokio::spawn(async move {
            let _session = a;
            let payload = vec![0; MAX_RELIABLE_MESSAGE];
            for _ in 0..1024 {
                file.send(&payload).await?;
            }
            Ok::<_, anyhow::Error>(())
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            !writer.is_finished(),
            "write must be flow-controlled before revocation"
        );
        client_lease.valid.store(false, Ordering::Release);
        let error = timeout(Duration::from_secs(2), writer)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.to_string(), "test permit denied");
        drop(b);

        let (a, b) = pair(&client, &host, key).await;
        let server_lease = lease(b.binding(), true);
        let _a = a.authorize(lease(b.binding(), true)).unwrap();
        let b = b.authorize(server_lease.clone()).unwrap();
        let revoke = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            server_lease.valid.store(false, Ordering::Release);
        };
        let (read, _) = tokio::join!(timeout(Duration::from_secs(2), b.recv_media()), revoke);
        assert_eq!(read.unwrap().unwrap_err().to_string(), "test permit denied");
    }
    #[tokio::test]
    async fn admission_can_refuse_before_identity_handshake() {
        let (client, host, key) = endpoints();
        let address = host.local_addr().unwrap();
        let attempt = tokio::spawn(async move { client.connect(address, "localhost", key).await });
        let incoming = host.incoming().await.unwrap();
        assert!(incoming.remote_address().ip().is_loopback());
        assert_ne!(incoming.remote_address().port(), 0);
        incoming.refuse();
        assert!(attempt.await.unwrap().is_err());
        let (client, host, key) = endpoints();
        let (a, b) = pair(&client, &host, key).await;
        let a_auth = lease(a.binding(), true);
        let b_auth = lease(b.binding(), true);
        let a = a.authorize(a_auth).unwrap();
        let b = b.authorize(b_auth).unwrap();
        host.close();
        assert!(host.incoming().await.is_none());
        assert!(timeout(Duration::from_secs(2), a.recv_media())
            .await
            .unwrap()
            .is_err());
        assert!(timeout(Duration::from_secs(2), b.recv_media())
            .await
            .unwrap()
            .is_err());
    }
}
