use futures::{SinkExt, StreamExt};
use hbb_common::{
    config::{keys, APP_NAME, OVERWRITE_SETTINGS},
    socket_client,
    tcp::DynTcpStream,
    tls::{upsert_tls_cache, TlsType},
    Stream,
};
use rustls_pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use std::{sync::Arc, time::Duration};
use tokio::{net::TcpListener, time::timeout};
use tokio_rustls::{rustls, TlsAcceptor};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

const TIMEOUT: u64 = 5_000;
const PAYLOAD: &[u8] = b"RustDesk direct websocket test";

fn tls_identity() -> (TlsAcceptor, CertificateDer<'static>) {
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
    let cert = cert.der().clone();
    let key = PrivatePkcs8KeyDer::from(key_pair.serialize_der());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert.clone()], key.into())
    .unwrap();
    (TlsAcceptor::from(Arc::new(config)), cert)
}

async fn echo(stream: DynTcpStream, path: &str, host: &str) {
    let mut ws =
        tokio_tungstenite::accept_hdr_async(stream, |request: &Request, response: Response| {
            assert_eq!(request.uri().to_string(), path);
            assert_eq!(request.headers()["host"], host);
            assert!(!request.headers().contains_key("proxy-authorization"));
            Ok(response)
        })
        .await
        .unwrap();
    let message = ws.next().await.unwrap().unwrap();
    assert!(message.is_binary());
    assert_eq!(message.into_data().as_ref(), PAYLOAD);
    ws.send(PAYLOAD.to_vec().into()).await.unwrap();
    ws.close(None).await.unwrap();
}

async fn exchange(mut stream: Stream) {
    stream.send_bytes(PAYLOAD.to_vec().into()).await.unwrap();
    let reply = stream.next_timeout(TIMEOUT).await.unwrap().unwrap();
    assert_eq!(reply.as_ref(), PAYLOAD);
    if matches!(&stream, Stream::WebSocket(_)) {
        assert!(timeout(Duration::from_millis(TIMEOUT), stream.next())
            .await
            .unwrap()
            .is_none());
    }
}

async fn direct_round_trip(wss: bool) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("{}://{addr}/ws/id", if wss { "wss" } else { "ws" });
    upsert_tls_cache(&url, TlsType::Rustls, true);
    let server = tokio::spawn(async move {
        let (stream, client_addr) = listener.accept().await.unwrap();
        let mut stream = DynTcpStream(Box::new(stream));
        if wss {
            stream = DynTcpStream(Box::new(tls_identity().0.accept(stream).await.unwrap()));
        }
        echo(stream, "/ws/id", &addr.to_string()).await;
        client_addr
    });
    let stream = socket_client::connect_tcp(url, TIMEOUT).await.unwrap();
    let local_addr = stream.local_addr();
    exchange(stream).await;
    let client_addr = timeout(Duration::from_millis(TIMEOUT), server)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(local_addr, client_addr);
}

#[tokio::test]
async fn direct_websocket_connections() {
    *APP_NAME.write().unwrap() = format!("RustDesk Direct WebSocket Test {}", std::process::id());
    OVERWRITE_SETTINGS
        .write()
        .unwrap()
        .insert(keys::OPTION_ALLOW_INSECURE_TLS_FALLBACK.into(), "Y".into());
    for wss in [false, true] {
        direct_round_trip(wss).await;
    }
}

#[tokio::test]
async fn direct_wss_rejects_untrusted_certificate() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("wss://{}/", listener.local_addr().unwrap());
    upsert_tls_cache(&url, TlsType::Rustls, false);
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        assert!(tls_identity().0.accept(stream).await.is_err());
    });
    assert!(socket_client::connect_tcp(url, TIMEOUT).await.is_err());
    timeout(Duration::from_millis(TIMEOUT), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn direct_wss_accepts_explicitly_trusted_certificate() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("wss://{addr}/");
    let (acceptor, cert) = tls_identity();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let stream = acceptor.accept(stream).await.unwrap();
        echo(DynTcpStream(Box::new(stream)), "/", &addr.to_string()).await;
    });
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).unwrap();
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    let (mut stream, _) = timeout(
        Duration::from_millis(TIMEOUT),
        tokio_tungstenite::connect_async_tls_with_config(
            &url,
            None,
            false,
            Some(tokio_tungstenite::Connector::Rustls(Arc::new(config))),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    stream.send(PAYLOAD.to_vec().into()).await.unwrap();
    let reply = timeout(Duration::from_millis(TIMEOUT), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(reply.into_data().as_ref(), PAYLOAD);
    timeout(Duration::from_millis(TIMEOUT), server)
        .await
        .unwrap()
        .unwrap();
}
