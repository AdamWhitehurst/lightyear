# Tracy Terrain Profiling Implementation Plan

## Overview

Enable `bevy/trace_tracy` profiling with custom `info_span!` instrumentation on terrain generation hot paths. Goal: identify per-tick and per-task bottlenecks in the voxel chunk pipeline.

## Current State Analysis

- Zero profiling infrastructure
- `profile.dev` already has `debug = true` (symbols available for dev-mode profiling)
- No `profile.release` defined (no debug symbols if profiling in release)
- Bevy `0.18` with `default-features = false`, no trace features
- Six chained lifecycle systems in `Update`: `ensure_pending_chunks` → `update_chunks` → `poll_chunk_tasks` → `despawn_out_of_range_chunks` → `spawn_remesh_tasks` → `poll_remesh_tasks`
- Async terrain generation and meshing on `AsyncComputeTaskPool` worker threads

### Key Discoveries:
- `ChunkData::from_voxels` runs palettization synchronously on the main thread (`lifecycle.rs:271`)
- `remove_out_of_range_chunks` does synchronous `save_chunk` disk I/O on the main thread (`lifecycle.rs:172`)
- `PalettedChunk::to_voxels` runs synchronous palette expansion on the main thread (`lifecycle.rs:353`)
- `generate_heightmap_chunk` builds noise functions + 324 fractal noise evaluations per chunk (async)
- `greedy_quads` is the dominant cost in meshing (async)
- Bevy's `trace` feature auto-instruments all ECS systems — custom spans are for sub-operation breakdown

## Desired End State

Running `cargo server --features bevy/trace_tracy` (or `cargo client --features bevy/trace_tracy`) connects to a running Tracy GUI and shows:

1. **Every ECS system** as a named span on the timeline (automatic from `bevy/trace`)
2. **Sub-operations within hot systems** as nested spans:
   - `update_chunks` → `collect_desired_positions`, `remove_out_of_range_chunks`
   - `poll_chunk_tasks` → `palettize_chunk` (per completed task)
   - `spawn_remesh_tasks` → `expand_palette` (per remesh)
3. **Async worker thread spans** for terrain gen and meshing:
   - `generate_chunk` → `terrain_gen`, `mesh_chunk`
   - `terrain_gen` → `build_height_cache`, `build_moisture_cache`, `fill_voxels`
   - `mesh_chunk` → `greedy_quads`, `assemble_vertices`

Verification: Tracy Statistics panel (View > Statistics) sorts spans by mean duration, revealing the slowest operations.

## What We're NOT Doing

- No `profile.release` changes (user wants dev-mode profiling)
- No diagnostic plugins (`LogDiagnosticsPlugin`, etc.) — separate concern
- No lightyear metrics — separate concern
- No cargo aliases for tracy — just pass `--features bevy/trace_tracy` to existing aliases via the full command
- No chrome tracing setup — Tracy is the chosen tool
- No performance optimization — this plan is instrumentation only

## Implementation Approach

Add `info_span!` calls to sub-operations within the terrain pipeline. No new dependencies needed — `bevy::log::info_span` is already available. The `bevy/trace_tracy` feature is passed at build time.

## Phase 1: Add Custom Spans to Main-Thread Systems

### Overview
Instrument sub-operations within the lifecycle systems that run on the main thread each tick. These are the spans that will appear nested under the auto-generated system spans in Tracy.

### Changes Required:

#### 1. `crates/voxel_map_engine/src/lifecycle.rs`

Add `use bevy::log::info_span;` to imports.

**`update_chunks` (line 78)** — span the two heavy sub-calls:

```rust
pub fn update_chunks(/* ... */) {
    // ... existing preamble ...
    for (map_entity, mut instance, config, generator, mut pending, map_transform) in &mut map_query
    {
        let desired = {
            let _span = info_span!("collect_desired_positions").entered();
            collect_desired_positions(map_entity, map_transform, config, &target_query)
        };

        // ... existing logging ...

        {
            let _span = info_span!("remove_out_of_range_chunks").entered();
            remove_out_of_range_chunks(&mut instance, &desired, config.save_dir.as_deref());
        }
        if config.generates_chunks {
            spawn_missing_chunks(&mut instance, &mut pending, config, generator, &desired);
        }
    }
}
```

