use crate::{
    broker::{Broker, RateLimit},
    *,
};
use futures_util::{SinkExt, StreamExt};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider},
    pki_types::{CertificateDer, ServerName, UnixTime},
    DigitallySignedStruct, SignatureScheme,
};
use sha2::{Digest, Sha256};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{mpsc as sync, Arc, Mutex},
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
};
use tokio_tungstenite::{
    tungstenite::{self, Message},
    Connector,
};

#[derive(Debug)]
pub struct PinnedCertificate {
    fingerprint: [u8; 32],
    provider: Arc<CryptoProvider>,
}
impl PinnedCertificate {
    pub fn config(fingerprint: [u8; 32]) -> rustls::ClientConfig {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .expect("TLS versions")
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Self {
                fingerprint,
                provider,
            }))
            .with_no_client_auth()
    }
}
impl ServerCertVerifier for PinnedCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fingerprint: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if !bool::from(fingerprint.ct_eq(&self.fingerprint)) {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ));
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            msg,
            cert,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        msg: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            msg,
            cert,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub enum Destination {
    Online(Invitation),
    Lan { address: SocketAddr, host: bool },
}
/// All I/O lives on a background runtime. Both queues are bounded and every
/// error returned to the renderer is a fixed, credential-free enum.
pub struct Connection {
    outgoing: mpsc::Sender<Request>,
    incoming: sync::Receiver<Event>,
    stop: Option<oneshot::Sender<()>>,
}
impl Connection {
    pub fn connect(destination: Destination, compatibility: Compatibility) -> Self {
        let (outgoing, rx) = mpsc::channel(QUEUE);
        let (tx, incoming) = sync::sync_channel(QUEUE * 4);
        let (stop, stopped) = oneshot::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build();
            if let Ok(runtime) = runtime {
                runtime.block_on(async {
                    let result = tokio::select! {
                        result = connect(destination, compatibility, rx, &tx) => result,
                        _ = stopped => Ok(()),
                    };
                    if let Err(error) = result {
                        let _ = tx.try_send(Event::Error(error));
                    }
                });
            } else {
                let _ = tx.try_send(Event::Error(NetError::Unavailable));
            }
        });
        Self {
            outgoing,
            incoming,
            stop: Some(stop),
        }
    }
    pub fn send(&self, request: Request) -> Result<(), NetError> {
        self.outgoing
            .try_send(request)
            .map_err(|_| NetError::TooLarge)
    }
    pub fn message(&self, to: Option<PeerId>, message: GameMessage) -> Result<(), NetError> {
        self.send(Request::Send { to, message })
    }
    pub fn poll(&self) -> Result<Option<Event>, NetError> {
        match self.incoming.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(sync::TryRecvError::Empty) => Ok(None),
            Err(_) => Err(NetError::Disconnected),
        }
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

async fn connect(
    destination: Destination,
    compatibility: Compatibility,
    mut rx: mpsc::Receiver<Request>,
    tx: &sync::SyncSender<Event>,
) -> Result<(), NetError> {
    match destination {
        Destination::Online(invitation) => {
            invitation.validate()?;
            let tls = Connector::Rustls(Arc::new(PinnedCertificate::config(
                invitation.server.fingerprint,
            )));
            let mut config = tungstenite::protocol::WebSocketConfig::default();
            config.max_message_size = Some(MAX_MESSAGE);
            config.max_frame_size = Some(MAX_MESSAGE);
            let result = tokio::time::timeout(
                Duration::from_secs(10),
                tokio_tungstenite::connect_async_tls_with_config(
                    &invitation.server.endpoint,
                    Some(config),
                    true,
                    Some(tls),
                ),
            )
            .await
            .map_err(|_| NetError::Unavailable)?;
            let (mut socket, _) = result.map_err(|e| match e {
                tungstenite::Error::Tls(_) => NetError::Certificate,
                tungstenite::Error::Io(ref io)
                    if io
                        .get_ref()
                        .is_some_and(|inner| inner.downcast_ref::<rustls::Error>().is_some()) =>
                {
                    NetError::Certificate
                }
                _ => NetError::Unavailable,
            })?;
            socket
                .send(Message::Binary(
                    encode(&Request::Open(Open {
                        token: invitation.server.token,
                        room: invitation.room,
                        compatibility,
                    }))?
                    .into(),
                ))
                .await
                .map_err(|_| NetError::Disconnected)?;
            let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
            let mut last_received = tokio::time::Instant::now();
            loop {
                tokio::select! {
                    request = rx.recv() => {
                        let request = request.ok_or(NetError::Disconnected)?;
                        tokio::time::timeout(Duration::from_secs(5), socket.send(Message::Binary(encode(&request)?.into())))
                            .await.map_err(|_| NetError::Disconnected)?.map_err(|_| NetError::Disconnected)?;
                    }
                    message = socket.next() => {
                        last_received = tokio::time::Instant::now();
                        match message {
                            Some(Ok(Message::Binary(bytes))) => { tx.try_send(decode(&bytes)?).map_err(|_| NetError::TooLarge)?; }
                            Some(Ok(Message::Ping(_))) => { socket.flush().await.map_err(|_| NetError::Disconnected)?; }
                            Some(Ok(Message::Pong(_))) => {},
                            _ => return Err(NetError::Disconnected),
                        }
                    }
                    _ = heartbeat.tick() => {
                        if last_received.elapsed() > Duration::from_secs(30) { return Err(NetError::Disconnected); }
                        socket.send(Message::Ping(vec![].into())).await.map_err(|_| NetError::Disconnected)?;
                    }
                }
            }
        }
        Destination::Lan { address, host } => {
            if host {
                let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, PORT))
                    .await
                    .map_err(|_| NetError::Unavailable)?;
                let broker = Arc::new(Mutex::new(Broker::new(None)));
                tokio::spawn(serve_lan(listener, broker));
                tokio::spawn(crate::discovery::advertise(compatibility.clone()));
            }
            let mut socket =
                tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(address))
                    .await
                    .map_err(|_| NetError::Unavailable)?
                    .map_err(|_| NetError::Unavailable)?;
            socket
                .set_nodelay(true)
                .map_err(|_| NetError::Unavailable)?;
            write_packet(
                &mut socket,
                &Request::Open(Open {
                    token: String::new(),
                    room: (!host).then(|| "LAN1997X".into()),
                    compatibility,
                }),
            )
            .await?;
            let (mut read, mut write) = socket.into_split();
            let reader = async {
                loop {
                    let event: Event =
                        tokio::time::timeout(Duration::from_secs(30), read_packet(&mut read))
                            .await
                            .map_err(|_| NetError::Disconnected)??;
                    tx.try_send(event).map_err(|_| NetError::TooLarge)?;
                }
                #[allow(unreachable_code)]
                Ok::<(), NetError>(())
            };
            let writer = async {
                let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
                loop {
                    tokio::select! {
                        request = rx.recv() => write_packet(&mut write, &request.ok_or(NetError::Disconnected)?).await?,
                        _ = heartbeat.tick() => write_packet(&mut write, &Request::Ping).await?,
                    }
                }
                #[allow(unreachable_code)]
                Ok::<(), NetError>(())
            };
            tokio::select! { result = reader => result, result = writer => result }
        }
    }
}

