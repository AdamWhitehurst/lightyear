---
date: 2026-03-20T14:41:26-07:00
researcher: claude
git_commit: a3d0a6df805a1709361535e79a316eedcc78736d
branch: bevy-lightyear-template-2
repository: bevy-lightyear-template-2
topic: "Minecraft's chunk loading ticket system and how to implement it in voxel_map_engine"
tags: [research, minecraft, chunk-loading, ticket-system, voxel-map-engine, lifecycle]
status: complete
last_updated: 2026-03-20
last_updated_by: claude
---

# Research: Minecraft's Chunk Loading Ticket System

**Date**: 2026-03-20T14:41:26-07:00
**Researcher**: claude
**Git Commit**: a3d0a6df805a1709361535e79a316eedcc78736d
**Branch**: bevy-lightyear-template-2
**Repository**: bevy-lightyear-template-2

## Research Question

How does Minecraft's chunk loading ticket system work, and how would it map onto this project's `voxel_map_engine` crate?

## Summary

Minecraft's ticket system (Java 1.14+) is the sole mechanism that causes chunks to load. Every loaded chunk traces back to at least one *ticket* — a data object with a **type**, a **load level** (22-44), and an optional **lifetime**. The level propagates outward from the ticket source (+1 per chunk in Chebyshev distance), creating concentric rings of decreasing load state. When multiple tickets overlap, the lowest (strongest) level wins. Chunks transition through load states: entity ticking (level ≤31), block ticking (32), border (33), inaccessible (34-44), and unloaded (45+).

The current `voxel_map_engine` has a simpler model: `ChunkTarget` components with a flat `distance` field produce a binary loaded/unloaded set. There is no concept of load levels, ticket types, expiration, or propagation — all desired chunks are equally "loaded." Mapping Minecraft's system onto the engine would mean replacing the flat `HashSet<IVec3>` desired set with a per-chunk level computed from multiple ticket sources, and gating chunk processing (generation, meshing, entity ticking) by level thresholds.

---

## Minecraft's Ticket System

### Ticket Properties

A ticket has three properties:

| Property | Description |
|---|---|
| **Type** | Source category (player, portal, forced, start, etc.) |
| **Level** | Integer 22-44; lower = stronger/more processing |
| **Lifetime** | Ticks remaining before expiration; some types never expire |

### Ticket Types

| Type | Level | Lifetime | Persists | Notes |
|---|---|---|---|---|
| Start (spawn) | 22 | Never | No | World spawn chunk. Creates largest loaded area. |
| Dragon | 24 | Never | No | Chunk (0,0) in The End during dragon fight. |
| Player Spawn | 30 | 1s (refreshed) | No | During respawn. |
| Portal | 30 | 15s (300 ticks) | Yes | Entity teleport via portal. |
| Player Loading | 31 | Never | No | Square grid around player (server render distance). |
| Forced | 31 | Never | Yes | `/forceload` command. Survives restart. |
| Ender Pearl | 31 | 2s (refreshed) | No | Follows thrown pearl. |
| Post-teleport | 32-33 | 5 ticks | No | After `/tp`, spread, etc. |

### Chunk Load States

Level determines processing:

| State | Level | Processing |
|---|---|---|
| Entity Ticking | ≤31 | Full gameplay: entity AI, spawning, block ticks, redstone |
| Block Ticking | 32 | Block mechanics (redstone, scheduled ticks), entities frozen |
| Border | 33 | No mechanics; blocks accessible to neighbors, mobs count for cap |
| Inaccessible | 34-44 | World generation pipeline runs, not accessible for gameplay |
| Unloaded | 45+ | Not loaded |

### Level Propagation

From each ticket source, the level increases by 1 per chunk outward (Chebyshev/8-neighbor distance). A chunk's effective level is the **minimum** of all levels it receives from all sources.

Example — player ticket (level 31):
- Player chunk: 31 (entity ticking)
- +1 ring: 32 (block ticking)
- +2 ring: 33 (border)
- +3 ring: 34 (inaccessible, generating)

