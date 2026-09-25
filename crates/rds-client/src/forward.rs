//! Owned local TCP forwarding workers.
use rds_core::TcpTarget;
use rds_net::Connection;
use std::net::SocketAddr;
use std::num::NonZeroU16;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::task::JoinSet;

use crate::request::RequestStreams;

/// Default number of local connections served concurrently by one listener.
// A positive compile-time literal, so constructing this limit cannot fail.
pub const DEFAULT_FORWARD_LIMIT: NonZeroU16 = NonZeroU16::new(64).unwrap();

/// Listen on `bind` and forward local TCP sockets. Uses the default 64-worker
/// budget; use `forward_bound_listener` to supply a listener and explicit limit.
pub async fn forward_listener(
    conn: Arc<Connection>,
    bind: SocketAddr,
    remote_host: String,
    remote_port: u16,
) -> anyhow::Result<()> {
    let target = TcpTarget::new(&remote_host, remote_port)?;
    let listener = TcpListener::bind(bind).await?;
    forward_bound_listener(conn, listener, target, DEFAULT_FORWARD_LIMIT).await
}

/// Serve an already-bound local listener with a positive worker limit. At
/// capacity, stop accepting until a worker ends (the OS backlog is separate).
/// QUIC closure ends the listener and joins canceled workers. Dropping this
/// future aborts its children; it does not close the shared QUIC connection.
pub async fn forward_bound_listener(
    conn: Arc<Connection>,
    listener: TcpListener,
    target: TcpTarget,
    max_connections: NonZeroU16,
) -> anyhow::Result<()> {
    println!("listening on {} -> {target}", listener.local_addr()?);
    let mut workers = JoinSet::new();
    let result = loop {
        tokio::select! {
            biased;
            _ = conn.wait_closed() => break Ok(()),
            result = workers.join_next(), if !workers.is_empty() => {
                if let Some(Err(error)) = result { tracing::debug!(%error, "forward worker ended"); }
            }
            incoming = listener.accept(), if workers.len() < usize::from(max_connections.get()) => {
                let (mut local, peer) = match incoming {
                    Ok(pair) => pair,
                    Err(error) => break Err(error.into()),
                };
                let conn = conn.clone();
                let target = target.clone();
                workers.spawn(async move {
                    let (host, port) = target.into_parts();
                    match crate::open_tcp(&conn, &host, port).await {
                        Ok(pair) => {
                            // Reset/stop on cancellation too: ordinary SendStream
                            // drop could otherwise finish buffered request/body data.
                            let mut streams = RequestStreams::new(pair);
                            let (send, recv) = streams.get_mut();
                            let mut remote = tokio::io::join(recv, send);
                            if let Err(error) = tokio::io::copy_bidirectional(&mut local, &mut remote).await {
                                tracing::debug!(%peer, %error, "forward closed");
                            }
                        }
                        Err(error) => tracing::warn!(%peer, %error, "forward rejected"),
                    }
                });
            }
        }
    };
    drop(listener);
    workers.shutdown().await;
    result
}
