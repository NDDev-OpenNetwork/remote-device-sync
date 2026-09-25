//! Cancellation-safe ownership of a request's QUIC streams.
use std::future::Future;
use std::time::Duration;

use anyhow::Context;
use rds_core::{HelloAck, StreamHello, read_frame, write_frame};
use rds_net::{Connection, RecvStream, SendStream};

/// One deadline includes stream-credit wait, framed write and response/body.
/// Long-lived TCP/sync bodies start after this prelude and use their own policy.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) async fn bounded<T>(
    service: &'static str,
    request: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    tokio::time::timeout(REQUEST_TIMEOUT, request)
        .await
        .with_context(|| format!("{service} request timed out after {REQUEST_TIMEOUT:?}"))?
}

/// A canceled or rejected prelude must reset its buffered request rather than
/// implicitly finishing it on SendStream::drop. Only successful streaming
/// services transfer ownership to their caller.
pub(crate) struct RequestStreams(Option<(SendStream, RecvStream)>);

impl RequestStreams {
    pub(crate) fn new(streams: (SendStream, RecvStream)) -> Self {
        Self(Some(streams))
    }

    pub(crate) fn get_mut(&mut self) -> (&mut SendStream, &mut RecvStream) {
        // Invariant: release consumes self, so every live borrower has a pair.
        let (send, recv) = self.0.as_mut().expect("live request has streams");
        (send, recv)
    }

    /// One-shot responses end after their declared payload. Observe FIN inside
    /// the same deadline before releasing the guard, so a successful Authz does
    /// not race a STOP_SENDING against the server's ACK finish/commit path.
    pub(crate) async fn complete(mut self) -> anyhow::Result<()> {
        self.get_mut().1.read_to_end(0).await?;
        let (mut send, _recv) = self.release();
        // The peer may already have stopped reading a complete one-shot request.
        let _ = send.finish();
        Ok(())
    }

    pub(crate) fn release(mut self) -> (SendStream, RecvStream) {
        // Invariant: this is the sole consuming extraction.
        self.0.take().expect("live request has streams")
    }
}

impl Drop for RequestStreams {
    fn drop(&mut self) {
        if let Some((send, recv)) = &mut self.0 {
            let _ = send.reset(0u32.into());
            let _ = recv.stop(0u32.into());
        }
    }
}

pub(crate) async fn exchange(
    conn: &Connection,
    hello: &StreamHello,
) -> anyhow::Result<(RequestStreams, HelloAck)> {
    let mut streams = RequestStreams::new(conn.open_bi().await?);
    let (send, recv) = streams.get_mut();
    write_frame(send, hello).await?;
    let ack = read_frame(recv).await?;
    Ok((streams, ack))
}

/// Authz is a connection transaction: cancel/error must close that connection,
/// even if a successful ACK was lost after the server reserved the grant.
pub(crate) struct Authorization<'a> {
    conn: &'a Connection,
    committed: bool,
}

impl<'a> Authorization<'a> {
    pub(crate) fn new(conn: &'a Connection) -> Self {
        Self {
            conn,
            committed: false,
        }
    }
    pub(crate) fn commit(&mut self) {
        self.committed = true;
    }
}
impl Drop for Authorization<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.conn
                .close(2u32.into(), b"authorization did not complete");
        }
    }
}
