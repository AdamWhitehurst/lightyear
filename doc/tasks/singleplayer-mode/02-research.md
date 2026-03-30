---
date: 2026-03-29
git_commit: 8ae4b353
branch: master
topic: "Lightyear client-server architecture, transport, state machines, and host mode"
tags: [research, codebase, lightyear, networking, singleplayer]
status: complete
---

# Research: Lightyear Client-Server Architecture

**Date**: 2026-03-29
**Git Commit**: 8ae4b353

## Findings

### 1. Client-Server Connection Establishment

#### ClientNetworkConfig

`ClientNetworkConfig` is a `Resource` at `crates/client/src/network.rs:33-43` with fields: `client_addr`, `server_addr`, `client_id`, `protocol_id`, `private_key`, `transport`, `token_expire_secs`. Defaults: bind `0.0.0.0:0`, server `127.0.0.1:5001`, `client_id: 0`, transport `WebTransport` with compile-time certificate digest from `certificates/digest.txt` (line 11).

#### ClientTransport Enum

`crates/client/src/network.rs:14-22`: Three variants:
- `Udp` -- native only (`#[cfg(not(target_family = "wasm"))]`, line 113)
- `WebTransport { certificate_digest }` -- default for both native and web
- `Crossbeam(CrossbeamIo)` -- in-memory for tests

#### Connection Flow

1. **Startup**: `ClientNetworkPlugin::build()` (line 72-82) inserts `ClientNetworkConfig`, registers `setup_client()` as a `Startup` system, adds `on_connected`/`on_disconnected` observers.

2. **Entity spawn**: `setup_client()` (line 84-127) spawns a `Client` entity with `NetcodeClient`, `ReplicationReceiver`, `PredictionManager`, and transport-specific IO component. Entity is **not connected** yet.

3. **Connect trigger**: `on_entering_connecting_state` in `crates/ui/src/lib.rs:94-118` runs on `OnEnter(ClientState::Connecting)`. It replaces `NetcodeClient` with a fresh one (new connect token) and fires `commands.trigger(Connect { entity })`.

4. **Connection observers**: `on_client_connected` (ui/lib.rs:132) transitions to `ClientState::InGame`. `on_client_disconnected` (ui/lib.rs:120) transitions to `ClientState::MainMenu`.

5. **Reconnection**: Each connect attempt replaces the `NetcodeClient` component entirely (line 111-113), generating a fresh token.

#### Disconnect Sources
- Cancel button during `Connecting`: `crates/ui/src/lib.rs:316`
- Main Menu button during `InGame`: `crates/ui/src/lib.rs:450`

Both fire `Disconnect { entity }`.

---

### 2. Server Initialization and Game Loop

#### Entry Point

`crates/server/src/main.rs:18-46`. Plugin order:

1. `MinimalPlugins` (line 19) -- headless, no renderer
2. `StatesPlugin` (line 20), `LogPlugin` (line 21)
3. `AssetPlugin` (lines 22-25) with path to workspace `assets/`
4. `TransformPlugin` (line 26), `ScenePlugin` (line 27)
5. Manual asset type registration: `Mesh`, `StandardMaterial`, `Shader`, `Image` (lines 29-35) -- needed for vox model mesh generation without `DefaultPlugins`
6. `ServerPlugins` (lines 36-38) with tick duration from `FIXED_TIMESTEP_HZ` (64 Hz)
7. `SharedGameplayPlugin` (line 39)
8. `ServerNetworkPlugin` (line 40), `ServerGameplayPlugin` (line 41), `ServerMapPlugin` (line 42)
9. `SharedDiagnosticsPlugin` (line 43), `ServerDiagnosticsPlugin` (line 44)

#### ServerNetworkPlugin

`crates/server/src/network.rs:66-78`:
- Inserts `ServerNetworkConfig` (default: WebTransport port 5001, bind `0.0.0.0`)
- Required component: `ClientOf` entities get `ReplicationSender` with `REPLICATION_INTERVAL` 100ms, `SendUpdatesMode::SinceLastAck` (lines 70-72)
- `Startup` system `start_server` (lines 93-212): spawns server entity per transport with `Server`, `NetcodeServer`, transport IO, triggers `Start`

