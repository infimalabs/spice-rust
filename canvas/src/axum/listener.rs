//! Listener acquisition and cancellation that interrupts accepted Canvas transports.

use std::io::IoSlice;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpSocket, TcpStream};
use tokio_util::sync::{CancellationToken, WaitForCancellationFutureOwned};

/// Pending connections the kernel queues, matching `TcpListener::bind`.
const LISTEN_BACKLOG: u32 = 1024;
/// How long the served-address probe waits; every probed address is local.
const SERVED_PROBE_TIMEOUT: Duration = Duration::from_millis(250);

/// Listen on `requested` only when no other listener already serves it.
///
/// Address reuse lets a restart bind while its earlier connections sit in
/// TIME_WAIT, but on BSD-derived stacks it also admits a specific address
/// beside a wildcard listener on the same port, and the reverse. Both bind,
/// and local clients silently reach whichever is more specific. The probe
/// runs before bind, because a bound socket already captures connections to
/// its exact address even before it listens. An ephemeral request has no
/// port to probe: the kernel assigns only a free one.
pub(super) async fn listen_exclusively(
    requested: SocketAddr,
) -> std::io::Result<tokio::net::TcpListener> {
    let probe = SocketAddr::new(served_probe_ip(requested.ip()), requested.port());
    if requested.port() != 0
        && matches!(
            tokio::time::timeout(SERVED_PROBE_TIMEOUT, TcpStream::connect(probe)).await,
            Ok(Ok(_))
        )
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("{requested} is already served by another listener"),
        ));
    }
    let socket = if requested.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    socket.set_reuseaddr(true)?;
    socket.bind(requested)?;
    socket.listen(LISTEN_BACKLOG)
}

/// Where a client of `ip` connects: a wildcard is reached through loopback.
fn served_probe_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    }
}

#[derive(Debug)]
pub(super) struct ClosingListener {
    listener: tokio::net::TcpListener,
    connections: CancellationToken,
}

impl ClosingListener {
    pub(super) fn new(listener: tokio::net::TcpListener, connections: CancellationToken) -> Self {
        Self {
            listener,
            connections,
        }
    }

    pub(super) fn bound_address(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }
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

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffers: &[IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        if self.cancelled.as_mut().poll(context).is_ready() {
            return Poll::Ready(Err(connection_closed()));
        }
        Pin::new(&mut self.stream).poll_write_vectored(context, buffers)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
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

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWrite as _;

    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn closing_stream_preserves_tcp_vectored_writes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture listener binds");
        let address = listener.local_addr().expect("fixture address");
        let client = tokio::spawn(TcpStream::connect(address));
        let (stream, _) = listener.accept().await.expect("fixture stream accepts");
        let stream = ClosingStream {
            stream,
            cancelled: Box::pin(CancellationToken::new().cancelled_owned()),
        };
        client
            .await
            .expect("fixture connect task completes")
            .expect("fixture client connects");

        assert!(
            stream.stream.is_write_vectored(),
            "the fixture platform's TCP transport supports vectored writes"
        );
        assert!(
            stream.is_write_vectored(),
            "the cancellation wrapper must not make Hyper flatten large bodies"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_port_another_listener_serves_is_refused_and_left_serving() {
        for (held, requested) in [
            ("0.0.0.0", "127.0.0.1"),
            ("127.0.0.1", "0.0.0.0"),
            ("127.0.0.1", "127.0.0.1"),
            ("::", "::1"),
            ("::1", "::"),
        ] {
            let holder = tokio::net::TcpListener::bind((held, 0))
                .await
                .expect("fixture holder binds");
            let port = holder.local_addr().expect("holder address").port();
            let requested: SocketAddr =
                (requested.parse::<IpAddr>().expect("fixture ip"), port).into();

            let refused = listen_exclusively(requested)
                .await
                .expect_err("an overlapping served port must not bind");
            assert_eq!(
                refused.kind(),
                std::io::ErrorKind::AddrInUse,
                "{held} then {requested}"
            );

            let reached = SocketAddr::new(served_probe_ip(requested.ip()), port);
            let client = tokio::spawn(TcpStream::connect(reached));
            holder.accept().await.expect("the holder keeps serving");
            client
                .await
                .expect("fixture connect task completes")
                .expect("fixture client connects");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_restart_binds_while_its_closed_connections_wait() {
        let first = listen_exclusively("127.0.0.1:0".parse().expect("fixture address"))
            .await
            .expect("first listener binds");
        let address = first.local_addr().expect("first address");
        let client = tokio::spawn(TcpStream::connect(address));
        let (accepted, _) = first.accept().await.expect("first listener accepts");
        let mut client = client
            .await
            .expect("fixture connect task completes")
            .expect("fixture client connects");
        drop(accepted);
        let mut byte = [0; 1];
        tokio::io::AsyncReadExt::read(&mut client, &mut byte)
            .await
            .expect("the server close reaches the client");
        drop(client);
        drop(first);

        listen_exclusively(address)
            .await
            .expect("the server-closed connection's TIME_WAIT must not block a restart");
    }
}
