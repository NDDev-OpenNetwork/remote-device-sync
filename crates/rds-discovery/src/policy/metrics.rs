use crate::{
    clock::{Lease, Reading},
    observation::Observer,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(super) struct Stream {
    pub revision: u64,
    pub entries: u64,
    pub lease: Lease,
}

#[derive(Clone, Copy)]
pub(super) struct Summary {
    pub healthy: bool,
    pub durable: bool,
    pub epoch: u64,
    pub wall_floor: u64,
    pub streams: [Option<Stream>; 3],
}

impl Summary {
    pub(super) fn empty(epoch: u64, durable: bool) -> Self {
        Self {
            healthy: true,
            durable,
            epoch,
            wall_floor: 0,
            streams: [None; 3],
        }
    }
}

/// Weak metadata-only observer of a policy owner, with no file/lock ownership.
#[derive(Clone)]
pub struct PolicyMetrics(pub(super) Observer<Summary>);

impl PolicyMetrics {
    /// Nonblocking, no store locks, filesystem reads or signature verification.
    /// All fields in this policy group come from one completed commit/open.
    pub fn snapshot(&self) -> BTreeMap<&'static str, u64> {
        self.snapshot_at(Reading::cached_now())
    }

    pub(super) fn snapshot_at(&self, now: Option<Reading>) -> BTreeMap<&'static str, u64> {
        let summary = self.0.snapshot();
        let mut values =
            BTreeMap::from([("rds_directory_policy_known", u64::from(summary.is_some()))]);
        let Some(summary) = summary else {
            return values;
        };
        values.extend([
            ("rds_directory_policy_healthy", u64::from(summary.healthy)),
            ("rds_directory_policy_durable", u64::from(summary.durable)),
        ]);
        // An uncertain disk outcome cannot be presented as committed revision
        // or freshness. A successful reopen establishes the next observation.
        if !summary.healthy {
            return values;
        }
        values.insert("rds_directory_policy_epoch", summary.epoch);
        values.insert("rds_directory_policy_clock_known", u64::from(now.is_some()));
        for (stream, names) in summary.streams.into_iter().zip([
            [
                "rds_directory_registry_present",
                "rds_directory_registry_revision",
                "rds_directory_registry_entries",
                "rds_directory_registry_fresh",
                "rds_directory_registry_lease_remaining_seconds",
            ],
            [
                "rds_directory_revocations_present",
                "rds_directory_revocations_revision",
                "rds_directory_revocations_entries",
                "rds_directory_revocations_fresh",
                "rds_directory_revocations_lease_remaining_seconds",
            ],
            [
                "rds_directory_name_cache_present",
                "rds_directory_name_cache_revision",
                "rds_directory_name_cache_entries",
                "rds_directory_name_cache_fresh",
                "rds_directory_name_cache_lease_remaining_seconds",
            ],
        ]) {
            values.insert(names[0], u64::from(stream.is_some()));
            let Some(stream) = stream else { continue };
            values.insert(names[1], stream.revision);
            values.insert(names[2], stream.entries);
            if let Some(now) = now {
                let fresh = now.wall.as_secs() >= summary.wall_floor && stream.lease.valid_at(now);
                values.insert(names[3], u64::from(fresh));
                values.insert(
                    names[4],
                    if fresh {
                        stream.lease.remaining_at(now).as_secs()
                    } else {
                        0
                    },
                );
            }
        }
        values
    }
}
