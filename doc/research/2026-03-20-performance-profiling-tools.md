---
date: 2026-03-20T11:26:12-07:00
researcher: Claude
git_commit: 5c0960726a7751137bd1c3f3f7aa146cf7f67d22
branch: bevy-lightyear-template-2
repository: bevy-lightyear-template-2
topic: "Idiomatic Rust/Bevy performance profiling tools for identifying per-tick/per-frame bottlenecks"
tags: [research, profiling, performance, tracy, diagnostics, lightyear, metrics]
status: complete
last_updated: 2026-03-20
last_updated_by: Claude
last_updated_note: "Addressed follow-up annotations: #[instrument], Tracy best practices/version, trace_chrome troubleshooting, DiagnosticsStore, lightyear metrics setup, terrain profiling"
---

# Research: Performance Profiling Tools for Bevy/Lightyear

**Date**: 2026-03-20T11:26:12-07:00
**Researcher**: Claude
**Git Commit**: 5c0960726a7751137bd1c3f3f7aa146cf7f67d22
**Branch**: bevy-lightyear-template-2
**Repository**: bevy-lightyear-template-2

## Research Question

Idiomatic Rust/Bevy ways to measure server or client performance for bottlenecks to improve efficiency -- find what systems, functions, etc. are causing the most lag per-tick/frame.

## Summary

The project has **zero** profiling infrastructure today. The Bevy/Rust ecosystem offers a layered profiling stack:

1. **Tracy** (recommended primary tool) -- per-system per-frame flamegraphs via `bevy/trace_tracy`
2. **Chrome tracing** -- lightweight alternative via `bevy/trace_chrome`, viewable in Perfetto
3. **Bevy diagnostic plugins** -- FPS, entity count, CPU/memory logged to console
4. **Lightyear metrics** -- rollback counts, RTT, replication stats via `metrics` crate + dashboard
5. **Flamegraphs** -- aggregate CPU sampling via `cargo flamegraph` or `samply`
6. **Allocation profiling** -- `dhat`, Tracy memory mode, or jemalloc profiling

## Detailed Findings

### Current State: No Profiling Infrastructure

The codebase has no diagnostic plugins, no `#[instrument]` attributes, no custom timing code, no profiler integrations, and no profiling cargo features enabled. The only diagnostics-adjacent code is:

#### `#[instrument]` Attribute

`#[instrument]` is from the `tracing` crate (re-exported by Bevy as `bevy::log::tracing`). It wraps a function in a tracing span automatically:

```rust
use bevy::prelude::*;

#[tracing::instrument(skip(query))]
fn expensive_system(query: Query<&Transform>) {
    // entire function body is wrapped in a span named "expensive_system"
    for transform in &query {
        // ...
    }
}
```

Key options:
- `skip(param)` -- exclude parameters from span fields (required for non-Debug Bevy types like `Query`, `Res`)
- `name = "custom_name"` -- override the span name
- `level = "info"` -- set tracing level (must be `info` or lower for Tracy/chrome to capture)
- `fields(entity_count = query.iter().count())` -- add custom fields to the span

Bevy systems already get automatic spans when `bevy/trace` is enabled, so `#[instrument]` is most useful for non-system functions called within systems (e.g., a helper that does expensive computation).

- `bevy::diagnostic::DiagnosticsPlugin` in test scaffolding (`crates/protocol/tests/physics_isolation.rs`) -- required by `MeshPlugin`/`PhysicsPlugins`
- `PhysicsDebugPlugin` for visual collision wireframes (not performance-related)
- `git/bevy_metrics_dashboard` excluded from workspace but present on disk
- Standard `info!/warn!/error!/trace!` logging via `LogPlugin::default()`

### Tier 1: Tracy Profiler (Per-System Per-Frame Analysis)

Tracy is the Bevy-maintainer-recommended tool for identifying which systems are slow per frame/tick.

**What it provides:**
- Timeline view of every ECS system as a colored span per thread
- Mean time per call (MTPC) statistics for every system
- Parallelism visualization (which systems ran in parallel vs serialized)
- Memory allocation tracking per span (with `trace_tracy_memory`)
- Works headless (TCP-based) -- ideal for server profiling

