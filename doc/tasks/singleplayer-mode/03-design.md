# Design: Singleplayer Mode

**Research**: `doc/tasks/singleplayer-mode/02-research.md`
**Date**: 2026-03-29
**Status**: draft

## Current State

The game runs as two separate processes: a headless server (`MinimalPlugins`, `crates/server/src/main.rs:18-46`) and a rendered client (`DefaultPlugins`, `crates/client/src/main.rs`). They connect over WebTransport. The UI state machine (`ClientState::MainMenu -> Connecting -> InGame` at `crates/ui/src/state.rs:4-13`) assumes a remote server. Lightyear supports "host mode" — `ClientPlugins` + `ServerPlugins` in the same `App` with `LinkOf` instead of a transport (`git/lightyear/lightyear_connection/src/host.rs:44-158`).

## Desired End State

A "Singleplayer" button on the main menu launches an in-process server. The player enters gameplay immediately (no `Connecting` state). Server gameplay, map, and persistence systems run in the same app. Prediction/rollback are automatically bypassed via `HostClient` marker. Works on both native and web. Native persists the world to disk; web does not.

Verification: player clicks Singleplayer, spawns in the overworld, can move, use abilities, edit voxels, and (on native) quit and resume with world intact.

## Patterns to Follow

- **Lightyear host mode**: `ClientPlugins` + `ServerPlugins` in same app, `Client` entity with `LinkOf { server }`, no IO transport — `git/lightyear/examples/common/src/cli.rs:93-201`
- **`HostClient` auto-detection**: `HostPlugin` observers insert `Connected`, skip replication/prediction/rollback — `git/lightyear/lightyear_connection/src/host.rs:65-79`
- **`HostServerPlugin` fake markers**: adds `Predicted`/`Interpolated` for query compatibility — `git/lightyear/lightyear_replication/src/host.rs:37-96`
- **`SharedPlugins` double-registration guard**: checks `is_plugin_added::<CorePlugins>()` — `git/lightyear/lightyear/src/shared.rs:15,47`
- **Observer-driven connection**: `Add<Connected>` / `Add<Disconnected>` observers handle state transitions — `crates/ui/src/lib.rs:120-137`

## Patterns to AVOID

- **Crossbeam transport for singleplayer**: it's for tests with `RawServer`/`RawClient`. Host mode uses `LinkOf` and in-process `HostClient.buffer`, which is simpler and skips more overhead.
- **Running a local server process**: unnecessary complexity for singleplayer.
- **Going through `Connecting` state**: host mode connection is synchronous. The connecting UI (spinner, cancel button) is meaningless.

## Design Decisions

### 1. Entry Point: Mode in Existing Clients

**Choice**: Both native and web clients gain singleplayer support. Native client accepts `--singleplayer` CLI flag. Both clients show a "Singleplayer" button on the main menu.

**Reasoning**: Single binary per platform. Singleplayer is a runtime mode, not a separate app. The `--singleplayer` flag allows headless/automated launch for testing.

**Alternatives rejected**: Separate binary (unnecessary duplication), runtime-only without CLI flag (less testable).

### 2. Server Crate as Lib+Bin

**Choice**: Convert `crates/server/` to lib+bin. Add `lib.rs` that `pub mod`s existing modules (`gameplay`, `map`, `network`, `persistence`, `diagnostics`). `main.rs` imports from the library. Client and web crates depend on `server` as a library.

**Reasoning**: Least disruptive. Server plugins are already modular. The binary is just plugin composition. Dependency chain becomes `client -> server -> protocol`, which is acceptable.

**Alternatives rejected**: New `server-logic` crate (unnecessary indirection), moving plugins to protocol (wrong abstraction level — server logic is not shared protocol).

### 3. Network Plugin for Host Mode

**Choice**: New `SingleplayerNetworkPlugin` (~30 lines) that spawns a `Server` entity (without transport IO) and a `Client` entity with `LinkOf { server }`, then triggers `Start` + `Connect`. Replaces both `ServerNetworkPlugin` and `ClientNetworkPlugin` in singleplayer mode.

