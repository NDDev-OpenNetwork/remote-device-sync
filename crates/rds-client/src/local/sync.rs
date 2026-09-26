//! File I/O delegated by the authenticated same-UID CLI to its agent.
use std::path::Path;
use std::sync::Arc;

use rds_core::local::{Command, ErrorCode, Reply, SessionId, SyncOperation, SyncStats};
use rds_core::{HelloAck, StreamHello};
use tokio::sync::Semaphore;

use super::{Client, Error, state};

pub(super) const MAX_TRANSFERS: usize = 8;
const MAX_PATH: usize = 8192;

fn local_path(path: &str) -> Result<&Path, ErrorCode> {
    if path.len() > MAX_PATH || path.contains('\0') || !Path::new(path).is_absolute() {
        return Err(ErrorCode::InvalidRequest);
    }
    Ok(Path::new(path))
}

pub(super) async fn run(
    shared: &state::Shared,
    session: SessionId,
    operation: SyncOperation,
    transfers: Arc<Semaphore>,
) -> Result<Reply, ErrorCode> {
    // Refuse malformed paths before opening a service or touching files.
    match &operation {
        SyncOperation::Send { path } => {
            local_path(path)?;
        }
        SyncOperation::Recv {
            rel_path,
            directory,
        } => {
            local_path(directory)?;
            if rel_path.len() > MAX_PATH || rds_sync::proto::check_rel_path(rel_path).is_err() {
                return Err(ErrorCode::InvalidRequest);
            }
        }
    }
    let _budget = transfers
        .try_acquire_owned()
        .map_err(|_| ErrorCode::Capacity)?;
    let (conn, _slot) = {
        let state = state::lock(shared)?;
        let (_, conn) = state.connection(Some(session))?;
        let entry = state.entries.get(&session).ok_or(ErrorCode::NotFound)?;
        let slot = entry
            .sync
            .clone()
            .try_acquire_owned()
            .map_err(|_| ErrorCode::TransferBusy)?;
        (conn, slot)
    };
    // New route even after cancellation: delayed old tags can only be dropped,
    // never routed into a replacement transfer. Do not retry the legacy hello.
    let id = rand::random();
    let streams = crate::request::bounded("sync transfer", async {
        let (streams, ack) =
            crate::request::exchange(&conn, &StreamHello::SyncTransfer { id }).await?;
        match ack {
            HelloAck::Ok => Ok(streams.release()),
            _ => anyhow::bail!("sync transfer not accepted"),
        }
    })
    .await
    .map_err(|_| ErrorCode::Remote)?;
    let transfer = rds_sync::engine::Transfer::new(id);
    let timeout = rds_sync::engine::TRANSFER_TIMEOUT;
    let stats = match operation {
        SyncOperation::Send { path } => {
            transfer
                .send_file(&conn, Path::new(&path), streams, timeout)
                .await
        }
        SyncOperation::Recv {
            rel_path,
            directory,
        } => transfer
            .recv_file(&conn, &rel_path, Path::new(&directory), streams, timeout)
            .await
            .map(|(_, stats)| stats),
    }
    .map_err(|_| ErrorCode::Transfer)?;
    Ok(Reply::Synced {
        session,
        stats: SyncStats {
            fetched: stats.fetched,
            total: stats.total,
            bytes: stats.bytes,
        },
    })
}

fn absolute(path: &Path) -> Result<String, Error> {
    let path = std::path::absolute(path)?;
    let text = path.to_str().ok_or(ErrorCode::InvalidRequest)?;
    local_path(text)?;
    Ok(text.to_owned())
}

impl Client {
    async fn sync(&self, session: SessionId, operation: SyncOperation) -> Result<SyncStats, Error> {
        match self.request(Command::Sync { session, operation }).await? {
            Reply::Synced {
                session: returned,
                stats,
            } if returned == session => Ok(stats),
            _ => Err(Error::Protocol),
        }
    }

    /// Delegate a regular local file to the same-UID agent's pinned session.
    /// Dropping the future closes IPC and cancels this transfer. An already
    /// started filesystem commit may still complete; never automatically replay.
    pub async fn send_file(&self, session: SessionId, path: &Path) -> Result<SyncStats, Error> {
        self.sync(
            session,
            SyncOperation::Send {
                path: absolute(path)?,
            },
        )
        .await
    }

    /// Receive into the caller-resolved absolute directory, through the sync
    /// engine's confined journal and atomic verified-file assembly.
    pub async fn recv_file(
        &self,
        session: SessionId,
        rel_path: &str,
        directory: &Path,
    ) -> Result<SyncStats, Error> {
        self.sync(
            session,
            SyncOperation::Recv {
                rel_path: rel_path.to_owned(),
                directory: absolute(directory)?,
            },
        )
        .await
    }
}
