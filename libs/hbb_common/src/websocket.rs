use crate::{
    protobuf::Message,
    sodiumoxide::crypto::secretbox::Key,
    tcp::{Encrypt, KxTranscript},
    tls::{get_cached_tls_accept_invalid_cert, get_cached_tls_type, upsert_tls_cache, TlsType},
    ResultType,
};
use anyhow::bail;
use async_recursion::async_recursion;
use bytes::{Bytes, BytesMut};
use futures::{SinkExt, StreamExt};
use std::{
    io::{Error, ErrorKind},
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::TcpStream,
    time::timeout,
};
use tokio_native_tls::native_tls::TlsConnector;
use tokio_tungstenite::{
    connect_async_tls_with_config,
    tungstenite::protocol::Message as WsMessage, Connector, MaybeTlsStream, WebSocketStream,
};
use tungstenite::client::IntoClientRequest;

/// tungstenite's own defaults. `set_max_packet_length` clamps to these, so `usize::MAX` puts the
/// transport back exactly where it started rather than above it.
const DEFAULT_MAX_MESSAGE_SIZE: usize = 64 << 20;
const DEFAULT_MAX_FRAME_SIZE: usize = 16 << 20;

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct WsFramedStream {
    stream: WsStream,
    addr: SocketAddr,
    encrypt: Option<Encrypt>,
    send_timeout: u64,
}

impl WsFramedStream {
    #[inline]
    fn get_connector(
        tls_type: &TlsType,
        danger_accept_invalid_certs: bool,
    ) -> ResultType<Option<Connector>> {
        match tls_type {
            TlsType::Plain => Ok(Some(Connector::Plain)),
            TlsType::NativeTls => {
                let connector = TlsConnector::builder()
                    .danger_accept_invalid_certs(danger_accept_invalid_certs)
                    .build()?;
                Ok(Some(Connector::NativeTls(connector)))
            }
            TlsType::Rustls => {
                let connector = match crate::verifier::client_config(danger_accept_invalid_certs) {
                    Ok(client_config) => Some(Connector::Rustls(Arc::new(client_config))),
                    Err(e) => {
                        log::warn!(
                            "Failed to get client config: {:?}, fallback to default connector",
                            e
                        );
                        None
                    }
                };
                Ok(connector)
            }
        }
    }

    async fn connect(
        url: &str,
        ms_timeout: u64,
    ) -> ResultType<(WsStream, SocketAddr)> {
        let tls_type = get_cached_tls_type(url);
        let is_tls_type_cached = tls_type.is_some();
        let tls_type = tls_type.unwrap_or(TlsType::Rustls);
        let danger_accept_invalid_cert = get_cached_tls_accept_invalid_cert(&url);
        Self::try_connect(
            url,
            ms_timeout,
            tls_type,
            is_tls_type_cached,
            danger_accept_invalid_cert,
            danger_accept_invalid_cert,
        )
        .await
    }

    #[async_recursion]
    async fn try_connect(
        url: &str,
        ms_timeout: u64,
        tls_type: TlsType,
        is_tls_type_cached: bool,
        danger_accept_invalid_cert: Option<bool>,
        original_danger_accept_invalid_certs: Option<bool>,
    ) -> ResultType<(WsStream, SocketAddr)> {
        let ws_config = None;
        let request = url
            .into_client_request()
            .map_err(|e| Error::new(ErrorKind::Other, e))?;
        let connector =
            Self::get_connector(&tls_type, danger_accept_invalid_cert.unwrap_or(false))?;
        let disable_nagle = false;
        let result = timeout(
            Duration::from_millis(ms_timeout),
            connect_async_tls_with_config(request, ws_config, disable_nagle, connector),
        )
        .await?
        .map(|(stream, _)| stream);
        match result {
            Ok(stream) => {
                upsert_tls_cache(url, tls_type, danger_accept_invalid_cert.unwrap_or(false));
                let addr = stream.get_ref().get_ref().local_addr()?;
                Ok((stream, addr))
            }
            Err(e) => match (tls_type, is_tls_type_cached, danger_accept_invalid_cert) {
                (TlsType::Rustls, _, None) => {
                    log::warn!(
                            "WebSocket connection with rustls-tls failed, try accept invalid certs: {}, {:?}",
                            url,
                            e
                        );
                    Self::try_connect(
                        url,
                        ms_timeout,
                        tls_type,
                        is_tls_type_cached,
                        Some(true),
                        original_danger_accept_invalid_certs,
                    )
                    .await
                }
                (TlsType::Rustls, false, Some(_)) => {
                    log::warn!(
                        "WebSocket connection with rustls-tls failed, try native-tls: {}, {:?}",
                        url,
                        e
                    );
                    Self::try_connect(
                        url,
                        ms_timeout,
                        TlsType::NativeTls,
                        is_tls_type_cached,
                        original_danger_accept_invalid_certs,
                        original_danger_accept_invalid_certs,
                    )
                    .await
                }
                (TlsType::NativeTls, _, None) => {
                    log::warn!(
                            "WebSocket connection with native-tls failed, try accept invalid certs: {}, {:?}",
                            url,
                            e
                        );
                    Self::try_connect(
                        url,
                        ms_timeout,
                        tls_type,
                        is_tls_type_cached,
                        Some(true),
                        original_danger_accept_invalid_certs,
                    )
                    .await
                }
                _ => {
                    log::error!(
                        "WebSocket connection failed with tls_type {:?}: {}, {:?}",
                        tls_type,
                        url,
                        e
                    );
                    if let tungstenite::Error::Http(response) = &e {
                        if response.status().is_redirection() {
                            bail!(
                                "WebSocket connection failed ({}). The server may not support WebSocket.",
                                e
                            )
                        }
                    }
                    bail!("WebSocket error: {}", e)
                }
            },
        }
    }

