use crate::{bail, tcp, websocket, ResultType};
use sodiumoxide::crypto::secretbox::Key;
use std::net::SocketAddr;
use tokio::net::TcpStream;

// support Websocket and tcp.
pub enum Stream {
    WebSocket(websocket::WsFramedStream),
    Tcp(tcp::FramedStream),
}

impl Stream {
    #[inline]
    pub fn set_send_timeout(&mut self, ms: u64) {
        match self {
            Stream::WebSocket(s) => s.set_send_timeout(ms),
            Stream::Tcp(s) => s.set_send_timeout(ms),
        }
    }

    #[inline]
    pub fn set_raw(&mut self) {
        match self {
            Stream::WebSocket(s) => s.set_raw(),
            Stream::Tcp(s) => s.set_raw(),
        }
    }

    /// The largest message the peer may send. Lowered before authorization, where nothing a peer
    /// legitimately sends is large, so an unauthenticated connection cannot make us hold more than
    /// that. Each transport clamps `n` to its own ceiling, so `usize::MAX` restores the default.
    ///
    /// Set it before the first read of untrusted data. Lowering it later is not a way to constrain
    /// a message already in progress: what that does to one is up to the transport.
    #[inline]
    pub fn set_max_packet_length(&mut self, n: usize) {
        match self {
            Stream::WebSocket(s) => s.set_max_packet_length(n),
            Stream::Tcp(s) => s.codec_mut().set_max_packet_length(n),
        }
    }

    #[inline]
    pub async fn send_bytes(&mut self, bytes: bytes::Bytes) -> ResultType<()> {
        match self {
            Stream::WebSocket(s) => s.send_bytes(bytes).await,
            Stream::Tcp(s) => s.send_bytes(bytes).await,
        }
    }

    #[inline]
    pub async fn send_raw(&mut self, bytes: Vec<u8>) -> ResultType<()> {
        match self {
            Stream::WebSocket(s) => s.send_raw(bytes).await,
            Stream::Tcp(s) => s.send_raw(bytes).await,
        }
    }

    #[inline]
    pub fn set_key(&mut self, key: Key) {
        match self {
            Stream::WebSocket(s) => s.set_key(key),
            Stream::Tcp(s) => s.set_key(key),
        }
    }

    /// The stream key for the key exchange version in `t.picked`: version 0 is `set_key`, and
    /// version 1 derives one key per direction from the transcript. Any other version is refused
    /// rather than run as one of these. `is_initiator` is the side that sent the sealed key.
    #[inline]
    pub fn set_negotiated_key(
        &mut self,
        key: Key,
        is_initiator: bool,
        t: &tcp::KxTranscript,
    ) -> ResultType<()> {
        match t.picked {
            0 => {
                self.set_key(key);
                Ok(())
            }
            1 => match self {
                Stream::WebSocket(s) => s.set_key_split(key, is_initiator, t),
                Stream::Tcp(s) => s.set_key_split(key, is_initiator, t),
            },
            v => bail!("unsupported key exchange version {}", v),
        }
    }

    #[inline]
    pub fn is_secured(&self) -> bool {
        match self {
            Stream::WebSocket(s) => s.is_secured(),
            Stream::Tcp(s) => s.is_secured(),
        }
    }

    #[inline]
    pub async fn next_timeout(
        &mut self,
        timeout: u64,
    ) -> Option<Result<bytes::BytesMut, std::io::Error>> {
        match self {
            Stream::WebSocket(s) => s.next_timeout(timeout).await,
            Stream::Tcp(s) => s.next_timeout(timeout).await,
        }
    }

    /// send message
    #[inline]
    pub async fn send(&mut self, msg: &impl protobuf::Message) -> ResultType<()> {
        match self {
            Self::WebSocket(ws) => ws.send(msg).await,
            Self::Tcp(tcp) => tcp.send(msg).await,
        }
    }

    /// receive message
    #[inline]
    pub async fn next(&mut self) -> Option<Result<bytes::BytesMut, std::io::Error>> {
        match self {
            Self::WebSocket(ws) => ws.next().await,
            Self::Tcp(tcp) => tcp.next().await,
        }
    }

    #[inline]
    pub fn local_addr(&self) -> SocketAddr {
        match self {
            Self::WebSocket(ws) => ws.local_addr(),
            Self::Tcp(tcp) => tcp.local_addr(),
        }
    }

    #[inline]
    pub fn from(stream: TcpStream, stream_addr: SocketAddr) -> Self {
        Self::Tcp(tcp::FramedStream::from(stream, stream_addr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_set_negotiated_key_refuses_unknown_versions() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut stream = Stream::from(TcpStream::connect(addr).await.unwrap(), addr);
        let t = |picked| tcp::KxTranscript {
            initiator_pk: &[1u8; 32],
            responder_pk: &[2u8; 32],
            advertised: 1,
            picked,
        };
        let mut set = |picked| stream.set_negotiated_key(Key([0u8; 32]), true, &t(picked));
        assert!(set(2).is_err());
        assert!(set(1).is_ok());
    }
}
