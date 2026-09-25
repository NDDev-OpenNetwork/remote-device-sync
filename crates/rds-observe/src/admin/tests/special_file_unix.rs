//! rustix has no mknodat on Apple targets. A Unix socket exercises special-file
//! refusal there without a helper executable or unsafe test-only OS calls.

pub(super) fn create(path: &std::path::Path) {
    let socket = std::os::unix::net::UnixListener::bind(path).unwrap();
    // Closing does not remove the filesystem socket entry.
    drop(socket);
}
