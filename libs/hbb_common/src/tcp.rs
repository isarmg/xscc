use crate::{bail, bytes_codec::BytesCodec, ResultType};
use anyhow::Context as AnyhowCtx;
use bytes::{Buf, Bytes, BytesMut};
use futures::{SinkExt, StreamExt};
use protobuf::Message;
use sodiumoxide::crypto::{
    box_, generichash,
    secretbox::{self, Key, Nonce},
};
use std::{
    io::{self, Error, ErrorKind},
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    ops::{Deref, DerefMut},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf, ReadHalf, WriteHalf},
    net::{lookup_host, TcpListener, TcpSocket, ToSocketAddrs},
};
use tokio_util::codec::{Framed, FramedParts};

pub trait TcpStreamTrait: AsyncRead + AsyncWrite + Unpin {}
pub struct DynTcpStream(pub Box<dyn TcpStreamTrait + Send + Sync>);

/// The newest key exchange version this build speaks. Version 0 is the original scheme, and
/// what an absent field means: one key for both directions. Version 1 splits the exchanged key
/// into one per direction, the way Noise splits its cipher states after the handshake, binds
/// the handshake transcript into both, and keeps the nonce layout.
pub const KX_VERSION_LATEST: u32 = 1;

/// Prefix of what the server signs as `KxParams`. The same key signs other messages, and a
/// protobuf carries no type of its own, so the prefix is what keeps them apart.
pub const KX_PARAMS_DOMAIN: &[u8] = b"rdkx-params";

/// The version to run against a peer that advertised `advertised`: the highest both sides
/// support.
#[inline]
pub fn kx_version_for(advertised: u32) -> u32 {
    advertised.min(KX_VERSION_LATEST)
}

const KX_SPLIT_CONTEXT: &[u8] = b"rdkx-spl";
const KX_SPLIT_INITIATOR: u8 = 1;
const KX_SPLIT_RESPONDER: u8 = 2;

/// What both sides saw during the key exchange, each from its own side of the wire, mixed
/// into the split keys.
pub struct KxTranscript<'a> {
    /// The ephemeral public key of the side that sent the sealed key.
    pub initiator_pk: &'a [u8],
    /// The ephemeral public key the sealed key was sealed to.
    pub responder_pk: &'a [u8],
    /// The version the responder advertised: what it sent, what the initiator received.
    pub advertised: u32,
    /// The version the initiator picked.
    pub picked: u32,
}

/// The sending key, the send and receive counters, and the receiving key where it differs.
#[derive(Clone)]
pub struct Encrypt(pub Key, pub u64, pub u64, Option<Key>);

pub struct FramedStream(
    pub Framed<DynTcpStream, BytesCodec>,
    pub SocketAddr,
    pub Option<Encrypt>,
    pub u64,
);

/// Read half of a framed connection after its key exchange has completed.
pub struct FramedStreamReader {
    stream: Framed<ReadHalf<DynTcpStream>, BytesCodec>,
    local_addr: SocketAddr,
    encrypt: Option<Encrypt>,
}

/// The single owner of the connection's send order and encryption sequence.
pub struct FramedStreamWriter {
    stream: Framed<WriteHalf<DynTcpStream>, BytesCodec>,
    local_addr: SocketAddr,
    encrypt: Option<Encrypt>,
    send_timeout: u64,
}