**Setup:**
```bash
# Run app with Tracy
cargo run --release --features bevy/trace_tracy

# With memory tracking (higher overhead)
cargo run --release --features bevy/trace_tracy_memory

# Headless capture (recommended for accuracy)
./tracy-capture -o my_capture.tracy  # terminal 1
cargo run --release --features bevy/trace_tracy  # terminal 2
```

**Version matching:** Bevy 0.18 uses `tracing-tracy = "0.11.4"` and `tracy-client = "0.18.3"`, which maps to **Tracy v0.13.0** (tracy-client-sys 0.27.0). On Arch, the AUR `tracy` package is at v0.13.1 -- if using that, let Cargo resolve `tracy-client-sys 0.28.0`.

| Tracy GUI | tracy-client-sys | tracy-client | tracing-tracy |
|-----------|------------------|--------------|---------------|
| v0.13.0 | 0.27.0 | 0.18.3 | 0.11.4 |
| v0.13.1 | 0.28.0 | 0.18.4 | 0.11.4 |

**Protocol mismatch = connection failure.** Tracy GUI and client must match exactly. Pin with `tracy-client-sys = "=0.27.0"` in workspace `Cargo.toml` if needed. Verify with `cargo tree --features bevy/trace_tracy | grep tracy`.

**Download:** [Tracy releases (official)](https://github.com/wolfpld/tracy/releases), [tracy-builds (Linux/macOS)](https://github.com/tracy-builds/tracy-builds/releases), or `paru -S tracy` on Arch.

**Getting the most out of Tracy:**

1. **Statistics panel** (View > Statistics): Sort by "Mean time" to find the slowest ECS systems per frame. This is the single most useful view for identifying bottlenecks.
2. **Find Zone** (Ctrl+F): Search for a specific system by name (e.g. `update_chunks`) to see its timing distribution across frames.
3. **Timeline**: Horizontal bars per thread. Wide bars = slow. Gaps between bars = parallelism inefficiency (systems that could overlap but don't due to data dependencies).
4. **Frame marks**: Bevy automatically emits frame boundary markers when `trace_tracy` is enabled. Tracy uses these to compute per-frame statistics and show frame-over-frame histograms.
5. **GPU profiling**: With `trace_tracy`, GPU spans appear in a separate row labeled "RenderQueue" at the top of the timeline.
6. **Compare captures**: File > Compare lets you diff two `.tracy` captures to measure before/after optimization impact.
7. **Memory view** (with `trace_tracy_memory`): Shows allocations attributed to active spans. Identifies systems that allocate per-frame (a common perf antipattern).
8. **Headless server workflow**: Use `tracy-capture -o server.tracy` in one terminal, run the server in another. Analyze the `.tracy` file offline in the GUI later.
9. **Lock contention**: Tracy visualizes mutex contention -- visible as "lock" annotations on the timeline. Useful for diagnosing Bevy's multi-threaded executor stalls.

**References:** [Bevy profiling.md](https://github.com/bevyengine/bevy/blob/main/docs/profiling.md), [tracing-tracy docs](https://docs.rs/tracing-tracy/latest/tracing_tracy/), [Tracy version hell (Rust forum)](https://users.rust-lang.org/t/tracy-profiler-version-hell/94190), [rust_tracy_client version table](https://github.com/nagisa/rust_tracy_client)

**Custom spans in your code:**
```rust
use bevy::log::info_span;

fn my_system(/* ... */) {
    let _span = info_span!("my_expensive_work").entered();
    // ... work ...
}
```

#### Spans vs Log Macros

`info!`/`debug!`/`warn!` are **log events** -- they produce a single point-in-time message. Spans (`info_span!`) are **duration markers** -- they measure how long a block of code takes. Tracy and chrome tracing visualize spans as bars on a timeline; log events appear as dots.

- Use `info_span!` when you want to **measure duration** of a code block in the profiler
- Use `info!` when you want to **log a value** at a point in time
- When `bevy/trace` is enabled, every system already gets an automatic span -- you don't need to manually span the system function itself
- Add custom `info_span!` inside a system to break down sub-operations (e.g., separate spans for "generate voxels" and "mesh voxels" within a single system)
**References:** [Bevy profiling.md](https://github.com/bevyengine/bevy/blob/main/docs/profiling.md), [tracing-tracy docs](https://docs.rs/tracing-tracy/latest/tracing_tracy/)

### Tier 2: Chrome Tracing (Lowest Friction)

Produces a JSON trace file viewable in [Perfetto UI](https://ui.perfetto.dev). No external tool installation needed.

```bash
cargo run --release --features bevy/trace_chrome
# Produces trace-TIMESTAMP.json in working directory
```

Shows same per-system timeline as Tracy but as a static file. Good for quick triage -- takes 2 minutes total.

**Why output may appear empty:**

The feature chain is `trace_chrome` → `trace` + `bevy_internal/trace_chrome` + `debug`. The `trace` feature enables `bevy_ecs/trace` which adds `info_span!` around every system. Without `trace`, there are no spans for chrome to capture.

Troubleshooting checklist:

1. **Verify the feature reached Bevy:** `cargo tree -p client --features bevy/trace_chrome -e features | grep trace` -- confirm `trace_chrome, trace` both appear
2. **Check file size:** A working 5-second capture = 5-50 MB. If < 1 KB, spans were not compiled in.
3. **Exit cleanly:** The trace is flushed when `FlushGuard` drops. If the app is `kill -9`'d or panics, the file is truncated. Close the window normally.
4. **Check `RUST_LOG`:** If `RUST_LOG=warn` is set in the shell, it overrides LogPlugin's default `info` level and suppresses all `info_span!` data. Unset it or use `RUST_LOG=info`.
5. **Custom output path:** Set `TRACE_CHROME=/tmp/trace.json` to control where the file is written (default: `./trace-{timestamp}.json` in CWD).
6. **Open in [Perfetto](https://ui.perfetto.dev):** Drag the JSON file into the UI. Chrome's built-in `chrome://tracing` also works but has fewer features.

**Constraint:** Tracing level must be at least `info`. Don't set `max_level_warn` or `max_level_error` features.

### Tier 3: Bevy Built-in Diagnostic Plugins

Aggregate metrics (not per-system), logged to console or readable from `DiagnosticsStore`.

```rust
use bevy::diagnostic::{
    FrameTimeDiagnosticsPlugin, EntityCountDiagnosticsPlugin,
    LogDiagnosticsPlugin, SystemInformationDiagnosticsPlugin,
};

app.add_plugins((
    LogDiagnosticsPlugin::default(),
    FrameTimeDiagnosticsPlugin::default(),
    EntityCountDiagnosticsPlugin::default(),
    SystemInformationDiagnosticsPlugin,
));
```

**Available diagnostics:**

| Plugin | Metrics |
|--------|---------|
| `FrameTimeDiagnosticsPlugin` | FPS, frame time (ms), frame count |
| `EntityCountDiagnosticsPlugin` | Total entity count |
| `SystemInformationDiagnosticsPlugin` | Process CPU %, process memory MB, system CPU %, system memory % |
| `RenderDiagnosticsPlugin` | Mesh allocator stats, render asset counts |

**Programmatic access:**
```rust
fn my_system(diagnostics: Res<DiagnosticsStore>) {
    if let Some(fps) = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
    {
        info!("FPS: {fps:.1}");
    }
}
```

#### When to use DiagnosticsStore

These plugins provide **aggregate** metrics -- they answer "what is my FPS?" and "how many entities exist?", not "which system is slow?". Use them for:

- **Runtime HUD**: Display FPS/entity count in-game during development
- **Server health monitoring**: Log frame time on the headless server to detect when tick rate drops
- **Regression detection**: Check if a change causes FPS to drop

**Custom diagnostics:**
```rust
use bevy::diagnostic::{Diagnostics, RegisterDiagnostic};

const CHUNK_COUNT: DiagnosticPath = DiagnosticPath::const_new("game/chunk_count");

app.register_diagnostic(Diagnostic::new(CHUNK_COUNT).with_suffix(" chunks"));

fn track_chunks(mut diagnostics: Diagnostics, chunks: Query<&VoxelChunk>) {
    diagnostics.add_measurement(&CHUNK_COUNT, || chunks.iter().count() as f64);
}
```

Custom diagnostics integrate with `LogDiagnosticsPlugin` (logged to console) and community HUD plugins (`iyes_perf_ui`, `bevy_screen_diagnostics`).

**Limitation**: `DiagnosticsStore` tracks smoothed averages over a rolling window. It does not provide per-frame timing breakdown or per-system attribution. For that, use Tracy or chrome tracing.

**References:** [bevy::diagnostic docs](https://docs.rs/bevy/latest/bevy/diagnostic/index.html)

### Tier 4: Lightyear Metrics

Lightyear has its own metrics system via the `metrics` crate.

**Feature flags (on lightyear dependency):**
- `"metrics"` -- Enables `MetricsPlugin`, counters/gauges/histograms for rollbacks, replication, messages, latency
- `"trace"` -- Enables tracing spans around send/receive/replication operations

**Key resources:**

| Type | Purpose |
|------|---------|
| `PredictionMetrics` | Client resource: `rollbacks` count, `rollback_ticks` cumulative depth |
| `LinkStats` | Per-connection: `rtt: Duration`, `jitter: Duration` |
| `MetricsRegistry` | All lightyear metrics, implements `metrics::Recorder` |

**Fork status:** The `AdamWhitehurst/lightyear` fork fully supports `metrics`. Defined at `git/lightyear/lightyear/Cargo.toml:118-130`, propagates into 7 sub-crates (transport, replication, prediction, inputs, messages, utils, udp).

**Note:** `bevy_metrics_dashboard` is listed in workspace `Cargo.toml` exclude but **does not exist on disk** -- never cloned. Not needed because lightyear ships its own UI.

#### Built-in Debug Overlay (`lightyear_ui`)

Lightyear includes a native `bevy_ui` debug panel via the compound `debug` feature (`git/lightyear/lightyear/Cargo.toml:131`), which enables both `metrics` and `lightyear_ui`:

```toml
# In your crate's lightyear dependency:
lightyear = { workspace = true, features = ["debug", ...] }
```

This activates `DebugUIPlugin`, which provides:

- **Profiler (ms)**: Replication receive/apply/buffer/send time, Transport recv/send time
- **Prediction**: Rollback count and tick depth
- **Bandwidth**: Total send/recv bytes and packets_lost, per-channel breakdown (Replication, Inputs, Sync)
- Rolling-window averages (50-frame window) with `"{latest:.3} (avg {avg:.3})"` format
- Collapsible sections, send/recv visibility filtering
- Positioned at top-right, 300px wide, semi-transparent background

**Setup:**
```rust
// Just add the "debug" feature to lightyear in Cargo.toml
// The DebugUIPlugin auto-adds MetricsPlugin if not present
app.add_plugins(lightyear::prelude::server::ServerPlugins { .. });
// Debug panel appears automatically
```

**What it's useful for:**
- **Identifying network bottlenecks**: If replication send time is high, you're serializing too much data per tick
- **Tracking rollback cost**: High rollback count = prediction diverging from server, high rollback ticks = deep re-simulation
- **Bandwidth monitoring**: See per-channel bytes to understand where network overhead comes from
- **RTT/jitter**: `LinkStats` resource gives `rtt: Duration` and `jitter: Duration` per connection

**Programmatic access (without the UI):**
```rust
// Just enable "metrics" feature (not "debug") for headless server
lightyear = { workspace = true, features = ["metrics", ...] }

fn log_rollbacks(metrics: Res<MetricsRegistry>) {
    if let Some(count) = metrics.get_counter_value(&"rollback_count".into()) {
        info!("Rollbacks this frame: {count}");
    }
}
```

**Prometheus export:** `MetricsRegistry` implements `metrics::Recorder`, compatible with `metrics-exporter-prometheus` for production monitoring.

**References:** [Lightyear docs.rs](https://docs.rs/lightyear/latest/lightyear/), `git/lightyear/lightyear_metrics/src/registry.rs`, `git/lightyear/lightyear_ui/src/debug.rs`

### Tier 5: Sampling Profilers (Aggregate, Not Per-Frame)

#### cargo flamegraph

```bash
cargo install flamegraph

# Cargo.toml:
# [profile.release]
# debug = true

RUSTFLAGS='-C force-frame-pointers=y' cargo flamegraph --bin server --release
```

Produces `flamegraph.svg` -- wide boxes = hot functions. No per-frame breakdown; aggregate over entire run.

#### samply

```bash
cargo install --locked samply
samply record target/release/server
```

Opens Firefox Profiler in browser with interactive call tree, timeline, and source view. Better UI than raw flamegraph.

**References:** [flamegraph-rs](https://github.com/flamegraph-rs/flamegraph), [samply](https://github.com/mstange/samply)

### Tier 6: Allocation Profiling

#### dhat-rs
```toml
[dependencies]
dhat = { version = "0.3", optional = true }
[features]
dhat-heap = ["dhat"]
[profile.release]
debug = 1
```
```rust
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();
    // ...
}
```
Run: `cargo run --release --features dhat-heap`. View `dhat-heap.json` at [dh_view](https://nnethercote.github.io/dh_view/dh_view.html).

#### Tracy memory mode
`--features bevy/trace_tracy_memory` -- attributes allocations to active spans. Best for per-frame allocation tracking.

#### jemalloc profiling
```toml
[dependencies]
tikv-jemallocator = { version = "0.6", features = ["profiling"] }
```
Low-overhead continuous profiling, dumps pprof-compatible profiles. Best for long-running servers.

### Community Bevy Plugins

| Plugin | Description | Bevy 0.18 Support |
|--------|-------------|------------------|
| [`iyes_perf_ui`](https://github.com/IyesGames/iyes_perf_ui) | Native Bevy UI overlay: FPS, frame time, entity count, CPU/RAM, color-coded thresholds | Check latest version |
| [`bevy_perf_hud`](https://github.com/ZoOLForge/bevy_perf_hud) | Multi-curve graphs with resource bars, extensible via `PerfMetricProvider` trait | 0.16 |
| [`bevy_screen_diagnostics`](https://github.com/laundmo/bevy_screen_diagnostics) | Simple on-screen diagnostic display | Check latest version |
| [`bevy_mod_debugdump`](https://lib.rs/crates/bevy_mod_debugdump) | Schedule graph visualization as DOT/SVG | 0.15 = Bevy 0.18 |

### Schedule Visualization

No first-party schedule visualizer exists ([issue #10981](https://github.com/bevyengine/bevy/issues/10981)). Use `bevy_mod_debugdump`:

```bash
cargo run -- dump-schedule Update schedule.dot
dot -Tsvg schedule.dot > schedule.svg
```

### Cargo.toml Setup for Profiling

```toml
[profile.release]
debug = true          # Debug symbols for profilers

# Optional: dedicated profiling profile
[profile.profiling]
inherits = "release"
debug = true
strip = false
```

## Server vs Client Profiling Matrix

| Tool | Headless Server | Client | Per-Tick | Setup Effort |
|------|----------------|--------|----------|-------------|
| Tracy (`trace_tracy`) | Yes | Yes | Yes | Medium (install Tracy) |
| Chrome tracing (`trace_chrome`) | Yes | Yes | Yes | Low (just a feature flag) |
| `LogDiagnosticsPlugin` | Yes | Yes | No (aggregate) | Trivial |
| Lightyear metrics dashboard | Yes | Yes | Yes | Low (feature flag + plugin) |
| cargo flamegraph | Yes | Yes | No (aggregate) | Low |
| samply | Yes | Yes | No (aggregate) | Low |
| dhat | Yes | Yes | No (aggregate) | Medium |
| Tracy memory | Yes | Yes | Yes | Medium |
| puffin + puffin_http | Yes | Yes | Yes | Medium |

## Recommended Workflow

1. **Quick triage**: `cargo run --release --features bevy/trace_chrome` -- run a few seconds, open JSON in [ui.perfetto.dev](https://ui.perfetto.dev)
2. **Deep analysis**: Tracy with `trace_tracy` -- sort systems by mean duration, inspect parallelism gaps
3. **Allocation hotspots**: Tracy with `trace_tracy_memory` -- find systems that allocate per tick
4. **Uninstrumented hotspots**: `cargo flamegraph` when the hot code is inside a dependency without tracing spans
5. **Network/replication**: Lightyear `metrics` feature + `bevy_metrics_dashboard` for rollback counts, RTT, bandwidth
6. **Live dev monitoring**: `iyes_perf_ui` or `bevy_perf_hud` gated behind a cargo feature

## Code References

- `crates/protocol/tests/physics_isolation.rs:15` -- Only existing `DiagnosticsPlugin` usage (test scaffolding)
- `crates/server/src/main.rs:18` -- `LogPlugin::default()` (no custom log config)
- `crates/client/src/main.rs:33` -- `DefaultPlugins` (includes default `LogPlugin`)
- `Cargo.toml:34` -- `bevy = { version = "0.18", default-features = false }` (no trace features enabled)
- `git/lightyear/lightyear/Cargo.toml:118-130` -- Lightyear `metrics` feature definition
- `git/lightyear/lightyear/Cargo.toml:131` -- Lightyear `debug` compound feature (metrics + lightyear_ui)
- `git/lightyear/lightyear_metrics/src/registry.rs` -- `MetricsRegistry` implementation
- `git/lightyear/lightyear_ui/src/debug.rs` -- `DebugUIPlugin` with native bevy_ui overlay
- `git/bevy/crates/bevy_log/Cargo.toml:27` -- `tracing-chrome = "0.7.0"`
- `git/bevy/crates/bevy_log/Cargo.toml:34` -- `tracing-tracy = "0.11.4"`
- `crates/voxel_map_engine/src/lifecycle.rs` -- Per-tick chunk lifecycle systems
- `crates/voxel_map_engine/src/terrain.rs` -- Procedural noise terrain generation
- `crates/voxel_map_engine/src/meshing.rs` -- Greedy quads meshing

## External References

- [Bevy profiling.md (canonical)](https://github.com/bevyengine/bevy/blob/main/docs/profiling.md)
- [Bevy Discussion #6715: Profiling App Logic](https://github.com/bevyengine/bevy/discussions/6715)
- [Bevy Cheat Book: Performance](https://bevy-cheatbook.github.io/setup/perf.html)
- [Bevy Cheat Book: Show Framerate](https://bevy-cheatbook.github.io/cookbook/print-framerate.html)
- [The Rust Performance Book](https://nnethercote.github.io/perf-book/profiling.html)
- [Lightyear Book: System Order](https://cbournhonesque.github.io/lightyear/book/concepts/bevy_integration/system_order.html)
- [Lightyear Releases (v0.19.0 metrics)](https://github.com/cBournhonesque/lightyear/releases)
- [bevy_metrics_dashboard](https://github.com/bonsairobo/bevy_metrics_dashboard)
- [Profiling Rust with Tracy (blog)](https://mikeder.net/blog/profiling-rust-with-tracy/)
- [How to Profile Rust with perf/flamegraph/samply](https://oneuptime.com/blog/post/2026-01-07-rust-profiling-perf-flamegraph/view)

## Open Questions

1. ~~Does the `AdamWhitehurst/lightyear` fork support the `metrics` feature flag?~~ **Resolved:** Yes, fully supported at `git/lightyear/lightyear/Cargo.toml:118-130`.
2. ~~Is the `git/bevy_metrics_dashboard` checkout compatible with the current Bevy 0.18 version?~~ **Resolved:** `bevy_metrics_dashboard` does not exist on disk. Not needed -- lightyear ships `lightyear_ui` with the `debug` feature.
3. ~~What Tracy version is needed for the current `tracing-tracy` version in Bevy 0.18's dependency tree?~~ **Resolved:** Bevy 0.18 uses `tracing-tracy 0.11.4` / `tracy-client 0.18.3` → **Tracy v0.13.0** (or v0.13.1 with `tracy-client-sys 0.28.0`). See Tier 1 section for version table.

## Profiling Terrain Generation

The terrain generation pipeline has several hot paths that can be profiled at different granularities.

### Hot Path

`update_chunks` → `spawn_missing_chunks` → `spawn_chunk_gen_task` → (async) `generate_chunk` → `VoxelGenerator` closure → `generate_heightmap_chunk` → (back in `poll_chunk_tasks`) → `mesh_chunk_greedy` + `attach_chunk_colliders`

### Per-Tick Systems to Profile

These systems run every frame in `Update` (chained), registered by `VoxelPlugin` in `crates/voxel_map_engine/src/lib.rs`:

| System | File | What it does |
|---|---|---|
| `ensure_pending_chunks` | `lifecycle.rs` | Inserts `PendingChunks` on maps missing it |
| `update_chunks` | `lifecycle.rs` | Determines desired chunks, spawns gen tasks |
| `poll_chunk_tasks` | `lifecycle.rs` | Polls async tasks, meshes results, spawns entities |
| `despawn_out_of_range_chunks` | `lifecycle.rs` | Removes far-away chunk entities |
| `spawn_remesh_tasks` | `lifecycle.rs` | Kicks off async remesh for edited chunks |
| `poll_remesh_tasks` | `lifecycle.rs` | Applies completed remesh results |

Server-side additional systems (`ServerMapPlugin` in `server/src/map.rs`):

| System | What it does |
|---|---|
| `apply_terrain_defs` | Copies terrain def components onto map entities |
| `build_terrain_generators` | Builds `VoxelGenerator` closures from terrain components |
| `save_dirty_chunks_debounced` | Periodically flushes dirty chunks to disk |
| `handle_chunk_requests` | Responds to client chunk data requests |

### Profiling Steps

1. **Quick triage with chrome tracing:**
   ```bash
   cargo run -p server --release --features bevy/trace_chrome
   # Run for 10-20 seconds during chunk generation, then close cleanly
   # Open trace-*.json in ui.perfetto.dev
   ```
   Look for: `update_chunks`, `poll_chunk_tasks` duration per frame. If `poll_chunk_tasks` dominates, the bottleneck is meshing or collider generation. If `update_chunks` dominates, too many chunks are being spawned per frame.

2. **Deep analysis with Tracy:**
   ```bash
   cargo run -p server --release --features bevy/trace_tracy
   ```
   Sort Statistics by mean time. The voxel systems will appear as `ensure_pending_chunks`, `update_chunks`, etc.

3. **Add custom spans for sub-operations** (if system-level granularity is insufficient):
   ```rust
   // In lifecycle.rs poll_chunk_tasks:
   let _span = info_span!("mesh_chunk").entered();
   let mesh = mesh_chunk_greedy(&chunk_data);
   drop(_span);

   let _span = info_span!("attach_colliders").entered();
   attach_chunk_colliders(&mut commands, entity, &mesh);
   ```

4. **Profile the async task itself** (noise generation):
   The `VoxelGenerator` closure runs on Bevy's `AsyncComputeTaskPool`. Tracy will show these on worker threads. If `generate_heightmap_chunk` in `terrain.rs` is the bottleneck, the noise functions (`build_noise_fn`, biome selection) need optimization.

5. **Measure allocation overhead:**
   ```bash
   cargo run -p server --release --features bevy/trace_tracy_memory
   ```
   Each `generate_heightmap_chunk` call allocates a `Vec<WorldVoxel>` (32^3 = 32768 voxels). Each `mesh_chunk_greedy` call allocates mesh vertex/index buffers. Tracy memory mode will show if these dominate.

6. **Flamegraph for uninstrumented code:**
   ```bash
   RUSTFLAGS='-C force-frame-pointers=y' cargo flamegraph -p server --release
   ```
   Useful if noise crate internals or block-mesh-rs greedy algorithm is the bottleneck -- these don't have tracing spans.

### Key Files

- `crates/voxel_map_engine/src/lifecycle.rs` -- per-tick systems
- `crates/voxel_map_engine/src/generation.rs` -- async chunk generation
- `crates/voxel_map_engine/src/terrain.rs` -- noise-based procedural generation
- `crates/voxel_map_engine/src/meshing.rs` -- greedy quads meshing
- `git/block-mesh-rs/src/greedy.rs` -- greedy quads algorithm
- `crates/protocol/src/map/colliders.rs` -- physics collider generation
