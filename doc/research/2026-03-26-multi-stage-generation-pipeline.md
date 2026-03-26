---
date: 2026-03-26T10:39:25-07:00
researcher: claude
git_commit: ac0c8c45c6dcece0d80674c707487cf1d8bbbf3a
branch: master
repository: bevy-lightyear-template
topic: "Multi-stage generation pipeline in voxel_map_engine modelled after Minecraft's system to support feature placement"
tags: [research, voxel-map-engine, multi-stage-generation, feature-placement, chunk-pipeline, minecraft]
status: complete
last_updated: 2026-03-26
last_updated_by: claude
last_updated_note: "Resolved all open questions, elaborated NeighborAccess structure, future stage expansion, confirmed Option C hybrid approach"
---

# Research: Multi-Stage Generation Pipeline for Feature Placement

**Date**: 2026-03-26T10:39:25-07:00
**Researcher**: claude
**Git Commit**: ac0c8c45c6dcece0d80674c707487cf1d8bbbf3a
**Branch**: master
**Repository**: bevy-lightyear-template

## Research Question

How to implement a multi-stage generation pipeline in `voxel_map_engine` modelled after Minecraft's system (see `doc/research/2026-03-20-minecraft-chunk-ticket-system.md`) to support feature placement.

## Summary

The current generation pipeline is single-pass: one `VoxelGenerator` closure produces a complete 18³ padded voxel array per chunk, followed immediately by greedy meshing. There is no `ChunkStatus` enum, no per-stage neighbor requirements, and no mechanism for a generator to access adjacent chunk data. Feature placement (trees, ores, structures) does not exist.

Minecraft's pipeline has 12 stages (EMPTY → STRUCTURE_STARTS → ... → FULL), each with a `taskMargin` defining how many neighbor chunks must be at the previous stage. The key insight: features that cross chunk boundaries (trees, structures) require a 1-ring of neighbors at terrain stage before the center chunk can run its feature stage. The ticket level determines maximum achievable status — chunks deep in the inaccessible range only advance to early stages, serving as neighbor data for closer chunks.

The existing ticket system and level propagator (`TicketLevelPropagator`, `LOAD_LEVEL_THRESHOLD = 20`) already produce effective levels per column far beyond the Border (2) threshold. Levels 3–20 currently map to "Inaccessible" but are loaded anyway — this range is exactly where multi-stage generation would gate maximum achievable `ChunkStatus`. The infrastructure for level-gated generation already exists; only the per-chunk stage tracking and neighbor-aware generation logic need to be added.

---

## Current State of the Generation Pipeline

### Single-Pass Architecture

The generator is a closure: `VoxelGenerator(Arc<dyn Fn(IVec3) -> Vec<WorldVoxel> + Send + Sync>)` (`config.rs:12-13`). It receives only a chunk position. No world reference, no neighbor access, no ECS queries. Each chunk is generated in complete isolation on `AsyncComputeTaskPool`.

**Generation flow** (`generation.rs:33-102`):
1. `drain_gen_queue` pops from priority heap, batches 4 positions (`GEN_BATCH_SIZE`)
2. `spawn_chunk_gen_batch` spawns one async task per batch
3. Each position: disk load attempt → on miss, `generator(position)` → `ChunkData::from_voxels` → `mesh_chunk_greedy`
4. Returns `Vec<ChunkGenResult>` containing position, mesh, chunk_data, from_disk flag

**`ChunkGenResult`** (`generation.rs:16-22`):
```rust
pub struct ChunkGenResult {
    pub position: IVec3,
    pub mesh: Option<Mesh>,
    pub chunk_data: ChunkData,
    pub from_disk: bool,
}
```

**`ChunkData`** (`types.rs:36-63`):
```rust
pub struct ChunkData {
    pub voxels: PalettedChunk,
    pub fill_type: FillType,
    pub hash: u64,
}
```

No `status` field. A chunk is either absent from the octree, in-flight (tracked in `ChunkWorkTracker.generating`), or fully generated.

### Terrain Generator Construction

`build_generator` (`terrain.rs:307-331`) reads optional components from the map entity:
- `HeightMap` — noise params for terrain shape
- `MoistureMap` — noise params for biome moisture axis
- `BiomeRules` — biome selection by height/moisture, surface/subsurface materials

If `HeightMap` present, closure captures clones and calls `generate_heightmap_chunk` (`terrain.rs:188-229`): builds 2D height/moisture caches over 18×18 padded XZ footprint, fills 18³ voxels by comparing world_y against cached height, picks material via `BiomeRules`.