impl Deref for FramedStream {
    type Target = Framed<DynTcpStream, BytesCodec>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for FramedStream {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Deref for DynTcpStream {
    type Target = Box<dyn TcpStreamTrait + Send + Sync>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for DynTcpStream {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

pub(crate) fn new_socket(
    addr: std::net::SocketAddr,
    reuse: bool,
) -> Result<TcpSocket, std::io::Error> {
    let socket = match addr {
        std::net::SocketAddr::V4(..) => TcpSocket::new_v4()?,
        std::net::SocketAddr::V6(..) => TcpSocket::new_v6()?,
    };
    if reuse {
        // windows has no reuse_port, but its reuse_address
        // almost equals to unix's reuse_port + reuse_address,
        // though may introduce nondeterministic behavior
        // illumos has no support for SO_REUSEPORT
        #[cfg(all(unix, not(target_os = "illumos")))]
        socket.set_reuseport(true).ok();
        socket.set_reuseaddr(true).ok();
    }
    socket.bind(addr)?;
    Ok(socket)
}

impl FramedStream {
    pub async fn new<T: ToSocketAddrs + std::fmt::Display>(
        remote_addr: T,
        local_addr: Option<SocketAddr>,
        ms_timeout: u64,
    ) -> ResultType<Self> {
        for remote_addr in lookup_host(&remote_addr).await? {
            let local = if let Some(addr) = local_addr {
                addr
            } else {
                crate::config::Config::get_any_listen_addr(remote_addr.is_ipv4())
            };
            if let Ok(socket) = new_socket(local, true) {
                if let Ok(Ok(stream)) =
                    super::timeout(ms_timeout, socket.connect(remote_addr)).await
                {
                    stream.set_nodelay(true).ok();
                    let addr = stream.local_addr()?;
                    return Ok(Self(
                        Framed::new(DynTcpStream(Box::new(stream)), BytesCodec::new()),
                        addr,
                        None,
                        0,
                    ));
                }
            }
        }
        bail!(format!("Failed to connect to {remote_addr}"));
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.1
    }

    pub fn set_send_timeout(&mut self, ms: u64) {
        self.3 = ms;
    }

    pub fn from(stream: impl TcpStreamTrait + Send + Sync + 'static, addr: SocketAddr) -> Self {
        Self(
            Framed::new(DynTcpStream(Box::new(stream)), BytesCodec::new()),
            addr,
            None,
            0,
        )
    }

    pub fn set_raw(&mut self) {
        self.0.codec_mut().set_raw();
        self.2 = None;
    }

    pub fn is_secured(&self) -> bool {
        self.2.is_some()
    }

    #[inline]
    pub async fn send(&mut self, msg: &impl Message) -> ResultType<()> {
        let bytes = match self.2.as_mut() {
            Some(key) => key.enc_message(msg)?,
            None => msg.write_to_bytes()?,
        };
        self.send_bytes(Bytes::from(bytes)).await
    }

    #[inline]
    pub async fn send_raw(&mut self, msg: Vec<u8>) -> ResultType<()> {
        let mut msg = msg;
        if let Some(key) = self.2.as_mut() {
            msg = key.enc_owned(msg);
        }
        self.send_bytes(bytes::Bytes::from(msg)).await?;
        Ok(())
    }

    #[inline]
    pub async fn send_bytes(&mut self, bytes: Bytes) -> ResultType<()> {
        if self.3 > 0 {
            super::timeout(self.3, self.0.send(bytes)).await??;
        } else {
            self.0.send(bytes).await?;
        }
        Ok(())
    }

    #[inline]
    pub async fn next(&mut self) -> Option<Result<BytesMut, Error>> {
        let mut res = self.0.next().await;
        if let Some(Ok(bytes)) = res.as_mut() {
            if let Some(key) = self.2.as_mut() {
                if let Err(err) = key.dec(bytes) {
                    return Some(Err(err));
                }
            }
        }
        res
    }

    #[inline]
    pub async fn next_timeout(&mut self, ms: u64) -> Option<Result<BytesMut, Error>> {
        if let Ok(res) = super::timeout(ms, self.next()).await {
            res
        } else {
            None
        }
    }

    /// Split only after negotiation: moving both buffers and the decoder state also preserves
    /// any frame that arrived together with the last handshake message.
    pub fn into_split(self) -> (FramedStreamReader, FramedStreamWriter) {
        let boundary = self.0.backpressure_boundary();
        let parts = self.0.into_parts();
        let (read, write) = tokio::io::split(parts.io);
        let mut read_parts = FramedParts::new(read, parts.codec);
        read_parts.read_buf = parts.read_buf;
        let reader = Framed::from_parts(read_parts);
        let mut write_parts = FramedParts::new(write, parts.codec);
        write_parts.write_buf = parts.write_buf;
        let mut writer = Framed::from_parts(write_parts);
        writer.set_backpressure_boundary(boundary);
        (
            FramedStreamReader {
                stream: reader,
                local_addr: self.1,
                encrypt: self.2.clone(),
            },
            FramedStreamWriter {
                stream: writer,
                local_addr: self.1,
                encrypt: self.2,
                send_timeout: self.3,
            },
        )
    }

    pub fn set_key(&mut self, key: Key) {
        self.2 = Some(Encrypt::new(key));
    }

    pub fn set_key_split(
        &mut self,
        key: Key,
        is_initiator: bool,
        t: &KxTranscript,
    ) -> ResultType<()> {
        self.2 = Some(Encrypt::new_split(key, is_initiator, t)?);
        Ok(())
    }

    fn get_nonce(seqnum: u64) -> Nonce {
        let mut nonce = Nonce([0u8; secretbox::NONCEBYTES]);
        nonce.0[..std::mem::size_of_val(&seqnum)].copy_from_slice(&seqnum.to_le_bytes());
        nonce
    }
}

impl FramedStreamReader {
    pub async fn next(&mut self) -> Option<Result<BytesMut, Error>> {
        let mut result = self.stream.next().await;
        if let Some(Ok(bytes)) = &mut result {
            if let Some(key) = self.encrypt.as_mut() {
                if let Err(err) = key.dec(bytes) {
                    return Some(Err(err));
                }
            }
        }
        result
    }

    pub async fn next_timeout(&mut self, ms: u64) -> Option<Result<BytesMut, Error>> {
        crate::timeout(ms, self.next()).await.unwrap_or_default()
    }

    pub fn set_max_packet_length(&mut self, n: usize) {
        self.stream.codec_mut().set_max_packet_length(n);
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn is_secured(&self) -> bool {
        self.encrypt.is_some()
    }
}

impl FramedStreamWriter {
    pub async fn send(&mut self, msg: &impl Message) -> ResultType<()> {
        let bytes = match self.encrypt.as_mut() {
            Some(key) => key.enc_message(msg)?,
            None => msg.write_to_bytes()?,
        };
        self.send_bytes(Bytes::from(bytes)).await
    }

    pub async fn send_raw(&mut self, bytes: Vec<u8>) -> ResultType<()> {
        let bytes = match self.encrypt.as_mut() {
            Some(key) => key.enc_owned(bytes),
            None => bytes,
        };
        self.send_bytes(Bytes::from(bytes)).await
    }

    /// As on FramedStream, these bytes are already sealed, or belong to a raw connection.
    pub async fn send_bytes(&mut self, bytes: Bytes) -> ResultType<()> {
        if self.send_timeout > 0 {
            crate::timeout(self.send_timeout, self.stream.send(bytes)).await??;
        } else {
            self.stream.send(bytes).await?;
        }
        Ok(())
    }

    pub async fn close(&mut self) -> ResultType<()> {
        if self.send_timeout > 0 {
            crate::timeout(self.send_timeout, self.stream.close()).await??;
        } else {
            self.stream.close().await?;
        }
        Ok(())
    }

    pub fn set_send_timeout(&mut self, ms: u64) {
        self.send_timeout = ms;
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn is_secured(&self) -> bool {
        self.encrypt.is_some()
    }
}

const DEFAULT_BACKLOG: u32 = 128;

pub async fn new_listener<T: ToSocketAddrs>(addr: T, reuse: bool) -> ResultType<TcpListener> {
    if !reuse {
        Ok(TcpListener::bind(addr).await?)
    } else {
        let addr = lookup_host(&addr)
            .await?
            .next()
            .context("could not resolve to any address")?;
        new_socket(addr, true)?
            .listen(DEFAULT_BACKLOG)
            .map_err(anyhow::Error::msg)
    }
}

pub async fn listen_any(port: u16) -> ResultType<TcpListener> {
    if let Ok(mut socket) = TcpSocket::new_v6() {
        #[cfg(unix)]
        {
            // illumos has no support for SO_REUSEPORT
            #[cfg(not(target_os = "illumos"))]
            socket.set_reuseport(true).ok();
            socket.set_reuseaddr(true).ok();
            use std::os::unix::io::{FromRawFd, IntoRawFd};
            let raw_fd = socket.into_raw_fd();
            let sock2 = unsafe { socket2::Socket::from_raw_fd(raw_fd) };
            sock2.set_only_v6(false).ok();
            socket = unsafe { TcpSocket::from_raw_fd(sock2.into_raw_fd()) };
        }
        #[cfg(windows)]
        {
            use std::os::windows::prelude::{FromRawSocket, IntoRawSocket};
            let raw_socket = socket.into_raw_socket();
            let sock2 = unsafe { socket2::Socket::from_raw_socket(raw_socket) };
            sock2.set_only_v6(false).ok();
            socket = unsafe { TcpSocket::from_raw_socket(sock2.into_raw_socket()) };
        }
        if socket
            .bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port))
            .is_ok()
        {
            if let Ok(l) = socket.listen(DEFAULT_BACKLOG) {
                return Ok(l);
            }
        }
    }
    Ok(new_socket(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port),
        true,
    )?
    .listen(DEFAULT_BACKLOG)?)
}

impl Unpin for DynTcpStream {}

impl AsyncRead for DynTcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        AsyncRead::poll_read(Pin::new(&mut self.0), cx, buf)
    }
}

impl AsyncWrite for DynTcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.0), cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.0), cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.0), cx)
    }
}

