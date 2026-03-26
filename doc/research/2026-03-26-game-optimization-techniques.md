---
date: 2026-03-26T10:48:52-07:00
researcher: Claude
git_commit: ac0c8c45c6dcece0d80674c707487cf1d8bbbf3a
branch: master
repository: bevy-lightyear-template
topic: "Game optimization techniques for terrain, sprite rigs, and world objects"
tags: [research, optimization, flyweight, instancing, LOD, terrain, sprite-rig, world-object, voxel]
status: complete
last_updated: 2026-03-26
last_updated_by: Claude
---

# Research: Game Optimization Techniques for Terrain, Sprite Rigs, and World Objects

**Date**: 2026-03-26T10:48:52-07:00
**Researcher**: Claude
**Git Commit**: ac0c8c45c6dcece0d80674c707487cf1d8bbbf3a
**Branch**: master
**Repository**: bevy-lightyear-template

## Research Question

Game optimization techniques and how they can improve efficiency of terrain, sprite rigs, and/or world objects. Example: flyweight pattern.

## Summary

The project already employs several optimization techniques: paletted chunk compression (~16x), greedy quads meshing, mesh handle caching infrastructure, time-budgeted chunk work scheduling, ticket-based chunk loading/unloading, async generation batching, LOD mesh generation for vox models, and shared material resources. However, several optimization opportunities exist: LOD meshes are generated but never selected at runtime, mesh caching is built but not wired into spawn paths, sprite rig bones create per-entity meshes/materials that could be shared, and no texture atlas or GPU instancing is explicitly leveraged.

Below is a catalog of optimization techniques organized by what they target, documenting both what exists today and what relevant external techniques are available.

---

## Detailed Findings

### 1. Flyweight Pattern (Handle-Based Asset Sharing)

Bevy's `Handle<T>` system is the native flyweight implementation. `Handle::clone()` increments a reference count without duplicating data. Multiple entities reference the same underlying GPU resource.

#### What Exists Today

| System | Material Sharing | Mesh Sharing | Details |
|--------|-----------------|--------------|---------|
| Voxel chunks | Shared `DefaultVoxelMaterial` resource | Unique per chunk (geometry differs) | `lifecycle.rs:796` |
| World objects | Shared `DefaultVoxModelMaterial` resource | Shared via `VoxModelAsset.lod_meshes` sub-asset handles | `client/world_object.rs:161-163` |
| Sprite rig bones | Per-entity unique `StandardMaterial` | Per-entity unique `Plane3d` mesh | `sprite_rig/spawn.rs:126-134` |
| Health bars | Per-entity unique (mutated for color) | Per-entity unique `Plane3d` | `render/health_bar.rs:19-65` |

**Vox model sharing works well**: all instances of the same `.vox` file clone the same `Handle<Mesh>` from `VoxModelAsset::lod_meshes[0]` (`protocol/src/vox_model/loading.rs`). Combined with the shared material, Bevy's renderer can automatically batch draw calls for identical world objects.

**Sprite rig bones do not share**: each bone on each character creates its own `meshes.add(Plane3d::new(...))` and `materials.add(StandardMaterial {...})` at `spawn.rs:126-134`. For N characters with M visible bones each, this produces N*M unique mesh handles and N*M unique material handles.

#### Optimization Opportunity: Shared Bone Meshes/Materials

Bones with identical `size` parameters produce geometrically identical `Plane3d` meshes. A registry mapping `(size) -> Handle<Mesh>` would deduplicate these. Similarly, bones with the same placeholder color produce identical materials. Since these materials are `unlit: true` with only `base_color` varying, a small `HashMap<Color, Handle<StandardMaterial>>` would suffice.

When real textures replace placeholder colors, all bones sharing the same sprite atlas texture could share one material handle, enabling full sprite batching.

---

### 2. GPU Instancing and Automatic Batching

#### What Bevy Provides (0.16+)

