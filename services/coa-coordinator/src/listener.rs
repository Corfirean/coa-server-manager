//! A TCP listener that never has more than a fixed number of connections open: a new one waits in the kernel backlog until a slot is free.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub struct LimitedListener {
    inner: TcpListener,
    slots: Arc<Semaphore>,
}

impl LimitedListener {
    pub fn new(inner: TcpListener, max_connections: usize) -> Self {
        Self { inner, slots: Arc::new(Semaphore::new(max_connections)) }
    }
}

pin_project! {
    pub struct LimitedIo {
        #[pin]
        stream: TcpStream,
        _slot: OwnedSemaphorePermit,
    }
}

impl AsyncRead for LimitedIo {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        self.project().stream.poll_read(cx, buf)
    }
}

impl AsyncWrite for LimitedIo {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        self.project().stream.poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().stream.poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.project().stream.poll_shutdown(cx)
    }
}

impl axum::serve::Listener for LimitedListener {
    type Io = LimitedIo;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let slot = self.slots.clone().acquire_owned().await.expect("the semaphore is never closed");
            match self.inner.accept().await {
                Ok((stream, addr)) => return (LimitedIo { stream, _slot: slot }, addr),
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

/// The address of the other end of a connection, whichever listener accepted it.
#[derive(Clone, Copy, Debug)]
pub struct Peer(pub SocketAddr);

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, LimitedListener>> for Peer {
    fn connect_info(stream: axum::serve::IncomingStream<'_, LimitedListener>) -> Self {
        Peer(*stream.remote_addr())
    }
}

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, TcpListener>> for Peer {
    fn connect_info(stream: axum::serve::IncomingStream<'_, TcpListener>) -> Self {
        Peer(*stream.remote_addr())
    }
}