Example — spawn ticket (level 22):
- Spawn chunk: 22 → up to +9 chunks out = 31 (entity ticking)
- +10: 32 (block ticking)
- +11: 33 (border)
- ...propagates to +22 before capping at 44

### Scheduling and Throttling

- ~50 chunk operations per tick budget
- Chunks closer to players are prioritized
- Generation progresses through a multi-stage pipeline (empty → structures → biomes → noise → surface → carvers → features → light → spawn → full)
- Each stage has a `taskMargin` requiring neighbors at sufficient status

---

## Current `voxel_map_engine` Architecture

### Chunk Loading Model

The engine uses `ChunkTarget` components to drive loading:

```
ChunkTarget { map_entity: Entity, distance: u32 }
```

Each target generates a cubic volume of desired chunk positions (`-dist..=dist` on all three axes). All targets for a map are unioned into a `HashSet<IVec3>` of desired positions. Chunks in the set are loaded; chunks outside are unloaded. There is no level differentiation.

### Key Components and Flow

| Component | File | Role |
|---|---|---|
| `ChunkTarget` | `chunk.rs:5-16` | Drives loading; attached to player entities |
| `VoxelMapConfig` | `config.rs:17-29` | Per-map settings (seed, bounds, generates_chunks) |
| `VoxelGenerator` | `config.rs:12-13` | `Arc<dyn Fn(IVec3) -> Vec<WorldVoxel>>` generation function |
| `VoxelMapInstance` | `instance.rs:28-37` | Owns octree, loaded_chunks set, dirty/remesh sets |
| `PendingChunks` | `generation.rs:23-27` | In-flight async generation tasks |
| `PendingRemeshes` | `lifecycle.rs:40-43` | In-flight async remesh tasks |

### Lifecycle System Chain (lib.rs:32-40)

```
ensure_pending_chunks → update_chunks → poll_chunk_tasks
→ despawn_out_of_range_chunks → spawn_remesh_tasks → poll_remesh_tasks
```

1. **`update_chunks`**: Collects desired positions from all `ChunkTarget`s, removes out-of-range chunks, spawns generation tasks for missing chunks (server only, gated on `generates_chunks`).
2. **`poll_chunk_tasks`**: Polls async tasks, inserts `ChunkData` into octree, spawns mesh entities.
3. **`despawn_out_of_range_chunks`**: Despawns mesh entities for unloaded chunks.

### Throttling

- `MAX_TASKS_PER_FRAME = 32` — caps new generation tasks spawned per frame (`lifecycle.rs:14`).
- No prioritization; `desired.iter()` order is arbitrary (HashSet iteration).

### Server vs. Client

- **Server**: `generates_chunks = true`. `ChunkTarget` on player entities (distance=10) and dummy target (distance=1) drive generation. Also handles `ChunkRequest` messages from clients.
- **Client**: `generates_chunks = false`. `ChunkTarget` on predicted player (distance=10 for steady state, distance=4 during transitions). Sends `ChunkRequest` to server for missing chunks, receives `ChunkDataSync` responses. `update_chunks` still runs to compute desired set and evict out-of-range chunks.

### Current Distance Values

| Location | Distance | File |
|---|---|---|
| Server player spawn | 10 | `server/src/gameplay.rs:317` |
| Server dummy NPC | 1 | `server/src/gameplay.rs:82` |
| Server map transition | 10 | `server/src/map.rs:806` |
| Client auto-attach | 10 | `client/src/map.rs:130` |
| Client map transition | 4 | `client/src/map.rs:525` |

### What's Missing vs. Minecraft

