use super::{BINCODE_CONFIG, CONTROL_PORT, LOCAL_IP, Ldc, serialized_size};
use alvr_common::prelude::*;
use bytes::{BufMut, Bytes, BytesMut};
use futures::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use serde::{Serialize, de::DeserializeOwned};
use std::{marker::PhantomData, net::IpAddr};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

pub struct ControlBuffer<T> {
    inner: Bytes,
    _phantom: PhantomData<T>,
}

/// Reusable buffer for control packets (no stream framing).
/// Use `encoded()` for constant packets, `encode()` for variable packets.
pub struct ControlBufferMut<T> {
    inner: BytesMut,
    _phantom: PhantomData<T>,
}

impl<T: Serialize + Default> ControlBufferMut<T> {
    #[inline(always)]
    pub fn new() -> StrResult<Self> {
        let capacity = serialized_size(&T::default())?;
        Ok(Self {
            inner: BytesMut::with_capacity(capacity),
            _phantom: PhantomData,
        })
    }
}

impl<T: Serialize> ControlBufferMut<T> {
    /// Create a pre-encoded buffer for constant packets.
    /// Can be sent multiple times without re-encoding.
    #[inline(always)]
    pub fn encoded(packet: &T) -> StrResult<Self> {
        let capacity = serialized_size(packet)?;
        let mut buffer = Self {
            inner: BytesMut::with_capacity(capacity),
            _phantom: PhantomData,
        };
        buffer.encode(packet)?;
        Ok(buffer)
    }

    /// Encode packet into buffer (clears first).
    /// Call before each send for variable packets.
    #[inline(always)]
    pub fn encode(&mut self, packet: &T) -> StrResult {
        self.inner.clear();
        trace_err!(bincode::serde::encode_into_std_write(
            packet,
            &mut (&mut self.inner).writer(),
            BINCODE_CONFIG
        ))?;
        Ok(())
    }
}

impl<T: Serialize> ControlBuffer<T> {
    #[inline(always)]
    pub fn encoded(packet: &T) -> StrResult<Self> {
        let ctrl_buf = ControlBufferMut::encoded(packet)?;
        Ok(Self {
            inner: ctrl_buf.inner.freeze(),
            _phantom: PhantomData,
        })
    }
}

pub struct ControlSocketSender<T> {
    inner: SplitSink<Framed<TcpStream, Ldc>, Bytes>,
    _phantom: PhantomData<T>,
}

impl<S: Serialize> ControlSocketSender<S> {
    #[inline(always)]
    pub async fn send(&mut self, packet: &S) -> StrResult {
        let packet_types = ControlBuffer::<S>::encoded(packet)?;
        self.send_buffer(&packet_types).await
    }

    #[inline(always)]
    pub async fn send_buffer(&mut self, buffer: &ControlBuffer<S>) -> StrResult {
        trace_err!(self.inner.send(buffer.inner.clone()).await)
    }

    /// Send using a reusable buffer - does NOT consume the buffer.
    /// Buffer data is preserved after send, ready for next `encode()` or re-send.
    #[inline]
    pub async fn send_buffer_mut(&mut self, buffer: &mut ControlBufferMut<S>) -> StrResult {
        let bytes = std::mem::take(&mut buffer.inner).freeze();
        let result = self.inner.send(bytes.clone()).await;
        buffer.inner = bytes
            .try_into_mut()
            .expect("buffer refcount should be 1 after send");
        trace_err!(result)
    }
}

pub struct ControlSocketReceiver<T> {
    inner: SplitStream<Framed<TcpStream, Ldc>>,
    _phantom: PhantomData<T>,
}

impl<R: DeserializeOwned> ControlSocketReceiver<R> {
    #[inline]
    pub async fn recv(&mut self) -> StrResult<R> {
        let packet_bytes = trace_err!(trace_none!(self.inner.next().await)?)?;
        let (packet, _) = trace_err!(bincode::serde::decode_from_slice(
            &packet_bytes,
            BINCODE_CONFIG
        ))?;
        Ok(packet)
    }
}

// Proto-control-socket that can send and receive any packet. After the split, only the packets of
// the specified types can be exchanged
pub struct ProtoControlSocket {
    inner: Framed<TcpStream, Ldc>,
}

pub enum PeerType {
    AnyClient(Vec<IpAddr>),
    Server,
}

impl ProtoControlSocket {
    pub async fn connect_to(peer: PeerType) -> StrResult<(Self, IpAddr)> {
        let socket = match peer {
            PeerType::AnyClient(ips) => {
                let client_addresses = ips
                    .iter()
                    .map(|&ip| (ip, CONTROL_PORT).into())
                    .collect::<Vec<_>>();
                trace_err!(TcpStream::connect(client_addresses.as_slice()).await)?
            }
            PeerType::Server => {
                let listener = trace_err!(TcpListener::bind((LOCAL_IP, CONTROL_PORT)).await)?;
                let (socket, _) = trace_err!(listener.accept().await)?;
                socket
            }
        };

        trace_err!(socket.set_nodelay(true))?;
        let peer_ip = trace_err!(socket.peer_addr())?.ip();
        let socket = Framed::new(socket, Ldc::new());

        Ok((Self { inner: socket }, peer_ip))
    }

    #[inline(always)]
    pub async fn send<S: Serialize>(&mut self, packet: &S) -> StrResult {
        let packet_bytes = ControlBuffer::<S>::encoded(packet)?;
        self.send_buffer(&packet_bytes).await
    }

    #[inline(always)]
    pub async fn send_buffer<S: Serialize>(&mut self, buffer: &ControlBuffer<S>) -> StrResult {
        trace_err!(self.inner.send(buffer.inner.clone()).await)
    }

    /// Send using a reusable buffer - does NOT consume the buffer.
    /// Buffer data is preserved after send, ready for next `encode()` or re-send.
    #[inline]
    pub async fn send_buffer_mut<S: Serialize>(
        &mut self,
        buffer: &mut ControlBufferMut<S>,
    ) -> StrResult {
        let bytes = std::mem::take(&mut buffer.inner).freeze();
        let result = self.inner.send(bytes.clone()).await;
        buffer.inner = bytes
            .try_into_mut()
            .expect("buffer refcount should be 1 after send");
        trace_err!(result)
    }

    #[inline]
    pub async fn recv<R: DeserializeOwned>(&mut self) -> StrResult<R> {
        let packet_bytes = trace_err!(trace_none!(self.inner.next().await)?)?;
        let (packet, _) = trace_err!(bincode::serde::decode_from_slice(
            &packet_bytes,
            BINCODE_CONFIG
        ))?;
        Ok(packet)
    }

    #[inline]
    pub fn split<S: Serialize, R: DeserializeOwned>(
        self,
    ) -> (ControlSocketSender<S>, ControlSocketReceiver<R>) {
        let (sender, receiver) = self.inner.split();

        (
            ControlSocketSender {
                inner: sender,
                _phantom: PhantomData,
            },
            ControlSocketReceiver {
                inner: receiver,
                _phantom: PhantomData,
            },
        )
    }
}
