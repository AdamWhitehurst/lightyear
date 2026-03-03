---
date: 2026-03-02T15:04:15-08:00
researcher: Claude
git_commit: 83fd0e438dd4f31232b281deb82248657f171a49
branch: master
repository: bevy-lightyear-template
topic: "How to replace voxel_map_engine's SDF generation with blocky greedy quads generation"
tags: [research, codebase, voxel-map-engine, greedy-meshing, block-mesh, sdf-removal, migration]
status: complete
last_updated: 2026-03-02
last_updated_by: Claude
---

# Research: Replacing SDF Generation with Greedy Quads

**Date**: 2026-03-02T15:04:15-08:00
**Researcher**: Claude
**Git Commit**: 83fd0e438dd4f31232b281deb82248657f171a49
**Branch**: master
**Repository**: bevy-lightyear-template

## Research Question

How to replace voxel_map_engine's SDF generation with blocky greedy quads generation — eliminating SDF entirely, not adding a second mode.

## Summary

The replacement touches 8 source files in the engine, 2 consumer files (server/client), 3 examples, and 2 integration test files. The core change is: replace `SdfGenerator` (`Arc<dyn Fn(IVec3) -> Vec<f32>>`) with `VoxelGenerator` (`Arc<dyn Fn(IVec3) -> Vec<WorldVoxel>>`), replace `SurfaceNetsMesher` with a `GreedyQuadsMesher`, and simplify all API/raycast code that currently evaluates SDF and thresholds at `< 0.0`. The `block-mesh` crate is already a dependency with zero imports; `fast-surface-nets` can be removed entirely. `WorldVoxel` needs `Voxel` + `MergeVoxel` trait impls (reference code exists in `git/bevy_voxel_world`). The pipeline simplification is significant: the API layer (`get_voxel`, `raycast`) currently generates a full 5832-element `Vec<f32>` and thresholds a single element — with `VoxelGenerator`, it generates `Vec<WorldVoxel>` and indexes directly, which is cleaner and faster.

---

## Detailed Findings

### 1. What Gets Removed

#### SDF Types and Functions

