//! Network side: one Modbus TCP listener answering for every simulated unit id.

use crate::image::SimState;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpSocket, TcpStream};
use tokio::sync::watch;
use tokio_modbus::server::tcp::Server;
use tokio_modbus::server::Service;
use tokio_modbus::{Exception, Response, SlaveRequest};

pub(crate) type Shared = Arc<Mutex<SimState>>;

pub(crate) fn lock(state: &Shared) -> std::sync::MutexGuard<'_, SimState> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) struct SimService(pub(crate) Shared);

impl Service for SimService {
    type Request = SlaveRequest<'static>;
    type Response = Response;
    type Exception = Exception;
    type Future = std::future::Ready<Result<Response, Exception>>;

    fn call(&self, req: Self::Request) -> Self::Future {
        std::future::ready(lock(&self.0).handle(req.slave, req.request))
    }
}

/// A TCP stream that reads EOF as soon as the simulator stops, so stopping it
/// drops every open connection like a device being switched off.
pub(crate) struct Killable {
    inner: TcpStream,
    kill: Pin<Box<dyn Future<Output = ()> + Send>>,
    killed: bool,
}

impl Killable {
    pub(crate) fn new(inner: TcpStream, mut stop: watch::Receiver<bool>) -> Self {
        let kill = Box::pin(async move {
            // A dropped sender also means "stopped".
            let _ = stop.wait_for(|stopped| *stopped).await;
        });
        Self { inner, kill, killed: false }
    }

    fn poll_killed(&mut self, cx: &mut Context<'_>) -> bool {
        if !self.killed && self.kill.as_mut().poll(cx).is_ready() {
            self.killed = true;
        }
        self.killed
    }
}

impl AsyncRead for Killable {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.poll_killed(cx) {
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for Killable {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.poll_killed(cx) {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        Pin::new(&mut this.inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

pub(crate) fn bind(addr: SocketAddr) -> io::Result<TcpListener> {
    let socket = if addr.is_ipv4() { TcpSocket::new_v4()? } else { TcpSocket::new_v6()? };
    // Lets a restarted simulator take its port back while old connections
    // linger in TIME_WAIT. On Windows the flag would also allow two live
    // listeners on one port, so it is left off there.
    #[cfg(not(windows))]
    socket.set_reuseaddr(true)?;
    socket.bind(addr)?;
    socket.listen(128)
}

/// Accepts connections until `stop` turns true.
pub(crate) async fn accept_loop(listener: TcpListener, state: Shared, stop: watch::Receiver<bool>) -> io::Result<()> {
    let server = Server::new(listener);
    let on_connected = |stream: TcpStream, peer: SocketAddr| {
        let service = SimService(Arc::clone(&state));
        let stop = stop.clone();
        async move {
            tracing::debug!(%peer, "simulator: client connected");
            let _ = stream.set_nodelay(true);
            Ok(Some((service, Killable::new(stream, stop))))
        }
    };
    let on_error = |e: io::Error| tracing::debug!("simulator: connection ended: {e}");
    let mut stop_wait = stop.clone();
    tokio::select! {
        r = server.serve(&on_connected, on_error) => r,
        _ = stop_wait.wait_for(|s| *s) => Ok(()),
    }
}