#### Startup Sequence

1. `start_server` -- spawns transport entities, triggers `Start`
2. `spawn_overworld` then `load_startup_entities` -- creates overworld map, loads persisted entities
3. `spawn_dummy_target` and `validate_respawn_points` -- after `load_startup_entities`
4. `Update` loop: `check_assets_loaded` polls `TrackedAssets`; terrain defs applied when available
5. `AppState::Ready`: `spawn_test_tree` runs, `update_facing` begins in `FixedUpdate`
6. Clients connect: `handle_connected` observer spawns character entities

#### Connection Handling

`handle_connected` at `crates/server/src/gameplay.rs:262-327`: Observer on `Add<Connected>`. Spawns character entity with `Replicate::to_clients(NetworkTarget::All)`, `PredictionTarget`, `ControlledBy`, `CharacterPhysicsBundle`, `Health(100)`, `ChunkTicket::player`, `ClientChunkVisibility`. Adds client to overworld Lightyear room.

---

### 3. Crossbeam Transport and Test Infrastructure

#### CrossbeamIo

`git/lightyear/lightyear_crossbeam/src/lib.rs:34-55`: A Bevy `Component` holding `Sender<Bytes>` + `Receiver<Bytes>`. `CrossbeamIo::new_pair()` creates two unbounded crossbeam channels, cross-wired into a bidirectional pipe.

#### CrossbeamPlugin

`git/lightyear/lightyear_crossbeam/src/lib.rs:61-132`:
- Observer on `LinkStart`: immediately inserts `Linked` (no handshake, line 83)
- `PreUpdate` system `receive`: drains channel into `link.recv`
- `PostUpdate` system `send`: drains `link.send` to channel

#### CrossbeamTestStepper

`crates/server/tests/integration.rs:31-179`: Two separate `App` instances with manual time control.

Construction (lines 43-128):
1. `CrossbeamIo::new_pair()` -- creates paired channels
2. Server app: `MinimalPlugins` + `ServerPlugins` + `ProtocolPlugin` + `RoomPlugin`
3. Client app: `MinimalPlugins` + `ClientPlugins` + `ProtocolPlugin`
4. Both get `TimeUpdateStrategy::ManualInstant`
5. Uses `RawServer`/`RawClient` (not `NetcodeServer`/`NetcodeClient`) to skip netcode handshake
6. `ClientOf` entity spawned on server side with cloned `CrossbeamIo`

Stepping (`tick_step`, lines 151-161): advances time, updates server then client.
Connection polling (`wait_for_connection`, lines 164-178): polls up to 50 ticks for `Connected`.

#### Test Inventory (CrossbeamTestStepper-based)

All in `crates/server/tests/integration.rs`:
- `test_crossbeam_connection_establishment` (line 589)
- `test_crossbeam_client_to_server_messages` (line 634) -- `VoxelEditRequest`
- `test_crossbeam_server_to_client_messages` (line 703) -- `VoxelEditBroadcast`
- `test_crossbeam_event_triggers` (line 771) -- `TestTrigger`
- `test_crossbeam_reconnection` (line 443) -- reconnects 3 times
- `test_client_server_plugin_initialization` (line 338) -- full plugin stack with crossbeam
- Map transition tests (lines 883, 996, 1082)
- Voxel edit/chunk tests (lines 1304, 1427, 1534)

Helper types: `MessageBuffer<M>` (line 183), `EventBuffer<E>` (line 209), `collect_messages` system (line 196).

---

### 4. ClientState State Machine

#### States

`crates/ui/src/state.rs:4-13`:
```rust
enum ClientState { MainMenu, Connecting, InGame }  // default: MainMenu
```