impl<R: AsyncRead + AsyncWrite + Unpin> TcpStreamTrait for R {}

impl Encrypt {
    pub fn new(key: Key) -> Self {
        Self(key, 0, 0, None)
    }

    /// Version 1: the initiator, the side that sent the sealed key, sends under one subkey and
    /// receives under the other; the responder the reverse.
    pub fn new_split(key: Key, is_initiator: bool, t: &KxTranscript) -> ResultType<Self> {
        let initiator = Self::derive_subkey(&key, KX_SPLIT_INITIATOR, t)?;
        let responder = Self::derive_subkey(&key, KX_SPLIT_RESPONDER, t)?;
        Ok(if is_initiator {
            Self(initiator, 0, 0, Some(responder))
        } else {
            Self(responder, 0, 0, Some(initiator))
        })
    }

    fn derive_subkey(key: &Key, direction: u8, t: &KxTranscript) -> ResultType<Key> {
        if t.initiator_pk.len() != box_::PUBLICKEYBYTES
            || t.responder_pk.len() != box_::PUBLICKEYBYTES
        {
            bail!("key exchange transcript has a public key of the wrong length");
        }
        let failed = |_| anyhow::anyhow!("key derivation failed");
        let mut h =
            generichash::State::new(Some(secretbox::KEYBYTES), Some(&key.0)).map_err(failed)?;
        for part in [
            KX_SPLIT_CONTEXT,
            &[direction],
            &t.advertised.to_le_bytes(),
            &t.picked.to_le_bytes(),
            t.initiator_pk,
            t.responder_pk,
        ] {
            h.update(part).map_err(failed)?;
        }
        let mut subkey = [0u8; secretbox::KEYBYTES];
        subkey.copy_from_slice(h.finalize().map_err(failed)?.as_ref());
        Ok(Key(subkey))
    }