**Reasoning**: `ServerNetworkPlugin` has transport-specific logic (WebTransport/UDP IO setup) that is irrelevant in host mode. `ClientNetworkPlugin` creates `NetcodeClient` and transport IO. Neither is needed. A small dedicated plugin is cleaner than conditional branches in existing plugins.

**Alternatives rejected**: Refactoring existing network plugins to support both modes (adds complexity to working code for a mode they weren't designed for).

### 4. UI Flow

**Choice**: "Singleplayer" button on `MainMenu` transitions directly to `InGame`, bypassing `Connecting`. The `SingleplayerNetworkPlugin` startup system spawns server + client entities. The `HostPlugin` synchronously inserts `Connected` on the `Connect` trigger when `LinkOf` is present, which fires the existing `on_client_connected` observer that transitions to `InGame`.

**Reasoning**: Host mode connection is instant. The `Connecting` state with its spinner and cancel button is meaningless. Direct transition is correct UX.

### 5. Reusing Server Plugins

**Choice**: `ServerGameplayPlugin` and `ServerMapPlugin` are reused directly in singleplayer. `ServerNetworkPlugin` is replaced by `SingleplayerNetworkPlugin`. `ServerDiagnosticsPlugin` is optionally included (native only).

**Reasoning**: Server gameplay and map logic are the core simulation. They don't depend on transport or headless mode. The only server-specific assumptions are in `ServerNetworkPlugin` (transport setup) and `start_server` (transport IO spawning).

### 6. Headless Server Resources in Rendered App

**Choice**: The headless server manually registers `Mesh`, `StandardMaterial`, `Shader`, `Image` (`main.rs:29-35`) because `MinimalPlugins` doesn't include them. In host mode with `DefaultPlugins`, these are already registered. No manual registration needed — skip those lines in the singleplayer plugin composition.

### 7. Persistence: Native Yes, Web No

**Choice**: `#[cfg(not(target_family = "wasm"))]` around persistence systems in `ServerMapPlugin` and related code. On WASM, `save_dirty_chunks_debounced`, `save_world_on_shutdown`, `load_startup_entities`, and `WorldSavePath` are compiled out.

**Reasoning**: `std::fs` is unavailable on WASM. Cfg-gating is idiomatic for platform capabilities. No runtime overhead or dead code on web.

**Alternatives rejected**: Runtime flag (`PersistenceEnabled`) — adds runtime checks for a compile-time platform constraint.

### 8. `--singleplayer` CLI Flag

**Choice**: Native client accepts `--singleplayer`. When set, the app adds `ServerPlugins`, `ServerGameplayPlugin`, `ServerMapPlugin`, and `SingleplayerNetworkPlugin` instead of `ClientNetworkPlugin`. On web, there is no CLI — the mode is selected via the UI button (or could be a URL parameter if needed later).

**Reasoning**: CLI flag enables automated testing and quick launch. Web doesn't have CLI args but the UI button serves the same purpose.

## Constraints

- `SharedPlugins` can coexist: guarded by `is_plugin_added::<CorePlugins>()` (`shared.rs:15`)
- Server plugins registering duplicate asset types in a `DefaultPlugins` app will panic — must skip manual asset registration
- `ServerMapPlugin` persistence uses `std::fs` — must be cfg-gated for WASM
- `handle_connected` (`gameplay.rs:262-327`) spawns character entities — must still work in host mode (it triggers on `Add<Connected>`, which `HostPlugin` inserts)
- Lightyear `ServerPlugins` tick rate must match: `FIXED_TIMESTEP_HZ` from protocol (`protocol/src/lib.rs`)

## Open Risks

- Server plugins may have subtle assumptions about running in a headless app (no window, no renderer) that surface at runtime — requires testing
- `ServerMapPlugin` systems gated on resources inserted by `start_server` (e.g., `MapRegistry`, `RoomRegistry`) need to be inserted by `SingleplayerNetworkPlugin` or a startup system
- Double `SharedGameplayPlugin` addition: the guard should handle it, but needs verification
- Web client's lightyear feature flags may not include everything server plugins need — dependency audit required
