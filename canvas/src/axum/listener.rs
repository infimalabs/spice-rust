//! Listener cancellation that interrupts accepted Canvas transports.

use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

#[derive(Debug)]
pub(super) struct ClosingListener {
    pub(super) listener: tokio::net::TcpListener,
    pub(super) connections: CancellationToken,
}

impl ::axum::serve::Listener for ClosingListener {
    type Io = ClosingStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (stream, address) = ::axum::serve::Listener::accept(&mut self.listener).await;
        (
            ClosingStream {
                stream,
                cancelled: Box::pin(self.connections.clone().cancelled_owned()),
            },
            address,
        )
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

#[derive(Debug)]
pub(super) struct ClosingStream {
    stream: TcpStream,
    cancelled: Pin<Box<WaitForCancellationFutureOwned>>,
}

impl AsyncRead for ClosingStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.cancelled.as_mut().poll(context).is_ready() {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.stream).poll_read(context, buffer)
    }
}

impl AsyncWrite for ClosingStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.cancelled.as_mut().poll(context).is_ready() {
            return Poll::Ready(Err(connection_closed()));
        }
        Pin::new(&mut self.stream).poll_write(context, buffer)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.cancelled.as_mut().poll(context).is_ready() {
            return Poll::Ready(Err(connection_closed()));
        }
        Pin::new(&mut self.stream).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(context)
    }
}

fn connection_closed() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "canvas connection closed with the resident",
    )
}