pub async fn read_packet<R: AsyncRead + Unpin, T: serde::de::DeserializeOwned>(
    read: &mut R,
) -> Result<T, NetError> {
    let len = read.read_u32().await.map_err(|_| NetError::Disconnected)? as usize;
    if len == 0 || len > MAX_MESSAGE {
        return Err(NetError::TooLarge);
    }
    let mut bytes = vec![0; len];
    read.read_exact(&mut bytes)
        .await
        .map_err(|_| NetError::Disconnected)?;
    decode(&bytes)
}
pub async fn write_packet<W: AsyncWrite + Unpin, T: serde::Serialize>(
    write: &mut W,
    value: &T,
) -> Result<(), NetError> {
    let bytes = encode(value)?;
    tokio::time::timeout(Duration::from_secs(5), async {
        write.write_u32(bytes.len() as u32).await?;
        write.write_all(&bytes).await
    })
    .await
    .map_err(|_| NetError::Disconnected)?
    .map_err(|_| NetError::Disconnected)
}
async fn serve_lan(listener: TcpListener, broker: Arc<Mutex<Broker>>) {
    let limit = Arc::new(tokio::sync::Semaphore::new(16));
    while let Ok((socket, _)) = listener.accept().await {
        if let Ok(permit) = limit.clone().try_acquire_owned() {
            let broker = broker.clone();
            tokio::spawn(async move {
                let _permit = permit;
                let _ = serve_tcp(socket, broker).await;
            });
        }
    }
}
async fn serve_tcp(mut socket: TcpStream, broker: Arc<Mutex<Broker>>) -> Result<(), NetError> {
    socket
        .set_nodelay(true)
        .map_err(|_| NetError::Unavailable)?;
    let request = tokio::time::timeout(Duration::from_secs(5), read_packet(&mut socket))
        .await
        .map_err(|_| NetError::Protocol)??;
    let Request::Open(open) = request else {
        return Err(NetError::Protocol);
    };
    let (tx, mut rx) = mpsc::channel(QUEUE);
    let opened = broker.lock().unwrap().open(open, tx);
    let (code, id) = match opened {
        Ok(identity) => identity,
        Err(error) => {
            write_packet(&mut socket, &Event::Error(error)).await?;
            return Err(error);
        }
    };
    let (mut read, mut write) = socket.into_split();
    let reader = async {
        let mut limit = RateLimit::default();
        loop {
            let request: Request =
                tokio::time::timeout(Duration::from_secs(30), read_packet(&mut read))
                    .await
                    .map_err(|_| NetError::Disconnected)??;
            if !limit.accept(encode(&request)?.len()) {
                return Err(NetError::RateLimit);
            }
            broker.lock().unwrap().route(&code, id, request)?;
        }
    };
    let writer = async {
        while let Some(delivery) = rx.recv().await {
            let fatal = matches!(delivery.event, Event::Error(_));
            write_packet(&mut write, &delivery.event).await?;
            if fatal {
                break;
            }
        }
        Ok(())
    };
    let result = tokio::select! { result = reader => result, result = writer => result };
    broker.lock().unwrap().close(&code, id);
    result
}