`crates/ui/src/state.rs:16-22`:
```rust
#[source(ClientState = ClientState::InGame)]
enum MapTransitionState { Playing, Transitioning }  // SubStates, default: Playing
```

`crates/protocol/src/app_state.rs:4-9`:
```rust
enum AppState { Loading, Ready }  // orthogonal to ClientState
```

#### Transition Map

```
[MainMenu] --Connect btn (lib.rs:234)--> [Connecting]
[Connecting] --Connected component (lib.rs:137)--> [InGame]
[Connecting] --Cancel btn (lib.rs:316-321)--> [MainMenu]
[InGame] --Disconnected component (lib.rs:128)--> [MainMenu]
[InGame] --MainMenu btn (lib.rs:450-455)--> [MainMenu]

Sub-state of InGame:
[Playing] --MapSwitchButton (lib.rs:510)--> [Transitioning]
[Transitioning] --MapTransitionEnd msg (client/map.rs:570)--> [Playing]
```

#### UI Per State

Each state spawns a root UI node tagged with `DespawnOnExit(state)`:
- **MainMenu** (lib.rs:140): title, `ConnectButton`, `QuitButton`
- **Connecting** (lib.rs:247): "Connecting..." text, `CancelButton`
- **InGame** (lib.rs:326): `MapSwitchButton`, `MainMenuButton`, `QuitButton`
- **Transitioning** (lib.rs:542): fullscreen "Loading..." overlay at `GlobalZIndex(100)`

#### Observers

| Observer | Trigger | Action |
|---|---|---|
| `on_client_connected` (ui/lib.rs:132) | `Add<Connected>` | Set `InGame` |
| `on_client_disconnected` (ui/lib.rs:120) | `Add<Disconnected>` | Set `MainMenu` |
| `on_connected` (network.rs:129) | `Add<Connected>` | Log only |
| `on_disconnected` (network.rs:133) | `Add<Disconnected>` | Log only |

#### State-Gated Systems

- `ClientMapPlugin` chunk/voxel systems: `run_if(in_state(ClientState::InGame))` (`client/map.rs:75`)
- Transition chunk loading: `run_if(in_state(MapTransitionState::Transitioning))` (`client/map.rs:88`)
- `on_world_object_replicated`: `run_if(in_state(AppState::Ready))` (`client/gameplay.rs:23`)

---

### 5. Server-Side Gameplay Systems

#### ServerGameplayPlugin (`crates/server/src/gameplay.rs:20-39`)

**Observers**: `handle_connected` on `Add<Connected>` (line 22)

**Startup**: `spawn_dummy_target` (line 68-85), `validate_respawn_points` (line 120-132) -- both after `load_startup_entities`

**FixedUpdate**:
- `handle_character_movement` (line 87-116) -- queries `CharacterMarker` without `RespawnTimer`, calls `apply_movement`
- `start_respawn_timer` (line 136-159) -- inserts `RespawnTimer` + physics disable on death
- `process_respawn_timers` (line 163-206) -- teleports to `RespawnPoint`, restores health, removes timer
- `expire_invulnerability` (line 223-234) -- removes `Invulnerable` when expired

**Update**: `sync_ability_manifest` (line 48-66) -- writes `abilities.manifest.ron` on change

**OnEnter(Ready)**: `spawn_test_tree` (line 237-260)

#### ServerMapPlugin (`crates/server/src/map.rs:431-466`)

**Sub-plugins**: `RoomPlugin`, `VoxelPlugin`

**Resources**: `ChunkGenerationEnabled`, `MapRegistry`, `RoomRegistry`, `WorldDirtyState`, `PendingVoxelBroadcasts`, `WorldSavePath`

**Startup**: `spawn_overworld` then `load_startup_entities`

**Update** (chained): `apply_terrain_defs` -> `build_terrain_generators` -> voxel editing, chunk streaming, persistence, map transitions

**Observers**: `on_map_instance_id_added` -- inserts `NetworkVisibility`, adds entity to Lightyear room

