use bevy::prelude::*;
use std::collections::HashMap;

/// Per-instance mesh cache keyed by chunk data hash.
/// Stores mesh handles to avoid re-uploading identical meshes.
#[derive(Component, Default)]
pub struct MeshCache {
    cache: HashMap<u64, Handle<Mesh>>,
}

impl MeshCache {
    pub fn get(&self, hash: u64) -> Option<&Handle<Mesh>> {
        if hash == 0 {
            return None;
        }
        self.cache.get(&hash)
    }

    pub fn insert(&mut self, hash: u64, handle: Handle<Mesh>) {
        if hash != 0 {
            self.cache.insert(hash, handle);
        }
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_insert_and_get() {
        let mut cache = MeshCache::default();
        let handle = Handle::<Mesh>::default();
        cache.insert(42, handle.clone());
        assert_eq!(cache.get(42).unwrap(), &handle);
    }

    #[test]
    fn cache_rejects_zero_hash() {
        let mut cache = MeshCache::default();
        cache.insert(0, Handle::<Mesh>::default());
        assert!(cache.get(0).is_none());
    }

    #[test]
    fn cache_len() {
        let mut cache = MeshCache::default();
        cache.insert(1, Handle::<Mesh>::default());
        cache.insert(2, Handle::<Mesh>::default());
        assert_eq!(cache.len(), 2);
    }
}
