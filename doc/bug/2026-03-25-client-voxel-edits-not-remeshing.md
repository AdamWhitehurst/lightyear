---
date: 2026-03-25T03:20:00-07:00
researcher: Claude
git_commit: 4fb0bcf
branch: master
repository: bevy-lightyear-template
topic: "Client voxel edits not visually updating until chunk reload"
tags: [bug, voxel-engine, chunk-pipeline, remesh, client]
status: complete
last_updated: 2026-03-25
last_updated_by: Claude
---

# Bug: Client voxel edits not visually updating until chunk reload

**Date**: 2026-03-25T03:20:00-07:00
**Researcher**: Claude
**Git Commit**: 4fb0bcf
**Branch**: master
**Repository**: bevy-lightyear-template

## User's Prompt

Voxel edits are not being applied/recognized by clients until the chunk is reloaded e.g. disconnecting and reconnecting, or transition from and back to the map.

## Summary

`ChunkWorkBudget` was never reset on clients, permanently starving the remesh pipeline. The budget is an `Instant`-based time window (4ms). The only reset call was inside `update_chunks`, which is gated on `ChunkGenerationEnabled` — a resource clients never insert. After ~4ms from app start, `has_time()` always returned `false`, causing `spawn_remesh_tasks` to break immediately without spawning any work.

Voxel data was correctly updated in `VoxelMapInstance` (via `handle_voxel_broadcasts` or `handle_voxel_input`), which is why edits appeared after chunk reload — the reload re-meshes from the updated data.

## Investigation

### Voxel Edit Data Flow (client)

1. **Local edit**: `handle_voxel_input` (PostUpdate) calls `voxel_world.set_voxel()` and sends `VoxelEditRequest` to server
2. **Remote edit**: `handle_voxel_broadcasts` (Update) receives `VoxelEditBroadcast`, calls `voxel_world.set_voxel()`
3. Both paths call `VoxelMapInstance::set_voxel()` (`instance.rs:131`) which inserts into `chunks_needing_remesh`

### Remesh Pipeline

4. `spawn_remesh_tasks` (`lifecycle.rs:878`) drains `chunks_needing_remesh`, spawns async `mesh_chunk_greedy()` tasks
5. `poll_remesh_tasks` (`lifecycle.rs:969`) polls completed tasks, swaps `Mesh3d` handles on chunk entities

### The Budget Gate

`spawn_remesh_tasks` checks `budget.has_time()` before spawning each task (`lifecycle.rs:914`). `ChunkWorkBudget` is created with `start = Instant::now()` and `budget = 4ms`. After 4ms, `has_time()` is permanently false.

`budget.reset()` is called only in `update_chunks` (`lifecycle.rs:346`), which is gated:
```rust
(lifecycle::update_chunks, lifecycle::poll_chunk_tasks).run_if(resource_exists::<ChunkGenerationEnabled>)
```

Clients set `config.generates_chunks = false` and never insert `ChunkGenerationEnabled`, so `update_chunks` never runs, and the budget is never reset.

### System Registration (lib.rs)

```rust
(
    lifecycle::ensure_pending_chunks,
    (lifecycle::update_chunks, lifecycle::poll_chunk_tasks).run_if(generation_enabled),
    lifecycle::despawn_out_of_range_chunks,
    lifecycle::drain_pending_saves,
    lifecycle::spawn_remesh_tasks,  // <-- uses budget, but budget never reset on client
    lifecycle::poll_remesh_tasks,
)
    .chain(),
```

## Code References

- `crates/voxel_map_engine/src/lifecycle.rs:346` — `budget.reset()` inside `update_chunks` (gated)
- `crates/voxel_map_engine/src/lifecycle.rs:914` — `budget.has_time()` check in `spawn_remesh_tasks`
- `crates/voxel_map_engine/src/lifecycle.rs:46-54` — `ChunkWorkBudget` default and `has_time()` impl
- `crates/voxel_map_engine/src/lib.rs:44` — `update_chunks` gated on `ChunkGenerationEnabled`
- `crates/client/src/map.rs:100` — client sets `generates_chunks = false`
- `crates/voxel_map_engine/src/instance.rs:149-150` — `set_voxel` inserts into `dirty_chunks` and `chunks_needing_remesh`

## Architecture Documentation

The chunk pipeline uses a shared per-frame time budget (`ChunkWorkBudget`) to limit CPU time. Generation (`update_chunks`, `poll_chunk_tasks`) and remeshing (`spawn_remesh_tasks`, `poll_remesh_tasks`) share this budget on the server. Clients skip generation entirely but still need the remesh pipeline for in-place voxel edits.

## Hypotheses

### H1: ChunkWorkBudget never reset on client (VALIDATED)

**Hypothesis:** `budget.has_time()` always returns false on client because `reset()` only runs inside the gated `update_chunks`.

**Prediction:** Adding a `warn!` before the budget break will show `has_time=false, spawned=0` every frame.

**Test:** Added `warn!` diagnostic to `spawn_remesh_tasks` budget check.

**Result:** Confirmed. Logs showed `has_time=false, spawned=0, heap_remaining=0` every frame a remesh was queued.

**Decision:** Approved

## Fixes

### Fix: Add `reset_chunk_budgets` system gated on `not(ChunkGenerationEnabled)`

Added a `reset_chunk_budgets` system that resets all `ChunkWorkBudget` components, gated on `not(resource_exists::<ChunkGenerationEnabled>)`. Placed in the system chain after the gated `update_chunks` slot and before `spawn_remesh_tasks`.

- On server: `update_chunks` resets budget after propagation (unchanged). `reset_chunk_budgets` is skipped.
- On client: `update_chunks` is skipped. `reset_chunk_budgets` resets the budget. Remesh pipeline works.

**Decision:** Approved — verified at runtime, voxel edits now remesh immediately on client.

## Solutions

Files changed:
- `crates/voxel_map_engine/src/lifecycle.rs` — added `reset_chunk_budgets` system
- `crates/voxel_map_engine/src/lib.rs` — added `lifecycle::reset_chunk_budgets.run_if(not(generation_enabled))` to system chain
