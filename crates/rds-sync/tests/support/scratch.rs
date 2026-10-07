//! Synthetic roots stay alive until their last asynchronous fixture owner exits.
use std::{
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub struct Scratch(Arc<Root>);

struct Root(PathBuf);

impl Scratch {
    pub fn new(prefix: &str) -> Self {
        assert!(!prefix.contains(['/', '\\']));
        let path = std::env::temp_dir().join(format!("{prefix}-{:032x}", rand::random::<u128>()));
        std::fs::create_dir(&path).unwrap();
        Self(Arc::new(Root(path)))
    }
}

impl Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0.0
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        self
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        // remove_dir_all does not follow fixture-created symlinks. Cleanup is
        // best-effort and must not panic again while unwinding a failed test.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_last_owner_removes_the_root_without_following_aliases() {
    let outside = Scratch::new("rds-scratch-outside");
    std::fs::write(outside.join("retained"), b"outside this root").unwrap();
    let root = Scratch::new("rds-scratch-owner");
    let path = root.to_path_buf();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, root.join("alias")).unwrap();
    let worker = root.clone();
    drop(root);
    assert!(path.is_dir(), "a fixture worker still owns this root");
    drop(worker);
    assert!(!path.exists(), "the finished fixture leaked its root");
    assert_eq!(
        std::fs::read(outside.join("retained")).unwrap(),
        b"outside this root"
    );
}
