---
date: 2026-03-17T20:43:46-07:00
researcher: Claude Opus 4.6
git_commit: 167fcfb055e012fff5f4201edc97fe266b489c5f
branch: master
repository: bevy-lightyear-template
topic: "Singleplayer Mode Implementation Analysis — Server Systems, Lightyear Host-Server, and WASM Constraints"
tags: [research, codebase, networking, lightyear, singleplayer, host-server, wasm, implementation-analysis]
status: complete
last_updated: 2026-03-17
last_updated_by: Claude Opus 4.6
last_updated_note: "Resolved open questions 2-4,6: plugin duplication (panics), physics double-sim (confirmed), ReplicationSender conflict (confirmed), voxel prediction (works as-is)"
---

# Research: Singleplayer Mode Implementation Analysis

**Date**: 2026-03-17T20:43:46-07:00
**Researcher**: Claude Opus 4.6
**Git Commit**: 167fcfb055e012fff5f4201edc97fe266b489c5f
**Branch**: master
**Repository**: bevy-lightyear-template

## Research Question

How to implement a singleplayer mode where the client runs the server, including web (with limitations) and native. Thorough analysis of how lightyear networking will be affected and what would need to be updated.

## Summary

Lightyear provides a **HostClient** pattern where `ClientPlugins` and `ServerPlugins` coexist in a single `App`. The local client entity uses `LinkOf { server }` instead of a transport — messages bypass packetization entirely and pass through an in-memory buffer (`HostClient.buffer`). Lightyear's `HostServerPlugin` automatically inserts `Predicted`/`Interpolated`/`Controlled` markers on replicated entities so client-side queries still match, even though no actual replication occurs.

This works natively. For WASM, the `server` lightyear feature must compile to `wasm32-unknown-unknown` (untested), and server-side code that uses filesystem I/O (persistence, voxel saves) must be feature-gated or replaced with in-memory alternatives.

This document extends the earlier research ([2026-03-17-singleplayer-and-client-host-modes.md](2026-03-17-singleplayer-and-client-host-modes.md)) with system-by-system analysis, the HostClient internal architecture, and WASM constraints.

## Detailed Findings

### 1. Lightyear HostClient Architecture (Internal Mechanics)

#### Connection Flow

When `Connect` is triggered on a host client entity, the `HostPlugin::connect` observer ([git/lightyear/lightyear_connection/src/host.rs:55-81](../../git/lightyear/lightyear_connection/src/host.rs)) checks that the entity has `Client` + `LinkOf` pointing to a `Started` server. It inserts:

- `Connected` — marks as connected
- `LocalId(PeerId::Local(0))` / `RemoteId(PeerId::Local(0))` — local peer IDs
- `ClientOf` — creates the server-side client relationship
- `HostClient { buffer: Vec::new() }` — the in-memory message buffer
- `HostServer { client: Entity }` — inserted on the server entity

#### Message Passing Bypass

In the `buffer_send` system ([git/lightyear/lightyear_transport/src/plugin.rs:277-283](../../git/lightyear/lightyear_transport/src/plugin.rs)), if an entity has `HostClient`, serialized bytes are pushed directly into `host_client.buffer` instead of going through packetization, bandwidth limiting, and priority filtering.

On the receive side ([git/lightyear/lightyear_messages/src/receive.rs:273-289](../../git/lightyear/lightyear_messages/src/receive.rs)), the buffer is drained and each message is deserialized directly with a faked tick and message_id.

#### Replication for Host Clients

`HostServerPlugin` ([git/lightyear/lightyear_replication/src/host.rs:37-96](../../git/lightyear/lightyear_replication/src/host.rs)) adds `Predicted`/`Interpolated`/`Controlled` components to entities when `Replicated` is added, so client-side queries (e.g., `Query<_, With<Predicted>>`) still work even though no actual network replication occurs. It uses the `Replicated` component's `is_added()` check and propagates to `ReplicateLikeChildren`.

#### Entity Spawning Example (from lobby example)

