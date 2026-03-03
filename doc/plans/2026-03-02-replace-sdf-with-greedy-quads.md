# Replace SDF with Greedy Quads Implementation Plan

## Overview

Replace the voxel_map_engine's SDF-based generation and surface-nets meshing with direct `WorldVoxel` generation and greedy quads meshing. This eliminates the `fast-surface-nets` dependency, simplifies the API layer, and produces blocky (Minecraft-style) meshes with UV coordinates.

## Current State Analysis

The engine uses `SdfGenerator = Arc<dyn Fn(IVec3) -> Vec<f32>>`. Generators produce 5832-element float arrays, `SurfaceNetsMesher` runs fast-surface-nets for smooth meshes, and the API layer (`get_voxel`, `raycast`) generates full chunk SDFs and thresholds at `< 0.0` to produce `WorldVoxel` values. Overrides stamp `-1.0`/`1.0` sentinels, losing material information.

### Key Discoveries:
- `block-mesh` crate is already a dependency with zero imports ([Cargo.toml:12](crates/voxel_map_engine/Cargo.toml#L12))
- `WorldVoxel` already exists with `Air`, `Unset`, `Solid(u8)` variants ([types.rs:13-17](crates/voxel_map_engine/src/types.rs#L13-L17))
- `PaddedChunkShape` (18^3) is shared by both algorithms — no change needed
- `block-mesh::greedy_quads` requires `MergeVoxel: Voxel` trait impls on the voxel type
- `greedy_quads` signature: `greedy_quads(voxels, shape, [0;3], [17;3], &faces, &mut buffer)`
- `OrientedBlockFace` provides `quad_mesh_positions`, `quad_mesh_normals`, `quad_mesh_indices`, `tex_coords`

## Desired End State

- `SdfGenerator` type replaced by `VoxelGenerator = Arc<dyn Fn(IVec3) -> Vec<WorldVoxel>>`
- `SurfaceNetsMesher` replaced by `mesh_chunk_greedy(&[WorldVoxel]) -> Option<Mesh>`
- `fast-surface-nets` dependency removed
- API layer indexes `Vec<WorldVoxel>` directly (no SDF threshold conversion)
- `apply_overrides` writes `WorldVoxel` values directly (preserves material ID)
- All tests pass, examples compile, server/client work at runtime
- Meshes are blocky with UV coordinates

## What We're NOT Doing

- Ambient occlusion (deferred — data is available but not part of this change)
- Per-material texture atlas or custom vertex attributes (future work)
- Dual-mode SDF+greedy support (SDF is removed entirely)

## Implementation Approach

Single atomic phase — all files change together since the type substitution (`Vec<f32>` → `Vec<WorldVoxel>`) must be consistent for compilation.

## Phase 1: Replace SDF with Greedy Quads

### Overview
Replace `SdfGenerator` with `VoxelGenerator`, `SurfaceNetsMesher` with greedy quads meshing, and simplify all API/generation code that previously converted between SDF floats and `WorldVoxel`.

### Changes Required:

#### 1. Add Voxel Trait Impls
**File**: `crates/voxel_map_engine/src/types.rs`
**Changes**: Add `block_mesh::Voxel` and `block_mesh::MergeVoxel` implementations for `WorldVoxel`.

```rust
impl block_mesh::Voxel for WorldVoxel {
    fn get_visibility(&self) -> block_mesh::VoxelVisibility {
        match self {
            WorldVoxel::Air | WorldVoxel::Unset => block_mesh::VoxelVisibility::Empty,
            WorldVoxel::Solid(_) => block_mesh::VoxelVisibility::Opaque,
        }
    }
}

impl block_mesh::MergeVoxel for WorldVoxel {
    type MergeValue = u8;
    type MergeValueFacingNeighbour = u8;

    fn merge_value(&self) -> u8 {
        match self {
            WorldVoxel::Solid(m) => *m,
            _ => 0,
        }
    }

    fn merge_value_facing_neighbour(&self) -> u8 {
        self.merge_value()
    }
}
```

#### 2. Replace SdfGenerator Type
**File**: `crates/voxel_map_engine/src/config.rs`
**Changes**: Rename type alias, update doc comment, update field type.

```rust
// Before
pub type SdfGenerator = Arc<dyn Fn(IVec3) -> Vec<f32> + Send + Sync>;

// After
pub type VoxelGenerator = Arc<dyn Fn(IVec3) -> Vec<WorldVoxel> + Send + Sync>;
```

All references to `SdfGenerator` in the struct and constructor become `VoxelGenerator`. Import `WorldVoxel` from `crate::types`.

#### 3. Replace Meshing
**File**: `crates/voxel_map_engine/src/meshing.rs`
**Changes**: Remove `fast_surface_nets` import, `VoxelMesher` trait, `SurfaceNetsMesher`. Add `block_mesh` import and `mesh_chunk_greedy` function. Replace `flat_terrain_sdf` with `flat_terrain_voxels`.

```rust
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use block_mesh::{greedy_quads, GreedyQuadsBuffer, RIGHT_HANDED_Y_UP_CONFIG};
use ndshape::ConstShape;

use crate::types::{CHUNK_SIZE, PaddedChunkShape, WorldVoxel};

/// Mesh a padded 18^3 voxel array into a Bevy Mesh using greedy quads.
pub fn mesh_chunk_greedy(voxels: &[WorldVoxel]) -> Option<Mesh> {
    debug_assert_eq!(voxels.len(), PaddedChunkShape::USIZE);

    let mut buffer = GreedyQuadsBuffer::new(voxels.len());
    let faces = RIGHT_HANDED_Y_UP_CONFIG.faces;
    greedy_quads(
        voxels,
        &PaddedChunkShape {},
        [0; 3],
        [17; 3],
        &faces,
        &mut buffer,
    );

    if buffer.quads.num_quads() == 0 {
        return None;
    }

    let num_vertices = buffer.quads.num_quads() * 4;
    let num_indices = buffer.quads.num_quads() * 6;

    let mut positions = Vec::with_capacity(num_vertices);
    let mut normals = Vec::with_capacity(num_vertices);
    let mut indices = Vec::with_capacity(num_indices);
    let mut tex_coords = Vec::with_capacity(num_vertices);

    for (group, face) in buffer.quads.groups.iter().zip(faces.iter()) {
        for quad in group.iter() {
            indices.extend_from_slice(&face.quad_mesh_indices(positions.len() as u32));
            positions.extend_from_slice(&face.quad_mesh_positions(quad, 1.0));
            normals.extend_from_slice(&face.quad_mesh_normals());
            tex_coords.extend_from_slice(&face.tex_coords(
                RIGHT_HANDED_Y_UP_CONFIG.u_flip_face,
                true,
                quad,
            ));
        }
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, tex_coords);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

/// Generate voxels for flat terrain at y=0.
/// world_y < 0 → Solid(0), world_y >= 0 → Air.
pub fn flat_terrain_voxels(chunk_pos: IVec3) -> Vec<WorldVoxel> {
    let mut voxels = vec![WorldVoxel::Air; PaddedChunkShape::USIZE];
    for i in 0..PaddedChunkShape::SIZE {
        let [_x, y, _z] = PaddedChunkShape::delinearize(i);
        let world_y = chunk_pos.y * CHUNK_SIZE as i32 + y as i32 - 1;
        if world_y < 0 {
            voxels[i as usize] = WorldVoxel::Solid(0);
        }
    }
    voxels
}
```

Update tests to use `flat_terrain_voxels` and `mesh_chunk_greedy`.

#### 4. Update Generation Pipeline
**File**: `crates/voxel_map_engine/src/generation.rs`
**Changes**: Replace `SdfGenerator` → `VoxelGenerator` in imports and `spawn_chunk_gen_task` signature. `generate_chunk` calls `mesh_chunk_greedy(&voxels)`. `apply_overrides` takes `&mut [WorldVoxel]` and writes voxels directly.

```rust
// generate_chunk
fn generate_chunk(
    position: IVec3,
    generator: &VoxelGenerator,
    overrides: &[(IVec3, WorldVoxel)],
) -> ChunkGenResult {
    let mut voxels = generator(position);
    apply_overrides(&mut voxels, position, overrides);
    let mesh = mesh_chunk_greedy(&voxels);
    ChunkGenResult { position, mesh }
}

// apply_overrides — directly writes WorldVoxel values
fn apply_overrides(voxels: &mut [WorldVoxel], chunk_pos: IVec3, overrides: &[(IVec3, WorldVoxel)]) {
    let chunk_origin = chunk_pos * CHUNK_SIZE as i32;
    for &(world_pos, voxel) in overrides {
        let local = world_pos - chunk_origin;
        let padded = [
            (local.x + 1) as u32,
            (local.y + 1) as u32,
            (local.z + 1) as u32,
        ];
        let index = PaddedChunkShape::linearize(padded) as usize;
        if index < voxels.len() {
            voxels[index] = voxel;
        }
    }
}
```

#### 5. Simplify API Layer
**File**: `crates/voxel_map_engine/src/api.rs`
**Changes**: Remove `evaluate_sdf_at`, `sdf_to_voxel`. Replace with direct voxel lookup. Update raycast cache from `Vec<f32>` to `Vec<WorldVoxel>`.

```rust
// get_voxel fallback (replaces evaluate_sdf_at)
fn evaluate_voxel_at(pos: IVec3, generator: &VoxelGenerator) -> WorldVoxel {
    let chunk_pos = voxel_to_chunk_pos(pos);
    let voxels = generator(chunk_pos);
    lookup_voxel_in_chunk(&voxels, pos, chunk_pos)
}

// Direct index into voxel array (replaces sdf_to_voxel)
fn lookup_voxel_in_chunk(voxels: &[WorldVoxel], voxel_pos: IVec3, chunk_pos: IVec3) -> WorldVoxel {
    let local = voxel_pos - chunk_pos * CHUNK_SIZE as i32;
    let padded = [
        (local.x + 1) as u32,
        (local.y + 1) as u32,
        (local.z + 1) as u32,
    ];
    let index = PaddedChunkShape::linearize(padded) as usize;
    if index < voxels.len() {
        voxels[index]
    } else {
        WorldVoxel::Unset
    }
}

// Raycast cache type changes
let mut cached_chunk: Option<(IVec3, Vec<WorldVoxel>)> = None;

// lookup_voxel uses lookup_voxel_in_chunk instead of sdf_to_voxel
fn lookup_voxel(
    voxel_pos: IVec3,
    instance: &VoxelMapInstance,
    generator: &VoxelGenerator,
    cached_chunk: &mut Option<(IVec3, Vec<WorldVoxel>)>,
) -> WorldVoxel {
    if let Some(&voxel) = instance.modified_voxels.get(&voxel_pos) {
        return voxel;
    }
    let chunk_pos = voxel_to_chunk_pos(voxel_pos);
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

Update doc comments on `get_voxel` and `raycast` to reference "voxel generator" instead of "SDF generator".

#### 6. Update Instance Factory Methods
**File**: `crates/voxel_map_engine/src/instance.rs`
**Changes**: Replace `SdfGenerator` → `VoxelGenerator` in imports and all factory method signatures (`overworld`, `homebase`, `arena`). Update test `dummy_generator` to return `Vec<WorldVoxel>`.

```rust
// dummy_generator in tests
fn dummy_generator() -> VoxelGenerator {
    Arc::new(|_| vec![WorldVoxel::Air; 1])
}
```

#### 7. Remove fast-surface-nets Dependency
**File**: `crates/voxel_map_engine/Cargo.toml`
**Changes**: Remove line `fast-surface-nets = { path = "../../git/fast-surface-nets-rs" }`.

#### 8. Update Consumers
**File**: `crates/server/src/map.rs`
**Changes**: Import `flat_terrain_voxels` instead of `flat_terrain_sdf`. Update `Arc::new(flat_terrain_sdf)` → `Arc::new(flat_terrain_voxels)`.

**File**: `crates/client/src/map.rs`
**Changes**: Same — `flat_terrain_sdf` → `flat_terrain_voxels` in import and usage.

#### 9. Update Examples
**File**: `crates/voxel_map_engine/examples/terrain.rs`
**Changes**: `SdfGenerator` → `VoxelGenerator`, `flat_terrain_sdf` → `flat_terrain_voxels`.

**File**: `crates/voxel_map_engine/examples/editing.rs`
**Changes**: Same renames.

**File**: `crates/voxel_map_engine/examples/multi_instance.rs`
**Changes**: Replace `flat_terrain_sdf` → `flat_terrain_voxels`. Replace `raised_terrain_sdf` and `bowl_terrain_sdf` with voxel equivalents:

```rust
fn raised_terrain_voxels(chunk_pos: IVec3) -> Vec<WorldVoxel> {
    let mut voxels = vec![WorldVoxel::Air; PaddedChunkShape::USIZE];
    for i in 0..PaddedChunkShape::SIZE {
        let [_x, y, _z] = PaddedChunkShape::delinearize(i);
        let world_y = chunk_pos.y * CHUNK_SIZE as i32 + y as i32 - 1;
        if world_y < 4 {
            voxels[i as usize] = WorldVoxel::Solid(0);
        }
    }
    voxels
}

fn bowl_terrain_voxels(chunk_pos: IVec3) -> Vec<WorldVoxel> {
    let mut voxels = vec![WorldVoxel::Air; PaddedChunkShape::USIZE];
    for i in 0..PaddedChunkShape::SIZE {
        let [x, y, z] = PaddedChunkShape::delinearize(i);
        let world_x = (chunk_pos.x * CHUNK_SIZE as i32 + x as i32 - 1) as f32;
        let world_y = (chunk_pos.y * CHUNK_SIZE as i32 + y as i32 - 1) as f32;
        let world_z = (chunk_pos.z * CHUNK_SIZE as i32 + z as i32 - 1) as f32;
        let dist = (world_x * world_x + world_z * world_z).sqrt();
        let surface_y = -2.0 + dist * 0.15;
        if world_y < surface_y {
            voxels[i as usize] = WorldVoxel::Solid(0);
        }
    }
    voxels
}
```

#### 10. Update Tests
**File**: `crates/voxel_map_engine/tests/api.rs`
**Changes**: `SdfGenerator` → `VoxelGenerator`, `flat_terrain_sdf` → `flat_terrain_voxels`. Replace `all_air_sdf`:

```rust
fn all_air_voxels(_chunk_pos: IVec3) -> Vec<WorldVoxel> {
    vec![WorldVoxel::Air; PaddedChunkShape::USIZE]
}
```

**File**: `crates/voxel_map_engine/tests/lifecycle.rs`
**Changes**: Same type/function renames.

### Success Criteria:

#### Automated Verification:
- [x] Compiles: `cargo check -p voxel_map_engine`
- [x] All engine tests pass: `cargo test -p voxel_map_engine`
- [x] Examples compile: `cargo build -p voxel_map_engine --examples`
- [x] Full workspace builds: `cargo check`
- [x] Server builds and runs: `cargo server`
- [ ] Client builds and runs: `cargo client`

#### Manual Verification:
- [ ] `cargo run -p voxel_map_engine --example terrain` — blocky flat terrain renders
- [ ] `cargo run -p voxel_map_engine --example editing` — voxel place/remove works
- [ ] `cargo run -p voxel_map_engine --example multi_instance` — all three map types render
- [ ] Server + client: terrain loads, voxel editing works over network

---

## Testing Strategy

### Unit Tests (updated in-place):
- `meshing::tests` — `flat_terrain_voxels` produces mesh at surface, no mesh underground/sky, mesh has position+normal+UV attributes
- `api::tests` — `voxel_to_chunk_pos`, `evaluate_voxel_at` with flat terrain, `lookup_voxel_in_chunk` roundtrip

### Integration Tests (updated in-place):
- `tests/api.rs` — set/get roundtrip, SDF fallback (now voxel fallback), raycast, multi-instance isolation
- `tests/lifecycle.rs` — chunk spawn/despawn, bounds, target routing

### Manual Testing:
1. Run terrain example — confirm blocky mesh with flat surface at y=0
2. Run editing example — confirm place/remove voxels with click
3. Run multi_instance example — confirm all three terrain types (flat, raised, bowl) render as blocky meshes

## Performance Considerations

- Greedy quads produces fewer triangles than surface nets for blocky terrain (merged faces)
- `apply_overrides` preserves material IDs (was lossy with SDF sentinels)
- API `get_voxel` is slightly simpler (no threshold comparison)
- `GreedyQuadsBuffer` could be cached/reused per-thread for reduced allocations (future optimization)

## References

- Research: [doc/research/2026-03-02-replacing-sdf-with-greedy-quads.md](doc/research/2026-03-02-replacing-sdf-with-greedy-quads.md)
- block-mesh API: [git/block-mesh-rs/src/greedy.rs](git/block-mesh-rs/src/greedy.rs)
- Reference impl: [git/bevy_voxel_world/src/voxel.rs:14-47](git/bevy_voxel_world/src/voxel.rs#L14-L47)
