//! Collision-checked, byte- and count-bounded text caches. Hits allocate nothing.
use rustc_hash::FxHasher;
use std::hash::{Hash, Hasher};

pub(super) struct TextCache<K, V> {
    entries: lru::LruCache<u64, Entry<K, V>>,
    bytes: usize,
    limit: usize,
    #[cfg(test)]
    pub(super) allocations: usize,
}

struct Entry<K, V> {
    key: K,
    text: Box<str>,
    value: V,
    bytes: usize,
}

impl<K: Hash + Eq, V> TextCache<K, V> {
    pub(super) fn new(count: usize, bytes: usize) -> Self {
        Self {
            entries: lru::LruCache::new(std::num::NonZeroUsize::new(count).unwrap()),
            bytes: 0,
            limit: bytes,
            #[cfg(test)]
            allocations: 0,
        }
    }

    fn hash(key: &K, text: &str) -> u64 {
        let mut h = FxHasher::default();
        key.hash(&mut h);
        text.hash(&mut h);
        h.finish()
    }

    pub(super) fn get(&mut self, key: &K, text: &str) -> Option<&V> {
        let entry = self.entries.get(&Self::hash(key, text))?;
        // A digest is only an index, never proof of text/style identity.
        (entry.key == *key && entry.text.as_ref() == text).then_some(&entry.value)
    }

    pub(super) fn insert(&mut self, key: K, text: &str, value: V, payload_bytes: usize) {
        let bytes = text
            .len()
            .saturating_add(payload_bytes)
            .saturating_add(std::mem::size_of::<Entry<K, V>>());
        // Oversized one-offs must not evict the entire warm working set.
        if bytes > self.limit.min(256 * 1024) {
            return;
        }
        let hash = Self::hash(&key, text);
        if let Some(old) = self.entries.pop(&hash) {
            self.bytes -= old.bytes;
        }
        while self.bytes + bytes > self.limit
            || self.entries.len() == self.entries.cap().get()
        {
            if let Some((_, old)) = self.entries.pop_lru() {
                self.bytes -= old.bytes;
            } else {
                break;
            }
        }
        #[cfg(test)]
        {
            self.allocations += 1;
        }
        self.entries.put(
            hash,
            Entry {
                key,
                text: text.into(),
                value,
                bytes,
            },
        );
        self.bytes += bytes;
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    #[cfg(test)]
    pub(super) fn values(&self) -> impl Iterator<Item = &V> {
        self.entries.iter().map(|(_, e)| &e.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_lru_and_oversized_bypass() {
        let mut cache = TextCache::new(2, 1024);
        cache.insert(0, "a", 1, 20);
        cache.insert(0, "b", 2, 20);
        assert_eq!(cache.get(&0, "a"), Some(&1));
        cache.insert(0, "c", 3, 20);
        assert_eq!(cache.get(&0, "b"), None);
        cache.insert(0, "huge", 4, 1024);
        assert_eq!(cache.get(&0, "a"), Some(&1));
        assert!(cache.bytes <= cache.limit);
        for i in 0..100 {
            cache.insert(i, "x", i, 600);
        }
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.bytes <= cache.limit);
        cache.clear();
        assert_eq!(cache.bytes, 0);
    }
    #[test]
    fn digest_collisions_never_return_wrong_text_or_style() {
        let mut cache = TextCache::new(2, 1024);
        cache.insert(0, "a", 42, 0);
        // Force a conflicting entry into the lookup slot deterministically.
        let hash = TextCache::<i32, i32>::hash(&0, "a");
        cache.entries.get_mut(&hash).unwrap().text = "b".into();
        assert_eq!(cache.get(&0, "a"), None);
        cache.entries.get_mut(&hash).unwrap().text = "a".into();
        cache.entries.get_mut(&hash).unwrap().key = 1;
        assert_eq!(cache.get(&0, "a"), None);
    }
}
