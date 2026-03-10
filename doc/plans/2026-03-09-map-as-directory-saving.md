# Map-as-Directory Saving Implementation Plan

## Overview

Replace the single flat `world_save/voxel_world.bin` modifications file with a per-map directory structure that saves full chunk terrain data, entity state, and map metadata. Fix the unused octree bug so `VoxelMapInstance.tree` is the source of truth for loaded chunk voxel data. Extend persistence to support all map types (Overworld, Homebase).

## Current State Analysis

- **Persistence**: Single bincode file storing `Vec<(IVec3, VoxelType)>` modifications only
- **Octree**: `VoxelMapInstance.tree: OctreeI32<Option<ChunkData>>` is declared but never read/written at runtime — this is a bug
- **Chunk pipeline**: Generates voxels → meshes → **discards voxel data**. Lookups re-invoke the generator each time
- **Entity persistence**: None. Respawn points hardcoded at `Vec3(0.0, 5.0, 0.0)`
- **Multi-map**: Only Overworld saved. Homebases are ephemeral

### Key Discoveries:
- `WorldVoxel` already derives `Serialize, Deserialize` ([types.rs:12](crates/voxel_map_engine/src/types.rs#L12))
- `ChunkData` and `FillType` lack serde derives ([types.rs:26-39](crates/voxel_map_engine/src/types.rs#L26-L39))
- `ChunkGenResult` only carries `position` and `mesh` — voxel data is dropped ([generation.rs:14-17](crates/voxel_map_engine/src/generation.rs#L14-L17))
- `get_voxel` re-invokes generator every call, `raycast` uses a single-chunk cache ([api.rs:22-33](crates/voxel_map_engine/src/api.rs#L22-L33), [api.rs:69](crates/voxel_map_engine/src/api.rs#L69))
- OctreeI32 API: `fill_path_to_node_from_root` + `find_node` + `get_value_mut` for insert; `drop_tree` for removal
- Existing tests use `#[test]` functions with `MinimalPlugins` app or direct function calls ([voxel_persistence.rs](crates/server/tests/voxel_persistence.rs))
- `CrossbeamTestStepper` exists for client-server integration tests ([integration.rs:25-173](crates/server/tests/integration.rs#L25-L173))
- Spawn points hardcoded in 4 places: [gameplay.rs:109](crates/server/src/gameplay.rs#L109), [gameplay.rs:150](crates/server/src/gameplay.rs#L150), [gameplay.rs:192](crates/server/src/gameplay.rs#L192), [map.rs:515](crates/server/src/map.rs#L515)
- Current debounced save: 1s after last edit, 5s max dirty ([map.rs:85-113](crates/server/src/map.rs#L85-L113))

## Desired End State

```
worlds/
  overworld/
    map.meta.bin             # seed, generation_version, spawn points
    terrain/
      chunk_0_0_0.bin        # bincode + zstd compressed ChunkData
      chunk_1_0_0.bin
      ...
    entities.bin             # flat list of persistable entities

  homebase-<owner_id>/
    map.meta.bin
    terrain/
      ...
    entities.bin
```

- Full chunk voxel data saved per-chunk, not just modifications
- Chunk data retained in octree while loaded; octree used for voxel lookups (no generator re-invocation)
- Map metadata (seed, spawn points) persisted per-map
- Persistable entities (respawn points, future doodads) saved per-map
- All map types (Overworld, Homebase) persist independently
- Per-chunk dirty tracking — only modified chunks re-saved
- zstd compression on chunk files

### Verification:
- Server saves world state to directory structure on debounced timer and shutdown
- Server loads world state from directories on startup
- Voxel modifications survive server restart
- Spawn points load from metadata (no hardcoded fallback needed after first save)
- Homebase state persists across server restarts
- Integration test validates full save/load cycle with client-server setup

## What We're NOT Doing

- Region files (8x8x8 chunk grouping) — per-chunk files for now, region files can be added later for filesystem efficiency
- Arena persistence — Arenas are not in `MapInstanceId` yet
- Player data persistence (inventory, stats) — separate feature
- Pre-authored terrain import — save format supports it but no tooling
- Client-side save/load — server-only persistence
- Live migration from old save format — old `world_save/voxel_world.bin` files become incompatible (acceptable since this is pre-release)

## Implementation Approach

Four phases, each building on the previous. The octree fix (Phase 1) is a prerequisite since it makes chunk data available for saving. Persistence infrastructure (Phase 2) builds the directory/file I/O layer. Entity persistence (Phase 3) and multi-map support (Phase 4) extend the system.

Each phase includes unit tests for all new code paths. An integration test file `crates/server/tests/world_persistence.rs` is started in Phase 2 and extended through Phase 4.

---

## Phase 1: Fix Octree — Retain ChunkData After Generation

### Overview
Fix the bug where `VoxelMapInstance.tree` is never populated. After chunk generation, store the voxel data in the octree. Use the octree for voxel lookups instead of re-invoking the generator.

### Changes Required:

#### 1. Return voxel data from chunk generation
**File**: `crates/voxel_map_engine/src/generation.rs`

Add voxels to `ChunkGenResult`:

```rust
pub struct ChunkGenResult {
    pub position: IVec3,
    pub mesh: Option<Mesh>,
    pub voxels: Vec<WorldVoxel>,
}
```

In `generate_chunk`, return the voxels:

```rust
fn generate_chunk(...) -> ChunkGenResult {
    let mut voxels = generator(position);
    apply_overrides(&mut voxels, position, overrides);
    let mesh = mesh_chunk_greedy(&voxels);
    ChunkGenResult { position, mesh, voxels }
}
```

#### 2. Add octree helper methods to VoxelMapInstance
**File**: `crates/voxel_map_engine/src/instance.rs`

```rust
use grid_tree::{NodeKey, VisitCommand};
use crate::types::ChunkData;

impl VoxelMapInstance {
    /// Insert chunk data into the octree at the given chunk position.
    pub fn insert_chunk_data(&mut self, chunk_pos: IVec3, data: ChunkData) {
        let key = NodeKey::new(0, chunk_pos);
        self.tree.fill_path_to_node_from_root(key, |_key, entry| {
            entry.or_insert_with(|| None);
            VisitCommand::Continue
        });
        let relation = self.tree.find_node(key).expect("just created path");
        *self.tree.get_value_mut(relation.child).expect("just created node") = Some(data);
    }

    /// Remove chunk data from the octree. Returns the data if it existed.
    pub fn remove_chunk_data(&mut self, chunk_pos: IVec3) -> Option<ChunkData> {
        let key = NodeKey::new(0, chunk_pos);
        let relation = self.tree.find_node(key)?;
        let value = self.tree.get_value_mut(relation.child)?;
        value.take()
    }

    /// Get a reference to chunk data in the octree.
    pub fn get_chunk_data(&self, chunk_pos: IVec3) -> Option<&ChunkData> {
        let key = NodeKey::new(0, chunk_pos);
        let relation = self.tree.find_node(key)?;
        self.tree.get_value(relation.child)?.as_ref()
    }
}
```

#### 3. Store chunk data in octree on completion
**File**: `crates/voxel_map_engine/src/lifecycle.rs`

In `handle_completed_chunk`, after `instance.loaded_chunks.insert(result.position)`:

```rust
use crate::types::{ChunkData, FillType, WorldVoxel};

fn classify_fill_type(voxels: &[WorldVoxel]) -> FillType {
    let first = voxels.first().copied().unwrap_or(WorldVoxel::Air);
    if voxels.iter().all(|&v| v == first) {
        if first == WorldVoxel::Air { FillType::Empty } else { FillType::Uniform(first) }
    } else {
        FillType::Mixed
    }
}

fn compute_chunk_hash(voxels: &[WorldVoxel]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    voxels.hash(&mut hasher);
    hasher.finish()
}

// In handle_completed_chunk, after loaded_chunks.insert:
let fill_type = classify_fill_type(&result.voxels);
let hash = compute_chunk_hash(&result.voxels);
let chunk_data = ChunkData { voxels: result.voxels, fill_type, hash };
instance.insert_chunk_data(result.position, chunk_data);
```

#### 4. Evict chunk data from octree on unload
**File**: `crates/voxel_map_engine/src/lifecycle.rs`

Chunks are evicted from the octree when they leave the loaded range. This keeps memory bounded to only the chunks currently in view. In Phase 2, dirty chunks will be saved to disk before eviction; in Phase 1, eviction simply drops the data (it can be regenerated).

In `remove_out_of_range_chunks`, when removing from `loaded_chunks`, also evict from octree:

```rust
fn remove_out_of_range_chunks(instance: &mut VoxelMapInstance, desired: &HashSet<IVec3>) {
    let removed: Vec<IVec3> = instance.loaded_chunks.iter()
        .filter(|pos| !desired.contains(pos))
        .copied()
        .collect();
    for pos in removed {
        instance.loaded_chunks.remove(&pos);
        instance.remove_chunk_data(pos);
    }
}
```

In `flush_write_buffer`, when invalidating chunks, also evict from octree (the chunk will be regenerated with the new voxel data applied):

```rust
for pos in invalidated {
    instance.loaded_chunks.remove(&pos);
    instance.remove_chunk_data(pos);
}
```

#### 5. Use octree for voxel lookups
**File**: `crates/voxel_map_engine/src/api.rs`

In `get_voxel`, check octree before falling back to generator:

```rust
pub fn get_voxel(&self, map: Entity, pos: IVec3) -> WorldVoxel {
    let Ok((instance, config)) = self.maps.get(map) else {
        warn!("get_voxel: entity {map:?} has no VoxelMapInstance");
        return WorldVoxel::Unset;
    };

    if let Some(&voxel) = instance.modified_voxels.get(&pos) {
        return voxel;
    }

    let chunk_pos = voxel_to_chunk_pos(pos);
    if let Some(chunk_data) = instance.get_chunk_data(chunk_pos) {
        return lookup_voxel_in_chunk(&chunk_data.voxels, pos, chunk_pos);
    }

    evaluate_voxel_at(pos, &config.generator)
}
```

Similarly update `lookup_voxel` for raycast to check octree:

```rust
fn lookup_voxel(
    voxel_pos: IVec3,
    instance: &VoxelMapInstance,
    generator: &crate::config::VoxelGenerator,
    cached_chunk: &mut Option<(IVec3, Vec<WorldVoxel>)>,
) -> WorldVoxel {
    if let Some(&voxel) = instance.modified_voxels.get(&voxel_pos) {
        return voxel;
    }

    let chunk_pos = voxel_to_chunk_pos(voxel_pos);

    // Check octree first
    if let Some(chunk_data) = instance.get_chunk_data(chunk_pos) {
        return lookup_voxel_in_chunk(&chunk_data.voxels, voxel_pos, chunk_pos);
    }

    // Fall back to generator with cache
    let needs_generate = match cached_chunk.as_ref() {
        Some((cached_pos, _)) if *cached_pos == chunk_pos => false,
        _ => true,
    };
    if needs_generate {
        *cached_chunk = Some((chunk_pos, generator(chunk_pos)));
    }

    let (_, voxels) = cached_chunk.as_ref().unwrap();
    lookup_voxel_in_chunk(voxels, voxel_pos, chunk_pos)
}
```

### Unit Tests

**File**: `crates/voxel_map_engine/src/instance.rs` (extend existing test module)

```rust
#[test]
fn insert_and_retrieve_chunk_data() {
    let mut instance = VoxelMapInstance::new(5);
    let pos = IVec3::new(1, 0, 2);
    let chunk = ChunkData::new_empty();
    instance.insert_chunk_data(pos, chunk);
    assert!(instance.get_chunk_data(pos).is_some());
    assert_eq!(instance.get_chunk_data(pos).unwrap().fill_type, FillType::Empty);
}

#[test]
fn remove_chunk_data_returns_data() {
    let mut instance = VoxelMapInstance::new(5);
    let pos = IVec3::ZERO;
    instance.insert_chunk_data(pos, ChunkData::new_empty());
    let removed = instance.remove_chunk_data(pos);
    assert!(removed.is_some());
    assert!(instance.get_chunk_data(pos).is_none());
}

#[test]
fn remove_nonexistent_chunk_returns_none() {
    let mut instance = VoxelMapInstance::new(5);
    assert!(instance.remove_chunk_data(IVec3::ZERO).is_none());
}

#[test]
fn get_nonexistent_chunk_returns_none() {
    let instance = VoxelMapInstance::new(5);
    assert!(instance.get_chunk_data(IVec3::new(99, 99, 99)).is_none());
}

#[test]
fn overwrite_chunk_data() {
    let mut instance = VoxelMapInstance::new(5);
    let pos = IVec3::ZERO;
    instance.insert_chunk_data(pos, ChunkData::new_empty());

    let mut solid_chunk = ChunkData::new_empty();
    solid_chunk.voxels[0] = WorldVoxel::Solid(1);
    solid_chunk.fill_type = FillType::Mixed;
    instance.insert_chunk_data(pos, solid_chunk);

    let data = instance.get_chunk_data(pos).unwrap();
    assert_eq!(data.fill_type, FillType::Mixed);
    assert_eq!(data.voxels[0], WorldVoxel::Solid(1));
}
```

**File**: `crates/voxel_map_engine/src/lifecycle.rs` (extend existing test module)

```rust
#[test]
fn classify_fill_type_empty() {
    let voxels = vec![WorldVoxel::Air; 100];
    assert_eq!(classify_fill_type(&voxels), FillType::Empty);
}

#[test]
fn classify_fill_type_uniform_solid() {
    let voxels = vec![WorldVoxel::Solid(5); 100];
    assert_eq!(classify_fill_type(&voxels), FillType::Uniform(WorldVoxel::Solid(5)));
}

#[test]
fn classify_fill_type_mixed() {
    let mut voxels = vec![WorldVoxel::Air; 100];
    voxels[0] = WorldVoxel::Solid(1);
    assert_eq!(classify_fill_type(&voxels), FillType::Mixed);
}
```

### Success Criteria:

#### Automated Verification:
- [ ] All tests pass: `cargo test-all`
- [ ] Server builds and runs: `cargo server`
- [ ] Client builds and runs: `cargo client -c 1`

#### Manual Verification:
- [ ] Voxel lookups work correctly in-game (place/break blocks, walk on terrain)
- [ ] No visual regressions — chunks render identically to before
- [ ] Memory usage is reasonable (chunk data retained in octree while loaded)

---

## Phase 2: Directory Structure, Map Metadata, and Per-Chunk Terrain Persistence

### Overview
Create the `worlds/<map>/` directory structure. Save map metadata to `map.meta.bin`. Save full chunk terrain data to per-chunk files with zstd compression. Replace the old single-file save system. Add per-chunk dirty tracking.

### Changes Required:

#### 1. Add dependencies
**File**: `crates/server/Cargo.toml`

```toml
zstd = "0.13"
```

**File**: `crates/voxel_map_engine/Cargo.toml`

```toml
bincode = "1.3"
zstd = "0.13"
```

#### 2. Add serde derives to ChunkData and FillType
**File**: `crates/voxel_map_engine/src/types.rs`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillType { ... }

#[derive(Clone, Serialize, Deserialize)]
pub struct ChunkData { ... }
```

#### 3. Create persistence module in voxel_map_engine
**File**: `crates/voxel_map_engine/src/persistence.rs` (new)

Handles chunk serialization/deserialization with compression. This keeps the low-level chunk I/O in the engine crate, while the server orchestrates when to save/load.

```rust
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::fs;
use bevy::prelude::*;
use crate::types::ChunkData;

const CHUNK_SAVE_VERSION: u32 = 1;
const ZSTD_COMPRESSION_LEVEL: i32 = 3;

#[derive(serde::Serialize, serde::Deserialize)]
struct ChunkFileSave {
    version: u32,
    data: ChunkData,
}

/// Build the file path for a chunk at the given position within a map directory.
pub fn chunk_file_path(map_dir: &Path, chunk_pos: IVec3) -> PathBuf {
    map_dir.join("terrain").join(format!(
        "chunk_{}_{}_{}. bin",
        chunk_pos.x, chunk_pos.y, chunk_pos.z
    ))
}

/// Save a single chunk's data to disk (bincode + zstd).
pub fn save_chunk(map_dir: &Path, chunk_pos: IVec3, data: &ChunkData) -> Result<(), String> {
    let path = chunk_file_path(map_dir, chunk_pos);
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| format!("mkdir: {e}"))?;

    let save = ChunkFileSave { version: CHUNK_SAVE_VERSION, data: data.clone() };
    let bytes = bincode::serialize(&save).map_err(|e| format!("serialize: {e}"))?;

    let tmp_path = path.with_extension("bin.tmp");
    let file = fs::File::create(&tmp_path).map_err(|e| format!("create tmp: {e}"))?;
    let mut encoder = zstd::Encoder::new(file, ZSTD_COMPRESSION_LEVEL)
        .map_err(|e| format!("zstd encoder: {e}"))?;
    encoder.write_all(&bytes).map_err(|e| format!("write: {e}"))?;
    encoder.finish().map_err(|e| format!("zstd finish: {e}"))?;

    fs::rename(&tmp_path, &path).map_err(|e| format!("rename: {e}"))?;
    Ok(())
}

/// Load a single chunk's data from disk.
pub fn load_chunk(map_dir: &Path, chunk_pos: IVec3) -> Result<Option<ChunkData>, String> {
    let path = chunk_file_path(map_dir, chunk_pos);
    if !path.exists() {
        return Ok(None);
    }

    let file = fs::File::open(&path).map_err(|e| format!("open: {e}"))?;
    let mut decoder = zstd::Decoder::new(file).map_err(|e| format!("zstd decoder: {e}"))?;
    let mut bytes = Vec::new();
    decoder.read_to_end(&mut bytes).map_err(|e| format!("read: {e}"))?;

    let save: ChunkFileSave = bincode::deserialize(&bytes)
        .map_err(|e| format!("deserialize: {e}"))?;

    if save.version != CHUNK_SAVE_VERSION {
        return Err(format!("version mismatch: expected {CHUNK_SAVE_VERSION}, got {}", save.version));
    }

    Ok(Some(save.data))
}

/// Delete a chunk file if it exists.
pub fn delete_chunk(map_dir: &Path, chunk_pos: IVec3) -> Result<(), String> {
    let path = chunk_file_path(map_dir, chunk_pos);
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("delete: {e}"))?;
    }
    Ok(())
}

/// List all chunk positions that have saved files in the terrain directory.
pub fn list_saved_chunks(map_dir: &Path) -> Result<Vec<IVec3>, String> {
    let terrain_dir = map_dir.join("terrain");
    if !terrain_dir.exists() {
        return Ok(Vec::new());
    }

    let mut positions = Vec::new();
    for entry in fs::read_dir(&terrain_dir).map_err(|e| format!("read_dir: {e}"))? {
        let entry = entry.map_err(|e| format!("entry: {e}"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(pos) = parse_chunk_filename(&name) {
            positions.push(pos);
        }
    }
    Ok(positions)
}

fn parse_chunk_filename(name: &str) -> Option<IVec3> {
    let name = name.strip_prefix("chunk_")?.strip_suffix(".bin")?;
    let parts: Vec<&str> = name.splitn(3, '_').collect();
    if parts.len() != 3 { return None; }
    Some(IVec3::new(
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}
```

#### 4. Add per-chunk dirty tracking to VoxelMapInstance
**File**: `crates/voxel_map_engine/src/instance.rs`

```rust
pub struct VoxelMapInstance {
    pub tree: OctreeI32<Option<ChunkData>>,
    pub modified_voxels: HashMap<IVec3, WorldVoxel>,
    pub write_buffer: Vec<(IVec3, WorldVoxel)>,
    pub loaded_chunks: HashSet<IVec3>,
    pub dirty_chunks: HashSet<IVec3>,   // NEW
    pub debug_colors: bool,
}
```

Mark chunks dirty in `flush_write_buffer` when voxels are modified (the invalidated chunks are dirty).

#### 5. Create MapMeta type and persistence
**File**: `crates/server/src/map.rs` (or a new `crates/server/src/persistence.rs`)

```rust
use serde::{Serialize, Deserialize};

const META_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct MapMeta {
    pub version: u32,
    pub seed: u64,
    pub generation_version: u32,
    pub spawn_points: Vec<Vec3>,
}

/// Resolve the save directory for a MapInstanceId.
pub fn map_save_dir(base: &Path, map_id: &MapInstanceId) -> PathBuf {
    match map_id {
        MapInstanceId::Overworld => base.join("overworld"),
        MapInstanceId::Homebase { owner } => base.join(format!("homebase-{owner}")),
    }
}

pub fn save_map_meta(map_dir: &Path, meta: &MapMeta) -> Result<(), String> { ... }
pub fn load_map_meta(map_dir: &Path) -> Result<Option<MapMeta>, String> { ... }
```

Uses the same atomic write pattern (tmp + rename), bincode serialization, no compression (metadata is small).

#### 6. Replace old save system
**File**: `crates/server/src/map.rs`

Remove `VoxelWorldSave`, `VoxelModifications`, `VoxelSavePath`, `save_voxel_world_to_disk_at`, `load_voxel_world_from_disk_at`, and the related systems.

Replace with:
- `WorldSavePath` resource (default: `"worlds/"`)
- `save_dirty_chunks` system — iterates all map instances, for each dirty chunk, reads ChunkData from octree and calls `save_chunk()`
- `save_map_on_shutdown` system — saves all dirty chunks + metadata on `AppExit`
- `load_map_terrain` — on startup, for the Overworld, load all saved chunks and insert into `modified_voxels` (or directly into octree once chunks are generated)
- `save_evicted_dirty_chunks` — when `remove_out_of_range_chunks` evicts a chunk, if it's in `dirty_chunks`, save it to disk first. This ensures dirty chunks are never lost on eviction.

**Debounced save approach**: Keep the same debounce timing (1s quiet, 5s max dirty). Track per-map dirty state. On save trigger, iterate `dirty_chunks`, save each to disk, clear the set. Separately, dirty chunks are also saved on eviction (when they leave the loaded range) to prevent data loss.

#### 7. Load saved chunks during generation
**File**: `crates/voxel_map_engine/src/generation.rs`

When spawning a chunk generation task, first check if a saved chunk file exists. If so, skip the generator and load from disk instead:

```rust
pub fn spawn_chunk_gen_task(
    pending: &mut PendingChunks,
    position: IVec3,
    generator: &VoxelGenerator,
    modified_voxels: &HashMap<IVec3, WorldVoxel>,
    map_dir: Option<&Path>,  // NEW
) {
    // Check for saved chunk first
    if let Some(dir) = map_dir {
        if let Ok(Some(saved_data)) = voxel_map_engine::persistence::load_chunk(dir, position) {
            // Skip generator, just mesh the saved data
            // ... spawn task that meshes saved_data directly
            return;
        }
    }

    // Fall through to normal generation
    let generator = Arc::clone(generator);
    let overrides = collect_chunk_overrides(position, modified_voxels);
    // ...
}
```

Alternatively, the loading logic could be in the server crate's startup system that pre-populates `modified_voxels` from saved chunks. This is simpler but means saved chunks still go through the generator first. Given we're saving full chunk data, loading from disk and bypassing the generator is more correct.

**Decision**: Add an optional `save_dir: Option<PathBuf>` field to `VoxelMapConfig`. When set, `spawn_chunk_gen_task` checks disk first. The server sets this field when spawning map instances.

#### 8. Replace hardcoded spawn points
**File**: `crates/server/src/gameplay.rs`

Load spawn points from `MapMeta` instead of hardcoding. On first run (no save exists), use default spawn position and save it to metadata.

```rust
fn spawn_respawn_points(mut commands: Commands, save_path: Res<WorldSavePath>) {
    let map_dir = map_save_dir(&save_path.0, &MapInstanceId::Overworld);
    let spawn_points = match load_map_meta(&map_dir) {
        Ok(Some(meta)) => meta.spawn_points,
        _ => vec![Vec3::new(0.0, 5.0, 0.0)], // default for first run
    };
    for pos in spawn_points {
        commands.spawn((RespawnPoint, Transform::from_translation(pos), MapInstanceId::Overworld));
    }
}
```

### Unit Tests

**File**: `crates/voxel_map_engine/src/persistence.rs`

```rust
#[cfg(test)]
mod tests {
    #[test] fn save_load_chunk_roundtrip() { ... }
    #[test] fn load_nonexistent_chunk_returns_none() { ... }
    #[test] fn save_chunk_creates_directories() { ... }
    #[test] fn corrupt_chunk_file_returns_error() { ... }
    #[test] fn parse_chunk_filename_valid() { ... }
    #[test] fn parse_chunk_filename_invalid() { ... }
    #[test] fn parse_chunk_filename_negative_coords() { ... }
    #[test] fn list_saved_chunks_empty_dir() { ... }
    #[test] fn list_saved_chunks_with_files() { ... }
    #[test] fn delete_chunk_removes_file() { ... }
    #[test] fn delete_nonexistent_chunk_is_ok() { ... }
    #[test] fn chunk_data_zstd_compression_reduces_size() { ... }
}
```

**File**: `crates/server/tests/voxel_persistence.rs` (update existing tests to use new system)

Update or replace existing tests to validate the new directory-based save/load:

```rust
#[test] fn save_load_map_meta_roundtrip() { ... }
#[test] fn map_meta_default_spawn_on_missing_file() { ... }
#[test] fn dirty_chunks_saved_on_debounce() { ... }
#[test] fn clean_chunks_not_re_saved() { ... }
```

### Integration Test (start)

**File**: `crates/server/tests/world_persistence.rs` (new)

Begin the integration test that will grow across phases:

```rust
/// Integration test: save terrain, restart server, verify terrain loads correctly.
#[test]
fn terrain_persists_across_server_restart() {
    // 1. Create server app with MinimalPlugins + map systems
    // 2. Spawn overworld, generate some chunks
    // 3. Modify a voxel
    // 4. Trigger save
    // 5. Drop app
    // 6. Create new server app, load from same directory
    // 7. Verify the modified voxel is present
    // 8. Verify unmodified terrain matches
}
```

### Success Criteria:

#### Automated Verification:
- [ ] All tests pass: `cargo test-all`
- [ ] Server builds and runs: `cargo server`
- [ ] Client builds and runs: `cargo client -c 1`

#### Manual Verification:
- [ ] Start server, place blocks, stop server → `worlds/overworld/terrain/` contains chunk files
- [ ] Restart server → placed blocks are still there
- [ ] `worlds/overworld/map.meta.bin` exists with correct seed
- [ ] Chunk files are zstd compressed (smaller than raw bincode)
- [ ] Spawn points load from metadata file

---

## Phase 3: Entity Persistence

### Overview
Add a `MapSaveTarget` marker component. Entities with this marker are serialized to `entities.bin` per map. Respawn points are the first entity type to persist.

### Changes Required:

#### 1. Define persistence types
**File**: `crates/protocol/src/map.rs`

```rust
/// Marker: this entity should be saved with its map.
#[derive(Component)]
pub struct MapSaveTarget;

/// Identifies the type of a saved entity for reconstruction.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum SavedEntityKind {
    RespawnPoint,
    // Future: Doodad, NPC, etc.
}

#[derive(Serialize, Deserialize)]
pub struct SavedEntity {
    pub kind: SavedEntityKind,
    pub position: Vec3,
}

#[derive(Serialize, Deserialize)]
pub struct MapEntitySave {
    pub version: u32,
    pub entities: Vec<SavedEntity>,
}
```

#### 2. Add MapSaveTarget to RespawnPoint spawning
**File**: `crates/server/src/gameplay.rs`

When spawning respawn points, add `MapSaveTarget`:

```rust
commands.spawn((
    RespawnPoint,
    MapSaveTarget,
    Transform::from_translation(pos),
    MapInstanceId::Overworld,
));
```

#### 3. Entity save/load functions
**File**: `crates/server/src/persistence.rs` (or extend map.rs)

```rust
pub fn save_entities(map_dir: &Path, entities: &[SavedEntity]) -> Result<(), String> { ... }
pub fn load_entities(map_dir: &Path) -> Result<Vec<SavedEntity>, String> { ... }
```

Saves to `entities.bin` in the map directory. Bincode serialization, no compression (entity data is small).

#### 4. Save/load entity systems
**File**: `crates/server/src/map.rs`

- `collect_and_save_entities` system — queries entities with `MapSaveTarget` + `MapInstanceId` + `Transform`, groups by map, saves each map's entities
- `load_map_entities` startup system — reads `entities.bin`, spawns entities based on `SavedEntityKind`

Entity save happens alongside chunk saves (same debounce trigger).

#### 5. Update spawn_respawn_points
**File**: `crates/server/src/gameplay.rs`

If entities loaded from disk, skip the default spawn creation:

```rust
fn spawn_respawn_points(
    mut commands: Commands,
    existing: Query<&RespawnPoint>,
    // ...
) {
    if !existing.is_empty() {
        return; // already loaded from disk
    }
    // Default spawns for first run
    commands.spawn((RespawnPoint, MapSaveTarget, Transform::from_translation(Vec3::new(0.0, 5.0, 0.0)), MapInstanceId::Overworld));
}
```

### Unit Tests

```rust
#[test] fn save_load_entities_roundtrip() { ... }
#[test] fn load_entities_missing_file_returns_empty() { ... }
#[test] fn save_entities_creates_file() { ... }
#[test] fn entity_kind_serialization() { ... }
```

### Integration Test (extend)

**File**: `crates/server/tests/world_persistence.rs`

Add to or extend the integration test:

```rust
#[test]
fn entities_persist_across_server_restart() {
    // 1. Create server app, spawn respawn points with MapSaveTarget
    // 2. Trigger save
    // 3. Drop app
    // 4. Create new server app, load from same directory
    // 5. Verify respawn point entities exist at correct positions
}
```

### Success Criteria:

#### Automated Verification:
- [ ] All tests pass: `cargo test-all`
- [ ] Server builds and runs: `cargo server`

#### Manual Verification:
- [ ] `worlds/overworld/entities.bin` exists after server run
- [ ] Respawn points persist across server restart
- [ ] Death/respawn uses loaded respawn points correctly

---

## Phase 4: Multi-Map Persistence

### Overview
Extend persistence to save/load Homebase maps. Each map type gets its own subdirectory under `worlds/`. Homebases save when the owning player disconnects or on server shutdown. Homebases load when a player transitions to their homebase.

### Changes Required:

#### 1. Add save_dir to VoxelMapConfig
**File**: `crates/voxel_map_engine/src/config.rs`

If not already added in Phase 2:

```rust
pub struct VoxelMapConfig {
    pub seed: u64,
    pub spawning_distance: u32,
    pub bounds: Option<IVec3>,
    pub tree_height: u32,
    pub generator: VoxelGenerator,
    pub save_dir: Option<PathBuf>,  // NEW
}
```

#### 2. Set save_dir when spawning maps
**File**: `crates/server/src/map.rs`

In `spawn_overworld` and `ensure_map_exists` (homebase), set `save_dir`:

```rust
// Overworld
config.save_dir = Some(map_save_dir(&save_path.0, &MapInstanceId::Overworld));

// Homebase
config.save_dir = Some(map_save_dir(&save_path.0, &MapInstanceId::Homebase { owner }));
```

#### 3. Save homebase state on player disconnect
**File**: `crates/server/src/map.rs`

Add system that triggers save when a player's homebase becomes unoccupied (no players in the homebase room). Alternatively, save all maps on the same debounce timer.

**Decision**: Save all map instances on the same debounce timer. Simpler, and the per-chunk dirty tracking means only modified chunks are written.

#### 4. Load homebase from disk in ensure_map_exists
**File**: `crates/server/src/map.rs`

When `ensure_map_exists` creates a homebase, check if a save directory exists. If so, load metadata (seed, bounds) from `map.meta.bin` and set `save_dir` on the config. Chunks will load from disk as they're generated (Phase 2's load-from-disk-first logic in `spawn_chunk_gen_task`).

#### 5. Clean up homebase entities on map unload
When a homebase map entity is despawned (e.g., all players left, server decides to unload), first save all dirty chunks and entities to disk.

### Unit Tests

```rust
#[test] fn map_save_dir_overworld() {
    assert_eq!(map_save_dir(Path::new("worlds"), &MapInstanceId::Overworld), Path::new("worlds/overworld"));
}
#[test] fn map_save_dir_homebase() {
    assert_eq!(map_save_dir(Path::new("worlds"), &MapInstanceId::Homebase { owner: 42 }), Path::new("worlds/homebase-42"));
}
#[test] fn multiple_maps_save_independently() { ... }
#[test] fn homebase_loads_from_existing_save() { ... }
```

### Integration Test (complete)

**File**: `crates/server/tests/world_persistence.rs`

The final integration test validates the full feature:

```rust
#[test]
fn full_world_persistence_cycle() {
    // 1. Create server with overworld
    // 2. Generate chunks, modify voxels, spawn entities
    // 3. Create a homebase for a player
    // 4. Modify homebase terrain
    // 5. Trigger full save
    // 6. Drop server app
    // 7. Create new server app from same save directory
    // 8. Verify:
    //    a. Overworld terrain modifications present
    //    b. Overworld entities (respawn points) present
    //    c. Overworld metadata (seed, spawn points) correct
    //    d. When homebase is loaded, its terrain modifications are present
    //    e. Homebase metadata correct (different seed from overworld)
}
```

### Success Criteria:

#### Automated Verification:
- [ ] All tests pass: `cargo test-all`
- [ ] Server builds and runs: `cargo server`
- [ ] Client builds and runs: `cargo client -c 1`

#### Manual Verification:
- [ ] `worlds/overworld/` and `worlds/homebase-<id>/` directories created
- [ ] Modify homebase terrain, restart server, transition to homebase → terrain persists
- [ ] Different players' homebases are isolated
- [ ] No performance regression during saves (per-chunk dirty tracking means minimal I/O)

---

## Testing Strategy

### Unit Tests (per phase):
- Phase 1: Octree insert/get/remove, fill type classification, chunk hash
- Phase 2: Chunk file save/load roundtrip, compression, filename parsing, metadata save/load, directory creation, corrupt file handling
- Phase 3: Entity serialization roundtrip, entity kind variants, missing file handling
- Phase 4: Map directory resolution, multi-map isolation

### Integration Test:
- Single test file `crates/server/tests/world_persistence.rs`
- Built incrementally across phases
- Final test validates: terrain persistence, entity persistence, metadata persistence, multi-map isolation
- Uses `MinimalPlugins` + server map systems (not full `CrossbeamTestStepper` — the persistence logic is server-only, no networking needed)

### Existing Test Updates:
- `crates/server/tests/voxel_persistence.rs` — update or replace tests to work with new directory-based system. Remove tests for the old `VoxelWorldSave` struct.

## Performance Considerations

- **zstd level 3**: Fast compression, ~90% size reduction on voxel data. Level 3 is the sweet spot.
- **Per-chunk dirty tracking**: Only modified chunks are re-saved. Most chunks save once (on generation) and never again unless a player edits them.
- **Atomic writes**: tmp file + rename prevents partial writes on crash.
- **Memory — octree eviction**: Chunk data is evicted from the octree when chunks leave the loaded range (same lifetime as the mesh entities). Only currently-loaded chunks occupy memory. At spawning_distance=2 (current overworld default), that's a 5x5x5 = 125 chunk cube per player × ~46KB/chunk ≈ ~6MB per player — negligible. Even at spawning_distance=10, eviction keeps memory bounded to the loaded set rather than accumulating indefinitely.
- **Eviction + persistence**: When a dirty chunk is evicted (Phase 2+), it must be saved to disk first. The save happens synchronously during eviction. This is acceptable because chunk saves are fast (bincode + zstd of ~46KB takes <1ms) and eviction only affects chunks at the edge of the loaded range (a few per frame at most).

## Migration Notes

- The old `world_save/voxel_world.bin` format is abandoned. No migration path — players must start fresh. Acceptable for pre-release.
- The old `VoxelWorldSave`, `VoxelModifications`, `VoxelDirtyState`, `VoxelSavePath` resources are removed.
- The `send_initial_voxel_state` observer that sends the full modification list to connecting clients should be reworked. Since modifications are now baked into chunk data (not tracked as a separate list), clients get the correct terrain from chunk generation (which now loads from disk). The `VoxelStateSync` message and `VoxelModifications` resource can be removed.

## References

- Research: [doc/research/2026-03-09-minecraft-style-map-directory-saving.md](doc/research/2026-03-09-minecraft-style-map-directory-saving.md)
- Original persistence research: [doc/research/2026-01-17-voxel-world-save-load.md](doc/research/2026-01-17-voxel-world-save-load.md)
- Current save system: [crates/server/src/map.rs:207-344](crates/server/src/map.rs#L207-L344)
- Octree API: [git/grid-tree-rs/src/tree.rs](git/grid-tree-rs/src/tree.rs)
- Existing persistence tests: [crates/server/tests/voxel_persistence.rs](crates/server/tests/voxel_persistence.rs)
- Integration test harness: [crates/server/tests/integration.rs:25-173](crates/server/tests/integration.rs#L25-L173)