Bevy's GPU-driven rendering pipeline automatically batches entities sharing the same `Handle<Mesh>` + `Handle<Material>` into single draw calls using multi-draw indirect (MDI). This is enabled automatically on Vulkan (Linux/Windows). No explicit instancing code is needed.

**Conditions for batching**:
- Same `Handle<Mesh>` (same asset, not just same geometry)
- Same `Handle<Material>` (same handle instance)
- Supported GPU backend (Vulkan, partial DX12/Metal/WebGPU, none on WebGL 2)

**MeshTag component (0.16+)**: Adds a `u32` per instance accessible in vertex shaders, enabling per-instance variation (color, animation frame) without breaking batches. Combined with `ShaderStorageBuffer`, provides flexible per-instance data.

#### Current State

- **World objects**: Already benefit from automatic batching -- shared mesh handle + shared material handle means identical world objects are batched.
- **Voxel chunks**: Cannot batch (each chunk has unique geometry), but share material.
- **Sprite rig bones**: Cannot batch (each bone has unique mesh + unique material handles).

#### Optimization Opportunity: Sprite Rig Batching via Shared Handles

If sprite rig bones shared mesh and material handles (see Flyweight section above), Bevy's automatic instancing would batch all same-bone-type draws across all characters into fewer draw calls.

**Diagnosing current batch counts**: Use `RenderDiagnosticsPlugin` or RenderDoc to measure actual draw call counts.