There is no feature placement. No trees, ores, structures, or decorations.

### Existing Throttling and Priority Infrastructure

All implemented per `doc/plans/2026-03-22-chunk-pipeline-optimizations.md`:

| Component | Description |
|---|---|
| `ChunkWorkBudget` | 4ms per-frame time budget (`lifecycle.rs:27-64`) |
| `GenQueue` | `BinaryHeap<ChunkWork>` sorted by (level ASC, distance ASC) (`lifecycle.rs:184-187`) |
| `ChunkWorkTracker` | `generating: HashSet<IVec3>`, `remeshing: HashSet<IVec3>` (`lifecycle.rs:173-177`) |
| Hard caps | Gen spawns: 64, gen polls: 64, remesh spawns: 32, remesh polls: 32 |
| In-flight caps | Gen tasks: 256, remesh tasks: 64, save tasks: 32 |
| Batch size | 4 chunks per async task (`GEN_BATCH_SIZE`) |

### Ticket System and Level Propagation

Already implemented (`ticket.rs`, `propagator.rs`):
- `ChunkTicket` with `TicketType` (Player/Npc/MapTransition), `radius`, `map_entity`
- `TicketLevelPropagator`: incremental bucket-queue BFS, 2D Chebyshev, amortized at 64 steps/frame
- `LevelDiff` classifies columns as loaded/changed/unloaded relative to `LOAD_LEVEL_THRESHOLD = 20`
- `LoadState` enum: `EntityTicking(0)`, `BlockTicking(1)`, `Border(2)`, `Inaccessible(3+)`
- Column-to-chunks expansion via `column_to_chunks(col, y_min, y_max)` with ±8 Y range

Player ticket radius is 200 (production), meaning levels 0–200 are propagated. Levels 3–20 are "Inaccessible" but still loaded. This is the natural range for multi-stage generation gating.

---

## Minecraft's Multi-Stage Pipeline

### Stages and Neighbor Requirements

From `doc/research/2026-03-20-minecraft-chunk-ticket-system.md:500-521`:

| # | Status | Description | taskMargin |
|---|---|---|---|
| 0 | EMPTY | Initial allocation | -1 |
| 1 | STRUCTURE_STARTS | Place structure start points | 0 |
| 2 | STRUCTURE_REFERENCES | Link neighboring chunks to structures | 8 |
| 3 | BIOMES | Determine biome data | 0 |
| 4 | NOISE | Base terrain shape | 0 |
| 5 | SURFACE | Biome-dependent surface blocks | 0 |
| 6 | CARVERS | Cave carving | 0 |
| 7 | FEATURES | Feature placement, structures | 1 |
| 8 | INITIALIZE_LIGHT | Init lighting engine | 0 |
| 9 | LIGHT | Calculate light levels | 1 |
| 10 | SPAWN | Mob spawning prep | 0 |
| 11 | FULL | Promotion to LevelChunk | 0 |

`taskMargin` = how many chunks outward must be at the previous status. FEATURES has margin 1 because features can write blocks in a 3×3 chunk area.

### Level-to-Status Mapping

`ChunkStatus.byDistanceFromFull(level - 33)`:
- Level 33 = FULL, Level 34 = LIGHT, Level 35 = FEATURES, ..., Level 44 = EMPTY

The ticket level determines maximum achievable status. To generate one FULL chunk, surrounding rings must be at progressively earlier stages — up to 11 chunks out for the outermost EMPTY stage.

### Key Design Principle

Chunks at the "inaccessible" level range serve as **neighbor data providers**. They're partially generated to the earliest stages, providing terrain/biome data that closer chunks need for their feature placement stage. They never reach FULL themselves — they exist solely to satisfy neighbor requirements of chunks closer to the ticket source.

---

## Proposed Pipeline for This Project

### Stage Design

The existing research (`doc/research/2026-03-20-minecraft-chunk-ticket-system.md:747-798`) proposes:

| # | Status | Description | Neighbor Requirement |
|---|---|---|---|
| 0 | Empty | Initial allocation | None |
| 1 | Terrain | Base terrain shape (noise, biomes) | None |
| 2 | Features | Trees, ores, structures | 1-ring at Terrain |
| 3 | Light | Lighting calculation | 1-ring at Features |
| 4 | Mesh | Greedy meshing | 1-ring at Light (for padding) |
| 5 | Full | Promoted to gameplay-ready | None |