```rust
// Spawn server entity (with or without transport)
let server = commands.spawn((Server::default(), /* transport components if client-host */)).id();
commands.trigger(Start { entity: server });

// Spawn local client via LinkOf
let client = commands.spawn((Client::default(), LinkOf { server })).id();
commands.trigger(Connect { entity: client });
```

Order matters: server must `Start` before client `Connect`.

### 2. Server Systems — Network Dependency Audit

Every server system was analyzed for its dependency on lightyear networking types. Systems fall into three categories:

#### Category A: Pure Gameplay (No Changes Needed)

These systems use no network-specific types and will work identically in host-server mode:

| System | File:Line | Description |
|--------|-----------|-------------|
| `handle_character_movement` | [server/src/gameplay.rs:84](../../crates/server/src/gameplay.rs) | Queries `CharacterMarker` + `ActionState`, calls shared `apply_movement()` |
| `check_death_and_respawn` | [server/src/gameplay.rs:131](../../crates/server/src/gameplay.rs) | Health check, respawn position, uses `LocalTimeline` for tick |
| `expire_invulnerability` | [server/src/gameplay.rs:184](../../crates/server/src/gameplay.rs) | Removes `Invulnerable` when tick exceeds expiry |
| `validate_respawn_points` | [server/src/gameplay.rs:117](../../crates/server/src/gameplay.rs) | Ensures maps have spawn points |
| `spawn_overworld` | [server/src/map.rs:93](../../crates/server/src/map.rs) | Creates VoxelMapInstance, loads map meta |
| `load_startup_entities` | [server/src/map.rs:316](../../crates/server/src/map.rs) | Loads entities.bin, spawns RespawnPoints |
| `attach_chunk_colliders` | [protocol/src/map/colliders.rs](../../crates/protocol/src/map/colliders.rs) | Shared system, generates physics colliders |

#### Category B: Uses Network Types That HostClient Handles Transparently

These systems use lightyear network types (`Replicate`, `Connected`, `ClientOf`, `MessageReceiver`, `MessageSender`, `Room`, `NetworkVisibility`) but will work in host-server mode because lightyear's HostClient layer provides these same components and message channels:

| System | File:Line | Network Types Used |
|--------|-----------|-------------------|
| `handle_connected` | [server/src/gameplay.rs:217](../../crates/server/src/gameplay.rs) | `Add<Connected>` observer, `RemoteId`, `ClientOf`, `Replicate`, `PredictionTarget`, `ControlledBy`, `RoomEvent` |
| `spawn_dummy_target` | [server/src/gameplay.rs:66](../../crates/server/src/gameplay.rs) | `Replicate::to_clients(NetworkTarget::All)`, `PredictionTarget` |
| `spawn_world_object` | [server/src/world_object.rs:15](../../crates/server/src/world_object.rs) | `Replicate::to_clients(NetworkTarget::All)` |
| `spawn_test_tree` | [server/src/gameplay.rs:198](../../crates/server/src/gameplay.rs) | Calls `spawn_world_object` |
| `handle_voxel_edit_requests` | [server/src/map.rs:441](../../crates/server/src/map.rs) | `MessageReceiver<VoxelEditRequest>`, `MessageSender<VoxelEditAck>` |
| `flush_voxel_broadcasts` | [server/src/map.rs:505](../../crates/server/src/map.rs) | `ServerMultiMessageSender`, `Room` queries |
| `handle_chunk_requests` | [server/src/map.rs:562](../../crates/server/src/map.rs) | `MessageReceiver<ChunkRequest>`, `MessageSender<ChunkDataSync>` |
| `handle_map_switch_requests` | [server/src/map.rs:609](../../crates/server/src/map.rs) | `MessageReceiver<PlayerMapSwitchRequest>`, `DisableRollback`, rooms |
| `handle_map_transition_ready` | [server/src/map.rs:842](../../crates/server/src/map.rs) | `MessageReceiver<MapTransitionReady>`, `MessageSender<MapTransitionEnd>` |
| `on_map_instance_id_added` | [server/src/map.rs:323](../../crates/server/src/map.rs) | `NetworkVisibility`, `RoomEvent` |

