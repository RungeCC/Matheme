//! Bounded O(1) LRU cache and poison-resilient lock helpers for
//! `matheme-graphics`.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Mutex, MutexGuard};

/// Acquire a `Mutex`, recovering the inner value if a previous holder panicked.
pub(crate) fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Node<K, V> {
    key: K,
    value: V,
    prev: Option<usize>,
    next: Option<usize>,
}

/// Fixed-capacity O(1) LRU cache backed by a `HashMap` and an intrusive
/// doubly-linked arena `Vec`.
///
/// Unlike a map that calls `clear()` at capacity, this evicts only the single
/// least-recently-used entry, preserving hot working sets under churn (e.g.,
/// large CJK / multi-font-size documents).
pub(crate) struct LruCache<K, V> {
    capacity: usize,
    map: HashMap<K, usize>,
    nodes: Vec<Node<K, V>>,
    head: Option<usize>, // most recently used
    tail: Option<usize>, // least recently used
}

impl<K: Eq + Hash + Clone, V> LruCache<K, V> {
    pub(crate) fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "LruCache capacity must be > 0");
        Self {
            capacity,
            map: HashMap::new(),
            nodes: Vec::new(),
            head: None,
            tail: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    #[cfg(test)]
    pub(crate) fn contains_key(&self, key: &K) -> bool {
        self.map.contains_key(key)
    }

    pub(crate) fn clear(&mut self) {
        self.map.clear();
        self.nodes.clear();
        self.head = None;
        self.tail = None;
    }

    /// Look up `key`, promoting it to the MRU front if present.
    pub(crate) fn get(&mut self, key: &K) -> Option<&V> {
        let idx = *self.map.get(key)?;
        self.move_to_front(idx);
        Some(&self.nodes[idx].value)
    }

    /// Look up `key` mutably, promoting it to the MRU front if present.
    #[cfg(feature = "wl")]
    pub(crate) fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let idx = *self.map.get(key)?;
        self.move_to_front(idx);
        Some(&mut self.nodes[idx].value)
    }

    /// Insert or update `key`, evicting the LRU tail if at capacity.
    pub(crate) fn insert(&mut self, key: K, value: V) {
        if let Some(&idx) = self.map.get(&key) {
            self.nodes[idx].value = value;
            self.move_to_front(idx);
            return;
        }

        if self.nodes.len() < self.capacity {
            let idx = self.nodes.len();
            self.nodes.push(Node {
                key: key.clone(),
                value,
                prev: None,
                next: self.head,
            });
            if let Some(old_head) = self.head {
                self.nodes[old_head].prev = Some(idx);
            } else {
                self.tail = Some(idx);
            }
            self.head = Some(idx);
            self.map.insert(key, idx);
        } else {
            // Reuse the tail slot directly in O(1) without growing the arena.
            let tail_idx = self.tail.expect("non-empty cache at capacity has tail");
            let old_key = std::mem::replace(&mut self.nodes[tail_idx].key, key.clone());
            self.map.remove(&old_key);
            self.nodes[tail_idx].value = value;
            self.move_to_front(tail_idx);
            self.map.insert(key, tail_idx);
        }
    }

    fn move_to_front(&mut self, idx: usize) {
        if self.head == Some(idx) {
            return;
        }
        let prev = self.nodes[idx].prev;
        let next = self.nodes[idx].next;

        if let Some(p) = prev {
            self.nodes[p].next = next;
        }
        if let Some(n) = next {
            self.nodes[n].prev = prev;
        } else {
            self.tail = prev;
        }

        self.nodes[idx].prev = None;
        self.nodes[idx].next = self.head;
        if let Some(old_head) = self.head {
            self.nodes[old_head].prev = Some(idx);
        } else {
            self.tail = Some(idx);
        }
        self.head = Some(idx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used_and_preserves_hot_entries() {
        let mut cache = LruCache::new(3);
        cache.insert(1, "a");
        cache.insert(2, "b");
        cache.insert(3, "c");
        // Touch 1 so 2 becomes the LRU tail.
        assert_eq!(cache.get(&1), Some(&"a"));
        cache.insert(4, "d");

        assert_eq!(cache.len(), 3);
        assert!(cache.contains_key(&1));
        assert!(!cache.contains_key(&2));
        assert!(cache.contains_key(&3));
        assert!(cache.contains_key(&4));

        // Repeatedly touching 1 while churning 100 cold keys keeps 1 resident.
        for cold in 10..110 {
            assert_eq!(cache.get(&1), Some(&"a"));
            cache.insert(cold, "cold");
        }
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.get(&1), Some(&"a"));
    }

    #[test]
    fn single_capacity_cache_updates_and_replaces_cleanly() {
        let mut cache = LruCache::new(1);
        cache.insert("k1", 10);
        assert_eq!(cache.get(&"k1"), Some(&10));
        cache.insert("k1", 20);
        assert_eq!(cache.get(&"k1"), Some(&20));
        cache.insert("k2", 30);
        assert_eq!(cache.get(&"k1"), None);
        assert_eq!(cache.get(&"k2"), Some(&30));
        cache.clear();
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.get(&"k2"), None);
    }
}