#### Key Systems
- `handle_voxel_edit_requests` (line 542-591): validates + applies edits, sends `VoxelEditAck`
- `push_chunks_to_clients` (line 680-731): sends up to 16 chunks/tick based on `ClientChunkVisibility`
- `handle_map_switch_requests` (line 846-898): disables physics, moves between rooms, sends `MapTransitionStart`
- `save_dirty_chunks_debounced` (line 197-258): saves after 1s idle or 5s since first dirty
- `save_world_on_shutdown` (line 288-334): synchronous flush on `AppExit`

#### persistence.rs (utility, no plugin)
- `WorldSavePath` resource (line 22-28, default `"worlds"`)
- `MapMeta` (line 12-18): version, seed, generation_version
- `save_map_meta`/`load_map_meta` (lines 39-65): bincode + atomic rename
- `save_entities`/`load_entities` (lines 77-107): bincode with `EntityFileEnvelope`

---

### 6. Shared vs. Server-Only vs. Client-Only Logic

#### SharedGameplayPlugin (`crates/protocol/src/lib.rs:222-251`)

Composes six sub-plugins:

| Sub-plugin | File | Purpose |
|---|---|---|
| `AppStatePlugin` | `protocol/src/app_state.rs:21` | `AppState`, `TrackedAssets`, transition system |
| `ProtocolPlugin` | `protocol/src/lib.rs:85` | Channels, messages, replicated components |
| `AbilityPlugin` | `protocol/src/ability/plugin.rs:29` | Asset loading, activation, effects, hit detection |
| `TerrainPlugin` | `protocol/src/terrain/plugin.rs:16` | Terrain def loading |
| `WorldObjectPlugin` | `protocol/src/world_object/plugin.rs:19` | Object def loading + hot-reload |
| `VoxModelPlugin` | `protocol/src/vox_model/plugin.rs:20` | `.vox` loading with LOD mesh gen |

Plus `LightyearAvianPlugin`, `PhysicsPlugins`, and `update_facing` in `FixedUpdate`.

#### Shared Systems (run on both server and client)

- All ability systems: `ability_activation`, `update_active_abilities`, effect application, projectile spawning (`ability/plugin.rs:78-116`)
- Hit detection chain: `update_hitbox_positions`, `process_hitbox_hits`, `process_projectile_hits`, `cleanup_hitbox_entities` (`ability/plugin.rs:93-104`)
- `apply_movement` function at `protocol/src/character/movement.rs:9-65` -- called by both server and client movement systems
- `update_facing` at `protocol/src/character/movement.rs:69-83`
- `MapCollisionHooks` at `protocol/src/physics.rs:10-23`
- All asset loading systems (`load_*`, `insert_*`, `reload_*`)

#### Server-Only

- `ServerNetworkPlugin`: transport setup, `start_server`
- `ServerGameplayPlugin`: `handle_connected`, `handle_character_movement` (no `Predicted` filter), respawn logic, `spawn_dummy_target`
- `ServerMapPlugin`: rooms, chunk streaming, voxel editing, map transitions, persistence
- `ServerDiagnosticsPlugin`: tracy plotting

#### Client-Only

- `ClientNetworkPlugin`/`WebClientPlugin`: transport setup, `setup_client`
- `ClientGameplayPlugin`: `handle_new_character` (inserts `InputMap`, `CharacterPhysicsBundle`), `handle_character_movement` (queries `With<Predicted>`), camera yaw sync, respawn visibility observers, world object mesh attachment
- `ClientMapPlugin`: voxel prediction, chunk sync, map transition client-side
- `RenderPlugin`: camera, lighting, `MaterialPlugin<BillboardMaterial>`, `MaterialPlugin<SpriteRigMaterial>`
- `UiPlugin`: state machine, UI screens, button interactions
- `PhysicsDebugPlugin`

#### Plugin Composition Matrix

