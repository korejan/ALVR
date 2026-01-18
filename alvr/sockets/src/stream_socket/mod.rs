// Note: for StreamSocket, the client uses a server socket, the server uses a client socket.
// This is because of certificate management. The server needs to trust a client and its certificate
//
// StreamSender and StreamReceiver endpoints allow for convenient conversion of the header to/from
// bytes while still handling the additional byte buffer with zero copies and extra allocations.

mod tcp;
mod throttled_udp;
mod udp;

use crate::{BINCODE_CONFIG, serialized_size};
use alvr_common::prelude::*;
use alvr_session::{SocketBufferSize, SocketProtocol};
use bytes::{Buf, BufMut, BytesMut};
use futures::SinkExt;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    marker::PhantomData,
    mem::MaybeUninit,
    net::IpAddr,
    ops::{Deref, DerefMut},
    sync::Arc,
};
use tcp::{TcpStreamReceiveSocket, TcpStreamSendSocket};
use throttled_udp::{ThrottledUdpStreamReceiveSocket, ThrottledUdpStreamSendSocket};
use tokio::net;
use tokio::sync::{Mutex, mpsc};
use udp::{UdpStreamReceiveSocket, UdpStreamSendSocket};

// todo: when const_generics reaches stable, convert this to an enum
pub type StreamId = u16;

pub fn set_socket_buffers(
    socket: &socket2::Socket,
    send_buffer_bytes: SocketBufferSize,
    recv_buffer_bytes: SocketBufferSize,
) -> StrResult {
    info!(
        "Initial socket buffer size: send: {}B, recv: {}B",
        socket.send_buffer_size().map_err(err!())?,
        socket.recv_buffer_size().map_err(err!())?
    );

    {
        let maybe_size = match send_buffer_bytes {
            SocketBufferSize::Default => None,
            SocketBufferSize::Maximum => Some(u32::MAX),
            SocketBufferSize::Custom(size) => Some(size),
        };

        if let Some(size) = maybe_size {
            if let Err(e) = socket.set_send_buffer_size(size as usize) {
                info!("Error setting socket send buffer: {e}");
            } else {
                info!(
                    "Set socket send buffer succeeded: {}",
                    socket.send_buffer_size().map_err(err!())?
                );
            }
        }
    }

    {
        let maybe_size = match recv_buffer_bytes {
            SocketBufferSize::Default => None,
            SocketBufferSize::Maximum => Some(u32::MAX),
            SocketBufferSize::Custom(size) => Some(size),
        };

        if let Some(size) = maybe_size {
            if let Err(e) = socket.set_recv_buffer_size(size as usize) {
                info!("Error setting socket recv buffer: {e}");
            } else {
                info!(
                    "Set socket recv buffer succeeded: {}",
                    socket.recv_buffer_size().map_err(err!())?
                );
            }
        }
    }

    Ok(())
}

#[derive(Clone)]
enum StreamSendSocket {
    Udp(UdpStreamSendSocket),
    ThrottledUdp(ThrottledUdpStreamSendSocket),
    Tcp(TcpStreamSendSocket),
}

enum StreamReceiveSocket {
    Udp(UdpStreamReceiveSocket),
    ThrottledUdp(ThrottledUdpStreamReceiveSocket),
    Tcp(TcpStreamReceiveSocket),
}

pub struct SendBufferLock<'a> {
    header_bytes: &'a mut BytesMut,
    buffer_bytes: BytesMut,
}

impl Deref for SendBufferLock<'_> {
    type Target = BytesMut;
    #[inline(always)]
    fn deref(&self) -> &BytesMut {
        &self.buffer_bytes
    }
}

impl DerefMut for SendBufferLock<'_> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut BytesMut {
        &mut self.buffer_bytes
    }
}

impl Drop for SendBufferLock<'_> {
    #[inline(always)]
    fn drop(&mut self) {
        // the extra split is to avoid moving buffer_bytes
        self.header_bytes.unsplit(self.buffer_bytes.split())
    }
}