| Item | Location | Action |
|------|----------|--------|
| `SdfGenerator` type alias | [config.rs:5](crates/voxel_map_engine/src/config.rs#L5) | Replace with `VoxelGenerator` |
| `SurfaceNetsMesher` struct + impl | [meshing.rs:15-37](crates/voxel_map_engine/src/meshing.rs#L15-L37) | Remove entirely |
| `VoxelMesher` trait (SDF-based) | [meshing.rs:10-12](crates/voxel_map_engine/src/meshing.rs#L10-L12) | Remove or redefine |
| `flat_terrain_sdf` | [meshing.rs:41-49](crates/voxel_map_engine/src/meshing.rs#L41-L49) | Replace with `flat_terrain_voxels` |
| `fast-surface-nets` import | [meshing.rs:4](crates/voxel_map_engine/src/meshing.rs#L4) | Remove |
| `fast-surface-nets` dependency | [Cargo.toml:11](crates/voxel_map_engine/Cargo.toml#L11) | Remove |
| `evaluate_sdf_at` | [api.rs:119-123](crates/voxel_map_engine/src/api.rs#L119-L123) | Remove |
| `sdf_to_voxel` | [api.rs:125-140](crates/voxel_map_engine/src/api.rs#L125-L140) | Remove |
| SDF cache in raycast | [api.rs:70](crates/voxel_map_engine/src/api.rs#L70) | Replace with `Vec<WorldVoxel>` cache |
| SDF-based `apply_overrides` | [generation.rs:80-99](crates/voxel_map_engine/src/generation.rs#L80-L99) | Replace with direct voxel write |

#### Custom SDF Functions in Examples

| Function | Location | Action |
|----------|----------|--------|
| `raised_terrain_sdf` | [multi_instance.rs:101-109](crates/voxel_map_engine/examples/multi_instance.rs#L101-L109) | Replace with voxel equivalent |
| `bowl_terrain_sdf` | [multi_instance.rs:111-123](crates/voxel_map_engine/examples/multi_instance.rs#L111-L123) | Replace with voxel equivalent |
| `all_air_sdf` | [tests/api.rs:352-354](crates/voxel_map_engine/tests/api.rs#L352-L354) | Replace with `all_air_voxels` |

### 2. What Gets Added

#### `VoxelGenerator` Type

Replace `SdfGenerator` in [config.rs:5](crates/voxel_map_engine/src/config.rs#L5):

```rust
// Before
pub type SdfGenerator = Arc<dyn Fn(IVec3) -> Vec<f32> + Send + Sync>;

// After
pub type VoxelGenerator = Arc<dyn Fn(IVec3) -> Vec<WorldVoxel> + Send + Sync>;
```

Same contract: given chunk position `IVec3`, return `PaddedChunkShape::USIZE` (5832) elements. Just `WorldVoxel` instead of `f32`.

#### `Voxel` and `MergeVoxel` Impls for `WorldVoxel`

Add to [types.rs](crates/voxel_map_engine/src/types.rs). Reference impl exists at [git/bevy_voxel_world/src/voxel.rs:14-47](git/bevy_voxel_world/src/voxel.rs#L14-L47):

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
        match self { WorldVoxel::Solid(m) => *m, _ => 0 }
    }

    fn merge_value_facing_neighbour(&self) -> u8 {
        self.merge_value()
    }
}
```

This enables voxels with different `Solid(material_id)` values to generate separate quads per material — faces only merge when adjacent voxels share the same material.

#### `GreedyQuadsMesher`

Replace `SurfaceNetsMesher` in [meshing.rs](crates/voxel_map_engine/src/meshing.rs). The core pattern from all reference implementations:

```rust
use block_mesh::{
    greedy_quads, GreedyQuadsBuffer, RIGHT_HANDED_Y_UP_CONFIG,
};

pub fn mesh_chunk_greedy(voxels: &[WorldVoxel]) -> Option<Mesh> {
    let mut buffer = GreedyQuadsBuffer::new(voxels.len());
    let faces = RIGHT_HANDED_Y_UP_CONFIG.faces;
    greedy_quads(voxels, &PaddedChunkShape {}, [0; 3], [17; 3], &faces, &mut buffer);

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
                RIGHT_HANDED_Y_UP_CONFIG.u_flip_face, true, quad,
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
```

Key parameters:
- `voxel_size = 1.0` — each voxel is 1 world unit
- `min = [0; 3]`, `max = [17; 3]` — full padded 18^3 array, same as current `surface_nets` call
- `flip_v = true` — standard for Bevy's texture coordinates

#### Replacement Generator Functions

`flat_terrain_sdf` → `flat_terrain_voxels`:

```rust
pub fn flat_terrain_voxels(chunk_pos: IVec3) -> Vec<WorldVoxel> {
    let mut voxels = vec![WorldVoxel::Air; PaddedChunkShape::USIZE];
    for i in 0..PaddedChunkShape::SIZE {
        let [_x, y, _z] = PaddedChunkShape::delinearize(i);
        let world_y = chunk_pos.y * CHUNK_SIZE as i32 + y as i32 - 1;
        voxels[i as usize] = if world_y < 0 {
            WorldVoxel::Solid(0)
        } else {
            WorldVoxel::Air
        };
    }
    voxels
}
```

Same logic, but instead of `sdf[i] = world_y as f32`, it's a discrete threshold: `world_y < 0 → Solid`, `>= 0 → Air`.

### 3. File-by-File Change Map

#### Engine Core (crates/voxel_map_engine/src/)

| File | Changes |
|------|---------|
| **config.rs** | `SdfGenerator` → `VoxelGenerator` (type alias). `VoxelMapConfig.generator` field type changes. Constructor parameter type changes. |
| **types.rs** | Add `use block_mesh::{...}`. Add `Voxel` and `MergeVoxel` impls for `WorldVoxel`. |
| **meshing.rs** | Remove `fast_surface_nets` import. Remove `VoxelMesher` trait, `SurfaceNetsMesher`. Add `block_mesh` import. Add `mesh_chunk_greedy(voxels: &[WorldVoxel]) -> Option<Mesh>`. Replace `flat_terrain_sdf` with `flat_terrain_voxels`. Update tests. |
| **generation.rs** | `SdfGenerator` → `VoxelGenerator` in imports and `spawn_chunk_gen_task` signature. `generate_chunk` calls `mesh_chunk_greedy(&voxels)` instead of `SurfaceNetsMesher.mesh_chunk(&sdf)`. `apply_overrides` directly writes `WorldVoxel` values into the array instead of stamping `-1.0`/`1.0`. |
| **api.rs** | `get_voxel`: call generator, index into `Vec<WorldVoxel>` directly (no `evaluate_sdf_at`). `lookup_voxel`: cache is `Option<(IVec3, Vec<WorldVoxel>)>` instead of `Option<(IVec3, Vec<f32>)>`. Remove `evaluate_sdf_at`, `sdf_to_voxel`. Add `lookup_voxel_in_chunk(voxels, voxel_pos, chunk_pos) -> WorldVoxel`. Update tests. |
| **instance.rs** | `SdfGenerator` → `VoxelGenerator` in factory method signatures. No structural changes. |
| **lifecycle.rs** | No changes (operates on `ChunkGenResult` which is meshing-agnostic). |
| **raycast.rs** | No changes (traversal algorithm is format-agnostic). |
| **mesh_cache.rs** | No changes (keyed by hash). |
| **lib.rs** | No changes (same system registration). |
| **Cargo.toml** | Remove `fast-surface-nets` line. `block-mesh` already present. |

#### Consumers

| File | Changes |
|------|---------|
| **crates/server/src/map.rs:27** | `Arc::new(flat_terrain_sdf)` → `Arc::new(flat_terrain_voxels)`. Import changes. |
| **crates/client/src/map.rs:49** | Same as server. |
| **examples/terrain.rs:16** | `SdfGenerator` → `VoxelGenerator`, `flat_terrain_sdf` → `flat_terrain_voxels`. |
| **examples/editing.rs:21** | Same as terrain. |
| **examples/multi_instance.rs** | Replace all three SDF functions with voxel equivalents. |
| **tests/api.rs** | `SdfGenerator` → `VoxelGenerator`. Replace `flat_terrain_sdf`, `all_air_sdf` with voxel equivalents. |
| **tests/lifecycle.rs** | Same type/function renames. |

### 4. Generation Pipeline: Before and After

**Before (SDF):**
```
SdfGenerator(chunk_pos)          → Vec<f32> (5832 elements)
  → apply_overrides(sdf, pos, overrides)   stamps -1.0/1.0 into float array
  → SurfaceNetsMesher.mesh_chunk(sdf)      fast-surface-nets isosurface extraction
  → Option<Mesh>
```

**After (Voxel):**
```
VoxelGenerator(chunk_pos)        → Vec<WorldVoxel> (5832 elements)
  → apply_overrides(voxels, pos, overrides)  directly writes WorldVoxel values
  → mesh_chunk_greedy(voxels)               block-mesh greedy_quads → Mesh
  → Option<Mesh>
```

### 5. API Simplification

**Before** — `get_voxel` evaluates a full chunk SDF and thresholds one element:
```rust
fn evaluate_sdf_at(pos, generator) -> WorldVoxel {
    let chunk_pos = voxel_to_chunk_pos(pos);
    let sdf = generator(chunk_pos);        // generate 5832 floats
    sdf_to_voxel(&sdf, pos, chunk_pos)     // threshold sdf[i] < 0.0
}
```

**After** — directly indexes into the voxel array:
```rust
fn evaluate_voxel_at(pos, generator) -> WorldVoxel {
    let chunk_pos = voxel_to_chunk_pos(pos);
    let voxels = generator(chunk_pos);     // generate 5832 WorldVoxels
    let local = pos - chunk_pos * CHUNK_SIZE as i32;
    let padded = [(local.x + 1) as u32, (local.y + 1) as u32, (local.z + 1) as u32];
    let index = PaddedChunkShape::linearize(padded) as usize;
    voxels[index]                          // direct lookup, no threshold
}
```

No `sdf_to_voxel` conversion — the generator output IS the voxel data.

### 6. apply_overrides Simplification

**Before** — stamps SDF sentinel values:
```rust
fn apply_overrides(sdf: &mut [f32], chunk_pos, overrides) {
    for &(world_pos, voxel) in overrides {
        let index = /* ... */;
        sdf[index] = match voxel {
            WorldVoxel::Solid(_) => -1.0,
            WorldVoxel::Air | WorldVoxel::Unset => 1.0,
        };
    }
}
```

**After** — directly writes the voxel:
```rust
fn apply_overrides(voxels: &mut [WorldVoxel], chunk_pos, overrides) {
    for &(world_pos, voxel) in overrides {
        let index = /* same indexing */;
        if index < voxels.len() {
            voxels[index] = voxel;
        }
    }
}
```

The override now preserves the exact `WorldVoxel` value (including material ID), instead of collapsing to a binary `-1.0`/`1.0` SDF stamp. This means `Solid(3)` overrides remain `Solid(3)` in the generated mesh, which was not possible with SDF.

### 7. Raycast Cache Change

The raycast in [api.rs:54-90](crates/voxel_map_engine/src/api.rs#L54-L90) caches the last chunk's data to avoid regenerating for adjacent voxels during traversal:

**Before:**
```rust
let mut cached_chunk: Option<(IVec3, Vec<f32>)> = None;
```

**After:**
```rust
let mut cached_chunk: Option<(IVec3, Vec<WorldVoxel>)> = None;
```

The `lookup_voxel` function changes from `sdf_to_voxel(sdf, pos, chunk_pos)` to a direct index into the `Vec<WorldVoxel>`.

### 8. Visual Differences

Surface nets produce smooth, interpolated meshes that cross voxel boundaries at the iso-surface. Greedy quads produce axis-aligned blocky meshes (Minecraft-style). This is a fundamental visual change:

- **Surface nets**: Smooth undulating terrain, gradients, rounded shapes
- **Greedy quads**: Flat faces, staircase slopes, sharp block boundaries

For the game's 2.5D brawler style, blocky terrain may be the intended aesthetic. The `Solid(u8)` material field becomes more meaningful with greedy quads because per-face material data can drive texture atlas lookups.

### 9. UV Coordinates (New Capability)

Surface nets meshes have no UV coordinates — the current `SurfaceNetsMesher` only outputs positions, normals, and indices. The current material (`DefaultVoxelMaterial`) uses `StandardMaterial` which doesn't rely on UVs.

Greedy quads naturally produce UV coordinates via `face.tex_coords()`. UV values are in voxel units (a 4-wide merged face gets U range 0..4), which enables tiling textures with `ImageAddressMode::Repeat`. This unlocks:
- Texture atlas per material ID
- Per-voxel-face texturing
- Tiling patterns that scale with quad size

### 10. Ambient Occlusion (Optional Enhancement)

The current engine has no AO. Reference implementation at [git/bevy_voxel_world/src/meshing.rs:270-370](git/bevy_voxel_world/src/meshing.rs#L270-L370) shows per-vertex AO for block meshes:

- Sample 8 neighbors per face corner
- Classic AO formula: `side1 && side2 → 0` (darkest), `nothing → 3` (brightest)
- Stored as vertex colors (`ATTRIBUTE_COLOR`)

This is not required for the initial replacement but is a natural follow-up since the voxel data needed for AO computation is already available in the `Vec<WorldVoxel>` array.

---

## Code References

### Engine Files to Modify
- [config.rs:5](crates/voxel_map_engine/src/config.rs#L5) — `SdfGenerator` type alias → `VoxelGenerator`
- [config.rs:14](crates/voxel_map_engine/src/config.rs#L14) — `generator` field type
- [meshing.rs:4](crates/voxel_map_engine/src/meshing.rs#L4) — `fast_surface_nets` import (remove)
- [meshing.rs:10-37](crates/voxel_map_engine/src/meshing.rs#L10-L37) — `VoxelMesher` trait + `SurfaceNetsMesher` (remove, replace)
- [meshing.rs:41-49](crates/voxel_map_engine/src/meshing.rs#L41-L49) — `flat_terrain_sdf` (replace)
- [generation.rs:9-10](crates/voxel_map_engine/src/generation.rs#L9-L10) — imports
- [generation.rs:27-41](crates/voxel_map_engine/src/generation.rs#L27-L41) — `spawn_chunk_gen_task`
- [generation.rs:67-76](crates/voxel_map_engine/src/generation.rs#L67-L76) — `generate_chunk`
- [generation.rs:80-99](crates/voxel_map_engine/src/generation.rs#L80-L99) — `apply_overrides`
- [api.rs:22-33](crates/voxel_map_engine/src/api.rs#L22-L33) — `get_voxel`
- [api.rs:70](crates/voxel_map_engine/src/api.rs#L70) — raycast cache type
- [api.rs:94-140](crates/voxel_map_engine/src/api.rs#L94-L140) — `lookup_voxel`, `evaluate_sdf_at`, `sdf_to_voxel`
- [types.rs](crates/voxel_map_engine/src/types.rs) — add trait impls
- [instance.rs:5](crates/voxel_map_engine/src/instance.rs#L5) — `SdfGenerator` import
- [instance.rs:46-97](crates/voxel_map_engine/src/instance.rs#L46-L97) — factory method signatures
- [Cargo.toml:11](crates/voxel_map_engine/Cargo.toml#L11) — `fast-surface-nets` dependency (remove)

### Consumer Files to Update
- [crates/server/src/map.rs:27](crates/server/src/map.rs#L27) — `Arc::new(flat_terrain_sdf)`
- [crates/client/src/map.rs:49](crates/client/src/map.rs#L49) — `Arc::new(flat_terrain_sdf)`
- [examples/terrain.rs:16](crates/voxel_map_engine/examples/terrain.rs#L16) — generator binding
- [examples/editing.rs:21](crates/voxel_map_engine/examples/editing.rs#L21) — generator binding
- [examples/multi_instance.rs:55-123](crates/voxel_map_engine/examples/multi_instance.rs#L55-L123) — three SDF functions + factory calls
- [tests/api.rs:18-30](crates/voxel_map_engine/tests/api.rs#L18-L30) — test helpers
- [tests/api.rs:352-354](crates/voxel_map_engine/tests/api.rs#L352-L354) — `all_air_sdf`
- [tests/lifecycle.rs:16-31](crates/voxel_map_engine/tests/lifecycle.rs#L16-L31) — test helpers

### block-mesh Crate Reference
- [git/block-mesh-rs/src/greedy.rs:64-83](git/block-mesh-rs/src/greedy.rs#L64-L83) — `greedy_quads` function
- [git/block-mesh-rs/src/greedy.rs:15-24](git/block-mesh-rs/src/greedy.rs#L15-L24) — `MergeVoxel` trait
- [git/block-mesh-rs/src/lib.rs:108-110](git/block-mesh-rs/src/lib.rs#L108-L110) — `Voxel` trait
- [git/block-mesh-rs/src/geometry.rs:171-183](git/block-mesh-rs/src/geometry.rs#L171-L183) — `RIGHT_HANDED_Y_UP_CONFIG`

### Reference Implementations
- [git/bevy_voxel_world/src/voxel.rs:14-47](git/bevy_voxel_world/src/voxel.rs#L14-L47) — `WorldVoxel` trait impls
- [git/bevy_voxel_world/examples/custom_meshing.rs:79-129](git/bevy_voxel_world/examples/custom_meshing.rs#L79-L129) — greedy quads → Bevy Mesh
- [git/bevy_voxel_world/src/meshing.rs:270-370](git/bevy_voxel_world/src/meshing.rs#L270-L370) — AO computation

## Architecture Documentation

### Dependency Changes

```
Remove:  fast-surface-nets = { path = "../../git/fast-surface-nets-rs" }
Keep:    block-mesh = { path = "../../git/block-mesh-rs" }  (already present, currently unused)
Keep:    ndshape (used for PaddedChunkShape — shared by both algorithms)
```

### Type Substitution Map

| Before (SDF) | After (Voxel) |
|---------------|---------------|
| `SdfGenerator` = `Arc<dyn Fn(IVec3) -> Vec<f32>>` | `VoxelGenerator` = `Arc<dyn Fn(IVec3) -> Vec<WorldVoxel>>` |
| `Vec<f32>` (chunk data) | `Vec<WorldVoxel>` (chunk data) |
| `SurfaceNetsMesher.mesh_chunk(&sdf)` | `mesh_chunk_greedy(&voxels)` |
| `sdf[i] < 0.0 → Solid(0)` | `voxels[i]` (direct) |
| `sdf[i] = -1.0` (override stamp) | `voxels[i] = WorldVoxel::Solid(m)` (direct write) |
| `Option<(IVec3, Vec<f32>)>` (cache) | `Option<(IVec3, Vec<WorldVoxel>)>` (cache) |
| `flat_terrain_sdf` | `flat_terrain_voxels` |

### Validation Commands

```bash
cargo check -p voxel_map_engine          # compilation
cargo test -p voxel_map_engine           # unit + integration tests
cargo run -p voxel_map_engine --example terrain   # visual check
cargo server                             # runtime verification
cargo client                             # runtime verification
```

## Historical Context (from doc/)

- [doc/research/2026-03-02-greedy-blocky-voxel-meshing-support.md](doc/research/2026-03-02-greedy-blocky-voxel-meshing-support.md) — Comprehensive research on adding greedy meshing as a second option alongside SDF. This document covers the replacement-only case (simpler subset).
- [doc/research/2026-02-27-bonsairobo-stack-multi-instance-voxel-replacement.md](doc/research/2026-02-27-bonsairobo-stack-multi-instance-voxel-replacement.md) — Original architecture design. Section 5 documents planned `WorldVoxel` trait impls. Section 12 describes smooth meshing as primary.
- [doc/research/2026-03-02-declarative-sdf-map-config.md](doc/research/2026-03-02-declarative-sdf-map-config.md) — Declarative config research. The `TerrainShape` enum approach works identically for `VoxelGenerator` closures as for `SdfGenerator`.
- [doc/research/2026-03-02-persisting-full-map-world-data.md](doc/research/2026-03-02-persisting-full-map-world-data.md) — Notes that blocky meshing is a prerequisite for discrete voxel persistence.

## Related Research

- [doc/research/2026-03-02-greedy-blocky-voxel-meshing-support.md](doc/research/2026-03-02-greedy-blocky-voxel-meshing-support.md)
- [doc/research/2026-02-27-bonsairobo-stack-multi-instance-voxel-replacement.md](doc/research/2026-02-27-bonsairobo-stack-multi-instance-voxel-replacement.md)
- [doc/research/2026-03-02-declarative-sdf-map-config.md](doc/research/2026-03-02-declarative-sdf-map-config.md)

## Decisions

1. **UV generation**: Include from day one. Low-cost (3 lines) and future-proofs for texturing.
2. **AO**: Defer. Not part of initial replacement.
3. **Material data**: Custom vertex attribute (like reference `ATTRIBUTE_TEX_INDEX`). Look up `voxels[PaddedChunkShape::linearize(quad.minimum)]` per quad.
4. **Surface threshold**: `world_y < 0` → Solid, `world_y >= 0` → Air. Surface at y=0 (exclusive — y=0 is air).
