//! Connection metadata is local only; addresses and TLS material are excluded.
use hyper::rt::{Read, ReadBufCursor, Write};
use hyper_util::client::legacy::connect::{Connected, Connection};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};

#[derive(Clone)]
pub(super) struct ConnectionTiming {
    pub established: Instant,
    pub setup_us: u64,
}

pub(super) struct DiagnosticConnection<T> {
    inner: T,
    timing: Option<ConnectionTiming>,
}

impl<T> DiagnosticConnection<T> {
    pub(super) fn new(inner: T, started: Option<Instant>) -> Self {
        Self {
            inner,
            timing: started.map(|started| ConnectionTiming {
                established: Instant::now(),
                setup_us: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            }),
        }
    }
}

impl<T: Connection> Connection for DiagnosticConnection<T> {
    fn connected(&self) -> Connected {
        let connected = self.inner.connected();
        match &self.timing {
            Some(timing) => connected.extra(timing.clone()),
            None => connected,
        }
    }
}

impl<T: Read + Unpin> Read for DiagnosticConnection<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl<T: Write + Unpin> Write for DiagnosticConnection<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write_vectored(cx, bufs)
    }
}