pub struct SenderBuffer<T> {
    inner: BytesMut,
    stream_id: StreamId,
    offset: usize, // Position after header, used by get_mut() for backward compat
    _phantom: PhantomData<T>,
}

impl<T> SenderBuffer<T> {
    /// Get the editable payload portion of the buffer (header excluded).
    /// Used by legacy new_sender_buffer() callers.
    #[inline(always)]
    pub fn get_mut(&mut self) -> SendBufferLock<'_> {
        let buffer_bytes = self.inner.split_off(self.offset);
        SendBufferLock {
            header_bytes: &mut self.inner,
            buffer_bytes,
        }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }
}

impl<T: Serialize + Default> SenderBuffer<T> {
    /// Create a reusable buffer for packets with header + payload.
    /// Uses T::default() + serialized_size() to compute header capacity.
    #[inline(always)]
    pub fn new(stream_id: StreamId, max_payload_size: usize) -> StrResult<Self> {
        let header_size = serialized_size(&T::default())?;
        let capacity = 2 + 4 + header_size + max_payload_size; // stream_id + packet_index + header + payload
        Ok(Self {
            inner: BytesMut::with_capacity(capacity),
            stream_id,
            offset: 0, // Will be set by encode()
            _phantom: PhantomData,
        })
    }
}

impl<T: Serialize> SenderBuffer<T> {
    /// Create a pre-encoded buffer for constant header-only packets.
    /// Can be sent multiple times without re-encoding - only packet_index changes.
    #[inline]
    pub fn encoded(stream_id: StreamId, max_payload_size: usize, header: &T) -> StrResult<Self> {
        let header_size = serialized_size(&header)?;
        // stream_id + packet_index + header + payload-size
        let capacity = 2 + 4 + header_size + max_payload_size;
        let mut buffer = Self {
            inner: BytesMut::with_capacity(capacity),
            stream_id,
            offset: 0,
            _phantom: PhantomData,
        };
        buffer.encode(header)?;
        Ok(buffer)
    }

    /// Encode header into buffer, returns lock to payload portion for writing.
    /// Clears the buffer first, so this must be called before each send for variable headers.
    #[inline]
    pub fn encode(&mut self, header: &T) -> StrResult<SendBufferLock<'_>> {
        self.inner.clear();
        self.inner.put_u16(self.stream_id);
        self.inner.put_u32(0); // packet_index placeholder

        // Use writer() to get a Write impl, encode directly into BytesMut
        trace_err!(bincode::serde::encode_into_std_write(
            header,
            &mut (&mut self.inner).writer(),
            BINCODE_CONFIG
        ))?;

        self.offset = self.inner.len();
        Ok(self.get_mut())
    }
}

pub struct StreamSender<T> {
    stream_id: StreamId,
    socket: StreamSendSocket,
    // if the packet index overflows the worst that happens is a false positive packet loss
    next_packet_index: u32,
    _phantom: PhantomData<T>,
}

impl<T> StreamSender<T> {
    /// Send using a reusable buffer - does NOT consume the buffer.
    /// Buffer data is preserved after send, ready for next `encode()` or re-send.
    pub async fn send_buffer_ref(&mut self, buffer: &mut SenderBuffer<T>) -> StrResult {
        buffer.inner[2..6].copy_from_slice(&self.next_packet_index.to_be_bytes());
        self.next_packet_index += 1;

        #[cfg(debug_assertions)]
        let (original_len, original_ptr) = (buffer.inner.len(), buffer.inner.as_ptr() as usize);

        let bytes = std::mem::take(&mut buffer.inner).freeze();
        let result = match &self.socket {
            StreamSendSocket::Udp(s) => {
                s.inner
                    .lock()
                    .await
                    .send((bytes.clone(), s.peer_addr))
                    .await
            }
            StreamSendSocket::Tcp(s) => s.lock().await.send(bytes.clone()).await,
            StreamSendSocket::ThrottledUdp(s) => s.send(bytes.clone()).await,
        };
        buffer.inner = bytes
            .try_into_mut()
            .expect("buffer refcount should be 1 after send");

        #[cfg(debug_assertions)]
        {
            debug_assert_eq!(
                buffer.inner.len(),
                original_len,
                "buffer length changed after send"
            );
            debug_assert_eq!(
                buffer.inner.as_ptr() as usize,
                original_ptr,
                "buffer was reallocated after send"
            );
        }

        trace_err!(result)
    }