| Plugin | Server | Native Client | Web Client |
|---|---|---|---|
| `SharedGameplayPlugin` | Yes | Yes | Yes |
| `ServerNetworkPlugin` | Yes | -- | -- |
| `ServerGameplayPlugin` | Yes | -- | -- |
| `ServerMapPlugin` | Yes | -- | -- |
| `ClientNetworkPlugin` | -- | Yes | Yes (via `WebClientPlugin`) |
| `ClientGameplayPlugin` | -- | Yes | Yes |
| `ClientMapPlugin` | -- | Yes | Yes |
| `RenderPlugin` | -- | Yes | Yes |
| `UiPlugin` | -- | Yes | Yes |

---

### 7. Asset Loading and AppState Gating

#### AppState Transition

`AppState` starts at `Loading` (default). `check_assets_loaded` at `protocol/src/app_state.rs:34-48` runs every `Update` frame while in `Loading`. Checks all handles in `TrackedAssets` via `asset_server.is_loaded_with_dependencies()`. When all loaded, transitions to `Ready`.

#### TrackedAssets

`protocol/src/app_state.rs:12-19`: A `Vec<UntypedHandle>`. Loading systems call `tracked.add(handle)` during `Startup`.

#### What Gets Tracked

Four asset domains:

| Domain | Native | WASM |
|---|---|---|
| Abilities | `load_folder("abilities")` | Manifest `abilities.manifest.ron` then individual loads |
| Ability slots | `load("default.ability_slots.ron")` | Same |
| Terrain | `load_folder("terrain")` | Manifest then individual loads |
| World objects | `load_folder("objects")` | Manifest then individual loads |
| Vox models | `load_folder("models")` | Manifest then individual loads |

On WASM, `TrackedAssets` grows dynamically during `Loading` as manifest-triggered individual loads add handles.

#### Shared Loading Path

All three apps (server, native client, web) add `SharedGameplayPlugin` -> `AppStatePlugin` + all loading sub-plugins. Identical loading path. The only difference is native `load_folder` vs WASM manifest-based loading.

#### Systems Gated on AppState::Ready

- `update_facing` -- `FixedUpdate` (`protocol/src/lib.rs:249`)
- Ability activation chain (6 systems) -- `FixedUpdate` (`ability/plugin.rs:78-91`)
- Hit detection chain (4 systems) -- `FixedUpdate` (`ability/plugin.rs:93-104`)
- `expire_buffs`, `aoe_hitbox_lifetime`, `ability_bullet_lifetime` -- `FixedUpdate`
- `reload_world_object_defs`, `reload_vox_models` -- `Update`
- `on_world_object_replicated` -- `Update` (client only)
- `spawn_test_tree` -- `OnEnter(AppState::Ready)` (server only)

---

### 8. Headless vs. DefaultPlugins

#### Server: Headless

`crates/server/src/main.rs`: Uses `MinimalPlugins` (line 19). Manually adds `StatesPlugin`, `LogPlugin`, `AssetPlugin`, `TransformPlugin`, `ScenePlugin`. Manually registers `Mesh`, `StandardMaterial`, `Shader`, `Image` asset types (lines 29-35) for CPU-side vox model mesh generation (used for trimesh colliders). Does **not** add `WindowPlugin`, `RenderPlugin`, `UiPlugin`, or any GPU plugin.

#### Native Client: Full Renderer

`crates/client/src/main.rs`: `DefaultPlugins` with custom `AssetPlugin` path. Adds `RenderPlugin`, `UiPlugin`, `PhysicsDebugPlugin`.

#### Web Client: Full Renderer

`crates/web/src/main.rs`: `DefaultPlugins` with custom `WindowPlugin` (title). Adds same renderer-dependent plugins as native client. No `SharedDiagnosticsPlugin` or `ClientDiagnosticsPlugin`.

#### Code Assuming a Window

