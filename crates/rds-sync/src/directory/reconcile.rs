//! Deterministic reconcile planning without filesystem mutation.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    DirectoryEntry, DirectoryManifest, EntryKind, MAX_DIRECTORY_DATA_BYTES, MAX_DIRECTORY_ENTRIES,
};
use crate::{ChunkHash, SyncError};

/// Opaque snapshot revision. Callers must persist and compare it verbatim;
/// the digest layout is intentionally not part of the sync API contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Revision(pub ChunkHash);

impl Revision {
    pub fn from_manifest(manifest: &DirectoryManifest) -> Result<Self, SyncError> {
        manifest.verify()?;
        Ok(Self(manifest.root))
    }
}

/// Whether destination-only paths are retained or removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeletePolicy {
    Keep,
    Delete,
}

/// A durable deletion intent. Apply code must journal this record before
/// removing the previous destination entry and retain it until convergence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tombstone {
    pub path: String,
    pub previous: DirectoryEntry,
    pub source_revision: Revision,
}

/// Filesystem-independent operations emitted by the one-way planner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReconcileOperation {
    Put {
        entry: DirectoryEntry,
    },
    Replace {
        before: DirectoryEntry,
        after: DirectoryEntry,
    },
    Move {
        from: DirectoryEntry,
        to: DirectoryEntry,
    },
    Delete {
        tombstone: Tombstone,
    },
}

impl ReconcileOperation {
    fn path(&self) -> &str {
        match self {
            Self::Put { entry } => &entry.path,
            Self::Replace { after, .. } => &after.path,
            Self::Move { to, .. } => &to.path,
            Self::Delete { tombstone } => &tombstone.path,
        }
    }

    fn validate(&self) -> Result<(), SyncError> {
        match self {
            Self::Put { entry } => entry.validate(),
            Self::Replace { before, after } => {
                before.validate()?;
                after.validate()?;
                if before.path != after.path || before.identity() == after.identity() {
                    return Err(SyncError::Manifest(
                        "invalid replacement precondition".into(),
                    ));
                }
                if before.kind == EntryKind::Directory || after.kind == EntryKind::Directory {
                    return Err(SyncError::Manifest(
                        "directory type replacement requires subtree conflict resolution".into(),
                    ));
                }
                Ok(())
            }
            Self::Move { from, to } => {
                from.validate()?;
                to.validate()?;
                if from.kind == EntryKind::Directory || to.kind == EntryKind::Directory {
                    return Err(SyncError::Manifest(
                        "directory moves require an explicit subtree identity".into(),
                    ));
                }
                if from.path == to.path || from.identity() != to.identity() {
                    return Err(SyncError::Manifest(
                        "move operation changes entry identity".into(),
                    ));
                }
                Ok(())
            }
            Self::Delete { tombstone } => {
                tombstone.previous.validate()?;
                if tombstone.previous.path != tombstone.path {
                    return Err(SyncError::Manifest(
                        "tombstone path does not match its previous entry".into(),
                    ));
                }
                Ok(())
            }
        }
    }
}

/// A deterministic one-way plan. The destination revision is an optimistic
/// precondition: apply must refuse if the destination changed after planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcilePlan {
    pub source_revision: Revision,
    pub expected_destination_revision: Revision,
    pub delete_policy: DeletePolicy,
    pub operations: Vec<ReconcileOperation>,
}

impl ReconcilePlan {
    pub fn one_way(
        source: &DirectoryManifest,
        destination: &DirectoryManifest,
        delete_policy: DeletePolicy,
    ) -> Result<Self, SyncError> {
        let source_revision = Revision::from_manifest(source)?;
        let expected_destination_revision = Revision::from_manifest(destination)?;
        // `destination.diff(source)` makes `added` and `modified` refer to
        // source entries, while `removed` refers to destination entries.
        let diff = destination.diff(source)?;
        let mut operations = Vec::new();
        for rename in diff.renamed {
            if delete_policy == DeletePolicy::Keep {
                // Moving would delete a destination-only path under Keep.
                operations.push(ReconcileOperation::Put { entry: rename.to });
            } else {
                operations.push(ReconcileOperation::Move {
                    from: rename.from,
                    to: rename.to,
                });
            }
        }
        for entry in diff.added {
            operations.push(ReconcileOperation::Put { entry });
        }
        for entry in diff.modified {
            let index = destination
                .entries
                .binary_search_by(|candidate| candidate.path.cmp(&entry.path))
                .map_err(|_| {
                    SyncError::Manifest("modified destination entry disappeared".into())
                })?;
            let before = destination.entries[index].clone();
            operations.push(ReconcileOperation::Replace {
                before,
                after: entry,
            });
        }
        if delete_policy == DeletePolicy::Delete {
            for previous in diff.removed {
                operations.push(ReconcileOperation::Delete {
                    tombstone: Tombstone {
                        path: previous.path.clone(),
                        previous,
                        source_revision,
                    },
                });
            }
        }
        let plan = Self {
            source_revision,
            expected_destination_revision,
            delete_policy,
            operations,
        };
        plan.canonicalize_and_verify()
    }