    pub fn dec(&mut self, bytes: &mut BytesMut) -> Result<(), Error> {
        if bytes.len() <= 1 {
            return Ok(());
        }
        self.2 += 1;
        let nonce = FramedStream::get_nonce(self.2);
        let error = || Error::new(ErrorKind::Other, "decryption error");
        if bytes.len() < secretbox::MACBYTES {
            return Err(error());
        }
        let tag = secretbox::Tag::from_slice(&bytes[..secretbox::MACBYTES]).ok_or_else(error)?;
        secretbox::open_detached(
            &mut bytes[secretbox::MACBYTES..],
            &tag,
            &nonce,
            self.3.as_ref().unwrap_or(&self.0),
        )
        .map_err(|()| error())?;
        bytes.advance(secretbox::MACBYTES);
        Ok(())
    }

    pub fn enc(&mut self, data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(secretbox::MACBYTES + data.len());
        bytes.resize(secretbox::MACBYTES, 0);
        bytes.extend_from_slice(data);
        self.enc_prefixed(&mut bytes);
        bytes
    }

    pub fn enc_owned(&mut self, mut bytes: Vec<u8>) -> Vec<u8> {
        let len = bytes.len();
        bytes.resize(len + secretbox::MACBYTES, 0);
        bytes.copy_within(..len, secretbox::MACBYTES);
        self.enc_prefixed(&mut bytes);
        bytes
    }