| Minecraft Feature | Current Engine |
|---|---|
| Multiple ticket types with different levels | Single `ChunkTarget` with flat distance |
%% Research how ticket levels are calculated in code
| Load levels (entity ticking, block ticking, border) | Binary loaded/unloaded |
%% Research how chunk levels are calculated in code
| Level propagation (+1 per chunk) | Flat radius, all chunks equal |
| Ticket expiration/lifetime | No expiration |
| Lowest-level-wins overlap resolution | Union of all desired positions |
| Priority scheduling (closer chunks first) | Arbitrary HashSet iteration order |
| Multi-stage generation pipeline | Single-step: generate → mesh |

---

## Mapping Minecraft's System to `voxel_map_engine`

### Concept Mapping

| Minecraft Concept | Engine Equivalent |
|---|---|
| Ticket | `ChunkTarget` component (or new `ChunkTicket` struct) |
%% We should replace ChunkTarget all-together with something closer to minecrafts implementation
| Ticket type | Enum field on the ticket (Player, Forced, Portal, etc.) |
| Ticket level | Starting level integer on the ticket |
| Ticket lifetime | Optional tick counter, decremented each frame |
| Effective chunk level | Computed per-chunk via propagation, stored in a map |
| `ChunkTicketManager` | New system or resource that computes effective levels |
| Entity ticking zone | Chunks with effective level ≤ some threshold |
| Block ticking zone | Chunks with level = threshold + 1 |
| Border/generation zone | Chunks with level = threshold + 2..N |
| `loaded_chunks: HashSet<IVec3>` | `chunk_levels: HashMap<IVec3, u32>` |

### Data Structures

**ChunkTicket** (replaces or extends `ChunkTarget`):
```rust
struct ChunkTicket {
    ticket_type: TicketType,
    level: u32,            // starting level (lower = stronger)
    lifetime: Option<u32>, // ticks remaining, None = permanent
}

enum TicketType {
    Player,   // follows entity position
    Forced,   // static position, persists across restarts
    Portal,   // temporary, created on teleport
    Spawn,    // world spawn area
}
```

**Per-chunk level map** (replaces `loaded_chunks: HashSet<IVec3>`):
```rust
// In VoxelMapInstance:
chunk_levels: HashMap<IVec3, ChunkLevel>,

struct ChunkLevel {
    effective_level: u32,
    load_state: LoadState,
}

enum LoadState {
    FullTick,    // entity + block ticking
    BlockTick,   // block ticking only
    Border,      // accessible to neighbors, no ticking
    Generating,  // world gen in progress
}
```

### Level Computation

Each frame, for each map:
1. Collect all tickets (from `ChunkTicket` components and any static tickets on the map).
2. For each ticket, compute its position (from entity `GlobalTransform` or stored position).
3. BFS/flood-fill from each ticket source: assign `ticket.level + chebyshev_distance` to each chunk.
4. For each chunk, take the minimum level across all ticket sources.
5. Map effective level to `LoadState` via thresholds.
6. Diff against previous frame's levels to determine which chunks to load, upgrade, downgrade, or unload.

### Relevant Thresholds for This Project

The project doesn't need Minecraft's full 22-44 range. A simplified mapping:

| Level | State | What Happens |
|---|---|---|
| 0 | Full | Server: entity ticking, physics. Client: rendered, collidable. |
| 1 | Loaded | Data in octree, meshed, but no entity processing. |
| 2 | Border | Data in octree, not meshed. Available for neighbor padding. |
| 3 | Generating | Async generation in flight. |
| 4+ | Unloaded | Not loaded. |

### Priority Scheduling

Replace `HashSet` iteration with a priority queue sorted by effective level (lower first), then by distance to nearest player. This ensures the most gameplay-critical chunks load before distant ones.

---

## Code References