    pub fn verify(&self) -> Result<(), SyncError> {
        if self.operations.len() > MAX_DIRECTORY_ENTRIES.saturating_mul(2) {
            return Err(SyncError::Manifest("reconcile plan is too large".into()));
        }
        let mut paths = BTreeSet::new();
        let mut move_sources = BTreeSet::new();
        let mut data_bytes = 0usize;
        for operation in &self.operations {
            operation.validate()?;
            if !paths.insert(operation.path()) {
                return Err(SyncError::Manifest(
                    "reconcile plan contains duplicate target paths".into(),
                ));
            }
            let entries: &[&DirectoryEntry] = match operation {
                ReconcileOperation::Put { entry } => &[entry],
                ReconcileOperation::Replace { before, after } => &[before, after],
                ReconcileOperation::Move { from, to } => {
                    if self.delete_policy != DeletePolicy::Delete
                        || !move_sources.insert(from.path.as_str())
                    {
                        return Err(SyncError::Manifest(
                            "invalid move policy or duplicate source".into(),
                        ));
                    }
                    &[from, to]
                }
                ReconcileOperation::Delete { tombstone } => {
                    if self.delete_policy != DeletePolicy::Delete
                        || tombstone.source_revision != self.source_revision
                    {
                        return Err(SyncError::Manifest(
                            "invalid tombstone policy or revision".into(),
                        ));
                    }
                    &[&tombstone.previous]
                }
            };
            for entry in entries {
                data_bytes = data_bytes
                    .saturating_add(entry.path.len())
                    .saturating_add(entry.symlink_target.as_ref().map_or(0, Vec::len));
                if data_bytes > 2 * MAX_DIRECTORY_DATA_BYTES {
                    return Err(SyncError::Manifest(
                        "reconcile metadata budget exceeded".into(),
                    ));
                }
            }
        }
        if !paths.is_disjoint(&move_sources) {
            return Err(SyncError::Manifest(
                "move source overlaps a plan target".into(),
            ));
        }
        if self
            .operations
            .windows(2)
            .any(|pair| operation_sort_key(&pair[0]) > operation_sort_key(&pair[1]))
        {
            return Err(SyncError::Manifest(
                "reconcile plan is not in canonical operation order".into(),
            ));
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<ChunkHash, SyncError> {
        self.verify()?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"rds-reconcile-plan-v1\0");
        let header = postcard::to_stdvec(&(
            self.source_revision,
            self.expected_destination_revision,
            self.delete_policy,
            self.operations.len(),
        ))
        .map_err(|_| SyncError::Manifest("reconcile plan cannot be encoded".into()))?;
        hasher.update(&header);
        for operation in &self.operations {
            let bytes = postcard::to_stdvec(operation)
                .map_err(|_| SyncError::Manifest("reconcile operation cannot be encoded".into()))?;
            hasher.update(&(bytes.len() as u64).to_le_bytes());
            hasher.update(&bytes);
        }
        Ok(*hasher.finalize().as_bytes())
    }

    pub fn check_destination(&self, current: &DirectoryManifest) -> Result<(), SyncError> {
        self.project_destination(current).map(|_| ())
    }

    /// Validate every operation against the destination's actual entries and
    /// return the predicted result without filesystem I/O. Structural `verify`
    /// alone cannot establish the truth of peer-supplied preconditions.
    pub fn project_destination(
        &self,
        current: &DirectoryManifest,
    ) -> Result<DirectoryManifest, SyncError> {
        self.verify()?;
        let current_revision = Revision::from_manifest(current)?;
        if current_revision != self.expected_destination_revision {
            return Err(SyncError::Manifest(
                "destination changed after reconcile planning".into(),
            ));
        }
        let mut entries: BTreeMap<String, DirectoryEntry> = current
            .entries
            .iter()
            .map(|entry| (entry.path.clone(), entry.clone()))
            .collect();
        for operation in &self.operations {
            match operation {
                ReconcileOperation::Put { entry } => {
                    if entries.contains_key(&entry.path) {
                        return Err(SyncError::Manifest("Put destination already exists".into()));
                    }
                    entries.insert(entry.path.clone(), entry.clone());
                }
                ReconcileOperation::Replace { before, after } => {
                    require_previous(&entries, before)?;
                    entries.insert(after.path.clone(), after.clone());
                }
                ReconcileOperation::Move { from, to } => {
                    require_previous(&entries, from)?;
                    if entries.contains_key(&to.path) {
                        return Err(SyncError::Manifest(
                            "Move destination already exists".into(),
                        ));
                    }
                    entries.remove(&from.path);
                    entries.insert(to.path.clone(), to.clone());
                }
                ReconcileOperation::Delete { tombstone } => {
                    require_previous(&entries, &tombstone.previous)?;
                    entries.remove(&tombstone.path);
                }
            }
        }
        DirectoryManifest::from_entries(entries.into_values().collect())
    }

    /// Require this plan to be exactly the deterministic plan derived from
    /// both validated inputs. A root digest is content identity, not authority
    /// to submit arbitrary filesystem operations.
    pub fn check_inputs(
        &self,
        source: &DirectoryManifest,
        destination: &DirectoryManifest,
    ) -> Result<(), SyncError> {
        self.verify()?;
        let expected = Self::one_way(source, destination, self.delete_policy)?;
        if self != &expected {
            return Err(SyncError::Manifest(
                "reconcile plan does not match its inputs".into(),
            ));
        }
        self.project_destination(destination)?;
        Ok(())
    }

    fn canonicalize_and_verify(mut self) -> Result<Self, SyncError> {
        self.operations
            .sort_by(|left, right| operation_sort_key(left).cmp(&operation_sort_key(right)));
        self.verify()?;
        Ok(self)
    }
}

fn require_previous(
    entries: &BTreeMap<String, DirectoryEntry>,
    previous: &DirectoryEntry,
) -> Result<(), SyncError> {
    if entries.get(&previous.path) != Some(previous) {
        return Err(SyncError::Manifest(
            "reconcile entry precondition does not match destination".into(),
        ));
    }
    Ok(())
}

/// A conflict is reported when both sides changed the same path from a common
/// base and their resulting identities differ. No automatic winner is chosen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictRecord {
    pub path: String,
    pub base: Option<DirectoryEntry>,
    pub local: Option<DirectoryEntry>,
    pub remote: Option<DirectoryEntry>,
}