- `crates/client/src/map.rs:262` -- `handle_voxel_input` queries `Query<&Window, With<PrimaryWindow>>`
- `crates/render/src/camera.rs:31-37` -- spawns `Camera3d`, reads `ButtonInput<KeyCode>`
- `crates/render/src/lib.rs:31-32` -- registers `MaterialPlugin<BillboardMaterial>` and `MaterialPlugin<SpriteRigMaterial>` (GPU pipeline)
- `crates/ui/src/lib.rs:43-91` -- spawns Bevy UI nodes requiring windowed renderer

#### Protocol: Renderer-Agnostic

Material types (`BillboardMaterial`, `SpriteRigMaterial`) are defined in protocol but only registered as GPU material plugins by the client `RenderPlugin`. The protocol uses `RenderAssetUsages` for CPU-side mesh generation at `protocol/src/vox_model/meshing.rs:1`.

---

### 9. Lightyear Host Mode (Combined Client+Server)

#### Plugin Coexistence

Both `ClientPlugins` and `ServerPlugins` include `SharedPlugins`. `SharedPlugins::is_unique()` returns `false` (`git/lightyear/lightyear/src/shared.rs:47`), but guards against double-registration by checking `app.is_plugin_added::<CorePlugins>()` at line 15. First add registers everything; second is no-op.

#### Host Mode Setup

Canonical example at `git/lightyear/examples/common/src/cli.rs:93-201`. In `Mode::HostClient`:

1. Both `ClientPlugins` and `ServerPlugins` added to same `App`
2. `Server` entity spawned (lines 169-188)
3. `Client` entity spawned with `LinkOf { server }` pointing to server entity (lines 190-197) -- **no IO transport needed**
4. `(start, connect).chain()` (line 200)

#### HostPlugin Detection

`git/lightyear/lightyear_connection/src/host.rs:44-158`: Three observers detect host clients:
- `On<Connect>`: If client has `LinkOf` -> `Started` `Server`, inserts `Connected`, `HostClient`, `ClientOf`, `LocalId`, `RemoteId` (lines 65-76). Server gets `HostServer { client }` (lines 77-79).
- `On<Add, (Client, Connected, LinkOf)>`: Re-checks conditions, inserts `HostClient` if matched.
- `On<Add, (Server, Started)>`: Checks all connected clients.

`HostClient` component (line 32) holds a `buffer: Vec<(Bytes, TypeId)>` for in-process message passing.

#### What Host Mode Bypasses

When `HostClient` is present:

1. **No replication send**: `send/buffer.rs:55-56` queries `Without<HostClient>` -- server skips serialization for host client
2. **No replication receive**: `receive.rs:103-105` queries `Without<HostClient>` -- client skips deserialization
3. **No prediction/rollback**: `PredictionFilter` at `plugin.rs:53-58` includes `Without<HostClient>` -- all prediction systems skipped
4. **No rollback check**: `rollback.rs:258` requires `Without<HostClient>`

#### Fake Markers for Query Compatibility

`HostServerPlugin::add_prediction_interpolation_components` at `git/lightyear/lightyear_replication/src/host.rs:37-96` adds `Predicted` and/or `Interpolated` markers to replicated entities, plus `Controlled` for owned entities. Client-side queries like `Query<_, With<Predicted>>` still match in host mode even though prediction/rollback never runs.

#### No Local/Loopback Transport Needed

In host mode, the `Client` entity needs only `LinkOf { server }` -- no `CrossbeamIo`, `UdpIo`, or `WebTransportClientIo`. Messages between host client and server use the in-process `HostClient.buffer`.

---

### 10. Native vs. Web Client Differences

#### Plugin Stack

| Plugin | Native | Web |
|---|---|---|
| `DefaultPlugins` | Custom `AssetPlugin.file_path` (line 37-40) | Custom `WindowPlugin.title` (lines 19-25) |
| Network | `ClientNetworkPlugin` directly (line 45-47) | `WebClientPlugin` wrapper (line 30) |
| `UiClientConfig` | From `ClientNetworkConfig` (lines 29-34) | Hardcoded defaults (lines 31-37) |
| `SharedDiagnosticsPlugin` | Yes (line 54) | No |
| `ClientDiagnosticsPlugin` | Yes (line 55) | No |

