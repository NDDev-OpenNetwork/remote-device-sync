//! Backend-independent persistent endpoint identity.

use std::path::{Path, PathBuf};

use crate::SecretKey;

/// Identity storage never replaces malformed or inaccessible existing keys.
#[derive(Debug, thiserror::Error)]
pub enum KeyStoreError {
    #[error("identity path must name a file outside the reserved .rds-key- namespace")]
    InvalidPath,
    #[error("identity parent must be owned by the current user and not writable by others")]
    UnsafeParent,
    #[error("identity state must be an owned private regular file with a single link")]
    UnsafeFile,
    #[error("identity seed must contain exactly 32 bytes")]
    InvalidLength,
    #[error("identity creation is busy; bounded lock wait expired")]
    Busy,
    #[error("identity transaction marker or pending state is invalid; left unchanged")]
    InvalidState,
    #[error("persistent identity storage is not implemented for this platform")]
    Unsupported,
    #[error("identity storage I/O: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;

/// Read a private raw 32-byte Ed25519 seed, or durably create one without
/// replacing an existing key. Concurrent creators reuse the committed winner.
///
/// This performs blocking filesystem I/O with a two-second advisory-lock wait;
/// async callers should use `spawn_blocking`. Configured ancestors and processes
/// with the same OS identity are trusted. See `docs/identity-storage.md` for the
/// reserved transaction names, recovery rules and filesystem assumptions.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn load_or_create_key(path: &Path) -> Result<SecretKey, KeyStoreError> {
    unix::load_or_create(path)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn load_or_create_key(_path: &Path) -> Result<SecretKey, KeyStoreError> {
    Err(KeyStoreError::Unsupported)
}

/// Default key file location for this OS user.
pub fn default_key_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|d| d.join("remote-device-sync").join("endpoint.key"))
}