This is simpler than Minecraft's 12 stages because:
- No structures spanning 8+ chunks (no STRUCTURE_REFERENCES with margin 8)
- No cave carving stage
- Terrain and surface combined into one Terrain stage

#### Future Stage Expansion

The pipeline is designed to accommodate new stages by inserting them into the `ChunkStatus` enum and adding corresponding trait methods to `VoxelGenerator`. Because `ChunkStatus` uses explicit integer discriminants and the scheduler checks neighbor readiness generically, new stages slot in without restructuring the pipeline.

**Cave carving**: Insert a `Carvers` stage between Terrain and Features (Terrain → Carvers → Features). Carvers need 0 neighbor margin (caves are carved per-chunk using 3D noise, same as Minecraft's `ChunkStatus::CARVERS` with `taskMargin = 0`). The trait gains `fn carve(&self, pos: IVec3, terrain: &mut [WorldVoxel])`. Feature placement then sees carved terrain rather than raw heightmap.

**Large structures** (dungeons, villages, boss arenas): Split into two stages mirroring Minecraft's pattern:
- `StructureStarts` (after Terrain): Each chunk deterministically checks if it's a structure origin using seeded RNG. Stores a `StructureStart` record (type, bounding box, rotation) but does not place blocks. Margin 0 — self-contained per chunk.
- `StructureReferences` (after StructureStarts): Each chunk queries its neighbors within the structure search radius (Minecraft uses 8) to find `StructureStart`s whose bounding boxes overlap this chunk. Stores back-references. Margin = search radius.
- Features stage then reads `StructureReferences` to actually place structure blocks, ensuring cross-chunk structures are assembled correctly.

**Biome blending**: Currently biomes have hard boundaries. A `BiomeBlend` stage between Terrain and Features could smooth material transitions using neighbor biome data (margin 1). Alternatively, blending can be folded into the existing Terrain stage by expanding the noise cache to sample neighbors.

Each addition only requires: (1) new `ChunkStatus` variant, (2) new trait method, (3) neighbor margin declaration. The scheduler, propagator, and persistence layer handle the rest generically.

### ChunkStatus Enum

New type needed on `ChunkData`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ChunkStatus {
    Empty = 0,
    Terrain = 1,
    Features = 2,
    Light = 3,
    Mesh = 4,
    Full = 5,
}
```

The `Ord` derivation means `Terrain < Features < ... < Full`, which enables neighbor checks like `neighbor.status >= ChunkStatus::Terrain`.

### ChunkData Extension

```rust
pub struct ChunkData {
    pub voxels: PalettedChunk,
    pub fill_type: FillType,
    pub hash: u64,
    pub status: ChunkStatus,  // NEW
}
```

Partially-generated chunks (e.g., at Terrain stage) are stored in the octree with `status: ChunkStatus::Terrain`. When the chunk advances, its data is read, passed to the next stage, and the result overwrites it with an updated status.

### ChunkGenResult Extension

```rust
pub struct ChunkGenResult {
    pub position: IVec3,
    pub mesh: Option<Mesh>,       // only Some if Mesh stage completed
    pub chunk_data: ChunkData,    // includes updated status
    pub from_disk: bool,
    pub completed_stage: ChunkStatus,
}
```

### VoxelGenerator Becomes a Trait

Current: `VoxelGenerator(Arc<dyn Fn(IVec3) -> Vec<WorldVoxel> + Send + Sync>)` — single closure.

Proposed (from research doc, line 770-776):

```rust
pub trait VoxelGenerator: Send + Sync {
    /// Stage 1: Base terrain shape. Receives only this chunk's position.
    fn generate_terrain(&self, pos: IVec3) -> Vec<WorldVoxel>;

    /// Stage 2: Feature placement. Receives this chunk + 1-ring neighbor terrain data.
    fn generate_features(&self, pos: IVec3, neighbors: &NeighborAccess) -> Vec<WorldVoxel>;
}
```

**`NeighborAccess`** provides read-only access to surrounding chunks' voxel data. This is the critical new capability: features like trees can check and modify across chunk boundaries.

#### NeighborAccess Structure and Construction

`NeighborAccess` wraps a fixed-size array of palette-compressed neighbor chunks, indexed by relative offset. It is built on the main thread before spawning the async task, then moved into the task as owned data.

```rust
/// Read-only access to neighbor chunk data for cross-boundary generation.
///
/// Covers the 26 neighbors (3×3×3 cube minus center) surrounding the
/// target chunk. Neighbors are palette-compressed to minimize memory.
/// Voxels are decompressed on demand via `get()`.
pub struct NeighborAccess {
    /// Neighbor data indexed by offset: (dx+1)*9 + (dy+1)*3 + (dz+1)
    /// where dx,dy,dz ∈ {-1, 0, 1}. Center slot (index 13) is unused.
    neighbors: [Option<PalettedChunk>; 27],
}

impl NeighborAccess {
    /// Sample a voxel at a world-relative position that may fall in any
    /// neighbor chunk. Returns `WorldVoxel::Unset` if the neighbor is
    /// not available (not yet generated to required stage).
    pub fn get(&self, local_pos: IVec3, offset: IVec3) -> WorldVoxel {
        // Compute which neighbor chunk the position falls in
        // Decompress from PalettedChunk on demand via palette.get(index)
    }

    /// Iterate all voxels in a neighbor at a given offset.
    /// Returns None if that neighbor is not available.
    pub fn neighbor_voxels(&self, offset: IVec3) -> Option<&PalettedChunk> {
        let idx = ((offset.x + 1) * 9 + (offset.y + 1) * 3 + (offset.z + 1)) as usize;
        self.neighbors[idx].as_ref()
    }
}
```

**Construction** (main thread, inside `drain_gen_queue` before spawning the async task):

```rust
fn build_neighbor_access(
    instance: &VoxelMapInstance,
    center_pos: IVec3,
) -> NeighborAccess {
    let mut neighbors = [const { None }; 27];
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                if dx == 0 && dy == 0 && dz == 0 { continue; }
                let neighbor_pos = center_pos + IVec3::new(dx, dy, dz);
                if let Some(chunk_data) = instance.get_chunk_data(neighbor_pos) {
                    // Clone the PalettedChunk (cheap for SingleValue, shared Arc for Indirect)
                    let idx = ((dx + 1) * 9 + (dy + 1) * 3 + (dz + 1)) as usize;
                    neighbors[idx] = Some(chunk_data.voxels.clone());
                }
            }
        }
    }
    NeighborAccess { neighbors }
}
```

**Memory cost**: Each `PalettedChunk::SingleValue` is 2 bytes. `PalettedChunk::Indirect` clones the palette `Vec` and bit-packed `Vec<u64>` — typically 200–800 bytes for a compressed chunk. Worst case 27 fully-mixed chunks ≈ 20KB per feature task, far below the 300KB of fully expanded arrays. Decompression via `PalettedChunk::get(index)` is O(1) bit extraction.

**Lifetime**: `NeighborAccess` is owned by the async task. It is built from `&VoxelMapInstance` on the main thread, moved into the `spawn(async move { ... })` block, and dropped when the task completes. No shared references or locks.

### Level-to-Maximum-Status Mapping

The existing `LOAD_LEVEL_THRESHOLD = 20` means columns at levels 0–20 are loaded. Within that range, levels can gate maximum achievable status:

| Effective Level | Maximum Status | Purpose |
|---|---|---|
| 0–2 | Full | Gameplay-ready, EntityTicking/BlockTicking/Border |
| 3 | Mesh | Has mesh, visible, but not promoted to gameplay |
| 4 | Light | Lighting computed |
| 5 | Features | Features placed |
| 6–20 | Terrain | Terrain only, serves as neighbor data |
| 21+ | Empty/Unloaded | Not tracked |

Formula: `max_status = ChunkStatus::from_level_distance(level)` where `level_distance = effective_level - Border_level(2)`.

This creates concentric rings:
- **Ring 0–2** (EntityTicking through Border): fully generated, meshed, gameplay-ready
- **Ring 3** (just beyond Border): meshed but no simulation
- **Ring 4**: lit
- **Ring 5**: features placed
- **Ring 6–20**: terrain only — exists solely to provide neighbor data for feature/light/mesh stages of closer chunks

### Scheduler Changes

The current `drain_gen_queue` spawns generation tasks for chunks that need data. With multi-stage generation, it must also check:

1. **Current status**: What stage is this chunk at? (Read from octree `ChunkData.status`)
2. **Target status**: What's the maximum status allowed by the chunk's effective level?
3. **Neighbor readiness**: Have all required neighbors reached the prerequisite stage?

If neighbors aren't ready, the chunk is **skipped** (not blocked) and retried next frame. This is how Minecraft handles it — the scheduler checks and moves on.

```
for each chunk_work in GenQueue:
    current_status = octree.get(pos).map(|c| c.status).unwrap_or(Empty)
    target_status = max_status_for_level(effective_level)
    next_stage = current_status.next()
    if next_stage > target_status: skip (level doesn't allow advancement)
    if !neighbors_ready(pos, next_stage): skip (retry next frame)
    spawn async task for next_stage
```

### Async Task Changes

Each async task advances a chunk by one stage:

**Terrain stage** (self-contained, no neighbors):
```
fn terrain_task(pos, generator) -> ChunkGenResult:
    voxels = generator.generate_terrain(pos)
    chunk_data = ChunkData::from_voxels(voxels, ChunkStatus::Terrain)
    // No mesh yet
    ChunkGenResult { pos, mesh: None, chunk_data, completed_stage: Terrain }
```

**Features stage** (needs 1-ring neighbor terrain data):
```
fn features_task(pos, generator, self_voxels, neighbor_voxels) -> ChunkGenResult:
    neighbors = NeighborAccess::new(neighbor_voxels)
    voxels = generator.generate_features(pos, &neighbors)
    chunk_data = ChunkData::from_voxels(voxels, ChunkStatus::Features)
    ChunkGenResult { pos, mesh: None, chunk_data, completed_stage: Features }
```

**Mesh stage** (needs 1-ring neighbor feature data for padding):
```
fn mesh_task(pos, padded_voxels) -> ChunkGenResult:
    mesh = mesh_chunk_greedy(&padded_voxels)
    chunk_data = /* voxels unchanged, status updated */
    ChunkGenResult { pos, mesh, chunk_data, completed_stage: Mesh }
```

The async task for a stage that requires neighbors must receive the neighbor data **before** being spawned (read from octree on the main thread, cloned into the task). The task itself runs off-thread with no ECS access.

### Neighbor Data Passing

Currently, padding is baked into the generator output (18³ includes 1-voxel padding). With multi-stage generation, padding comes from actual neighbor chunk data:

**Terrain stage**: Generator still produces 18³ with self-generated padding (same as today). No neighbor access needed.

**Features stage**: Main thread reads 1-ring neighbors from octree, passes their `PalettedChunk` data (or expanded voxel arrays) into the async task. The feature generator can read/write across boundaries.

**Mesh stage**: Main thread assembles the padded 18³ array from the center chunk + 1-voxel borders of the 6 face-adjacent neighbors (already the pattern used by `update_neighbor_padding` in `instance.rs:156-184`). Passes padded array into mesh task.

### Impact on Existing Systems

**`handle_completed_chunk`** (`lifecycle.rs:768-812`): Currently inserts ChunkData into octree and spawns mesh entity. Must be updated to:
- Insert ChunkData at whatever stage completed
- Only spawn mesh entity when Mesh stage completes
- Re-enqueue chunk for next stage if target status not yet reached

**`despawn_out_of_range_chunks`**: No change — still removes data and entities for unloaded columns.

**`GenQueue`**: Must track which stage each `ChunkWork` entry targets. Priority sorting should also consider stage (earlier stages first, to unblock dependent chunks).

**Persistence**: Chunks saved to disk should include their `ChunkStatus`. On reload, a chunk at Terrain status can resume from Features stage. The `generation_version` field on `VoxelMapConfig` already exists for save compatibility.

---

## Interaction with Feature Placement

### World Object Placement Research

Per `doc/research/2026-03-22-world-object-placement-and-per-chunk-entity-persistence.md`:

- Features are data-driven via `PlacementRules` component on map entity
- Poisson disk sampling per chunk, deterministically seeded
- Height sampled from just-generated terrain voxels (highest non-Air y)
- Biome/slope/density filtering
- "Generate once, save forever" — placement runs exactly once per chunk on first generation

The multi-stage pipeline provides the **natural hook** for feature placement: it runs during the Features stage, after Terrain is complete for this chunk and its 1-ring neighbors.

### Cross-Chunk Feature Requirements

The world object placement research concluded that **no neighbor terrain dependency is needed** for object placement in this project because:
- Objects don't modify terrain (they're entities placed on top)
- No multi-chunk structures exist
- Accept boundary artifacts for minimum-spacing constraints

However, if the project later adds trees that modify voxels (e.g., tree trunks/canopy placed as voxel data), the Features stage with its 1-ring neighbor requirement at Terrain provides exactly the mechanism needed: the center chunk's feature generator can read neighbor terrain to place trees that cross boundaries.

### Feature Stage Implementation Options

**Option A: Voxel-modifying features** (Minecraft model)
- Features modify the voxel array directly (trees, ores placed as voxels)
- Requires `generate_features(pos, neighbors) -> Vec<WorldVoxel>` on trait
- Cross-boundary writes handled by reading neighbor data and emitting "neighbor mutations" that the main thread applies
- Full Minecraft-style pipeline

**Option B: Entity-only features** (current world object research model)
- Features are entities placed on top of terrain, not voxel modifications
- `PlacementRules` runs during Features stage, returns `Vec<WorldObjectSpawn>`
- No neighbor voxel data needed (self-contained placement per chunk)
- Simpler, matches the "generate once, save forever" model

**Option C: Hybrid** (chosen)
- Trait has both `generate_features` for voxel modifications AND returns entity spawns
- Voxel modifications (caves, ore veins) happen in Features stage with neighbor access
- Entity placement (trees as .vox models, NPCs) also happens in Features stage
- `ChunkGenResult` gains `entity_spawns: Vec<WorldObjectSpawn>` field

```rust
pub struct FeatureOutput {
    pub voxels: Vec<WorldVoxel>,           // modified voxel array
    pub entity_spawns: Vec<WorldObjectSpawn>, // entities to spawn
}

pub trait VoxelGenerator: Send + Sync {
    fn generate_terrain(&self, pos: IVec3) -> Vec<WorldVoxel>;
    fn generate_features(&self, pos: IVec3, terrain: &[WorldVoxel], neighbors: &NeighborAccess) -> FeatureOutput;
}
```

---

## Existing Infrastructure That Supports Multi-Stage

| Infrastructure | Location | How It Helps |
|---|---|---|
| `TicketLevelPropagator` | `propagator.rs` | Already computes per-column levels 0–200; levels 3–20 naturally gate stages |
| `GenQueue` (BinaryHeap) | `lifecycle.rs:184-187` | Priority ordering by level/distance; stage can be added to priority |
| `ChunkWorkTracker` | `lifecycle.rs:173-177` | Prevents overlapping gen/remesh; extend to track per-stage work |
| `ChunkWorkBudget` | `lifecycle.rs:27-64` | 4ms time budget; multi-stage work fits naturally |
| Batched async tasks | `generation.rs:33-79` | Batch structure adapts to per-stage tasks |
| `VoxelMapInstance.tree` | `instance.rs:28-38` | Octree already stores `ChunkData`; partially-generated chunks stored there |
| `update_neighbor_padding` | `instance.rs:156-184` | Already handles cross-chunk boundary data; pattern extends to feature stage |
| `column_to_chunks` | `ticket.rs:146-148` | Column-to-3D expansion for staged generation |
| `LoadState` enum | `ticket.rs:93-125` | Already has Inaccessible(3+); maps to partially-generated stages |
| Tracy instrumentation | Throughout | `StopReason` pattern, plot macros; extends to per-stage metrics |

---

## Code References

- `crates/voxel_map_engine/src/config.rs:12-13` — `VoxelGenerator` closure type (to become trait)
- `crates/voxel_map_engine/src/generation.rs:16-22` — `ChunkGenResult` (needs `completed_stage`)
- `crates/voxel_map_engine/src/generation.rs:33-102` — `spawn_chunk_gen_batch` and `generate_chunk` (single-pass)
- `crates/voxel_map_engine/src/types.rs:36-63` — `ChunkData` (needs `status: ChunkStatus`)
- `crates/voxel_map_engine/src/lifecycle.rs:27-64` — `ChunkWorkBudget`
- `crates/voxel_map_engine/src/lifecycle.rs:80-108` — `ChunkWork` priority struct (needs stage field)
- `crates/voxel_map_engine/src/lifecycle.rs:173-177` — `ChunkWorkTracker`
- `crates/voxel_map_engine/src/lifecycle.rs:184-187` — `GenQueue`
- `crates/voxel_map_engine/src/lifecycle.rs:310-407` — `update_chunks` system (ticket collection → propagation → enqueue)
- `crates/voxel_map_engine/src/lifecycle.rs:580-609` — `enqueue_new_chunks` (needs to enqueue per-stage)
- `crates/voxel_map_engine/src/lifecycle.rs:615-694` — `drain_gen_queue` (needs neighbor readiness checks)
- `crates/voxel_map_engine/src/lifecycle.rs:697-758` — `poll_chunk_tasks` (needs per-stage result handling)
- `crates/voxel_map_engine/src/lifecycle.rs:768-812` — `handle_completed_chunk` (conditional mesh spawn)
- `crates/voxel_map_engine/src/propagator.rs:32-43` — `TicketLevelPropagator`
- `crates/voxel_map_engine/src/ticket.rs:93-135` — `LoadState`, `LOAD_LEVEL_THRESHOLD`
- `crates/voxel_map_engine/src/terrain.rs:188-229` — `generate_heightmap_chunk` (becomes `generate_terrain`)
- `crates/voxel_map_engine/src/terrain.rs:307-331` — `build_generator` (builds closure, needs to build trait impl)
- `crates/voxel_map_engine/src/instance.rs:28-38` — `VoxelMapInstance` (octree stores partial chunks)
- `crates/voxel_map_engine/src/instance.rs:156-184` — `update_neighbor_padding` (cross-chunk data pattern)

## Historical Context (from doc/)

- `doc/research/2026-03-20-minecraft-chunk-ticket-system.md` — Comprehensive Minecraft ticket system analysis including multi-stage pipeline design (lines 747-798), VoxelGenerator trait proposal, ChunkStatus, neighbor requirements
- `doc/plans/2026-03-22-chunk-pipeline-optimizations.md` — Implemented throttling/priority/backpressure; explicitly defers multi-stage to "Step 3"
- `doc/research/2026-03-22-world-object-placement-and-per-chunk-entity-persistence.md` — Feature placement via PlacementRules, Poisson disk sampling, generate-once model, per-chunk entity persistence
- `doc/research/2026-03-18-procedural-map-generation.md` — Data-driven terrain config, archetype-based component pattern, noise caching per chunk
- `doc/plans/2026-03-19-procedural-map-generation.md` — Implementation plan for data-driven terrain (implemented)
- `doc/research/2026-01-22-noise-generation-voxel-terrain.md` — Early noise generation research including procedural object placement
- `doc/plans/2026-02-28-voxel-map-engine.md` — Original voxel_map_engine plan

## Resolved Questions

1. **Light stage**: **Yes — include the Light stage in `ChunkStatus` for future support.** The enum will be Empty → Terrain → Features → Light → Mesh → Full. The Light stage implementation will be a no-op pass-through initially (just advances status without modifying voxels). This reserves the stage slot so adding real light propagation later doesn't require changing the status enum or breaking save compatibility.

2. **NeighborAccess granularity**: **Decompress on demand.** Pass `PalettedChunk` references (cloned into the async task). `NeighborAccess` provides `get(index) -> WorldVoxel` that does O(1) bit extraction from the palette. Memory cost ~20KB worst case vs ~300KB for full expansion. See the NeighborAccess section above for implementation details.

3. **Re-enqueueing partially-generated chunks**: **Next frame.** When `handle_completed_chunk` receives a partial result (e.g., Terrain stage), it inserts the `ChunkData` into the octree and returns. The next frame's `update_chunks` detects chunks that can advance further and enqueues them into `GenQueue`. This is simpler and naturally respects the time budget — no special re-enqueue path needed.

4. **Disk format versioning**: **No migration strategy needed.** The project will start with a fresh world. `ChunkData` serialization includes `status: ChunkStatus` from the start. No need to handle legacy chunks without the field.

5. **Batch composition**: **Per-chunk, per-stage — matching Minecraft.** Minecraft does not batch multiple chunks at the same stage into a single task. The scheduling unit is always `(one chunk, one stage)`. The `List<Chunk>` parameter in Minecraft's `GenerationTask` is the **neighborhood context** (center chunk + required neighbors at prerequisite stages), not a batch of unrelated chunks. Parallelism comes from running many independent `(chunk, stage)` tasks concurrently on the thread pool.

   However, the existing `GEN_BATCH_SIZE = 4` batching optimization (grouping 4 chunk positions into one async task to reduce pool overhead) can be retained for the Terrain stage, since terrain tasks are self-contained. For stages requiring neighbor data (Features, Light, Mesh), each task should process a single chunk with its neighborhood context, matching Minecraft's model. This avoids the complexity of passing different neighbor sets per chunk within a batch.