pub fn conflicts(
    base: &DirectoryManifest,
    local: &DirectoryManifest,
    remote: &DirectoryManifest,
) -> Result<Vec<ConflictRecord>, SyncError> {
    base.verify()?;
    local.verify()?;
    remote.verify()?;
    let maps = [entry_map(base), entry_map(local), entry_map(remote)];
    let paths: BTreeSet<&str> = maps.iter().flat_map(|map| map.keys().copied()).collect();
    let mut conflict_paths = BTreeSet::new();
    for path in &paths {
        let path = *path;
        let values = [
            maps[0].get(path).copied(),
            maps[1].get(path).copied(),
            maps[2].get(path).copied(),
        ];
        let local_changed = values[0] != values[1];
        let remote_changed = values[0] != values[2];
        if local_changed && remote_changed && values[1] != values[2] {
            conflict_paths.insert(path);
        }
        // Removing/replacing a directory conflicts with changes below it,
        // including a newly added descendant that was absent in the base.
        for (side, changed) in [(1, local_changed), (2, remote_changed)] {
            if !changed {
                continue;
            }
            let other = 3 - side;
            let mut ancestor = path;
            while let Some((parent, _)) = ancestor.rsplit_once('/') {
                let base_dir = maps[0]
                    .get(parent)
                    .is_some_and(|entry| entry.kind == EntryKind::Directory);
                let side_dir = maps[side]
                    .get(parent)
                    .is_some_and(|entry| entry.kind == EntryKind::Directory);
                let other_dir = maps[other]
                    .get(parent)
                    .is_some_and(|entry| entry.kind == EntryKind::Directory);
                if base_dir && side_dir && !other_dir {
                    conflict_paths.insert(parent);
                }
                ancestor = parent;
            }
        }
    }
    Ok(conflict_paths
        .into_iter()
        .map(|path| ConflictRecord {
            path: path.to_owned(),
            base: maps[0].get(path).copied().cloned(),
            local: maps[1].get(path).copied().cloned(),
            remote: maps[2].get(path).copied().cloned(),
        })
        .collect())
}

