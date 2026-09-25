//! Metadata-only publication, separate from transaction locks and storage.
use std::sync::{Arc, Mutex, Weak};

pub(crate) struct Published<T>(Arc<Mutex<(bool, T)>>);

#[derive(Clone)]
pub(crate) struct Observer<T>(Weak<Mutex<(bool, T)>>);

impl<T: Copy> Published<T> {
    pub(crate) fn new(value: T) -> Self {
        Self(Arc::new(Mutex::new((false, value))))
    }

    pub(crate) fn observer(&self) -> Observer<T> {
        Observer(Arc::downgrade(&self.0))
    }

    // Never hold this mutex across owner work. A panic during that work leaves
    // the observation unknown until the owner is discarded and reopened.
    pub(crate) fn begin(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).0 = true;
    }

    pub(crate) fn finish(&self, update: impl FnOnce(&mut T)) {
        let mut value = self.0.lock().unwrap_or_else(|e| e.into_inner());
        update(&mut value.1);
        value.0 = false;
    }
}

impl<T: Copy> Observer<T> {
    pub(crate) fn snapshot(&self) -> Option<T> {
        let owner = self.0.upgrade()?;
        let value = owner.try_lock().ok()?;
        (!value.0).then_some(value.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_is_atomic_nonblocking_and_does_not_retain_owner() {
        let publisher = Published::new((1, 10));
        let observer = publisher.observer();
        assert_eq!(observer.snapshot(), Some((1, 10)));
        let held = publisher.0.lock().unwrap();
        assert_eq!(observer.snapshot(), None);
        drop(held);
        publisher.begin();
        assert_eq!(observer.snapshot(), None);
        publisher.finish(|value| *value = (2, 20));
        assert_eq!(observer.snapshot(), Some((2, 20)));
        drop(publisher);
        assert_eq!(observer.snapshot(), None);
    }
}