Both use WebTransport by default. Both add `SharedGameplayPlugin`, `ClientGameplayPlugin`, `ClientMapPlugin`, `RenderPlugin`, `UiPlugin`, `PhysicsDebugPlugin`.

#### Transport

Both default to `ClientTransport::WebTransport`. Web client creates config via `WebClientPlugin` (`crates/web/src/network.rs:17-39`) which conditionally loads certificate digest (WASM vs non-WASM).

#### Dependencies

- Native: lightyear features include `udp`, `crossbeam`. Has `tracy-client`. `bevy` with `default-features = true`.
- Web: lightyear features include `websocket` (no `udp`/`crossbeam`). WASM deps: `wasm-bindgen`, `console_error_panic_hook`, `getrandom` with `js`. `bevy` with `default-features = false` + explicit minimal features including `webgl2`.

#### Platform-Specific Code

- Web `main.rs:15-16`: `console_error_panic_hook::set_once()` under `#[cfg(target_family = "wasm")]`
- Web: No CLI args (client ID hardcoded to 0)
- Native `main.rs:37-39`: Custom `AssetPlugin` path via `CARGO_MANIFEST_DIR`
- `setup_client()` in `network.rs:112-126`: `#[cfg]` gate -- UDP panics on WASM

#### Shared Code

Web crate imports from `client` crate: `ClientNetworkPlugin`, `ClientGameplayPlugin`, `ClientMapPlugin`. Also uses `protocol::SharedGameplayPlugin`, `render::RenderPlugin`, `ui::UiPlugin`.

---

### 11. Replication, Prediction, and Rollback at Runtime

#### Component Registration (`crates/protocol/src/lib.rs:154-203`)

- **Replicated only**: `MapInstanceId`, `WorldObjectId`, `PlayerId`, `Name`, `AbilitySlots`, `RespawnTimerConfig`, `AbilityProjectileSpawn`
- **Replicated + predicted**: `ColorComponent`, `CharacterMarker`, `Health`, `ActiveAbility`, `Position`, `Rotation`, `LinearVelocity`, `AngularVelocity`, etc.
- **Visual correction**: `Position` and `Rotation` have `.add_linear_correction_fn()` + `.add_linear_interpolation()`
- **Custom rollback thresholds**: `Position` (0.01 distance), `Rotation` (0.01 angle), `LinearVelocity`/`AngularVelocity` (0.01 magnitude)

#### Server Replication Send

`ReplicationSender` configured with `REPLICATION_INTERVAL` 100ms, `SendUpdatesMode::SinceLastAck` (`crates/server/src/network.rs:13,71`). Buffer system at `git/lightyear/lightyear_replication/src/send/buffer.rs:45-57` queries `Without<HostClient>`.

#### Rollback Check (Non-Host)

`check_rollback` at `git/lightyear/lightyear_prediction/src/rollback.rs:245-501`:
1. Requires `IsSynced<InputTimeline>` and `Without<HostClient>` (line 258)
2. Compares prediction history against confirmed state using registered `should_rollback` functions
3. Custom thresholds: `Position` >= 0.01 distance, `Rotation` >= 0.01 angle (protocol lines 206-219)
4. On mismatch: triggers rollback

#### Rollback Execution

`run_rollback` at `rollback.rs:772-843`:
1. Computes `num_rollback_ticks = current_tick - rollback_start_tick`
2. Rewinds `LocalTimeline` and `Time<Fixed>`
3. Disables `DisableRollback` entities via `DisabledDuringRollback`
4. Re-runs `FixedMain` schedule N times
5. Restores time resources

#### Visual Correction

`correction.rs:1-74`: After rollback, computes `VisualCorrection<D>` error between `PreviousVisual` and corrected state. Decays over time for smooth interpolation.

#### Interpolation

`InterpolationPlugin` stores confirmed updates in history buffer and interpolates between consecutive server updates. Applies to entities with `Interpolated` marker (non-controlled replicated entities).