In HostClient mode, `Connected` is inserted by the `HostPlugin::connect` observer. `MessageReceiver`/`MessageSender` work because messages go through the `HostClient.buffer`. `Replicate` components trigger the `HostServerPlugin` to add `Predicted`/`Controlled` markers. Rooms work because `RoomPlugin` is purely ECS.

#### Category C: Filesystem I/O (WASM-Incompatible)

| System | File:Line | Issue |
|--------|-----------|-------|
| `save_dirty_chunks_debounced` | [server/src/map.rs:127](../../crates/server/src/map.rs) | Writes chunks to filesystem |
| `save_world_on_shutdown` | [server/src/map.rs:198](../../crates/server/src/map.rs) | Writes to filesystem on `AppExit` |
| `sync_ability_manifest` | [server/src/gameplay.rs:46](../../crates/server/src/gameplay.rs) | Writes `abilities.manifest.ron` to disk |
| `spawn_overworld` (partial) | [server/src/map.rs:93](../../crates/server/src/map.rs) | Loads `map.meta.bin` from filesystem |
| `load_startup_entities` | [server/src/map.rs:316](../../crates/server/src/map.rs) | Loads `entities.bin` from filesystem |
| `persistence::*` | [server/src/persistence.rs](../../crates/server/src/persistence.rs) | All functions use `std::fs` |

### 3. Client Systems — How They Interact With Host-Server

Client systems primarily filter on lightyear markers (`Predicted`, `Controlled`, `Replicated`, `Interpolated`). In host-server mode, `HostServerPlugin` automatically inserts these markers on replicated entities, so **all client systems work without modification**.

Key client systems and their lightyear dependencies:

| System | File:Line | Lightyear Markers Used |
|--------|-----------|----------------------|
| `handle_new_character` | [client/src/gameplay.rs:20](../../crates/client/src/gameplay.rs) | `Added<Replicated>`, `Controlled`, `Added<Predicted>`, `Added<Interpolated>` |
| `handle_character_movement` | [client/src/gameplay.rs:59](../../crates/client/src/gameplay.rs) | `With<Predicted>` |
| `on_world_object_replicated` | [client/src/world_object.rs:11](../../crates/client/src/world_object.rs) | `Added<Replicated>` |
| `request_missing_chunks` | [client/src/map.rs:145](../../crates/client/src/map.rs) | `MessageSender<ChunkRequest>` |
| `handle_chunk_data_sync` | [client/src/map.rs:219](../../crates/client/src/map.rs) | `MessageReceiver<ChunkDataSync>` |
| `handle_voxel_input` | [client/src/map.rs:387](../../crates/client/src/map.rs) | `MessageSender<VoxelEditRequest>` |
| `handle_map_transition_start` | [client/src/map.rs:523](../../crates/client/src/map.rs) | `MessageReceiver<MapTransitionStart>` |

### 4. SharedGameplayPlugin Duplication Concern

`SharedGameplayPlugin` ([protocol/src/lib.rs:209-236](../../crates/protocol/src/lib.rs)) is currently added by both the server and client apps independently. In host-server mode (single App), both `ServerGameplayPlugin` and `ClientGameplayPlugin` attempt to add it. This plugin registers:

- `AppStatePlugin` — app state machine
- `ProtocolPlugin` — lightyear component/channel/message registration
- `AbilityPlugin` — ability definitions and asset loading
- `WorldObjectPlugin` — world object definitions
- `LightyearAvianPlugin` — physics replication bridge
- `PhysicsPlugins` — Avian3D physics

**Risk**: Double-registration will likely panic at runtime. Bevy panics when the same plugin is added twice (unless it implements `fn is_unique() -> false`). Each of these sub-plugins must either:
- Implement `is_unique() -> false` (idempotent registration)
- Or be conditionally added (only once)

The standard pattern is to check `app.is_plugin_added::<T>()` before adding, or use a wrapper that checks.

### 5. Server Crate as Library

The server crate already has a `lib.rs` ([server/src/lib.rs](../../crates/server/src/lib.rs)) that re-exports all modules. However, it only re-exports modules — it does not provide a reusable `ServerPlugin` struct. The server's `main.rs` directly assembles the App with `MinimalPlugins`, `AssetPlugin`, `TransformPlugin`, etc.

