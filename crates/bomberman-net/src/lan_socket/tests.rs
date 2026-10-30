use super::*;
use std::collections::VecDeque;

type Batch = Vec<(Vec<u8>, RecvMeta)>;
#[derive(Debug, Default)]
struct Fake {
    incoming: Mutex<VecDeque<io::Result<Batch>>>,
    replies: Mutex<Vec<(SocketAddr, Vec<u8>)>>,
    blocked: AtomicBool,
}
#[derive(Debug)]
struct Poller;
impl UdpPoller for Poller {
    fn poll_writable(self: Pin<&mut Self>, _: &mut Context) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
impl AsyncUdpSocket for Fake {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(Poller)
    }
    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        if self.blocked.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        assert_eq!(
            transmit.src_ip, None,
            "broadcast destinations must not become reply source IPs"
        );
        self.replies
            .lock()
            .unwrap()
            .push((transmit.destination, transmit.contents.to_vec()));
        Ok(())
    }
    fn poll_recv(
        &self,
        _: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let Some(batch) = self.incoming.lock().unwrap().pop_front() else {
            return Poll::Pending;
        };
        let batch = match batch {
            Ok(batch) => batch,
            Err(error) => return Poll::Ready(Err(error)),
        };
        for (index, (bytes, metadata)) in batch.iter().enumerate() {
            bufs[index][..bytes.len()].copy_from_slice(bytes);
            meta[index] = *metadata;
        }
        Poll::Ready(Ok(batch.len()))
    }
    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok("127.0.0.1:1997".parse().unwrap())
    }
    fn max_transmit_segments(&self) -> usize {
        32
    }
    fn max_receive_segments(&self) -> usize {
        64
    }
    fn may_fragment(&self) -> bool {
        false
    }
}
fn entry(bytes: &[u8], stride: usize) -> (Vec<u8>, RecvMeta) {
    (
        bytes.to_vec(),
        RecvMeta {
            addr: "192.168.50.7:32100".parse().unwrap(),
            len: bytes.len(),
            stride,
            ecn: Some(quinn::udp::EcnCodepoint::Ce),
            dst_ip: Some("192.168.50.255".parse().unwrap()),
        },
    )
}
fn socket(fake: &Arc<Fake>) -> LanSocket {
    LanSocket::new(fake.clone(), Compatibility::new("a".repeat(64)), None).unwrap()
}

#[tokio::test]
async fn mixed_discovery_and_gro_preserve_quic_payloads_boundaries_and_metadata() {
    let fake = Arc::new(Fake::default());
    let frame = vec![0xc0; QUERY.len()];
    let gro = [QUERY, &frame, QUERY, b"short"].concat();
    fake.incoming.lock().unwrap().push_back(Ok(vec![
        entry(QUERY, QUERY.len()),
        entry(&gro, QUERY.len()),
        entry(b"other QUIC", 10),
    ]));
    let adapter = socket(&fake);
    let mut storage = [[0u8; 512]; 3];
    let mut bufs: Vec<_> = storage
        .iter_mut()
        .map(|bytes| IoSliceMut::new(bytes))
        .collect();
    let mut metas = [RecvMeta::default(); 3];
    let count = std::future::poll_fn(|cx| adapter.poll_recv(cx, &mut bufs, &mut metas))
        .await
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(&bufs[0][..metas[0].len], [&frame[..], b"short"].concat());
    assert_eq!(metas[0].stride, QUERY.len());
    assert_eq!(metas[0].ecn, Some(quinn::udp::EcnCodepoint::Ce));
    assert_eq!(metas[0].dst_ip, Some("192.168.50.255".parse().unwrap()));
    assert_eq!(&bufs[1][..metas[1].len], b"other QUIC");
    let replies = fake.replies.lock().unwrap();
    assert_eq!(replies.len(), 3);
    for (destination, bytes) in replies.iter() {
        assert_eq!(*destination, metas[0].addr);
        let advertisement: Advertisement = serde_json::from_slice(bytes).unwrap();
        assert_eq!(
            advertisement.compatibility,
            Compatibility::new("a".repeat(64))
        );
    }
    assert_eq!(adapter.max_receive_segments(), 64);
    assert_eq!(adapter.max_transmit_segments(), 32);
    assert!(!adapter.may_fragment());
}

#[tokio::test]
async fn discovery_flood_yields_and_a_blocked_reply_never_holds_gameplay() {
    let fake = Arc::new(Fake::default());
    for _ in 0..9 {
        fake.incoming
            .lock()
            .unwrap()
            .push_back(Ok(vec![entry(QUERY, QUERY.len())]));
    }
    let adapter = socket(&fake);
    let mut data = [0u8; 512];
    let mut bufs = [IoSliceMut::new(&mut data)];
    let mut metas = [RecvMeta::default()];
    let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());
    assert!(adapter
        .poll_recv(&mut cx, &mut bufs, &mut metas)
        .is_pending());
    assert_eq!(fake.incoming.lock().unwrap().len(), 1);
    fake.blocked.store(true, Ordering::Relaxed);
    fake.incoming
        .lock()
        .unwrap()
        .push_back(Ok(vec![entry(b"gameplay", 8)]));
    let count = std::future::poll_fn(|cx| adapter.poll_recv(cx, &mut bufs, &mut metas))
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(&bufs[0][..metas[0].len], b"gameplay");
    assert_eq!(fake.replies.lock().unwrap().len(), 8);
}

#[tokio::test]
async fn permission_errors_retry_without_dropping_the_shared_socket() {
    let fake = Arc::new(Fake::default());
    fake.incoming
        .lock()
        .unwrap()
        .push_back(Err(io::ErrorKind::PermissionDenied.into()));
    fake.incoming
        .lock()
        .unwrap()
        .push_back(Ok(vec![entry(b"gameplay", 8)]));
    let (tx, rx) = std::sync::mpsc::sync_channel(4);
    let adapter =
        LanSocket::new(fake.clone(), Compatibility::new("a".repeat(64)), Some(tx)).unwrap();
    let mut data = [0u8; 512];
    let mut bufs = [IoSliceMut::new(&mut data)];
    let mut metas = [RecvMeta::default()];
    let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());
    assert!(adapter
        .poll_recv(&mut cx, &mut bufs, &mut metas)
        .is_pending());
    assert!(matches!(
        rx.try_recv(),
        Ok(Event::Warning(NetError::Discovery))
    ));
    assert!(adapter
        .poll_recv(&mut cx, &mut bufs, &mut metas)
        .is_pending());
    assert_eq!(fake.incoming.lock().unwrap().len(), 1);
    let count = tokio::time::timeout(
        Duration::from_secs(3),
        std::future::poll_fn(|cx| adapter.poll_recv(cx, &mut bufs, &mut metas)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(count, 1);
    assert!(rx.try_recv().is_err());
}