fn entry_map(manifest: &DirectoryManifest) -> BTreeMap<&str, &DirectoryEntry> {
    manifest
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect()
}

fn operation_sort_key(operation: &ReconcileOperation) -> (u8, std::cmp::Reverse<usize>, &[u8]) {
    let rank = match operation {
        ReconcileOperation::Put { entry } if entry.kind == EntryKind::Directory => 0,
        ReconcileOperation::Move { .. } => 1,
        ReconcileOperation::Put { .. } => 2,
        ReconcileOperation::Replace { .. } => 3,
        ReconcileOperation::Delete { tombstone }
            if tombstone.previous.kind != EntryKind::Directory =>
        {
            4
        }
        ReconcileOperation::Delete { .. } => 5,
    };
    let delete_depth = if rank >= 4 {
        operation.path().split('/').count()
    } else {
        0
    };
    (
        rank,
        std::cmp::Reverse(delete_depth),
        operation.path().as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, byte: u8) -> DirectoryEntry {
        DirectoryEntry::file(path, 1, *blake3::hash(&[byte]).as_bytes())
    }

    #[test]
    fn one_way_plan_is_deterministic_and_delete_is_tombstoned() {
        let source = DirectoryManifest::from_entries(vec![
            DirectoryEntry::directory("dir"),
            file("dir/new", 1),
            file("renamed", 2),
        ])
        .unwrap();
        let destination = DirectoryManifest::from_entries(vec![
            DirectoryEntry::directory("dir"),
            file("dir/old", 1),
            file("old-name", 2),
            file("stale", 3),
        ])
        .unwrap();
        let plan = ReconcilePlan::one_way(&source, &destination, DeletePolicy::Delete).unwrap();
        assert!(plan.verify().is_ok());
        assert_eq!(plan.operations.len(), 3);
        assert!(plan.operations.iter().any(|operation| matches!(
            operation,
            ReconcileOperation::Delete { tombstone } if tombstone.path == "stale"
        )));
        assert!(plan.operations.iter().any(|operation| matches!(
            operation,
            ReconcileOperation::Move { from, to }
                if from.path == "old-name" && to.path == "renamed"
        )));
        assert_eq!(plan.digest().unwrap(), plan.digest().unwrap());
        assert!(plan.check_destination(&destination).is_ok());
        let changed = DirectoryManifest::from_entries(vec![file("changed", 8)]).unwrap();
        assert!(plan.check_destination(&changed).is_err());
    }

    #[test]
    fn keep_policy_does_not_emit_destructive_operations() {
        let source = DirectoryManifest::from_entries(vec![file("wanted", 1)]).unwrap();
        let destination = DirectoryManifest::from_entries(vec![file("stale", 2)]).unwrap();
        let plan = ReconcilePlan::one_way(&source, &destination, DeletePolicy::Keep).unwrap();
        assert!(
            plan.operations
                .iter()
                .all(|operation| !matches!(operation, ReconcileOperation::Delete { .. }))
        );
    }

    #[test]
    fn conflicts_require_both_sides_to_change_differently() {
        let base =
            DirectoryManifest::from_entries(vec![file("same", 1), file("conflict", 1)]).unwrap();
        let local =
            DirectoryManifest::from_entries(vec![file("same", 2), file("conflict", 2)]).unwrap();
        let remote =
            DirectoryManifest::from_entries(vec![file("same", 2), file("conflict", 3)]).unwrap();
        let records = conflicts(&base, &local, &remote).unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.path.as_str())
                .collect::<Vec<_>>(),
            vec!["conflict"]
        );
        assert_eq!(records[0].base.as_ref().unwrap().path, "conflict");
    }
}