**Sources**:
- [Bevy 0.16 Release Notes -- GPU-Driven Rendering](https://bevy.org/news/bevy-0-16/)
- [Bevy 0.17 Release Notes](https://bevy.org/news/bevy-0-17/)
- [Automatic Instancing Example](https://bevy.org/examples/shaders/automatic-instancing/)

---

### 3. Sprite Atlasing and Batching

#### What Bevy Provides

- `TextureAtlasLayout` defines how to subdivide a sprite sheet (grid or custom rects)
- `TextureAtlas` component references a layout + index
- All sprites using the same atlas image batch together regardless of sub-sprite index
- Per-instance data (color tint, UV offset) is 80 bytes per sprite

**What breaks batches**: different source textures (`Handle<Image>`), different z-order layers, different blend modes.

#### Current State

**No texture atlas or `Sprite` component usage exists in the project.** The sprite rig crate uses 3D planes (`Plane3d` + `Mesh3d` + `MeshMaterial3d`) with solid placeholder colors, not Bevy's 2D sprite pipeline.

#### Optimization Opportunity: Atlas-Based Bone Rendering

When real textures are added to sprite rig bones, packing all bone sprites for a character type into one atlas texture would allow all bones of the same type to share one `Handle<Image>`, enabling the 2D sprite batcher to merge them. Alternatively, if staying with 3D planes (for depth sorting in 2.5D), a texture atlas on a shared `StandardMaterial` with per-bone UV offsets would achieve similar results.

**Sources**:
- [Sprite Sheet Example](https://bevy.org/examples/2d-rendering/sprite-sheet/)
- [TextureAtlas docs](https://docs.rs/bevy/latest/bevy/prelude/struct.TextureAtlas.html)

---

### 4. Frustum Culling

#### What Bevy Provides

- **CPU frustum culling**: automatic for all entities with `Aabb` component (auto-computed for `Mesh3d`, `Mesh2d`, `Sprite`)
- **GPU frustum culling (0.16+)**: enabled automatically on supported hardware, replaces CPU culling
- **Occlusion culling (experimental)**: two-phase, opt-in via `DepthPrepass` + `OcclusionCulling` on camera

#### Current State

All chunk entities and world object entities with `Mesh3d` components get automatic frustum culling via auto-computed `Aabb`. No custom `Aabb` or manual culling logic exists.

#### Optimization Opportunity: Pre-Frustum Spatial Check

For large worlds with thousands of chunk column positions, checking chunk positions against the camera frustum before spawning entities at all (server-side or in the ticket system) would avoid creating entities that are immediately culled. The ticket system's Chebyshev distance already provides crude spatial filtering, but frustum awareness would be tighter.

**Sources**:
- [NoFrustumCulling docs](https://docs.rs/bevy/latest/bevy/render/view/visibility/struct.NoFrustumCulling.html)
- [GPU culling PR](https://github.com/bevyengine/bevy/pull/16670)

---

### 5. LOD Systems

#### What Bevy Provides: `VisibilityRange`

Built-in since Bevy 0.14. Specifies distance ranges for fade-in/out with dithered crossfade:

```rust
// High-detail: visible 0-50m, fades out 50-55m
VisibilityRange { start_margin: 0.0..0.0, end_margin: 50.0..55.0, use_aabb: false }
// Low-detail: fades in 50-55m, visible 55-200m
VisibilityRange { start_margin: 50.0..55.0, end_margin: 200.0..210.0, use_aabb: false }
```

Rule: `end_margin` of LOD N must equal `start_margin` of LOD N+1.

#### Current State: Vox Model LOD

LOD meshes **are generated** during asset loading (`protocol/src/vox_model/lod.rs:22`):
- LOD 0: full resolution, greedy-quads meshed
- LOD 1: 2x downsampled via majority-vote on 2x2x2 blocks
- LOD 2: 4x downsampled (if dimensions permit, minimum 4 per axis)

Stored as sub-assets: `"mesh_lod0"`, `"mesh_lod1"`, `"mesh_lod2"` in `VoxModelAsset::lod_meshes`.

**Never selected at runtime**: `get_lod0_mesh()` always returns index 0. `attach_vox_mesh()` always takes `.first()`. No distance-based switching exists.

#### Current State: Chunk LOD

`VoxelChunk` has a `lod_level: u8` field (`chunk.rs:5`), always set to 0. No code reads this field for rendering or generation decisions.

#### Optimization Opportunity: Wire Up LOD Selection

For **world objects**: spawn sibling entities for each LOD level with `VisibilityRange` components. Since LOD meshes already exist in `VoxModelAsset::lod_meshes`, this is a matter of spawning 2-3 child entities instead of 1, each with appropriate distance ranges.

For **terrain chunks**: implement distance-based LOD where farther chunks use lower voxel resolution before meshing. The `lod_level` field is already on `VoxelChunk` but unused. The `PalettedChunk` could be downsampled using the same `downsample_2x` logic from `vox_model/lod.rs`.

#### Optimization Opportunity: Sprite Rig LOD

At distance, multi-bone sprite rigs (1+1+N entities per character) could be replaced with a single billboard sprite entity. This would dramatically reduce entity count for distant characters.

**Sources**:
- [VisibilityRange Example](https://bevy.org/examples/3d-rendering/visibility-range/)
- [Mesh LOD Support Issue](https://github.com/bevyengine/bevy/issues/6868)

---

### 6. Terrain-Specific Optimizations

#### What Exists Today

| Optimization | Implementation | Location |
|-------------|---------------|----------|
| Paletted compression | `PalettedChunk`: `SingleValue` variant for uniform chunks (0 bytes), bit-packed indices for mixed chunks (~16x compression for 2-type chunks) | `palette.rs` |
| Greedy quads meshing | `block_mesh` crate's `greedy_quads()`, merges adjacent faces with same material into larger quads | `meshing.rs` |
| Empty chunk skip | `FillType::Empty` skips meshing entirely -- returns `None` | `generation.rs:90` |
| Async generation | `AsyncComputeTaskPool` with batches of 4 chunks per task | `generation.rs:33` |
| Time budget | 4ms per frame (`ChunkWorkBudget`), ~25% of 60fps frame | `lifecycle.rs:27-33` |
| Per-frame caps | 64 gen spawns, 64 gen polls, 32 remesh spawns, 32 remesh polls, 16 save spawns | `lifecycle.rs:36-39,167` |
| In-flight caps | 256 pending gen tasks, 64 pending remesh, 32 pending save | `lifecycle.rs:42-44,170` |
| Priority queue | `GenQueue` min-heap: lower ticket level first, then closer distance | `lifecycle.rs:184` |
| Ticket propagation | Amortized BFS with 64 steps/frame budget, Chebyshev distance | `propagator.rs` |
| Mesh cache | `MeshCache` component: `HashMap<u64, Handle<Mesh>>` keyed by chunk data hash | `mesh_cache.rs` |
| Disk caching | Chunks loaded from disk skip generation, go straight to meshing | `generation.rs:47-71` |

#### MeshCache Gap

`MeshCache` exists as infrastructure (`mesh_cache.rs`) but is **not wired into spawn paths**. The chunk spawn code in `lifecycle.rs:796` and `client/map.rs:186` calls `meshes.add(mesh)` directly without consulting the cache. Wiring it in would avoid re-uploading identical meshes for chunks with the same voxel content (e.g., fully solid chunks of the same material).

#### External Technique: binary-greedy-meshing

The `binary-greedy-meshing` crate achieves 50-200us per 62^3 chunk vs ~3ms for `block-mesh-rs`. Fixed to 62^3 chunk size (64^3 with padding). ~30x faster but requires different chunk dimensions or adaptation.

**Sources**:
- [binary-greedy-meshing crate](https://crates.io/crates/binary-greedy-meshing)
- [Voxel World Optimisations (Vercidium)](https://vercidium.com/blog/voxel-world-optimisations/)

---

### 7. Object Pooling and Entity Recycling

#### What Bevy Provides

Entity IDs use index + generation counter. Despawned IDs return to the allocator; next spawn reuses the index with incremented generation. Archetype-based storage makes spawn/despawn relatively cheap.

#### Current State

Chunks are fully despawned when unloaded and freshly spawned when loaded. No entity recycling exists.

#### Optimization Opportunity: Chunk Entity Pooling

Instead of despawning chunk entities when unloaded, mark them `Visibility::Hidden` and clear their mesh. When a new chunk is needed, reactivate a pooled entity and swap in the new `Handle<Mesh>`. This avoids archetype table churn. The `lifecycle.rs:1003-1006` remesh path already demonstrates handle-swapping on existing entities.

---

### 8. Render Pipeline

#### What Bevy Provides (0.16+)

- **Retained render world**: skips re-extracting unchanged entity data each frame
- **Bindless resources**: efficient texture management
- **11x transform propagation improvement**: hierarchical dirty bits (1.1ms to 0.1ms)
- **Deferred rendering**: optional, beneficial for many dynamic lights

#### Current State

The project uses default forward rendering. No custom render pipeline configuration exists. GPU-driven rendering is automatically active on supported hardware.

For a 2.5D game with limited dynamic lights, forward rendering is appropriate. Deferred rendering would only help if many point/spot lights are added (torches, spell effects, etc).

---

## Code References

### Terrain / Voxel Engine
- `crates/voxel_map_engine/src/palette.rs:24-46` -- PalettedChunk storage model
- `crates/voxel_map_engine/src/meshing.rs:11` -- mesh_chunk_greedy entry point
- `crates/voxel_map_engine/src/mesh_cache.rs:6` -- MeshCache component
- `crates/voxel_map_engine/src/lifecycle.rs:27-44` -- ChunkWorkBudget and caps
- `crates/voxel_map_engine/src/lifecycle.rs:184` -- GenQueue priority heap
- `crates/voxel_map_engine/src/ticket.rs:5-37` -- TicketType and ChunkTicket
- `crates/voxel_map_engine/src/propagator.rs:33-116` -- TicketLevelPropagator BFS
- `crates/voxel_map_engine/src/generation.rs:33` -- spawn_chunk_gen_batch
- `crates/voxel_map_engine/src/chunk.rs:5` -- VoxelChunk with unused lod_level

### Sprite Rigs
- `crates/sprite_rig/src/spawn.rs:119-134` -- per-entity mesh/material creation
- `crates/sprite_rig/src/spawn.rs:251-275` -- billboard_rigs_face_camera
- `crates/sprite_rig/src/animation.rs:370-405` -- build_anim_graphs (shared)
- `crates/sprite_rig/src/animation.rs:489-518` -- attach_animation_players

### World Objects
- `crates/client/src/world_object.rs:13-23` -- DefaultVoxModelMaterial (shared)
- `crates/client/src/world_object.rs:139-163` -- attach_vox_mesh (LOD 0 only)
- `crates/protocol/src/vox_model/lod.rs:22` -- generate_lod_meshes (up to 3 levels)
- `crates/protocol/src/vox_model/loading.rs:49` -- get_lod0_mesh (always LOD 0)

### Shared Material Resources
- `crates/voxel_map_engine/src/lifecycle.rs:110-125` -- DefaultVoxelMaterial init
- `crates/client/src/world_object.rs:17-23` -- DefaultVoxModelMaterial init

## Architecture Documentation

### Asset Sharing Patterns Currently Used

1. **Shared Resource Handle**: Single material created at startup, handle cloned to all entities (`DefaultVoxelMaterial`, `DefaultVoxModelMaterial`)
2. **Sub-Asset Handle Sharing**: Mesh handles created during asset loading as labeled sub-assets, cloned when attaching to entities (`VoxModelAsset.lod_meshes`)
3. **Registry-Based Handle Sharing**: `RigRegistry` and `VoxModelRegistry` store asset handles, distribute clones to entities on spawn
4. **Conditional Sharing**: Debug mode creates per-entity materials, production mode clones shared handle (chunk debug colors toggle)

### Work Scheduling Architecture

The chunk pipeline uses a layered throttling approach:
1. **Priority queue** (GenQueue): ensures closest/most-important chunks process first
2. **Time budget** (4ms): prevents frame stalls
3. **Per-frame caps**: prevents any single stage from monopolizing
4. **In-flight caps**: prevents unbounded memory growth from pending async tasks
5. **Lazy deletion**: stale queue entries are skipped rather than eagerly removed

## Historical Context (from doc/)

- `doc/plans/2026-03-22-chunk-pipeline-optimizations.md` -- Prior optimization plan for chunk pipeline
- `doc/research/2026-03-20-performance-profiling-tools.md` -- Tracy profiling setup for terrain
- `doc/research/2026-02-03-bevy-voxel-world-replacement-audit.md` -- Features to re-implement from bevy_voxel_world
- `doc/research/2026-03-02-replacing-sdf-with-greedy-quads.md` -- Migration from SDF to greedy quads
- `doc/design/2026-03-14-archetype-from-asset-guide.md` -- Archetype-from-Asset pattern (flyweight-adjacent)
- `doc/research/2026-01-03-bevy-ecs-chunk-visibility-patterns.md` -- Chunk visibility ECS patterns
- `doc/research/2026-03-20-minecraft-chunk-ticket-system.md` -- Minecraft ticket system reference

## Open Questions

1. **MeshCache wiring**: The `MeshCache` component exists but is not consulted during chunk spawning. Is this intentional (deferred optimization) or an oversight?
2. **Vox model LOD activation**: LOD 1-2 meshes are generated and stored but never used. What distance thresholds would be appropriate for the game's camera range?
3. **Chunk LOD**: The `lod_level` field exists on `VoxelChunk` but is always 0. What's the intended plan for chunk LOD -- downsample voxels before meshing, or use `VisibilityRange` on chunk entities?
4. **Sprite rig entity count at scale**: With N characters * (1 root + 1 billboard + M bones) entities, what is the practical entity ceiling before performance degrades? Would distant characters benefit from a single-entity billboard fallback?
5. **binary-greedy-meshing compatibility**: The crate requires 62^3 chunks. Current chunk size is 16^3 (padded to 18^3). Would the performance gain justify changing chunk dimensions?
6. **Render diagnostics baseline**: No draw call count baseline exists. Profiling with `RenderDiagnosticsPlugin` would quantify whether instancing/batching improvements are worth pursuing.