    pub async fn new<T: AsRef<str>>(url: T, ms_timeout: u64) -> ResultType<Self> {
        let (stream, addr) = Self::connect(url.as_ref(), ms_timeout).await?;

        let ws = Self {
            stream,
            addr,
            encrypt: None,
            send_timeout: ms_timeout,
        };

        Ok(ws)
    }

    #[inline]
    pub fn set_raw(&mut self) {
        self.encrypt = None;
    }

    /// Both bounds, not just the message one: `max_frame_size` refuses an oversized frame on its
    /// header, before any of it is buffered, while `max_message_size` bounds reassembly across a
    /// fragmented one. What a header on its own can allocate is `read_buffer_size`, the patched
    /// fork growing the read buffer a chunk at a time rather than to the length a frame declares.
    #[inline]
    pub fn set_max_packet_length(&mut self, n: usize) {
        let set_config = |c: &mut tungstenite::protocol::WebSocketConfig| {
            c.max_message_size = Some(n.min(DEFAULT_MAX_MESSAGE_SIZE));
            c.max_frame_size = Some(n.min(DEFAULT_MAX_FRAME_SIZE));
        };
        self.stream.set_config(set_config);
    }

    #[inline]
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    #[inline]
    pub fn set_send_timeout(&mut self, ms: u64) {
        self.send_timeout = ms;
    }

    #[inline]
    pub fn set_key(&mut self, key: Key) {
        self.encrypt = Some(Encrypt::new(key));
    }

    #[inline]
    pub fn set_key_split(
        &mut self,
        key: Key,
        is_initiator: bool,
        t: &KxTranscript,
    ) -> ResultType<()> {
        self.encrypt = Some(Encrypt::new_split(key, is_initiator, t)?);
        Ok(())
    }

    #[inline]
    pub fn is_secured(&self) -> bool {
        self.encrypt.is_some()
    }

    #[inline]
    pub async fn send(&mut self, msg: &impl Message) -> ResultType<()> {
        self.send_raw(msg.write_to_bytes()?).await
    }

    #[inline]
    pub async fn send_raw(&mut self, msg: Vec<u8>) -> ResultType<()> {
        let mut msg = msg;
        if let Some(key) = self.encrypt.as_mut() {
            msg = key.enc(&msg);
        }
        self.send_bytes(Bytes::from(msg)).await
    }

    pub async fn send_bytes(&mut self, bytes: Bytes) -> ResultType<()> {
        let msg = WsMessage::Binary(bytes);
        if self.send_timeout > 0 {
            timeout(
                Duration::from_millis(self.send_timeout),
                self.stream.send(msg),
            )
            .await??
        } else {
            self.stream.send(msg).await?
        };
        Ok(())
    }

    #[inline]
    pub async fn next(&mut self) -> Option<Result<BytesMut, Error>> {
        while let Some(msg) = self.stream.next().await {
            let msg = match msg {
                Ok(msg) => msg,
                Err(e) => {
                    log::error!("{}", e);
                    return Some(Err(Error::new(
                        ErrorKind::Other,
                        format!("WebSocket protocol error: {}", e),
                    )));
                }
            };

            match msg {
                WsMessage::Binary(data) => {
                    let mut bytes = BytesMut::from(&data[..]);
                    if let Some(key) = self.encrypt.as_mut() {
                        if let Err(err) = key.dec(&mut bytes) {
                            return Some(Err(err));
                        }
                    }
                    return Some(Ok(bytes));
                }
                WsMessage::Text(text) => {
                    if self.is_secured() {
                        return Some(Err(Error::new(
                            ErrorKind::InvalidData,
                            "WebSocket text frame received after encryption was enabled",
                        )));
                    }
                    let bytes = BytesMut::from(text.as_bytes());
                    return Some(Ok(bytes));
                }
                WsMessage::Close(_) => {
                    return None;
                }
                _ => {
                    continue;
                }
            }
        }

        None
    }

    #[inline]
    pub async fn next_timeout(&mut self, ms: u64) -> Option<Result<BytesMut, Error>> {
        match timeout(Duration::from_millis(ms), self.next()).await {
            Ok(res) => res,
            Err(_) => None,
        }
    }
}

