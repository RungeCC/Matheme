//! Poison-resilient lazy singleton cache shared by fonts and packages.

use std::sync::{Arc, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Read an `RwLock`, recovering the inner state if a previous writer panicked.
pub(crate) fn read_unpoisoned<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Write an `RwLock`, recovering the inner state if a previous writer panicked.
pub(crate) fn write_unpoisoned<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A process-wide, lazily initialized and resettable `Arc<T>` cache.
///
/// Builders run outside the write lock so readers and resets are never blocked
/// on filesystem or network work; when two callers race to populate an empty
/// slot, both read back the single `Arc` that won the slot.
pub(crate) struct LazyCache<T> {
    slot: OnceLock<RwLock<Option<Arc<T>>>>,
}

impl<T> LazyCache<T> {
    pub const fn new() -> Self {
        Self {
            slot: OnceLock::new(),
        }
    }

    fn lock(&self) -> &RwLock<Option<Arc<T>>> {
        self.slot.get_or_init(|| RwLock::new(None))
    }

    /// Return the live snapshot or initialize one via `build`.
    pub fn get_or_init(&self, build: impl FnOnce() -> T) -> Arc<T> {
        if let Some(existing) = read_unpoisoned(self.lock()).clone() {
            return existing;
        }
        let built = Arc::new(build());
        let mut guard = write_unpoisoned(self.lock());
        // Always return the Arc stored in the slot so concurrent initializers
        // never walk away with a detached instance.
        Arc::clone(guard.get_or_insert(built))
    }

    /// Return the live snapshot without triggering initialization.
    pub fn cached(&self) -> Option<Arc<T>> {
        self.slot
            .get()
            .and_then(|lock| read_unpoisoned(lock).clone())
    }

    /// Replace the slot unconditionally and return the installed `Arc`.
    pub fn install(&self, value: T) -> Arc<T> {
        let next = Arc::new(value);
        *write_unpoisoned(self.lock()) = Some(Arc::clone(&next));
        next
    }

    /// Rebuild only if a snapshot is already live.
    pub fn refresh_if_live(&self, build: impl FnOnce() -> T) -> Option<Arc<T>> {
        let lock = self.slot.get()?;
        if read_unpoisoned(lock).is_none() {
            return None;
        }
        Some(self.install(build()))
    }

    /// Drop the cached snapshot; the next `get_or_init` rebuilds it.
    pub fn reset(&self) {
        if let Some(lock) = self.slot.get() {
            *write_unpoisoned(lock) = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn cached_is_read_only_until_initialized() {
        let cache = LazyCache::new();
        assert!(cache.cached().is_none());
        assert!(cache.refresh_if_live(|| 99).is_none());
        assert!(cache.cached().is_none());

        let first = cache.get_or_init(|| 42);
        assert_eq!(*first, 42);
        assert!(Arc::ptr_eq(&first, &cache.cached().unwrap()));
    }

    #[test]
    fn install_refresh_and_reset_update_the_slot() {
        let cache = LazyCache::new();
        let a = cache.get_or_init(|| 1);
        let b = cache.refresh_if_live(|| 2).unwrap();
        assert_eq!(*a, 1);
        assert_eq!(*b, 2);

        cache.reset();
        assert!(cache.cached().is_none());
        let c = cache.get_or_init(|| 3);
        assert_eq!(*c, 3);
    }

    #[test]
    fn racing_initializers_return_the_same_arc() {
        let cache = Arc::new(LazyCache::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..4 {
            let cache = Arc::clone(&cache);
            let calls = Arc::clone(&calls);
            handles.push(std::thread::spawn(move || {
                cache.get_or_init(|| calls.fetch_add(1, Ordering::SeqCst) + 1)
            }));
        }
        let arcs: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        for arc in &arcs[1..] {
            assert!(Arc::ptr_eq(&arcs[0], arc));
        }
    }
}