**`handle_completed_chunk` (line 260)** — span the palettization:

```rust
fn handle_completed_chunk(/* ... */) {
    instance.loaded_chunks.insert(result.position);

    let chunk_data = {
        let _span = info_span!("palettize_chunk").entered();
        ChunkData::from_voxels(&result.voxels)
    };
    instance.insert_chunk_data(result.position, chunk_data);
    // ... rest unchanged ...
}
```

**`spawn_remesh_tasks` (line 343)** — span the palette expansion:

```rust
pub fn spawn_remesh_tasks(/* ... */) {
    let pool = AsyncComputeTaskPool::get();
    for (mut instance, mut pending) in &mut map_query {
        let positions: Vec<IVec3> = instance.chunks_needing_remesh.drain().collect();

        for chunk_pos in positions {
            let Some(chunk_data) = instance.get_chunk_data(chunk_pos) else {
                trace!("spawn_remesh_tasks: chunk {chunk_pos} no longer in octree, skipping");
                continue;
            };
            let voxels = {
                let _span = info_span!("expand_palette").entered();
                chunk_data.voxels.to_voxels()
            };
            let task = pool.spawn(async move { mesh_chunk_greedy(&voxels) });
            pending.tasks.push(RemeshTask { chunk_pos, task });
        }
    }
}
```

---

## Phase 2: Add Custom Spans to Async Tasks

### Overview
Instrument the terrain generation and meshing code that runs on `AsyncComputeTaskPool` worker threads. Tracy shows these on separate thread rows, revealing parallelism and per-chunk cost.

### Changes Required:

#### 1. `crates/voxel_map_engine/src/generation.rs`

Add `use bevy::log::info_span;` to imports.

**`generate_chunk` (line 65)** — span terrain gen vs meshing:

```rust
fn generate_chunk(position: IVec3, generator: &dyn Fn(IVec3) -> Vec<WorldVoxel>) -> ChunkGenResult {
    let voxels = {
        let _span = info_span!("terrain_gen").entered();
        generator(position)
    };
    let mesh = {
        let _span = info_span!("mesh_chunk").entered();
        mesh_chunk_greedy(&voxels)
    };
    ChunkGenResult {
        position,
        mesh,
        voxels,
        from_disk: false,
    }
}
```

**`spawn_chunk_gen_task` async block (line 38)** — span the disk-load path too:

```rust
let task = pool.spawn(async move {
    if let Some(ref dir) = save_dir {
        match crate::persistence::load_chunk(dir, position) {
            Ok(Some(chunk_data)) => {
                let voxels = {
                    let _span = info_span!("disk_load_expand").entered();
                    chunk_data.voxels.to_voxels()
                };
                let mesh = {
                    let _span = info_span!("mesh_chunk").entered();
                    mesh_chunk_greedy(&voxels)
                };
                return ChunkGenResult {
                    position,
                    mesh,
                    voxels,
                    from_disk: true,
                };
            }
            Ok(None) => {}
            Err(e) => {
                bevy::log::warn!("Failed to load chunk at {position}: {e}, regenerating");
            }
        }
    }

    generate_chunk(position, &*generator)
});
```

#### 2. `crates/voxel_map_engine/src/terrain.rs`

Add `use bevy::log::info_span;` to imports.

**`generate_heightmap_chunk` (line 187)** — span the three phases:

```rust
pub fn generate_heightmap_chunk(/* ... */) -> Vec<WorldVoxel> {
    let height_noise = build_noise_fn(&height_map.noise, seed);
    let moisture_noise = moisture_map.map(|m| build_noise_fn(&m.noise, seed));

    let height_cache = {
        let _span = info_span!("build_height_cache").entered();
        build_height_cache(chunk_pos, &*height_noise, height_map)
    };
    let moisture_cache = moisture_noise.as_ref().map(|noise| {
        let _span = info_span!("build_moisture_cache").entered();
        build_2d_cache(chunk_pos, &**noise)
    });

    let _span = info_span!("fill_voxels").entered();
    let total = PaddedChunkShape::SIZE as usize;
    let mut voxels = vec![WorldVoxel::Air; total];

    for i in 0..total {
        // ... existing loop body unchanged ...
    }

    voxels
}
```

