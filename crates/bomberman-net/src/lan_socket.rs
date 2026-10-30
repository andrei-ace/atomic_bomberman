//! Discovery and QUIC share the host's single UDP socket. Gameplay stays on
//! Quinn's native socket, retaining its batching, ECN, GRO and GSO metadata.
use crate::{
    discovery::{Advertisement, QUERY},
    Compatibility, Event, NetError,
};
use quinn::{
    udp::{RecvMeta, Transmit},
    AsyncUdpSocket, UdpPoller,
};
use std::{
    future::Future,
    io::{self, IoSliceMut},
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
        Arc, Mutex,
    },
    task::{ready, Context, Poll},
    time::Duration,
};

#[derive(Debug)]
pub(crate) struct LanSocket {
    inner: Arc<dyn AsyncUdpSocket>,
    reply: Vec<u8>,
    warnings: Option<SyncSender<Event>>,
    warned: AtomicBool,
    retry: Mutex<Option<Pin<Box<tokio::time::Sleep>>>>,
}
impl LanSocket {
    pub(crate) fn new(
        inner: Arc<dyn AsyncUdpSocket>,
        compatibility: Compatibility,
        warnings: Option<SyncSender<Event>>,
    ) -> Result<Self, NetError> {
        Ok(Self {
            inner,
            reply: serde_json::to_vec(&Advertisement {
                name: "LAN game".into(),
                compatibility,
            })
            .map_err(|_| NetError::Discovery)?,
            warnings,
            warned: AtomicBool::new(false),
            retry: Mutex::new(None),
        })
    }

    fn warn(&self) {
        if !self.warned.swap(true, Ordering::Relaxed) {
            if let Some(warnings) = &self.warnings {
                let _ = warnings.try_send(Event::Warning(NetError::Discovery));
            }
        }
    }

    fn answer(&self, address: SocketAddr) {
        // Never wait behind discovery replies or queue gameplay. If the send
        // buffer is full, the browser's regular UDP probes retry discovery.
        let result = self.inner.try_send(&Transmit {
            destination: address,
            ecn: None,
            contents: &self.reply,
            segment_size: None,
            src_ip: None,
        });
        if result.is_err_and(|error| error.kind() != io::ErrorKind::WouldBlock) {
            self.warn();
        }
    }
}

impl AsyncUdpSocket for LanSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.inner.clone().create_io_poller()
    }
    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        self.inner.try_send(transmit)
    }
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
    fn max_transmit_segments(&self) -> usize {
        self.inner.max_transmit_segments()
    }
    fn max_receive_segments(&self) -> usize {
        self.inner.max_receive_segments()
    }
    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        {
            let mut retry = self.retry.lock().unwrap();
            if let Some(timer) = retry.as_mut() {
                ready!(timer.as_mut().poll(cx));
                *retry = None;
            }
        }
        // Discovery-only floods must yield too: Quinn's own receive budget
        // only sees the QUIC datagrams returned by this adapter.
        for _ in 0..8 {
            let received = match ready!(self.inner.poll_recv(cx, bufs, meta)) {
                Ok(received) => received,
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    self.warn();
                    let mut timer = Box::pin(tokio::time::sleep(Duration::from_secs(1)));
                    let _ = timer.as_mut().poll(cx);
                    *self.retry.lock().unwrap() = Some(timer);
                    return Poll::Pending;
                }
                Err(error) => return Poll::Ready(Err(error)),
            };
            let mut kept = 0;
            for index in 0..received {
                let mut item = meta[index];
                let stride = item.stride.max(1);
                let mut length = 0;
                for start in (0..item.len).step_by(stride) {
                    let end = (start + stride).min(item.len);
                    if &bufs[index][start..end] == QUERY {
                        self.answer(item.addr);
                    } else {
                        if length != start {
                            bufs[index].copy_within(start..end, length);
                        }
                        length += end - start;
                    }
                }
                // Keep unrelated empty datagrams so Quinn still charges its
                // receive budget. Only exact discovery queries are consumed.
                if length == 0 && item.len != 0 {
                    continue;
                }
                item.len = length;
                // Swap buffer views, rather than copying QUIC payloads when a
                // discovery-only buffer was removed from this receive batch.
                bufs.swap(kept, index);
                meta[kept] = item;
                kept += 1;
            }
            if kept > 0 || received == 0 {
                return Poll::Ready(Ok(kept));
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

#[cfg(test)]
mod tests;
