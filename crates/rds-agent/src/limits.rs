//! Application admission limits, independent of transport flow-control credit.

use std::num::NonZeroU16;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Per-agent connection slots and per-connection service tasks. The constructor
/// requires positive values; callers may choose smaller budgets for their host.
#[derive(Clone, Copy, Debug)]
pub struct AgentLimits {
    connections: usize,
    streams: usize,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            connections: 32,
            streams: 64,
        }
    }
}

impl AgentLimits {
    pub fn new(connections: NonZeroU16, streams: NonZeroU16) -> Self {
        Self {
            connections: usize::from(connections.get()),
            streams: usize::from(streams.get()),
        }
    }

    /// Pending handshakes plus admitted connections, shared by run and serve.
    pub fn connections(self) -> usize {
        self.connections
    }

    /// Concurrent service tasks in each connection, including hello/Authz I/O.
    pub fn streams(self) -> usize {
        self.streams
    }
}

#[derive(Clone, Default)]
pub(super) struct StreamCounter(Arc<AtomicUsize>);

impl StreamCounter {
    pub fn active(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }

    pub fn enter(&self) -> StreamTask {
        self.0.fetch_add(1, Ordering::Relaxed);
        StreamTask(self.0.clone())
    }
}

pub(super) struct StreamTask(Arc<AtomicUsize>);

impl Drop for StreamTask {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