    /// Send consuming the buffer
    pub async fn send_buffer(&mut self, mut buffer: SenderBuffer<T>) -> StrResult {
        buffer.inner[2..6].copy_from_slice(&self.next_packet_index.to_be_bytes());
        self.next_packet_index += 1;

        let bytes = buffer.inner.freeze();
        let result = match &self.socket {
            StreamSendSocket::Udp(s) => s.inner.lock().await.send((bytes, s.peer_addr)).await,
            StreamSendSocket::Tcp(s) => s.lock().await.send(bytes).await,
            StreamSendSocket::ThrottledUdp(s) => s.send(bytes).await,
        };
        trace_err!(result)
    }
}

enum StreamReceiverType {
    Queue(mpsc::UnboundedReceiver<BytesMut>),
    // QuicReliable(...)
}

pub struct ReceivedPacket<T> {
    pub header: T,
    pub buffer: BytesMut,
    pub had_packet_loss: bool,
}

pub struct StreamReceiver<T> {
    stream_id: StreamId,
    receiver: StreamReceiverType,
    next_packet_index: u32,
    _phantom: PhantomData<T>,
}

impl<T: DeserializeOwned> StreamReceiver<T> {
    pub async fn recv(&mut self) -> StrResult<ReceivedPacket<T>> {
        let mut bytes = match &mut self.receiver {
            StreamReceiverType::Queue(receiver) => trace_none!(receiver.recv().await)?,
        };

        let packet_index = bytes.get_u32();
        let had_packet_loss = packet_index != self.next_packet_index;
        self.next_packet_index = packet_index + 1;

        let (header, bytes_read) =
            trace_err!(bincode::serde::decode_from_slice(&bytes, BINCODE_CONFIG))?;
        // Advance past the header bytes
        bytes.advance(bytes_read);

        // At this point, "bytes" does not include the header anymore
        Ok(ReceivedPacket {
            header,
            buffer: bytes,
            had_packet_loss,
        })
    }
}

pub enum StreamSocketBuilder {
    Tcp(net::TcpListener),
    Udp(net::UdpSocket),
    ThrottledUdp(net::UdpSocket),
}

impl StreamSocketBuilder {
    pub async fn listen_for_server(
        port: u16,
        stream_socket_config: SocketProtocol,
        send_buffer_bytes: SocketBufferSize,
        recv_buffer_bytes: SocketBufferSize,
    ) -> StrResult<Self> {
        Ok(match stream_socket_config {
            SocketProtocol::Udp => StreamSocketBuilder::Udp(
                udp::bind(port, send_buffer_bytes, recv_buffer_bytes).await?,
            ),
            SocketProtocol::Tcp => StreamSocketBuilder::Tcp(
                tcp::bind(port, send_buffer_bytes, recv_buffer_bytes).await?,
            ),
            SocketProtocol::ThrottledUdp { .. } => StreamSocketBuilder::ThrottledUdp(
                udp::bind(port, send_buffer_bytes, recv_buffer_bytes).await?,
            ),
        })
    }