- `crates/voxel_map_engine/src/chunk.rs:5-16` — `ChunkTarget` definition
- `crates/voxel_map_engine/src/lifecycle.rs:79-118` — `update_chunks` system
- `crates/voxel_map_engine/src/lifecycle.rs:120-151` — `collect_desired_positions` (flat distance)
- `crates/voxel_map_engine/src/lifecycle.rs:164-188` — `remove_out_of_range_chunks`
- `crates/voxel_map_engine/src/lifecycle.rs:190-213` — `spawn_missing_chunks` (no priority)
- `crates/voxel_map_engine/src/lifecycle.rs:14` — `MAX_TASKS_PER_FRAME = 32`
- `crates/voxel_map_engine/src/instance.rs:28-37` — `VoxelMapInstance` fields
- `crates/voxel_map_engine/src/config.rs:12-29` — `VoxelGenerator`, `VoxelMapConfig`
- `crates/voxel_map_engine/src/generation.rs:23-27` — `PendingChunks`
- `crates/server/src/gameplay.rs:312-318` — Server player ChunkTarget insertion
- `crates/client/src/map.rs:117-132` — Client ChunkTarget auto-attach
- `crates/client/src/map.rs:135-202` — Client `request_missing_chunks`

## Historical Context (from doc/)

- `doc/research/2026-03-11-minecraft-world-sync-protocol.md` — Minecraft's chunk data transfer protocol (paletted sections, block change packets). Directly related: describes how chunks are sent to clients.
- `doc/research/2026-01-03-server-chunk-visibility-determination.md` — Early research on per-client chunk visibility. Predates current ChunkTarget system.
- `doc/research/2026-01-03-bevy-ecs-chunk-visibility-patterns.md` — ECS patterns for chunk visibility that informed current design.
- `doc/plans/2026-02-28-voxel-map-engine.md` — Original voxel_map_engine implementation plan.
- `doc/plans/2026-01-04-transform-based-chunk-visibility.md` — Plan that introduced transform-based (vs camera-based) chunk loading.

## Related Research

- `doc/research/2026-03-18-procedural-map-generation.md` — Terrain generation system
- `doc/research/2026-03-20-performance-profiling-tools.md` — Tracy profiling for terrain performance

## Sources

- [Chunk — Minecraft Wiki](https://minecraft.wiki/w/Chunk)
- [Spawn chunk — Minecraft Wiki](https://minecraft.wiki/w/Spawn_chunk)
- [/forceload — Minecraft Wiki](https://minecraft.wiki/w/Commands/forceload)
- [Chunk loading overview (empirical 1.14.4 testing) — GitHub Gist](https://gist.github.com/Drovolon/24bfaae00d57e7a8ca64b792e14fa7c6)
- [ChunkTicketManager — Fabric Yarn API](https://maven.fabricmc.net/docs/yarn-22w17a+build.3/net/minecraft/server/world/ChunkTicketManager.html)
- [Chunk Loading — Technical Minecraft Wiki](https://techmcdocs.github.io/pages/GameMechanics/ChunkLoading/)

## Open Questions

1. **How many load states does this project actually need?** Minecraft has 5+ states; the project might only need 3 (full, loaded-but-idle, border/padding).
%% Do it the way minecraft does so in the future we have support for different levels of simulation
2. **Should tickets be ECS components or a data structure on VoxelMapInstance?** Components are more ECS-idiomatic but require queries; a `Vec<Ticket>` on the map instance is simpler for propagation computation.
%% This depends on how we parallelize and defer the work. You need to research and think about how we can optimize this system with async tasks and processing caps to maintain stability even with high demand for chunk loading 
3. **How should the client interact with the ticket system?** Currently the client computes desired chunks locally and requests from server. With tickets, should the server communicate effective levels to clients, or should clients compute their own from local tickets?
%% How does minecraft do it?
4. **Is Chebyshev (8-neighbor) or Manhattan distance more appropriate?** Minecraft uses Chebyshev. The current engine uses cubic volumes (equivalent to Chebyshev). Spherical distance would be more natural for 3D but more expensive to compute.
%% Use Chebyshev
5. **Should the generation pipeline become multi-stage?** Minecraft's inaccessible levels exist because generation is multi-stage (neighbors must reach sufficient status). The current single-step generate-and-mesh approach doesn't need inaccessible levels, but multi-stage generation could improve perceived load times.
%% Yes make it multi-stage
