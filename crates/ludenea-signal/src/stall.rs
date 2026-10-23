//! A connection whose writes may not stay blocked for long: a client that stops reading
//! cannot hold its connection, and what is queued for it, forever.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

/// `inner`, whose writes, flushes and shutdown fail with [`io::ErrorKind::TimedOut`] once
/// one stays pending for `limit`.
pub(crate) struct WriteDeadline<S> {
    inner: S,
    limit: Duration,
    /// Armed while a write is pending.
    stalled: Option<Pin<Box<Sleep>>>,
}

impl<S> WriteDeadline<S> {
    pub(crate) fn new(inner: S, limit: Duration) -> WriteDeadline<S> {
        WriteDeadline {
            inner,
            limit,
            stalled: None,
        }
    }

    fn check<T>(&mut self, cx: &mut Context<'_>, poll: Poll<io::Result<T>>) -> Poll<io::Result<T>> {
        if poll.is_ready() {
            self.stalled = None;
            return poll;
        }
        let limit = self.limit;
        let stalled = self
            .stalled
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(limit)));
        match stalled.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("the client read nothing for {} seconds", limit.as_secs()),
            ))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for WriteDeadline<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for WriteDeadline<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write(cx, buf);
        self.check(cx, poll)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        self.check(cx, poll)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let poll = Pin::new(&mut self.inner).poll_flush(cx);
        self.check(cx, poll)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let poll = Pin::new(&mut self.inner).poll_shutdown(cx);
        self.check(cx, poll)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test(start_paused = true)]
    async fn a_write_blocked_for_the_limit_fails() {
        // A pipe of 64 bytes whose other end never reads.
        let (writer, _reader) = tokio::io::duplex(64);
        let mut guarded = WriteDeadline::new(writer, Duration::from_secs(1));
        guarded.write_all(&[0; 64]).await.unwrap();
        let error = guarded.write_all(&[0; 1]).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test(start_paused = true)]
    async fn writes_that_proceed_never_time_out() {
        let (writer, mut reader) = tokio::io::duplex(64);
        let mut guarded = WriteDeadline::new(writer, Duration::from_secs(1));
        let drain = tokio::spawn(async move {
            let mut sink = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut sink)
                .await
                .map(|_| sink.len())
        });
        guarded.write_all(&[0; 4096]).await.unwrap();
        guarded.shutdown().await.unwrap();
        drop(guarded);
        assert_eq!(drain.await.unwrap().unwrap(), 4096);
    }
}
