use std::{
    collections::HashMap,
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{ready, Context, Poll},
};

use quinn::udp::{self, RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};
use tokio::{io::Interest, net::UdpSocket as TokioUdpSocket, sync::oneshot};
use tracing::{debug, warn};

use super::stun::{STUN_BINDING_SUCCESS, STUN_HEADER_SIZE, STUN_MAGIC_COOKIE};

#[derive(Debug)]
pub(super) struct DirectUdpSocket {
    io: Arc<TokioUdpSocket>,
    inner: udp::UdpSocketState,
    stun_waiters: Mutex<HashMap<[u8; 12], oneshot::Sender<StunDatagram>>>,
}

#[derive(Debug)]
pub(super) struct StunDatagram {
    pub(super) data: Vec<u8>,
}

impl DirectUdpSocket {
    pub(super) fn new(socket: TokioUdpSocket) -> io::Result<Self> {
        let inner = udp::UdpSocketState::new((&socket).into())?;
        Ok(Self {
            io: Arc::new(socket),
            inner,
            stun_waiters: Mutex::new(HashMap::new()),
        })
    }

    pub(super) fn io(&self) -> Arc<TokioUdpSocket> {
        Arc::clone(&self.io)
    }

    pub(super) fn register_stun_waiter(
        &self,
        transaction_id: [u8; 12],
    ) -> oneshot::Receiver<StunDatagram> {
        let (tx, rx) = oneshot::channel();
        let mut waiters = self.stun_waiters.lock().expect("stun waiters poisoned");
        if waiters.insert(transaction_id, tx).is_some() {
            warn!("STUN waiter already existed for transaction id; replacing");
        }
        rx
    }

    pub(super) fn cancel_stun_waiter(&self, transaction_id: &[u8; 12]) {
        let mut waiters = self.stun_waiters.lock().expect("stun waiters poisoned");
        waiters.remove(transaction_id);
    }

    pub(super) fn try_handle_stun(&self, packet: &[u8], addr: SocketAddr) -> bool {
        if !Self::is_stun_packet(packet) {
            return false;
        }

        let message_type = u16::from_be_bytes([packet[0], packet[1]]);
        if message_type != STUN_BINDING_SUCCESS {
            debug!(
                ?addr,
                message_type, "Dropping non-success STUN packet received on direct socket"
            );
            return true;
        }

        let mut transaction_id = [0u8; 12];
        transaction_id.copy_from_slice(&packet[8..20]);

        let sender = {
            let mut waiters = self.stun_waiters.lock().expect("stun waiters poisoned");
            waiters.remove(&transaction_id)
        };

        if let Some(sender) = sender {
            let _ = sender.send(StunDatagram {
                data: packet.to_vec(),
            });
        } else {
            debug!(
                ?addr,
                transaction_id = ?transaction_id,
                "Dropping STUN response with no active waiter"
            );
        }

        true
    }

    pub(super) async fn send_raw(&self, payload: &[u8], target: SocketAddr) -> io::Result<usize> {
        self.io.as_ref().send_to(payload, target).await
    }
}

impl AsyncUdpSocket for DirectUdpSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(DirectUdpPoller {
            socket: Arc::clone(&self.io),
            waiter: Mutex::new(None),
        })
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        let io_ref = self.io.as_ref();
        io_ref.try_io(Interest::WRITABLE, || {
            self.inner.send(io_ref.into(), transmit)
        })
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [io::IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let io_ref = self.io.as_ref();
        loop {
            ready!(io_ref.poll_recv_ready(cx))?;
            if let Ok(res) = io_ref.try_io(Interest::READABLE, || {
                self.inner.recv(io_ref.into(), bufs, meta)
            }) {
                let kept = self.filter_stun_packets(bufs, meta, res);
                if kept == 0 {
                    continue;
                }
                return Poll::Ready(Ok(kept));
            }
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.io.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.inner.max_gso_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.inner.gro_segments()
    }

    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}

impl DirectUdpSocket {
    fn filter_stun_packets(
        &self,
        bufs: &mut [io::IoSliceMut<'_>],
        meta: &mut [RecvMeta],
        count: usize,
    ) -> usize {
        let mut kept = 0;
        for idx in 0..count {
            let len = meta[idx].len;
            let packet = &(*bufs[idx])[..len];
            if self.try_handle_stun(packet, meta[idx].addr) {
                continue;
            }

            if kept != idx {
                let packet_bytes = packet.to_vec();
                let target_slice: &mut [u8] = &mut *bufs[kept];
                target_slice[..len].copy_from_slice(&packet_bytes);
                meta[kept] = meta[idx];
            }
            kept += 1;
        }
        kept
    }

    fn is_stun_packet(packet: &[u8]) -> bool {
        if packet.len() < STUN_HEADER_SIZE {
            return false;
        }

        if packet[0] & 0b1100_0000 != 0 {
            return false;
        }

        let message_len = u16::from_be_bytes([packet[2], packet[3]]) as usize;
        if STUN_HEADER_SIZE + message_len > packet.len() {
            return false;
        }

        let cookie = STUN_MAGIC_COOKIE.to_be_bytes();
        &packet[4..8] == cookie.as_ref()
    }
}

struct DirectUdpPoller {
    socket: Arc<TokioUdpSocket>,
    waiter: Mutex<Option<Pin<Box<dyn Future<Output = io::Result<()>> + Send>>>>,
}

impl UdpPoller for DirectUdpPoller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let mut guard = self.waiter.lock().expect("poller waiter poisoned");
        if guard.is_none() {
            let socket = Arc::clone(&self.socket);
            *guard = Some(Box::pin(async move { socket.writable().await }));
        }

        if let Some(waiter) = guard.as_mut() {
            match waiter.as_mut().poll(cx) {
                Poll::Ready(result) => {
                    *guard = None;
                    Poll::Ready(result)
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            Poll::Ready(Ok(()))
        }
    }
}

impl std::fmt::Debug for DirectUdpPoller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectUdpPoller").finish()
    }
}