#### 3. `crates/voxel_map_engine/src/meshing.rs`

Add `use bevy::log::info_span;` to imports.

**`mesh_chunk_greedy` (line 10)** — span greedy quads vs vertex assembly:

```rust
pub fn mesh_chunk_greedy(voxels: &[WorldVoxel]) -> Option<Mesh> {
    debug_assert_eq!(voxels.len(), PaddedChunkShape::USIZE);

    let mut buffer = GreedyQuadsBuffer::new(voxels.len());
    let faces = RIGHT_HANDED_Y_UP_CONFIG.faces;
    {
        let _span = info_span!("greedy_quads").entered();
        greedy_quads(voxels, &PaddedChunkShape {}, [0; 3], [17; 3], &faces, &mut buffer);
    }

    if buffer.quads.num_quads() == 0 {
        return None;
    }

    let _span = info_span!("assemble_vertices").entered();
    // ... rest unchanged ...
}
```

---

## Phase 3: Add `profile.release` Debug Symbols (Optional)

### Overview
When the user later wants to profile optimized builds, debug symbols are needed. This is a one-line addition.

### Changes Required:

#### 1. `Cargo.toml` (workspace root)

After the existing `[profile.dev]` block (~line 63), add:

```toml
[profile.release]
debug = true
```

### Success Criteria:

#### Automated Verification:
- [x] `cargo check-all` passes (spans are zero-cost when `bevy/trace` is not enabled)
- [ ] `cargo run -p server --features bevy/trace_tracy` compiles and runs

#### Manual Verification:
- [ ] Tracy GUI connects to the running server
- [ ] System-level spans visible: `update_chunks`, `poll_chunk_tasks`, etc.
- [ ] Custom spans visible nested under systems: `collect_desired_positions`, `palettize_chunk`, etc.
- [ ] Worker thread spans visible: `terrain_gen`, `mesh_chunk`, `greedy_quads`, `build_height_cache`
- [ ] Statistics panel (View > Statistics) correctly sorts spans by mean duration

## Tracy Workflow Reference

```bash
# Terminal 1: Start Tracy capture (headless, recommended for accuracy)
tracy-capture -o capture.tracy

# Terminal 2: Run server with tracy
cargo run -p server --features bevy/trace_tracy

# OR: Run client with tracy
cargo run -p client --features bevy/trace_tracy

# After capturing, open in Tracy GUI:
tracy capture.tracy
```

**Key Tracy views:**
- **Statistics** (View > Statistics): Sort by "Mean time" to find slowest spans
- **Find Zone** (Ctrl+F): Search for a specific span like `greedy_quads`
- **Timeline**: Horizontal bars per thread; wide bars = slow; worker threads show async task spans
- **Compare** (File > Compare): Diff two captures for before/after optimization

## Span Hierarchy (Expected)

```
Main thread:
  update_chunks (auto)
    collect_desired_positions
    remove_out_of_range_chunks
  poll_chunk_tasks (auto)
    palettize_chunk (per completed task)
  spawn_remesh_tasks (auto)
    expand_palette (per chunk)

Worker threads:
  terrain_gen
    build_height_cache
    build_moisture_cache
    fill_voxels
  mesh_chunk
    greedy_quads
    assemble_vertices
  disk_load_expand
```

## References

- Research: `doc/research/2026-03-20-performance-profiling-tools.md`
- `crates/voxel_map_engine/src/lifecycle.rs` — per-tick systems
- `crates/voxel_map_engine/src/generation.rs` — async chunk generation
- `crates/voxel_map_engine/src/terrain.rs` — noise-based procedural generation
- `crates/voxel_map_engine/src/meshing.rs` — greedy quads meshing