For host-server mode, the server logic needs to be packaged as plugins that can be added to a client App that already has `DefaultPlugins`. This means:

1. `ServerGameplayPlugin` — already a proper Plugin struct
2. `ServerMapPlugin` — already a proper Plugin struct
3. `ServerNetworkPlugin` — already a proper Plugin struct, needs a "no transport" variant
4. The MinimalPlugins + AssetPlugin assembly in `main.rs` — must NOT be added in host mode (the client's DefaultPlugins covers these)

### 6. WASM-Specific Constraints

#### Threading

WASM in browsers is single-threaded for Bevy (tracking issue [bevyengine/bevy#4078](https://github.com/bevyengine/bevy/issues/4078), on hold). This rules out `CrossbeamIo`-based two-App patterns. HostClient (single App) is the only viable approach.

#### Lightyear `server` Feature on WASM

Untested. `ServerPlugins` is purely ECS-based (no I/O if no transport is spawned), but some transitive dependencies (e.g., `lightyear_netcode` which includes socket-related code) may fail to compile for `wasm32-unknown-unknown`. The `server` feature in lightyear enables:

```toml
# lightyear/lightyear/Cargo.toml (approximate)
server = ["lightyear_connection/server", "lightyear_replication/server", "lightyear_messages/server"]
```

Each sub-crate may have platform-conditional code. Verification requires attempting `cargo build -p web --target wasm32-unknown-unknown --features server`.

#### Networking

Browser WASM cannot bind ports (no UDP, no WebSocket server, no WebTransport server). Client-host mode where remote players join is **impossible from WASM**. Singleplayer-only is possible via `LinkOf` (no transport needed).

#### Filesystem

WASM has no `std::fs`. All persistence systems must be gated:

```rust
#[cfg(not(target_family = "wasm"))]
fn save_dirty_chunks_debounced(...) { ... }
```

Or replaced with IndexedDB/LocalStorage via `web-sys`. For an MVP, singleplayer-on-web can simply skip persistence entirely (no world saves).

#### Crossbeam Channels in WASM

`crossbeam-channel` compiles for `wasm32-unknown-unknown` but only works single-threaded (no parallelism). The HostClient pattern does not use crossbeam channels (it uses `HostClient.buffer` directly), so this is a non-issue.

### 7. Potential Approaches

#### Approach A: Dedicated Singleplayer Crate

Create a `crates/singleplayer/` crate that depends on both `client` and `server` crates. It assembles a single App with:
- `DefaultPlugins` (from client)
- `ClientPlugins` + `ServerPlugins` (from lightyear)
- `SharedGameplayPlugin` (once, guarded against duplication)
- `ServerGameplayPlugin` + `ServerMapPlugin` (from server crate, used as library)
- `ClientGameplayPlugin` + `ClientMapPlugin` (from client crate, used as library)
- `RenderPlugin` + `UiPlugin` (from render/ui crates)
- Host-server connection setup (server entity with no transport, client entity with `LinkOf`)

**Pros**: Clean separation, no conditional compilation in existing crates.
**Cons**: Third binary to maintain, potential duplication of App assembly logic.

#### Approach B: Mode Enum in Client Crate

Add a `GameMode` enum (`Multiplayer`, `Singleplayer`, `ClientHost`) to the client crate. Based on the mode:
- `Multiplayer`: current behavior (client only, connects to remote server)
- `Singleplayer`: adds `ServerPlugins` + server gameplay plugins, spawns host-server connection
- `ClientHost`: same as Singleplayer but also spawns a transport (UDP/WebTransport) on the server entity

**Pros**: Single binary, mode selection at startup.
**Cons**: Client crate gains dependency on server crate, increasing compile times and binary size.

#### Approach C: Web Singleplayer Crate

For WASM specifically, create `crates/web-singleplayer/` that combines both. Keep native singleplayer in the client crate via approach B. This isolates WASM-specific compromises (no persistence, feature flag gymnastics).

### 8. What Would Need to Change — Checklist

#### Lightyear Feature Changes

| Crate | Current Features | Required Addition |
|-------|-----------------|-------------------|
| `client` | `client, netcode, udp, crossbeam, webtransport, leafwing, prediction, replication, interpolation` | `server` (for native singleplayer/client-host) |
| `web` | `client, netcode, webtransport, websocket, leafwing, prediction, replication, interpolation` | `server` (for WASM singleplayer, if it compiles) |

#### Plugin Duplication Guards

Plugins that would be added by both client and server in a single App need duplication guards:

- `SharedGameplayPlugin` (contains `ProtocolPlugin`, `AbilityPlugin`, `WorldObjectPlugin`, `LightyearAvianPlugin`, `PhysicsPlugins`)
- `AppStatePlugin`
- Any plugin inside these that uses `app.add_plugins()` without `is_unique()` protection

#### Server Plugins as Reusable Library

`ServerGameplayPlugin`, `ServerMapPlugin`, and `ServerNetworkPlugin` are already Plugin structs. The server crate's `lib.rs` re-exports the modules. No structural changes needed — just ensure the client/singleplayer crate can depend on `server` as a library.

However, the server's `main.rs` plugin set (`MinimalPlugins`, `AssetPlugin`, `TransformPlugin`, `ScenePlugin`, asset type registration) overlaps with `DefaultPlugins`. In host-server mode, these must not be added again. The server plugins themselves (`ServerGameplayPlugin`, `ServerMapPlugin`) don't add these — they're only in `main.rs`. So this is fine.

#### Persistence Gating for WASM

All `std::fs` usage in `server/src/persistence.rs` and `server/src/map.rs` (save systems) must be conditionally compiled:

```rust
#[cfg(not(target_family = "wasm"))]
```

Alternatively, behind a cargo feature flag like `persistence` that is enabled by default but disabled for WASM builds.

#### Network Plugin for Host-Server Mode

`ServerNetworkPlugin` currently always spawns transport entities. For singleplayer, it needs a "no transport" mode:
- Either skip the `start_server` system entirely
- Or add a `ServerTransport::None` variant that spawns a server entity without any IO component

`ClientNetworkPlugin` currently always spawns a `NetcodeClient` with a transport. For host-server mode, it needs to spawn `(Client::default(), LinkOf { server })` instead. A new `ClientTransport::HostServer { server: Entity }` variant could handle this.

### 9. Prediction Behavior in Host-Server Mode

In standard multiplayer, the client predicts locally and reconciles with server state. In host-server mode:

- The `HostServerPlugin` inserts `Predicted` on replicated entities, but **prediction/rollback is effectively a no-op** because the server state IS the client state (same App, same World)
- Client-side prediction systems (`handle_character_movement` on `Predicted` entities) still run, but since server and client share the same ECS World, there's no divergence to reconcile
- `should_rollback` functions will never trigger because predicted and confirmed values are always identical
- This means singleplayer has zero prediction overhead — a net positive

### 10. WASM `server` Feature Compilation — Confirmed Compatible

The lightyear `server` feature **compiles for `wasm32-unknown-unknown`**. All WASM-incompatible code is properly gated throughout the dependency tree.

#### Feature Propagation

The `server` feature ([git/lightyear/lightyear/Cargo.toml:67-84](../../git/lightyear/lightyear/Cargo.toml)) activates sub-features on these core sub-crates, all of which are WASM-compatible:

- `lightyear_connection/server` — empty feature flag, no platform deps
- `lightyear_messages/server` — enables `lightyear_link` + connection server
- `lightyear_sync/server` — enables connection/messages/transport server features
- `lightyear_transport/server` — enables connection server

Optional transport crates (`lightyear_netcode?/server`, `lightyear_udp?/server`, etc.) only activate if the transport dependency is already present.

#### WASM Gating Mechanisms

1. **`lightyear_udp` excluded at dependency level**: `lightyear/lightyear/Cargo.toml:226-227` places it under `[target."cfg(not(target_family = \"wasm\"))".dependencies]`. On WASM, it's never a dependency — the `lightyear_udp?/server` feature activation is a no-op.

2. **Server IO plugins cfg-gated in `ServerPlugins`**: [git/lightyear/lightyear/src/server.rs:68-75](../../git/lightyear/lightyear/src/server.rs) registers all transport server plugins (UDP, WebTransport, WebSocket, Steam) under `#[cfg(all(feature = "...", not(target_family = "wasm")))]`.

3. **Transport sub-crate server modules gated**: [git/lightyear/lightyear_websocket/src/lib.rs:7](../../git/lightyear/lightyear_websocket/src/lib.rs) and [git/lightyear/lightyear_webtransport/src/lib.rs:7](../../git/lightyear/lightyear_webtransport/src/lib.rs) both guard `pub mod server` behind `#[cfg(all(feature = "server", not(target_family = "wasm")))]`.

4. **Aeronet native dependencies gated**: `tokio`, `wtransport`, `tokio-tungstenite`, `rustls`, etc. are all under `[target.'cfg(not(target_family = "wasm"))'.dependencies]` in their respective Cargo.toml files.

5. **`lightyear_netcode` is WASM-compatible**: Uses `core::net::SocketAddr` (not `std::net`), `web-time` for time on WASM ([git/lightyear/lightyear_netcode/src/utils.rs:1-2](../../git/lightyear/lightyear_netcode/src/utils.rs)), and `no_std_io2` instead of `std::io`. No sockets, no threads.

6. **Prelude re-exports gated**: [git/lightyear/lightyear/src/lib.rs:420-438](../../git/lightyear/lightyear/src/lib.rs) gates UDP, WebSocket, WebTransport, and Steam re-exports behind `cfg(not(target_family = "wasm"))`.

#### What the `server` Feature Provides on WASM

The full server ECS logic: connection handling (`Connected`/`Disconnected`/`ClientOf`), message routing (`MessageReceiver`/`MessageSender`), replication (`Replicate`/`ReplicationSender`), netcode authentication, room management, and the `HostPlugin`/`HostServerPlugin` for in-process client-server. No transport backends (those are excluded), but the `LinkOf`/HostClient pattern requires none.

### 11. Room System in Host-Server Mode

The server uses `RoomPlugin` to manage per-map visibility. In host-server mode:
- `RoomPlugin` is purely ECS, no network dependency
- The host client is added to rooms via the same `RoomEvent`/`RoomTarget` system
- `NetworkVisibility` on entities still works (controls what's "visible" to the client)
- Room-based message filtering (e.g., `ServerMultiMessageSender` sending to room members) correctly targets the host client

## Code References

- `crates/server/src/main.rs` — Server App assembly
- `crates/server/src/gameplay.rs` — Server gameplay systems
- `crates/server/src/map.rs` — Server map/chunk systems with `RoomPlugin`, `RoomRegistry`
- `crates/server/src/network.rs` — `ServerNetworkPlugin`, `ServerTransport` enum
- `crates/server/src/persistence.rs` — Filesystem persistence (WASM-incompatible)
- `crates/server/src/world_object.rs` — World object spawning with `Replicate`
- `crates/client/src/gameplay.rs` — Client prediction systems
- `crates/client/src/map.rs` — Client chunk streaming and voxel edit prediction
- `crates/client/src/network.rs` — `ClientNetworkPlugin`, `ClientTransport` enum
- `crates/web/src/main.rs` — Web client App assembly
- `crates/web/src/network.rs` — `WebClientPlugin` (WebTransport config)
- `crates/protocol/src/lib.rs:209-236` — `SharedGameplayPlugin` (duplication risk)
- `git/lightyear/lightyear_connection/src/host.rs` — `HostPlugin`, `HostClient`/`HostServer` components
- `git/lightyear/lightyear_replication/src/host.rs` — `HostServerPlugin` (inserts `Predicted`/`Controlled`)
- `git/lightyear/lightyear_transport/src/plugin.rs:277-283` — `buffer_send` bypass for `HostClient`
- `git/lightyear/lightyear_messages/src/receive.rs:273-289` — `HostClient` message receive bypass
- `git/lightyear/examples/common/src/cli.rs:165-200` — `Mode::HostClient` reference implementation
- `git/lightyear/examples/lobby/src/client.rs:60-97` — Runtime host transition

## Architecture Documentation

### Current Architecture (Multiplayer Only)

```
┌─────────────────┐         Network          ┌─────────────────┐
│  Server Binary  │◄═══════════════════════►  │  Client Binary  │
│  MinimalPlugins │     UDP/WebTransport      │  DefaultPlugins  │
│  ServerPlugins  │                           │  ClientPlugins   │
│  SharedGameplay │                           │  SharedGameplay  │
│  ServerGameplay │                           │  ClientGameplay  │
│  ServerMap      │                           │  ClientMap       │
│  ServerNetwork  │                           │  ClientNetwork   │
└─────────────────┘                           │  Render + UI     │
                                              └─────────────────┘
```

### Proposed Architecture (Host-Server / Singleplayer)

```
┌──────────────────────────────────────────────────────┐
│               Single Bevy App                         │
│  DefaultPlugins                                       │
│  ServerPlugins + ClientPlugins                        │
│  SharedGameplay (once, deduplicated)                  │
│  ServerGameplay + ClientGameplay                      │
│  ServerMap + ClientMap                                │
│  Render + UI                                          │
│                                                       │
│  ┌─────────┐  LinkOf  ┌─────────┐                    │
│  │ Server  │◄─────────│ Client  │                    │
│  │ Entity  │  (zero-  │ Entity  │                    │
│  │         │  cost)   │         │                    │
│  └─────────┘          └─────────┘                    │
│       ▲                                               │
│       │ (optional, client-host only)                  │
│  ┌────┴────────────┐                                  │
│  │ UDP/WebTransport│  ◄── Remote clients connect here │
│  └─────────────────┘                                  │
└──────────────────────────────────────────────────────┘
```

### Platform Capability Matrix

| Capability | Native | WASM |
|-----------|--------|------|
| Singleplayer (HostClient, no transport) | Yes | Yes (`server` feature compiles for WASM — all transport code is cfg-gated) |
| Client-host (HostClient + UDP/WT transport) | Yes | No (cannot bind ports) |
| World persistence (save/load) | Yes | No (no `std::fs`; possible via IndexedDB later) |
| Chunk generation | Yes | Yes (CPU-bound, no platform dependency) |
| Physics | Yes | Yes (Avian compiles for WASM) |
| Abilities / Combat | Yes | Yes (pure ECS logic) |

## Historical Context

- [2026-03-17-singleplayer-and-client-host-modes.md](2026-03-17-singleplayer-and-client-host-modes.md) — Initial research establishing `LinkOf`/HostClient as the recommended approach over `CrossbeamIo`. Identified the key open questions (WASM `server` feature, plugin duplication, asset loading collision, physics duplication, `ReplicationSender` on `ClientOf`)
- [2025-12-31-crossbeam-clientserverstepper-integration.md](2025-12-31-crossbeam-clientserverstepper-integration.md) — Deep dive on `CrossbeamIo` for testing; confirms it's test-oriented, not for gameplay
- [2025-11-16-lightyear-multi-crate-setup.md](2025-11-16-lightyear-multi-crate-setup.md) — Original multi-crate transport architecture
- [2026-03-14-integration-test-crate-refactor.md](2026-03-14-integration-test-crate-refactor.md) — `CrossbeamTestStepper` documentation

## Related Research

- [2026-01-31-webtransport-configuration.md](2026-01-31-webtransport-configuration.md) — WebTransport setup for web clients

## Open Questions

1. ~~**WASM `server` feature compilation**~~: **Resolved — yes, it compiles.** All transport code is behind `cfg(not(target_family = "wasm"))` guards. See Section 10 for full analysis.

2. ~~**`SharedGameplayPlugin` duplication**~~: **Resolved — will panic.** Every plugin in the `SharedGameplayPlugin` tree uses Bevy's default `is_unique() -> true`. The first sub-plugin (`AppStatePlugin`) would trigger `DuplicatePlugin` panic on the second `add_plugins` call. The only lightyear plugin with `is_unique() -> false` is `SharedPlugins` (part of `ClientPlugins`/`ServerPlugins`), which is separate from the project's `SharedGameplayPlugin`. **Fix required:** Either (a) have the host-server assembly add `SharedGameplayPlugin` once and exclude it from both `ServerGameplayPlugin` and `ClientGameplayPlugin`, or (b) guard with `app.is_plugin_added::<SharedGameplayPlugin>()` before adding.

3. ~~**Physics double-simulation**~~: **Resolved — yes, movement would be double-applied.** There is a single `FixedUpdate` schedule (no separate server/client schedules). The server's `handle_character_movement` filters on `With<CharacterMarker>` only. The client's filters on `(With<Predicted>, With<CharacterMarker>)`. In host-server mode, `HostServerPlugin` inserts `Predicted` on replicated entities, so character entities match BOTH queries — `apply_movement` runs twice per tick. Lightyear's spaceships demo avoids this by registering movement **once** in a shared plugin and using `Single<Has<Server>, Without<ClientOf>>` to branch on server-vs-client behavior at runtime. **Fix required:** Either (a) unify movement into `SharedGameplayPlugin` with runtime branching, or (b) add a `Without<Predicted>` filter to the server's movement system and rely on the client's `With<Predicted>` variant for host-server mode.

4. ~~**`ReplicationSender` required component**~~: **Resolved — it WILL apply globally, and that's a problem.** Bevy's `register_required_components_with` is global — any entity receiving `ClientOf` from any code path gets `ReplicationSender` auto-inserted. In host-server mode, `HostPlugin::connect` inserts `ClientOf` on the host-client entity, which would auto-insert `ReplicationSender`. This violates lightyear's expectation: lightyear's own tests explicitly assert `!host_client.contains::<ReplicationSender>()` ([git/lightyear/lightyear_tests/src/host_server/base.rs:56](../../git/lightyear/lightyear_tests/src/host_server/base.rs)). The replication buffer system has `Without<HostClient>` guards, but `handle_acks` and `send_replication_messages` in `ReplicationSendPlugin` query `(&mut ReplicationSender, &mut Transport), With<Connected>` without excluding `HostClient` — they would match and attempt replication operations on the host-client. **Fix required:** Replace `register_required_components_with` with an observer on `Add<Connected>` that checks `Without<HostClient>` before inserting `ReplicationSender`, matching the pattern used in lightyear examples (e.g., [git/lightyear/examples/simple_box/src/server.rs:35-37](../../git/lightyear/examples/simple_box/src/server.rs)).

5. **`RemoteId` for host client**: `handle_connected` ([server/src/gameplay.rs:217](../../crates/server/src/gameplay.rs)) reads `RemoteId` from the `ClientOf` entity to derive a player identifier. In HostClient mode, the `RemoteId` is `PeerId::Local(0)`. The current code uses `RemoteId` → `PeerId` → client_id to set the player name ("Player 0"). This works but the name would always be "Player 0" in singleplayer. Minor cosmetic issue — acceptable.

6. ~~**Voxel prediction redundancy**~~: **Resolved — client systems can run as-is.** In host-server mode, `MessageSender`/`MessageReceiver` work identically via a local shortcut path. The `HostClient` entity's `send_local` system ([git/lightyear/lightyear_messages/src/send.rs:288-377](../../git/lightyear/lightyear_messages/src/send.rs)) pushes messages directly from `MessageSender` to `MessageReceiver` on the same entity, bypassing serialization. Server-to-client messages go through `HostClient.buffer` ([git/lightyear/lightyear_messages/src/receive.rs:273-289](../../git/lightyear/lightyear_messages/src/receive.rs)). All client voxel systems (chunk requests, edit acks, broadcasts) will function without modification. The prediction pipeline is redundant but harmless — messages arrive within the same tick. **No conditional disabling needed for correctness.** Could be optimized later by skipping the prediction bookkeeping when `HostClient` is present, but this is a performance optimization, not a requirement. Note: lightyear's `is_host_server()` run condition exists at [git/lightyear/lightyear_connection/src/identity.rs:23-26](../../git/lightyear/lightyear_connection/src/identity.rs) but is currently `todo!()` — not implemented.
