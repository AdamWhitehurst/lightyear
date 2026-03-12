use std::fs;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use protocol::MapInstanceId;
use serde::{Deserialize, Serialize};

const META_VERSION: u32 = 1;

/// Metadata for a single map instance, saved to `map.meta.bin`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MapMeta {
    pub version: u32,
    pub seed: u64,
    pub generation_version: u32,
    pub spawn_points: Vec<Vec3>,
}

/// Resource holding the base save directory path.
#[derive(Resource)]
pub struct WorldSavePath(pub PathBuf);

impl Default for WorldSavePath {
    fn default() -> Self {
        Self(PathBuf::from("worlds"))
    }
}

/// Resolve the save directory for a `MapInstanceId` within the base save path.
pub fn map_save_dir(base: &Path, map_id: &MapInstanceId) -> PathBuf {
    match map_id {
        MapInstanceId::Overworld => base.join("overworld"),
        MapInstanceId::Homebase { owner } => base.join(format!("homebase-{owner}")),
    }
}

/// Save map metadata to `map.meta.bin`. Atomic via tmp+rename.
pub fn save_map_meta(map_dir: &Path, meta: &MapMeta) -> Result<(), String> {
    fs::create_dir_all(map_dir).map_err(|e| format!("mkdir map_dir: {e}"))?;
    let path = map_dir.join("map.meta.bin");
    let bytes = bincode::serialize(meta).map_err(|e| format!("serialize meta: {e}"))?;
    let tmp_path = path.with_extension("bin.tmp");
    fs::write(&tmp_path, &bytes).map_err(|e| format!("write meta tmp: {e}"))?;
    fs::rename(&tmp_path, &path).map_err(|e| format!("rename meta: {e}"))?;
    Ok(())
}

/// Load map metadata from `map.meta.bin`. Returns `None` if the file does not exist.
pub fn load_map_meta(map_dir: &Path) -> Result<Option<MapMeta>, String> {
    let path = map_dir.join("map.meta.bin");
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|e| format!("read meta: {e}"))?;
    let meta: MapMeta =
        bincode::deserialize(&bytes).map_err(|e| format!("deserialize meta: {e}"))?;
    if meta.version != META_VERSION {
        return Err(format!(
            "meta version mismatch: expected {META_VERSION}, got {}",
            meta.version
        ));
    }
    Ok(Some(meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_map_meta_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let meta = MapMeta {
            version: 1,
            seed: 42,
            generation_version: 3,
            spawn_points: vec![Vec3::new(1.0, 2.0, 3.0), Vec3::new(4.0, 5.0, 6.0)],
        };
        save_map_meta(dir.path(), &meta).unwrap();
        let loaded = load_map_meta(dir.path())
            .unwrap()
            .expect("meta should exist");
        assert_eq!(loaded.seed, 42);
        assert_eq!(loaded.generation_version, 3);
        assert_eq!(loaded.spawn_points.len(), 2);
    }

    #[test]
    fn load_map_meta_missing_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_map_meta(dir.path()).unwrap().is_none());
    }

    #[test]
    fn map_save_dir_overworld() {
        let base = Path::new("worlds");
        assert_eq!(
            map_save_dir(base, &MapInstanceId::Overworld),
            PathBuf::from("worlds/overworld")
        );
    }

    #[test]
    fn map_save_dir_homebase() {
        let base = Path::new("worlds");
        assert_eq!(
            map_save_dir(base, &MapInstanceId::Homebase { owner: 42 }),
            PathBuf::from("worlds/homebase-42")
        );
    }
}
