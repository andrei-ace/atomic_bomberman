//! Bound sockets before the TLS handshake, including unauthenticated peers.
use axum_server::accept::Accept;
use std::{
    future::{ready, Ready},
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    sync::{OwnedSemaphorePermit, Semaphore},
};

#[derive(Clone)]
pub struct ConnectionLimit(Arc<Semaphore>);
impl ConnectionLimit {
    pub fn new(limit: usize) -> Self {
        Self(Arc::new(Semaphore::new(limit)))
    }
}
pub struct LimitedStream {
    stream: TcpStream,
    _permit: OwnedSemaphorePermit,
}
impl<S> Accept<TcpStream, S> for ConnectionLimit {
    type Stream = LimitedStream;
    type Service = S;
    type Future = Ready<io::Result<(Self::Stream, S)>>;
    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        ready(
            self.0
                .clone()
                .try_acquire_owned()
                .map_err(|_| io::Error::other("connection limit reached"))
                .and_then(|permit| {
                    stream.set_nodelay(true)?;
                    Ok((
                        LimitedStream {
                            stream,
                            _permit: permit,
                        },
                        service,
                    ))
                }),
        )
    }
}
impl AsyncRead for LimitedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, bytes)
    }
}
impl AsyncWrite for LimitedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