    fn enc_message(&mut self, msg: &impl Message) -> ResultType<Vec<u8>> {
        msg.check_initialized()?;
        let mut bytes = Vec::with_capacity(secretbox::MACBYTES + msg.compute_size() as usize);
        bytes.resize(secretbox::MACBYTES, 0);
        {
            let mut output = protobuf::CodedOutputStream::vec(&mut bytes);
            msg.write_to_with_cached_sizes(&mut output)?;
            output.flush()?;
        }
        self.enc_prefixed(&mut bytes);
        Ok(bytes)
    }

    fn enc_prefixed(&mut self, bytes: &mut [u8]) {
        self.1 += 1;
        let nonce = FramedStream::get_nonce(self.1);
        let tag = secretbox::seal_detached(&mut bytes[secretbox::MACBYTES..], &nonce, &self.0);
        bytes[..secretbox::MACBYTES].copy_from_slice(tag.as_ref());
    }

    pub fn decode(
        symmetric_data: &[u8],
        their_pk_b: &[u8],
        our_sk_b: &box_::SecretKey,
    ) -> ResultType<Key> {
        if their_pk_b.len() != box_::PUBLICKEYBYTES {
            anyhow::bail!("Handshake failed: pk length {}", their_pk_b.len());
        }
        let nonce = box_::Nonce([0u8; box_::NONCEBYTES]);
        let mut pk_ = [0u8; box_::PUBLICKEYBYTES];
        pk_[..].copy_from_slice(their_pk_b);
        let their_pk_b = box_::PublicKey(pk_);
        let symmetric_key = box_::open(symmetric_data, &nonce, &their_pk_b, &our_sk_b)
            .map_err(|_| anyhow::anyhow!("Handshake failed: box decryption failure"))?;
        if symmetric_key.len() != secretbox::KEYBYTES {
            anyhow::bail!("Handshake failed: invalid secret key length from peer");
        }
        let mut key = [0u8; secretbox::KEYBYTES];
        key[..].copy_from_slice(&symmetric_key);
        Ok(Key(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INITIATOR_PK: [u8; 32] = [1u8; 32];
    const RESPONDER_PK: [u8; 32] = [2u8; 32];

    fn transcript(advertised: u32) -> KxTranscript<'static> {
        KxTranscript {
            initiator_pk: &INITIATOR_PK,
            responder_pk: &RESPONDER_PK,
            advertised,
            picked: KX_VERSION_LATEST,
        }
    }

    fn seal_and_open(a: &mut Encrypt, b: &mut Encrypt, msg: &[u8]) -> Vec<u8> {
        let sealed = a.enc(msg);
        let mut buf = BytesMut::from(&sealed[..]);
        b.dec(&mut buf).unwrap();
        assert_eq!(&buf[..], msg);
        sealed
    }

    #[test]
    fn in_place_crypto_matches_released_frames_and_reuses_storage() {
        let key = Key([0x41; secretbox::KEYBYTES]);
        let mut sender = Encrypt::new(key.clone());
        let mut receiver = Encrypt::new(key.clone());
        for (sequence, len) in [0, 1, 1024, 64 * 1024, 1024 * 1024].iter().copied().enumerate() {
            let plaintext = vec![0x7b; len];
            let expected = secretbox::seal(
                &plaintext,
                &FramedStream::get_nonce(sequence as u64 + 1),
                &key,
            );
            let mut owned = Vec::with_capacity(len + secretbox::MACBYTES);
            owned.extend_from_slice(&plaintext);
            let allocation = owned.as_ptr();
            let sealed = sender.enc_owned(owned);
            assert_eq!(sealed.as_ptr(), allocation);
            assert_eq!(sealed, expected);
            let mut frame = BytesMut::from(sealed.as_slice());
            let allocation = frame.as_ptr();
            receiver.dec(&mut frame).unwrap();
            assert_eq!(frame.as_ptr(), unsafe {
                allocation.add(secretbox::MACBYTES)
            });
            assert_eq!(frame.as_ref(), plaintext);
        }
    }

    #[test]
    fn in_place_crypto_rejects_tampering_and_short_ciphertexts() {
        let key = Key([0x42; secretbox::KEYBYTES]);
        let mut sender = Encrypt::new(key.clone());
        let mut receiver = Encrypt::new(key);
        let mut frame = BytesMut::from(sender.enc(b"authenticated").as_slice());
        frame[secretbox::MACBYTES] ^= 1;
        let before = frame.clone();
        assert!(receiver.dec(&mut frame).is_err());
        assert_eq!(frame, before);
        assert!(receiver.dec(&mut BytesMut::from(&b"short"[..])).is_err());
    }

    fn duplex_connection(stream: tokio::io::DuplexStream) -> FramedStream {
        FramedStream(
            Framed::new(DynTcpStream(Box::new(stream)), BytesCodec::new()),
            "127.0.0.1:0".parse().unwrap(),
            None,
            0,
        )
    }

    #[tokio::test]
    async fn split_read_progresses_while_video_write_is_blocked() {
        let (local, remote) = tokio::io::duplex(64);
        let (mut reader, mut writer) = duplex_connection(local).into_split();
        let mut peer = duplex_connection(remote);
        let writing = tokio::spawn(async move { writer.send_raw(vec![0x7b; 1024 * 1024]).await });
        peer.send_raw(b"input-ack".to_vec()).await.unwrap();
        let received = crate::timeout(1000, reader.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(received.as_ref(), b"input-ack");
        assert!(
            !writing.is_finished(),
            "the peer has not drained the video frame"
        );
        writing.abort();
    }

    #[tokio::test]
    async fn split_preserves_partial_decoder_and_both_encryption_sequences() {
        use tokio_util::codec::{Decoder, Encoder};
        let (local, remote) = tokio::io::duplex(1024);
        let mut stream = duplex_connection(local);
        let mut peer = duplex_connection(remote);
        let key = Key([0x43; secretbox::KEYBYTES]);
        stream.set_key(key.clone());
        peer.set_key(key);
        stream.send_raw(b"before split".to_vec()).await.unwrap();
        assert_eq!(
            peer.next_timeout(1000).await.unwrap().unwrap().as_ref(),
            b"before split"
        );
        peer.send_raw(b"received before split".to_vec())
            .await
            .unwrap();
        assert_eq!(
            stream.next_timeout(1000).await.unwrap().unwrap().as_ref(),
            b"received before split"
        );

        let encrypted = peer.2.as_mut().unwrap().enc(b"buffered during handshake");
        let mut buffered = BytesMut::new();
        let mut codec = BytesCodec::new();
        codec.encode(Bytes::from(encrypted), &mut buffered).unwrap();
        let mut prefix = buffered.split_to(3);
        assert!(stream.codec_mut().decode(&mut prefix).unwrap().is_none());
        prefix.extend_from_slice(&buffered);
        *stream.read_buffer_mut() = prefix;

        // An already-buffered write must be sent before a later writer call.
        let encrypted = stream.2.as_mut().unwrap().enc(b"pending write");
        codec
            .encode(Bytes::from(encrypted), stream.write_buffer_mut())
            .unwrap();
        let (mut reader, mut writer) = stream.into_split();
        assert_eq!(
            reader.next_timeout(1000).await.unwrap().unwrap().as_ref(),
            b"buffered during handshake"
        );
        writer.send_raw(b"after split".to_vec()).await.unwrap();
        assert_eq!(
            peer.next_timeout(1000).await.unwrap().unwrap().as_ref(),
            b"pending write"
        );
        assert_eq!(peer.next_timeout(1000).await.unwrap().unwrap().as_ref(), b"after split");
        peer.send_raw(b"input after split".to_vec()).await.unwrap();
        assert_eq!(
            reader.next_timeout(1000).await.unwrap().unwrap().as_ref(),
            b"input after split"
        );
    }

    #[tokio::test]
    async fn split_reader_keeps_negotiated_packet_cap() {
        let (local, remote) = tokio::io::duplex(64);
        let mut stream = duplex_connection(local);
        stream.codec_mut().set_max_packet_length(4);
        let (mut reader, _writer) = stream.into_split();
        let mut peer = duplex_connection(remote);
        peer.send_raw(vec![0; 5]).await.unwrap();
        assert_eq!(
            reader.next().await.unwrap().unwrap_err().kind(),
            ErrorKind::InvalidData
        );
    }

    #[test]
    #[ignore = "manual transport throughput measurement"]
    fn benchmark_in_place_crypto() {
        use std::{hint::black_box, time::Instant};
        const COUNT: usize = 512;
        let payload = vec![0x7b; 1024 * 1024];
        let key = Key([0x44; secretbox::KEYBYTES]);
        let start = Instant::now();
        for sequence in 1..=COUNT {
            let serialized = payload.clone();
            let sealed =
                secretbox::seal(&serialized, &FramedStream::get_nonce(sequence as u64), &key);
            let mut bytes = BytesMut::from(sealed.as_slice());
            let opened =
                secretbox::open(&bytes, &FramedStream::get_nonce(sequence as u64), &key).unwrap();
            bytes.clear();
            bytes.extend_from_slice(&opened);
            black_box(bytes);
        }
        let legacy = start.elapsed();
        let start = Instant::now();
        let mut sender = Encrypt::new(key.clone());
        let mut receiver = Encrypt::new(key);
        for _ in 0..COUNT {
            let sealed = sender.enc_owned(payload.clone());
            let mut bytes = BytesMut::from(sealed.as_slice());
            receiver.dec(&mut bytes).unwrap();
            black_box(bytes);
        }
        let in_place = start.elapsed();
        eprintln!(
            "crypto_roundtrip_1mib count={COUNT} legacy_ms={} in_place_ms={} ratio={:.3}",
            legacy.as_millis(),
            in_place.as_millis(),
            in_place.as_secs_f64() / legacy.as_secs_f64()
        );
    }

    #[test]
    fn test_v0_shares_one_key_across_directions() {
        let key = secretbox::gen_key();
        let (mut initiator, mut responder) = (Encrypt::new(key.clone()), Encrypt::new(key));
        let sent = seal_and_open(&mut initiator, &mut responder, b"hello");
        let back = seal_and_open(&mut responder, &mut initiator, b"hello");
        assert_eq!(sent, back);
    }

    // Fixed bytes: the version 0 frame is what released peers send, and the subkeys are the ones
    // hbbs's own copy of this code pins, so a change here breaks the wire, not just this test.
    #[test]
    fn test_wire_vectors() {
        let hex = |b: &[u8]| b.iter().map(|b| format!("{:02x}", b)).collect::<String>();
        let key = Key([0x11u8; 32]);
        assert_eq!(
            hex(&Encrypt::new(key.clone()).enc(b"hello")),
            "3b768f827fdcc4af555b9e42533be6611e2c333481"
        );
        let mut t = KxTranscript {
            initiator_pk: &[0x22u8; 32],
            responder_pk: &[0x33u8; 32],
            advertised: 1,
            picked: 1,
        };
        let Encrypt(send, _, _, recv) = Encrypt::new_split(key.clone(), true, &t).unwrap();
        assert_eq!(
            hex(&send.0),
            "c189ec6e1935c0751cfc85a0f075405cae507d7925bb19687caf56cbb54e0ecf"
        );
        assert_eq!(
            hex(&recv.unwrap().0),
            "8a62648e9195cb10ea900c0a24d2a166c1e982347a2dc6833c3e756bf5715d36"
        );
        // With the two versions equal, swapping their order in the derivation would go unseen.
        t.advertised = 3;
        let Encrypt(send, _, _, recv) = Encrypt::new_split(key, true, &t).unwrap();
        assert_eq!(
            hex(&send.0),
            "25b0778ae600f9731c7fa457d1cc5d40071b0aa3d34c90b53e1d5344b0e7123a"
        );
        assert_eq!(
            hex(&recv.unwrap().0),
            "5bbde0ac6a708d4cd1ff807125d79cb6030bde62f7a66b6cc1a79e31bd657756"
        );
    }

    #[test]
    fn test_split_directions_use_different_keys() {
        let key = secretbox::gen_key();
        let t = transcript(KX_VERSION_LATEST);
        let mut initiator = Encrypt::new_split(key.clone(), true, &t).unwrap();
        let mut responder = Encrypt::new_split(key.clone(), false, &t).unwrap();
        let sent = seal_and_open(&mut initiator, &mut responder, b"hello");
        let back = seal_and_open(&mut responder, &mut initiator, b"hello");
        assert_ne!(sent, back);
        // Neither direction's key is the exchanged key itself, so a version 0 peer cannot be
        // paired with a split one by accident.
        let mut v0 = Encrypt::new(key);
        let mut buf = BytesMut::from(&sent[..]);
        assert!(v0.dec(&mut buf).is_err());
    }

    #[test]
    fn test_split_roles_must_differ() {
        let key = secretbox::gen_key();
        let t = transcript(KX_VERSION_LATEST);
        let mut a = Encrypt::new_split(key.clone(), true, &t).unwrap();
        let mut b = Encrypt::new_split(key, true, &t).unwrap();
        let sealed = a.enc(b"hello");
        let mut buf = BytesMut::from(&sealed[..]);
        assert!(b.dec(&mut buf).is_err());
    }

    #[test]
    fn test_kx_version_for_picks_the_highest_shared() {
        assert_eq!(kx_version_for(0), 0);
        assert_eq!(kx_version_for(1), 1);
        assert_eq!(kx_version_for(KX_VERSION_LATEST), KX_VERSION_LATEST);
        assert_eq!(kx_version_for(KX_VERSION_LATEST + 7), KX_VERSION_LATEST);
    }

    #[test]
    fn test_split_binds_the_transcript() {
        let key = secretbox::gen_key();
        // The two sides put different advertisements into the transcript.
        let mut initiator =
            Encrypt::new_split(key.clone(), true, &transcript(KX_VERSION_LATEST + 1)).unwrap();
        let mut responder = Encrypt::new_split(key, false, &transcript(KX_VERSION_LATEST)).unwrap();
        let sealed = initiator.enc(b"hello");
        let mut buf = BytesMut::from(&sealed[..]);
        assert!(responder.dec(&mut buf).is_err());
    }

    #[test]
    fn test_split_rejects_a_short_public_key() {
        let t = KxTranscript {
            initiator_pk: &[1u8; 31],
            responder_pk: &RESPONDER_PK,
            advertised: KX_VERSION_LATEST,
            picked: KX_VERSION_LATEST,
        };
        assert!(Encrypt::new_split(secretbox::gen_key(), true, &t).is_err());
    }
}