#### Host Mode Behavior (Zero Latency)

When `HostClient` present on client entity:
- No replication send/receive (entities already in shared world)
- No prediction, rollback, or history tracking
- `Predicted`/`Interpolated` markers still applied by `HostServerPlugin` for query compatibility
- No confirmed/predicted entity split, no history buffer
- Entities are authoritative server entities directly

---

## Code References

- `crates/client/src/network.rs:14-127` -- ClientTransport, ClientNetworkConfig, setup_client
- `crates/client/src/gameplay.rs:13-164` -- ClientGameplayPlugin, client movement, character handling
- `crates/client/src/map.rs:49-573` -- ClientMapPlugin, chunk sync, map transitions, voxel input
- `crates/server/src/main.rs:18-46` -- Server entry point, plugin assembly
- `crates/server/src/network.rs:17-212` -- ServerTransport, ServerNetworkConfig, start_server
- `crates/server/src/gameplay.rs:20-327` -- ServerGameplayPlugin, handle_connected, movement, respawn
- `crates/server/src/map.rs:38-898` -- ServerMapPlugin, rooms, chunks, voxel edits, map transitions
- `crates/server/src/persistence.rs:12-107` -- WorldSavePath, MapMeta, entity save/load
- `crates/server/tests/integration.rs:31-179` -- CrossbeamTestStepper
- `crates/protocol/src/lib.rs:52-251` -- Constants, ProtocolPlugin, SharedGameplayPlugin
- `crates/protocol/src/app_state.rs:4-48` -- AppState, TrackedAssets, check_assets_loaded
- `crates/protocol/src/ability/plugin.rs:29-117` -- AbilityPlugin, shared gameplay systems
- `crates/protocol/src/character/movement.rs:9-83` -- apply_movement, update_facing
- `crates/ui/src/state.rs:4-22` -- ClientState, MapTransitionState
- `crates/ui/src/lib.rs:42-554` -- UiPlugin, state transitions, observers, UI screens
- `crates/web/src/network.rs:17-39` -- WebClientPlugin
- `git/lightyear/lightyear_crossbeam/src/lib.rs:34-132` -- CrossbeamIo, CrossbeamPlugin
- `git/lightyear/lightyear_connection/src/host.rs:44-158` -- HostPlugin, HostClient detection
- `git/lightyear/lightyear_replication/src/host.rs:37-96` -- HostServerPlugin, fake markers
- `git/lightyear/lightyear_prediction/src/plugin.rs:53-227` -- PredictionPlugin, PredictionFilter
- `git/lightyear/lightyear_prediction/src/rollback.rs:245-843` -- check_rollback, run_rollback

## Patterns Found

- **Shared-then-specialize**: Both server and client add `SharedGameplayPlugin`, then layer side-specific plugins. Movement function is shared; the calling system differs (server queries all characters, client queries `With<Predicted>`).
- **Observer-driven connection**: Connection state changes are handled via observers on `Add<Connected>`/`Add<Disconnected>`, not polling.
- **DespawnOnExit**: UI hierarchies use `DespawnOnExit(state)` for automatic cleanup on state transitions.
- **Crossbeam for testing**: Tests use `CrossbeamIo::new_pair()` + `RawServer`/`RawClient` to bypass netcode, with `TimeUpdateStrategy::ManualInstant` for deterministic stepping.
- **Host mode via HostClient marker**: A single `Without<HostClient>` filter on key system queries disables replication send/receive, prediction, and rollback when running combined. `HostServerPlugin` adds fake `Predicted`/`Interpolated` markers for query compatibility.
- **WASM manifest loading**: WASM cannot use `load_folder`, so uses manifest files + individual loads with dynamic `TrackedAssets` growth during `Loading` state.
- **Two movement systems pattern**: Server `handle_character_movement` queries without `Predicted` (authoritative). Client `handle_character_movement` queries `With<Predicted>` (predictive). Both call the same `apply_movement` function.