pub fn is_ws_endpoint(endpoint: &str) -> bool {
    endpoint.starts_with("ws://") || endpoint.starts_with("wss://")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::AsyncWriteExt, net::TcpListener};
    use tungstenite::protocol::Role;

    async fn ws_from_tcp(stream: TcpStream, addr: SocketAddr) -> WsFramedStream {
        let ws_stream =
            WebSocketStream::from_raw_socket(MaybeTlsStream::Plain(stream), Role::Client, None)
                .await;

        WsFramedStream {
            stream: ws_stream,
            addr,
            encrypt: None,
            send_timeout: 0,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_secured_stream_rejects_plaintext_text() {
        sodiumoxide::init().expect("failed to initialize sodiumoxide");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut stream = WebSocketStream::from_raw_socket(tcp, Role::Server, None).await;
            stream
                .send(WsMessage::Text("plaintext".into()))
                .await
                .unwrap();
        });

        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut client = ws_from_tcp(tcp, addr).await;
        client.set_key(Key([0x42; sodiumoxide::crypto::secretbox::KEYBYTES]));

        let result = client.next().await;
        assert!(
            matches!(result, Some(Err(ref err)) if err.kind() == ErrorKind::InvalidData),
            "secured stream accepted plaintext Text frame: {:?}",
            result
        );
        server.await.unwrap();
    }

    // A server-to-client frame is unmasked, so it can be put on the wire by hand: the header alone,
    // which is all it takes to ask tungstenite for the allocation, or with its payload. `head` is
    // the first byte, FIN and opcode.
    fn ws_frame(head: u8, len: usize, with_payload: bool) -> Vec<u8> {
        let mut f = vec![head];
        if len < 126 {
            f.push(len as u8);
        } else if len <= u16::MAX as usize {
            f.push(0x7E);
            f.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            f.push(0x7F);
            f.extend_from_slice(&(len as u64).to_be_bytes());
        }
        if with_payload {
            f.resize(f.len() + len, 0xCD);
        }
        f
    }

    fn ws_binary_frame(len: usize, with_payload: bool) -> Vec<u8> {
        ws_frame(0x82, len, with_payload)
    }

    async fn ws_loopback() -> (WsFramedStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let client = TcpStream::connect(addr).await.unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let ws = ws_from_tcp(client, addr).await;
        (ws, server)
    }

    #[tokio::test]
    async fn max_packet_length_refuses_a_frame_on_its_header() {
        const CAP: usize = 16 * 1024;
        let (mut ws, mut server) = ws_loopback().await;
        ws.set_max_packet_length(CAP);

        // One byte over, header only: refused before there is any payload to buffer.
        server
            .write_all(&ws_binary_frame(CAP + 1, false))
            .await
            .unwrap();
        match timeout(Duration::from_secs(5), ws.next()).await {
            Ok(Some(Err(e))) => assert!(e.to_string().contains("Message too long"), "{}", e),
            Ok(other) => panic!(
                "expected a refusal, got {:?}",
                other.map(|r| r.map(|b| b.len()))
            ),
            Err(_) => panic!("next() waited for a payload the header should have refused"),
        }
    }

    #[tokio::test]
    async fn max_packet_length_lowered_then_restored() {
        const CAP: usize = 16 * 1024;
        let (mut ws, mut server) = ws_loopback().await;

        ws.set_max_packet_length(CAP);
        server
            .write_all(&ws_binary_frame(8 * 1024, true))
            .await
            .unwrap();
        let got = ws.next().await.unwrap().unwrap();
        assert_eq!(got.len(), 8 * 1024, "a message under the cap still arrives");

        ws.set_max_packet_length(usize::MAX);
        server
            .write_all(&ws_binary_frame(200_000, true))
            .await
            .unwrap();
        let got = timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("next() hung after the cap was lifted")
            .unwrap()
            .unwrap();
        assert_eq!(
            got.len(),
            200_000,
            "lifting the cap lets a large message through again"
        );
    }

    // Two frames each under the cap that reassemble to a message over it: the frame bound lets
    // both through, so this is what the message bound alone refuses.
    #[tokio::test]
    async fn max_packet_length_bounds_fragmented_message() {
        const CAP: usize = 16 * 1024;
        let (mut ws, mut server) = ws_loopback().await;
        ws.set_max_packet_length(CAP);

        // Binary with FIN clear, then a continuation with FIN set: 12 KiB each, 24 KiB together.
        server
            .write_all(&ws_frame(0x02, 12 * 1024, true))
            .await
            .unwrap();
        server
            .write_all(&ws_frame(0x80, 12 * 1024, true))
            .await
            .unwrap();
        match timeout(Duration::from_secs(5), ws.next()).await {
            Ok(Some(Err(e))) => assert!(e.to_string().contains("Message too long"), "{}", e),
            Ok(other) => panic!(
                "expected a refusal, got {:?}",
                other.map(|r| r.map(|b| b.len()))
            ),
            Err(_) => panic!("next() hung on a fragmented message over the cap"),
        }
    }
}