    pub async fn accept_from_server(self, server_ip: IpAddr, port: u16) -> StrResult<StreamSocket> {
        let (send_socket, receive_socket) = match self {
            StreamSocketBuilder::Udp(socket) => {
                let (send_socket, receive_socket) = udp::connect(socket, server_ip, port).await?;
                (
                    StreamSendSocket::Udp(send_socket),
                    StreamReceiveSocket::Udp(receive_socket),
                )
            }
            StreamSocketBuilder::Tcp(listener) => {
                let (send_socket, receive_socket) =
                    tcp::accept_from_server(listener, server_ip).await?;
                (
                    StreamSendSocket::Tcp(send_socket),
                    StreamReceiveSocket::Tcp(receive_socket),
                )
            }
            StreamSocketBuilder::ThrottledUdp(socket) => {
                let (send_socket, receive_socket) =
                    throttled_udp::accept_from_server(socket, server_ip, port).await?;
                (
                    StreamSendSocket::ThrottledUdp(send_socket),
                    StreamReceiveSocket::ThrottledUdp(receive_socket),
                )
            }
        };

        Ok(StreamSocket {
            send_socket,
            receive_socket: Arc::new(Mutex::new(Some(receive_socket))),
            packet_queues: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn connect_to_client(
        client_ip: IpAddr,
        port: u16,
        protocol: SocketProtocol,
        video_byterate: u32,
        send_buffer_bytes: SocketBufferSize,
        recv_buffer_bytes: SocketBufferSize,
    ) -> StrResult<StreamSocket> {
        let (send_socket, receive_socket) = match protocol {
            SocketProtocol::Udp => {
                let socket = udp::bind(port, send_buffer_bytes, recv_buffer_bytes).await?;
                let (send_socket, receive_socket) = udp::connect(socket, client_ip, port).await?;
                (
                    StreamSendSocket::Udp(send_socket),
                    StreamReceiveSocket::Udp(receive_socket),
                )
            }
            SocketProtocol::Tcp => {
                let (send_socket, receive_socket) =
                    tcp::connect_to_client(client_ip, port, send_buffer_bytes, recv_buffer_bytes)
                        .await?;
                (
                    StreamSendSocket::Tcp(send_socket),
                    StreamReceiveSocket::Tcp(receive_socket),
                )
            }
            SocketProtocol::ThrottledUdp { bitrate_multiplier } => {
                let socket = udp::bind(port, send_buffer_bytes, recv_buffer_bytes).await?;

                let (send_socket, receive_socket) = throttled_udp::connect_to_client(
                    socket,
                    client_ip,
                    port,
                    video_byterate,
                    bitrate_multiplier,
                )
                .await?;
                (
                    StreamSendSocket::ThrottledUdp(send_socket),
                    StreamReceiveSocket::ThrottledUdp(receive_socket),
                )
            }
        };

        Ok(StreamSocket {
            send_socket,
            receive_socket: Arc::new(Mutex::new(Some(receive_socket))),
            packet_queues: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

pub struct StreamSocket {
    send_socket: StreamSendSocket,
    receive_socket: Arc<Mutex<Option<StreamReceiveSocket>>>,
    packet_queues: Arc<Mutex<HashMap<StreamId, mpsc::UnboundedSender<BytesMut>>>>,
}

impl StreamSocket {
    pub async fn request_stream<T>(&self, stream_id: StreamId) -> StrResult<StreamSender<T>> {
        Ok(StreamSender {
            stream_id,
            socket: self.send_socket.clone(),
            next_packet_index: 0,
            _phantom: PhantomData,
        })
    }

    pub async fn subscribe_to_stream<T>(
        &self,
        stream_id: StreamId,
    ) -> StrResult<StreamReceiver<T>> {
        let (enqueuer, dequeuer) = mpsc::unbounded_channel();
        self.packet_queues.lock().await.insert(stream_id, enqueuer);

        Ok(StreamReceiver {
            stream_id,
            receiver: StreamReceiverType::Queue(dequeuer),
            next_packet_index: 0,
            _phantom: PhantomData,
        })
    }

    pub async fn receive_loop(&self) -> StrResult {
        match self.receive_socket.lock().await.take().unwrap() {
            StreamReceiveSocket::Udp(socket) => {
                udp::receive_loop(socket, Arc::clone(&self.packet_queues)).await
            }
            StreamReceiveSocket::Tcp(socket) => {
                tcp::receive_loop(socket, Arc::clone(&self.packet_queues)).await
            }
            StreamReceiveSocket::ThrottledUdp(socket) => {
                throttled_udp::receive_loop(socket, Arc::clone(&self.packet_queues)).await
            }
        }
    }
}
