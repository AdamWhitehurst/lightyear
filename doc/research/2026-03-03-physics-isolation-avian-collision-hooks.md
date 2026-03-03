---
date: 2026-03-03T10:35:20-08:00
researcher: Claude
git_commit: defdadf3268f84bd08ddcb07eea0e5cc113a076a
branch: master
repository: bevy-lightyear-template
topic: "Physics isolation via Avian CollisionHooks for multi-instance maps"
tags: [research, physics, avian3d, collision-hooks, map-instances, spatial-query, map-transition, lightyear-rooms]
status: complete
last_updated: 2026-03-03
last_updated_by: Claude
last_updated_note: "Added map switch button and PlayerMapSwitchRequest message research"
---

# Research: Physics Isolation via Avian CollisionHooks

**Date**: 2026-03-03T10:35:20-08:00 **Researcher**: Claude **Git Commit**: defdadf3268f84bd08ddcb07eea0e5cc113a076a **Branch**: master **Repository**:
bevy-lightyear-template

## Research Question

Evaluate the proposed `MapInstanceId` + `CollisionHooks::filter_pairs` approach for isolating physics between map instances (Overworld, Homebase,
Arena) that share a single Avian physics world.

## Summary

The proposed approach is sound for **contact pair filtering** but has a critical gap: **`filter_pairs` does not affect `SpatialQuery` operations**.
The ground-detection raycast in `apply_movement` will still hit terrain from other map instances. This requires a separate solution using
`cast_ray_predicate` or equivalent. All other aspects of the proposal — the `MapInstanceId` component, `SystemParam`-based hooks,
`ActiveCollisionHooks` opt-in, and the single-hooks-impl constraint — are confirmed correct against avian3d 0.4.1's API.

## Detailed Findings

### 1. Current Physics Setup

The project uses **avian3d 0.4.1** configured in `SharedGameplayPlugin` ([lib.rs:242-248](crates/protocol/src/lib.rs#L242-L248)):

```rust
PhysicsPlugins::default()
    .build()
    .disable::<PhysicsTransformPlugin>()
    .disable::<PhysicsInterpolationPlugin>()
    .disable::<IslandSleepingPlugin>()
```

No `CollisionHooks` are registered. Physics runs in a single global world shared by server and client via `SharedGameplayPlugin`.

**Lightyear integration** ([lib.rs:237-240](crates/protocol/src/lib.rs#L237-L240)): `LightyearAvianPlugin` with `AvianReplicationMode::Position`.
Position, Rotation, LinearVelocity, AngularVelocity are registered for prediction with custom rollback thresholds.

### 2. Current Collision Layers

Defined at [hit_detection.rs:17-53](crates/protocol/src/hit_detection.rs#L17-L53):

| Layer                           | Membership | Collides With                          |
| ------------------------------- | ---------- | -------------------------------------- |
| `character_collision_layers()`  | Character  | Character, Terrain, Hitbox, Projectile |
| `terrain_collision_layers()`    | Terrain    | Character                              |
| `hitbox_collision_layers()`     | Hitbox     | Character                              |
| `projectile_collision_layers()` | Projectile | Character                              |

These separate entity _types_ but not map _instances_. With 32-bit layer limit, dedicating layers per instance is not scalable.

### 3. Physics Entity Types

| Entity Type   | RigidBody | Collider              | Sensor | Spawn Location                                                        |
| ------------- | --------- | --------------------- | ------ | --------------------------------------------------------------------- |
| Character     | Dynamic   | Capsule(r=2, h=2)     | No     | [server/gameplay.rs:189-207](crates/server/src/gameplay.rs#L189-L207) |
| Terrain Chunk | Static    | Trimesh (from mesh)   | No     | [protocol/map.rs:46-72](crates/protocol/src/map.rs#L46-L72)           |
| Melee Hitbox  | Kinematic | Cuboid(1.5, 2.0, 1.0) | Yes    | [ability.rs:1097-1136](crates/protocol/src/ability.rs#L1097-L1136)    |
| AoE Hitbox    | Kinematic | Sphere(radius)        | Yes    | [ability.rs:1138-1178](crates/protocol/src/ability.rs#L1138-L1178)    |
| Projectile    | Kinematic | Sphere(0.5)           | Yes    | [ability.rs:1449-1477](crates/protocol/src/ability.rs#L1449-L1477)    |

All need `MapInstanceId` + `ActiveCollisionHooks::FILTER_PAIRS` for isolation.

### 4. CollisionHooks API (avian3d 0.4.1)

Confirmed from local source at `git/avian/src/collision/hooks.rs`:

```rust
pub trait CollisionHooks: ReadOnlySystemParam + Send + Sync {
    fn filter_pairs(&self, collider1: Entity, collider2: Entity, commands: &mut Commands) -> bool { true }
    fn modify_contacts(&self, contacts: &mut ContactPair, commands: &mut Commands) -> bool { true }
}
```

- **`filter_pairs`**: Called in broad phase. Returns `false` to skip narrow phase entirely (efficient early-out).
- **`modify_contacts`**: Called in narrow phase after contact computation. Can modify friction, restitution, contact points.
- **Requires `ReadOnlySystemParam`**: No mutable queries. Deferred writes via `Commands` only.
- **No `ContactGraph` access**: Panics if attempted in either method.
- **One impl per app**: `PhysicsPlugins::default().with_collision_hooks::<T>()` accepts exactly one type.

#### ActiveCollisionHooks Component

```rust
#[derive(Component)]
#[component(immutable)]
pub struct ActiveCollisionHooks(u8);

// Flags:
// ActiveCollisionHooks::FILTER_PAIRS     (0b01)
// ActiveCollisionHooks::MODIFY_CONTACTS  (0b10)
```

Hooks are **opt-in per entity**. `filter_pairs` is only called for pairs where at least one entity has `ActiveCollisionHooks::FILTER_PAIRS`. Entities
without this component skip hook evaluation entirely.

**Static/Sleeping skip**: Avian does not call hooks when both entities are `RigidBody::Static` or `Sleeping`. This is an internal Avian optimization that is irrelevant to our use case — terrain-terrain non-interaction is already handled by collision layers (`terrain_collision_layers()` only collides with `Character`). All pairs that matter for map isolation (character-terrain, character-hitbox, etc.) have at least one non-Static entity, so `filter_pairs` is always called for them.

#### Registration

Current code at [lib.rs:242](crates/protocol/src/lib.rs#L242):

```rust
PhysicsPlugins::default().build().disable::<...>()
```

Must change to:

```rust
PhysicsPlugins::default().with_collision_hooks::<MapCollisionHooks>().build().disable::<...>()
```

Note: `.with_collision_hooks()` returns `PhysicsPluginsWithHooks<H>` which also implements `PluginGroup` and supports `.build()`.

### 5. Critical Gap: SpatialQuery Is Not Affected by CollisionHooks

`SpatialQuery::cast_ray` operates **independently** from the collision pipeline. It uses its own `SpatialQueryFilter` which supports:

- `CollisionLayers` mask filtering
- Entity include/exclude sets
- `ColliderDisabled` exclusion

**`filter_pairs` does NOT filter spatial queries.** The ground-detection raycast at [lib.rs:310-321](crates/protocol/src/lib.rs#L310-L321) currently
uses only self-exclusion:

```rust
let filter = &SpatialQueryFilter::from_excluded_entities([entity]);
if spatial_query.cast_ray(ray_cast_origin, Dir3::NEG_Y, 4.0, false, filter).is_some() {
    forces.apply_linear_impulse(Vec3::new(0.0, 400.0, 0.0));
}
```

Without additional filtering, a character in the Overworld could detect ground from a Homebase terrain chunk at an overlapping world position.

**Solution**: Use `SpatialQuery::cast_ray_predicate` which accepts a closure for per-entity filtering:

```rust
let map_id = map_ids.get(entity).map(|m| m.0);
let filter = SpatialQueryFilter::from_excluded_entities([entity]);
if spatial_query
    .cast_ray_predicate(ray_cast_origin, Dir3::NEG_Y, 4.0, false, &filter, &|hit_entity| {
        match (map_id, map_ids.get(hit_entity).map(|m| m.0)) {
            (Some(a), Ok(b)) => a == b,
            _ => true,
        }
    })
    .is_some()
```

This requires passing the `MapInstanceId` query into `apply_movement`, which currently takes `SpatialQuery` as a parameter.

### 6. Map Instance System — Existing Infrastructure

The voxel map engine already provides the entity-based multiplexing needed:

- **`VoxelMapInstance`** component on map entities ([instance.rs:25-32](crates/voxel_map_engine/src/instance.rs#L25-L32))
- **`ChunkTarget.map_entity: Entity`** tracks which map a player drives chunk loading for
  ([chunk.rs:13-17](crates/voxel_map_engine/src/chunk.rs#L13-L17))
- **Chunks are children** of their map entity ([lifecycle.rs:224](crates/voxel_map_engine/src/lifecycle.rs#L224))
- **Marker components**: `Overworld`, `Homebase { owner }`, `Arena { id }` ([instance.rs:8-22](crates/voxel_map_engine/src/instance.rs#L8-L22))

`MapInstanceId` value for each entity type:

- **Terrain chunks**: Parent map entity (already known via `ChildOf` hierarchy)
- **Characters**: `ChunkTarget.map_entity` (already set at spawn: [server/gameplay.rs:206](crates/server/src/gameplay.rs#L206))
- **Hitboxes/Projectiles**: Copy from caster's `MapInstanceId` at spawn time

### 7. Proposed Approach Evaluation

The `MapInstanceId` + `filter_pairs` approach from [doc/plans/2026-02-28-voxel-map-engine.md:868-919](doc/plans/2026-02-28-voxel-map-engine.md) is
correct with one addition:

**What works as proposed:**

- `MapInstanceId(Entity)` component — maps directly to the `VoxelMapInstance` entity
- `SystemParam`-based `MapCollisionHooks` with read-only query — confirmed valid pattern
- `filter_pairs` returning `false` for mismatched IDs — skips narrow phase, efficient
- Fallthrough `_ => true` for entities without `MapInstanceId` — allows global physics entities
- `ActiveCollisionHooks::FILTER_PAIRS` opt-in — only filtered entities pay the cost

**What needs addition:**

- `SpatialQuery` filtering — must use `cast_ray_predicate` or equivalent for ground detection
- `apply_movement` signature change — needs access to `MapInstanceId` query
- Future hooks (one-way platforms, conveyors) must be added to the same `MapCollisionHooks` SystemParam since only one hooks impl is allowed per app

### 8. Entity Insertion Points

Where `MapInstanceId` + `ActiveCollisionHooks::FILTER_PAIRS` must be inserted:

| Entity                        | Current Spawn                                                         | Component Source                                            |
| ----------------------------- | --------------------------------------------------------------------- | ----------------------------------------------------------- |
| Character (server)            | [server/gameplay.rs:189-207](crates/server/src/gameplay.rs#L189-L207) | `ChunkTarget.map_entity` (already available at spawn)       |
| Character (client prediction) | [client/gameplay.rs:46-52](crates/client/src/gameplay.rs#L46-L52)     | Must replicate `MapInstanceId` or derive from `ChunkTarget` |
| Terrain chunk                 | [protocol/map.rs:46-72](crates/protocol/src/map.rs#L46-L72)           | Parent map entity via `ChildOf`                             |
| Melee hitbox                  | [ability.rs:1097-1136](crates/protocol/src/ability.rs#L1097-L1136)    | Caster's `MapInstanceId`                                    |
| AoE hitbox                    | [ability.rs:1138-1178](crates/protocol/src/ability.rs#L1138-L1178)    | Caster's `MapInstanceId`                                    |
| Projectile                    | [ability.rs:1449-1477](crates/protocol/src/ability.rs#L1449-L1477)    | Caster's `MapInstanceId`                                    |
| Dummy target                  | [server/gameplay.rs:60-74](crates/server/src/gameplay.rs#L60-L74)     | `OverworldMap` resource                                     |

## Code References

- `crates/protocol/src/lib.rs:242-248` — PhysicsPlugins registration (no hooks)
- `crates/protocol/src/lib.rs:296-322` — `apply_movement` with SpatialQuery ground detection
- `crates/protocol/src/hit_detection.rs:17-53` — GameLayer and collision layer functions
- `crates/protocol/src/map.rs:46-72` — `attach_chunk_colliders` system
- `crates/protocol/src/ability.rs:1097-1178` — Hitbox spawning (melee + AoE)
- `crates/protocol/src/ability.rs:1449-1477` — Projectile spawning
- `crates/server/src/gameplay.rs:189-207` — Character spawn with ChunkTarget
- `crates/voxel_map_engine/src/instance.rs:25-32` — VoxelMapInstance component
- `crates/voxel_map_engine/src/chunk.rs:13-17` — ChunkTarget.map_entity
- `git/avian/src/collision/hooks.rs` — CollisionHooks trait source (local)

## Architecture Documentation

**Single physics world**: All entities share one Avian physics world. Isolation is currently type-based only (CollisionLayers). The proposed approach
adds instance-based isolation via broad-phase hook filtering.

**Existing entity-map association**: `ChunkTarget.map_entity` and chunk-parent hierarchy already track which map an entity belongs to. `MapInstanceId`
formalizes this for physics specifically.

**Hook extensibility constraint**: Only one `CollisionHooks` impl per app. The `MapCollisionHooks` SystemParam must be designed to accommodate future
hook needs (one-way platforms, etc.) by adding more queries to the same struct.

## Historical Context (from doc/)

- `doc/plans/2026-02-28-voxel-map-engine.md:868-919` — Original proposal for physics isolation via CollisionHooks, scoped as "Future Work"
- `doc/research/2026-01-09-raycast-chunk-collider-detection.md` — Prior research on raycast/chunk collider detection issues (schedule mismatch)
- `doc/research/2026-02-27-bonsairobo-stack-multi-instance-voxel-replacement.md` — Multi-instance voxel architecture research

## Related Research

- [doc/research/2026-02-27-bonsairobo-stack-multi-instance-voxel-replacement.md](doc/research/2026-02-27-bonsairobo-stack-multi-instance-voxel-replacement.md)
- [doc/research/2026-02-13-hit-detection-system.md](doc/research/2026-02-13-hit-detection-system.md)
- [doc/research/2026-01-09-raycast-chunk-collider-detection.md](doc/research/2026-01-09-raycast-chunk-collider-detection.md)

## External Sources

- [Avian 0.3 Blog Post (CollisionHooks introduction)](https://joonaa.dev/blog/08/avian-0-3)
- [avian3d docs.rs - collision module](https://docs.rs/avian3d/latest/avian3d/collision/index.html)
- [PhysicsPlugins docs.rs](https://docs.rs/avian3d/latest/avian3d/struct.PhysicsPlugins.html)
- [SpatialQuery docs.rs](https://docs.rs/avian3d/latest/avian3d/spatial_query/struct.SpatialQuery.html)
- [GitHub - avianphysics/avian](https://github.com/avianphysics/avian)
- [One-way platform example](https://github.com/Jondolf/avian/blob/main/crates/avian2d/examples/one_way_platform_2d.rs)

## Resolved Design Decisions

1. **Lightyear replication of MapInstanceId**: Replicate via lightyear. Requires `MapEntities` impl since it holds an `Entity`, same pattern as `ChunkTarget` ([chunk.rs:26-36](crates/voxel_map_engine/src/chunk.rs#L26-L36)).

2. **`ActiveCollisionHooks` via required component**: Use `#[require(ActiveCollisionHooks::FILTER_PAIRS)]` on `MapInstanceId` so inserting `MapInstanceId` automatically adds the hooks opt-in. No manual insertion at spawn sites needed.

3. **Map transition protocol**: Use a client-side loading state with physics pausing. See follow-up research below.

## Follow-up Research: Map Transition Loading State

### Problem

When a player transitions between maps, several things must update: `MapInstanceId`, `ChunkTarget.map_entity`, `Position`, and the new map's terrain chunks need to load. During this window, physics (raycasts, collisions) could interact with wrong-map terrain.

### Approach: SubState + RigidBodyDisabled + Lightyear Rooms

#### Client-Side: SubState for Transition

Bevy `SubStates` exist only when a parent state has a specific value, and are removed from the World otherwise. The project already has `ClientState::InGame` ([ui/state.rs:3-13](crates/ui/src/state.rs#L3-L13)) which is a natural parent:

```rust
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, SubStates)]
#[source(ClientState = ClientState::InGame)]
enum MapTransitionState {
    #[default]
    Playing,
    Transitioning,
}
```

- Gameplay systems run in `in_state(MapTransitionState::Playing)`
- `OnEnter(Transitioning)`: show loading UI, insert `RigidBodyDisabled` on player
- `OnExit(Transitioning)`: hide loading UI, remove `RigidBodyDisabled`
- Transition completes when new map's chunks are loaded (track pending chunk count)

#### Physics Pausing: RigidBodyDisabled

Avian provides `RigidBodyDisabled` ([avian/src/dynamics/rigid_body/mod.rs:376-380](git/avian/src/dynamics/rigid_body/mod.rs#L376-L380)) — a marker component that:
- Excludes the entity from the solver (no forces/impulses applied)
- Excludes from island computation
- Disables contact response and narrow-phase resolution
- **Preserves** Position, Rotation, LinearVelocity — so teleporting during transition works

```rust
// Enter transition
commands.entity(player).insert(RigidBodyDisabled);
// Exit transition
commands.entity(player).remove::<RigidBodyDisabled>();
```

Does NOT affect `SpatialQuery` — but during transition, gameplay systems (including `apply_movement` with its raycast) are gated on `MapTransitionState::Playing`, so raycasts don't run.

#### Server-Side: Lightyear Rooms

Lightyear has a room-based interest management system ([lightyear_replication/src/visibility/room.rs](git/lightyear/lightyear_replication/src/visibility/room.rs)). Each map instance should be a `Room`. Entities are visible to a client only if they share at least one room.

```rust
// Create room per map instance
let room = commands.spawn((Room::default(), Name::from("Overworld Room"))).id();

// Add client + player entity to room
commands.trigger(RoomEvent { room, target: RoomTarget::AddSender(client_entity) });
commands.trigger(RoomEvent { room, target: RoomTarget::AddEntity(player_entity) });
```

Room transitions move client+entity from old room to new room. The room system tracks shared room counts, so doing remove+add in the same frame preserves visibility (no despawn/respawn flicker, confirmed by `test_move_client_entity_room`).

##### Room Membership Details

**API**: `commands.trigger(RoomEvent { ... })` - `RoomEvent` is an `EntityEvent` with `#[event_target] room: Entity`. An observer registered by `RoomPlugin` handles the event ([room.rs:337](git/lightyear/lightyear_replication/src/visibility/room.rs#L337)).

**What needs room membership**: Every replicated entity must be explicitly added via `RoomTarget::AddEntity`. Every client must be added via `RoomTarget::AddSender`. An entity is visible to a client only if they share at least one room.

**Entities without `NetworkVisibility`**: Visible to all clients. Entities *with* `NetworkVisibility` but not in any room are invisible to all. The room system auto-inserts `NetworkVisibility` on entities that receive their first room event ([room.rs:219-221](git/lightyear/lightyear_replication/src/visibility/room.rs#L219-L221)).

**Child entities**: Do NOT inherit room membership, but lightyear's hierarchy system provides a fallback — when the buffer processes a child entity with `ReplicateLike`, it checks the child's `NetworkVisibility` first, then falls back to the root entity's ([buffer.rs:202-204](git/lightyear/lightyear_replication/src/buffer.rs#L202-L204)). So children of a replicated parent inherit visibility without needing explicit room adds.

##### Automatic Room Management via MapInstanceId

No built-in auto-room mechanism exists in lightyear. Each `RoomEvent` is explicit. To automate:

**Option A: Observer on `MapInstanceId` insert/change.** When `MapInstanceId` is inserted or changed, an observer fires `RoomEvent` to add the entity to the corresponding room. This requires a mapping from map entity → room entity (e.g. a `MapRoom(Entity)` component on the map entity, or a `HashMap<Entity, Entity>` resource).

```rust
fn on_map_instance_id_added(
    trigger: On<Add, MapInstanceId>,
    map_ids: Query<&MapInstanceId>,
    map_rooms: Query<&MapRoom>,
    mut commands: Commands,
) {
    let map_id = map_ids.get(trigger.target()).unwrap();
    let room = map_rooms.get(map_id.0).unwrap().0;
    commands.trigger(RoomEvent { room, target: RoomTarget::AddEntity(trigger.target()) });
}
```

**Option B: Spawn Room as part of VoxelMapInstance.** When a map instance is spawned, also spawn a `Room` and store the room entity on the map (e.g. `MapRoom(Entity)` component). Then the observer above links any entity with `MapInstanceId` to the correct room automatically.

**Chunks specifically**: Terrain chunks are children of the map entity. If the map entity is in a room and has `Replicate`, child chunks with `ReplicateLike` inherit visibility via the hierarchy fallback — no per-chunk room add needed. Player entities and hitboxes/projectiles (which are NOT children of the map entity) need explicit room membership via the observer approach.

#### Prediction During Transition: DisableRollback

`DisableRollback` ([lightyear_prediction/src/rollback.rs:238](git/lightyear/lightyear_prediction/src/rollback.rs#L238)) excludes an entity from rollback checks. Already used on hitboxes and projectiles ([ability.rs:1128, 1167, 1470](crates/protocol/src/ability.rs#L1128)). Insert during transition to prevent prediction from rolling back to stale pre-transition state.

#### Transition Sequence

**Server-side (atomic, single frame):**
1. Insert `RigidBodyDisabled` + `DisableRollback` on player entity
2. Remove client sender + player entity from old room
3. Add client sender + player entity to new room
4. Update `MapInstanceId` to new map entity
5. Update `ChunkTarget.map_entity` to new map entity
6. Set `Position` to new map spawn point
7. Zero `LinearVelocity`
8. Send a custom "map transition" channel message to the client

**Server-side (after client confirms chunks loaded, or after timeout):**
1. Remove `RigidBodyDisabled` + `DisableRollback` from player entity

**Client-side:**
1. Receive transition message → set `MapTransitionState::Transitioning`
2. `OnEnter(Transitioning)`: insert `RigidBodyDisabled` + `DisableRollback` on player, show loading UI
3. Wait for new map's chunks to load (track via `TrackedAssets` or chunk count)
4. When loaded → set `MapTransitionState::Playing`
5. `OnExit(Transitioning)`: remove `RigidBodyDisabled` + `DisableRollback`, hide loading UI

### Existing Infrastructure

| Component | Location | Role |
|-----------|----------|------|
| `AppState::Loading/Ready` | [app_state.rs:4-9](crates/protocol/src/app_state.rs#L4-L9) | Initial asset loading gate |
| `ClientState` | [ui/state.rs:3-13](crates/ui/src/state.rs#L3-L13) | Client UI state machine |
| `ChunkTarget.map_entity` | [chunk.rs:13-36](crates/voxel_map_engine/src/chunk.rs#L13-L36) | Drives chunk loading per-map, already implements `MapEntities` |
| `DisableRollback` | [ability.rs:1128](crates/protocol/src/ability.rs#L1128) | Already used on hitboxes/projectiles |
| `TrackedAssets` | [app_state.rs](crates/protocol/src/app_state.rs) | Tracks asset loading completion |
| `OverworldMap(Entity)` | [server/map.rs](crates/server/src/map.rs) | Single map reference (no multi-map yet) |

No `SubStates`, room management, or map transition logic exists yet. All are new work.

### Chunk Loading Completion Detection

The client needs to know when the new map's chunks are loaded to exit `MapTransitionState::Transitioning`.

#### Current State of Chunk Tracking

The voxel_map_engine tracks per-map loading state via two components on the map entity:

- **`VoxelMapInstance.loaded_chunks: HashSet<IVec3>`** — positions with completed generation ([instance.rs:30](crates/voxel_map_engine/src/instance.rs#L30))
- **`PendingChunks`** ([generation.rs:20-24](crates/voxel_map_engine/src/generation.rs#L20-L24)):
  - `tasks: Vec<Task<ChunkGenResult>>` — in-flight async tasks
  - `pending_positions: HashSet<IVec3>` — positions with tasks spawned but not yet complete

The **desired** chunk set is computed fresh each frame in `collect_desired_positions` ([lifecycle.rs:60-91](crates/voxel_map_engine/src/lifecycle.rs#L60-L91)) as a local `HashSet<IVec3>` and discarded. It is not persisted on any component.

There are **no events, observers, or callbacks** for chunk completion. The only way to detect it is to inspect component state.

#### Complication: MAX_TASKS_PER_FRAME Cap

`spawn_missing_chunks` limits to `MAX_TASKS_PER_FRAME = 32` tasks per frame ([lifecycle.rs:11](crates/voxel_map_engine/src/lifecycle.rs#L11)). This means `pending.tasks.is_empty()` can be `true` momentarily while more chunks still need spawning next frame. Checking pending alone is unreliable.

#### Reliable Completion Check

The reliable check is: `desired ⊆ loaded_chunks` AND `pending.tasks.is_empty()`. Since `desired` is not persisted, two approaches:

**Option A: Persist the desired set on the map component.** Add a `desired_chunks: HashSet<IVec3>` field to `VoxelMapInstance` (or a separate component). Populated by `update_chunks` each frame. The transition system checks `desired.is_subset(&loaded_chunks) && pending.tasks.is_empty()`.

**Option B: Recompute desired in the transition check system.** Factor `collect_desired_positions` into a public function and call it from the transition system to compare against `loaded_chunks`. Avoids storing extra state but duplicates computation.

**Decision: Option A** — persist the desired set. Makes chunk readiness available for any consumer without recomputation.

#### Proposed Transition Check System

```rust
fn check_map_transition_complete(
    maps: Query<(&VoxelMapInstance, &PendingChunks)>,
    player: Query<&ChunkTarget, With<Predicted>>,
    mut next_state: ResMut<NextState<MapTransitionState>>,
) {
    let Ok(target) = player.single() else { return };
    let Ok((instance, pending)) = maps.get(target.map_entity) else { return };
    if pending.tasks.is_empty() && instance.desired_chunks.is_subset(&instance.loaded_chunks) {
        next_state.set(MapTransitionState::Playing);
    }
}
```

Runs in `Update` gated on `in_state(MapTransitionState::Transitioning)`.

## Resolved: Collider Attach During Transition

`attach_chunk_colliders` does NOT need to be gated on `MapTransitionState::Playing`. Colliders can attach freely during `Transitioning` — `RigidBodyDisabled` on the player prevents the solver from generating contacts, and gameplay systems (including `apply_movement` raycasts) are gated on `Playing`. The colliders sit inert until the transition completes.

## Follow-up Research: Map Switch Button and PlayerMapSwitchRequest

### Research Question

How to add an in-game button that sends a `PlayerMapSwitchRequest` message from client to server, enabling server-controlled map transitions between Overworld and Homebase.

### Existing Patterns to Follow

#### Message Pattern: VoxelEditRequest

The codebase has exactly one client-to-server message: `VoxelEditRequest` ([protocol/map.rs:26-30](crates/protocol/src/map.rs#L26-L30)). `PlayerMapSwitchRequest` follows the same pattern:

| Step | VoxelEditRequest (existing) | PlayerMapSwitchRequest (new) |
|------|---------------------------|------------------------------|
| Define | `#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Reflect, Message)]` | Same derives |
| Channel | `VoxelChannel` (ordered reliable) | New `MapChannel` |
| Register | `register_message::<T>().add_direction(NetworkDirection::ClientToServer)` | Same |
| Client send | `MessageSender<T>.send::<Channel>(msg)` | Same |
| Server receive | `MessageReceiver<T>.receive()` | Same |

#### UI Button Pattern: In-Game HUD

The in-game HUD ([ui/lib.rs:312-384](crates/ui/src/lib.rs#L312-L384)) already has "Main Menu" and "Quit" buttons in the top-right. Pattern:

1. Marker component in [components.rs](crates/ui/src/components.rs): `#[derive(Component)] pub struct MapSwitchButton;`
2. Spawn in `setup_ingame_hud` with `(Button, Node, BorderColor, BackgroundColor, MapSwitchButton)` + child `Text`
3. Interaction system queries `Query<&Interaction, (Changed<Interaction>, With<MapSwitchButton>)>`, checks `== Interaction::Pressed`

### Proposed Implementation

#### 1. Message Type

In `crates/protocol/src/map.rs`:

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Reflect, Message)]
pub enum MapSwitchTarget {
    Overworld,
    Homebase,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Reflect, Message)]
pub struct PlayerMapSwitchRequest {
    pub target: MapSwitchTarget,
}
```

The target enum uses semantic variants rather than raw entity IDs. The server resolves which map entity to use. `Homebase` does not require the homebase to already exist — the server spawns it on demand if needed (see server handler below).

For server-initiated transitions (e.g. portal entry), the server skips `PlayerMapSwitchRequest` entirely and directly executes the transition protocol, sending a `MapTransitionStart` message to the client.

#### 2. Channel and Registration

In `crates/protocol/src/lib.rs`, in `ProtocolPlugin::build`:

```rust
app.add_channel::<MapChannel>(ChannelSettings {
    mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
    ..default()
})
.add_direction(NetworkDirection::Bidirectional);

app.register_message::<PlayerMapSwitchRequest>()
    .add_direction(NetworkDirection::ClientToServer);
app.register_message::<MapTransitionStart>()
    .add_direction(NetworkDirection::ServerToClient);
```

`MapChannel` is bidirectional — client sends `PlayerMapSwitchRequest`, server sends `MapTransitionStart`. Both ordered reliable.

#### 3. UI Button (Dynamic Toggle)

In `crates/ui/src/components.rs`, add:

```rust
#[derive(Component)]
pub struct MapSwitchButton;
```

The button text and target are dynamic — it shows the *other* map (the one you'd switch to). The client tracks which map the player is on via the replicated `MapInstanceId` component on the `Predicted` player entity.

In `crates/ui/src/map_switch.rs`:

```rust
fn setup_map_switch_button(mut commands: Commands, hud_root: Query<Entity, With<HudRoot>>) {
    let Ok(root) = hud_root.single() else { return };
    commands.entity(root).with_children(|parent| {
        parent.spawn((
            Button,
            Node {
                width: Val::Px(150.0),
                height: Val::Px(50.0),
                border: UiRect::all(Val::Px(3.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BorderColor::all(Color::WHITE),
            BackgroundColor(Color::srgba(0.2, 0.2, 0.2, 0.8)),
            MapSwitchButton,
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("Homebase"),
                TextFont { font_size: 24.0, ..default() },
                TextColor(Color::WHITE),
            ));
        });
    });
}
```

#### 4. Button Interaction → Send Message (Toggle)

```rust
fn handle_map_switch_button(
    button_query: Query<&Interaction, (Changed<Interaction>, With<MapSwitchButton>)>,
    mut message_sender: Query<&mut MessageSender<PlayerMapSwitchRequest>>,
    player_map: Query<&MapInstanceId, With<Predicted>>,
    overworld: Res<OverworldMap>,
) {
    for interaction in button_query.iter() {
        if *interaction != Interaction::Pressed { continue; }
        let target = match player_map.single() {
            Ok(map_id) if map_id.0 == overworld.0 => MapSwitchTarget::Homebase,
            _ => MapSwitchTarget::Overworld,
        };
        for mut sender in message_sender.iter_mut() {
            sender.send::<MapChannel>(PlayerMapSwitchRequest { target });
        }
    }
}

fn update_map_switch_button_text(
    player_map: Query<&MapInstanceId, (With<Predicted>, Changed<MapInstanceId>)>,
    overworld: Res<OverworldMap>,
    button: Query<&Children, With<MapSwitchButton>>,
    mut text: Query<&mut Text>,
) {
    let Ok(map_id) = player_map.single() else { return };
    let label = if map_id.0 == overworld.0 { "Homebase" } else { "Overworld" };
    let Ok(children) = button.single() else { return };
    for child in children.iter() {
        if let Ok(mut t) = text.get_mut(*child) {
            **t = label.to_string();
        }
    }
}
```

The button label updates reactively when `MapInstanceId` changes on the predicted player entity. When on the overworld, it shows "Homebase"; when on any other map, it shows "Overworld".

#### 5. Server-Side Handler

In `crates/server/src/map.rs`:

```rust
fn handle_map_switch_requests(
    mut receiver: Query<(Entity, &mut MessageReceiver<PlayerMapSwitchRequest>), With<ClientOf>>,
    mut commands: Commands,
    overworld: Res<OverworldMap>,
    homebases: Query<(Entity, &Homebase)>,
    players: Query<(Entity, &PlayerId), With<CharacterMarker>>,
) {
    for (client_entity, mut message_receiver) in receiver.iter_mut() {
        for request in message_receiver.receive() {
            // Future: validate request (cooldown, combat state, etc.)

            let Some((player_entity, _)) = players.iter()
                .find(|(_, pid)| /* match client_entity to player */) else { continue };

            let target_map = match request.target {
                MapSwitchTarget::Overworld => overworld.0,
                MapSwitchTarget::Homebase => {
                    find_or_spawn_homebase(&mut commands, player_entity, &homebases)
                }
            };

            initiate_map_transition(&mut commands, client_entity, player_entity, target_map);
        }
    }
}
```

##### Homebase Spawn-on-Demand

The server lazily spawns a player's homebase the first time they request it. The `Homebase { owner }` marker component tracks ownership, so subsequent requests find the existing instance:

```rust
fn find_or_spawn_homebase(
    commands: &mut Commands,
    player_entity: Entity,
    homebases: &Query<(Entity, &Homebase)>,
) -> Entity {
    // Check if homebase already exists for this player
    if let Some((map_entity, _)) = homebases.iter()
        .find(|(_, hb)| hb.owner == player_entity)
    {
        return map_entity;
    }

    // Spawn new homebase
    let (instance, config, marker) = VoxelMapInstance::homebase(
        player_entity,
        IVec3::new(8, 4, 8),  // bounded 8x4x8 chunk area
        Arc::new(flat_terrain_voxels),
    );
    commands.spawn((
        instance,
        config,
        marker,
        Transform::default(),
    )).id()
}
```

`VoxelMapInstance::homebase()` ([instance.rs:56-74](crates/voxel_map_engine/src/instance.rs#L56-L74)) handles setup:
- Seed derived deterministically from owner entity via `seed_from_entity` (reproducible across restarts if entity is stable)
- Bounded map with `Some(bounds)` — chunks only spawn within bounds
- `Homebase { owner }` marker for query-based lookup
- `PendingChunks` auto-inserted by `ensure_pending_chunks` lifecycle system

##### Transition Execution

Shared function for both client-requested and server-initiated transitions (e.g. portal entry):

```rust
fn initiate_map_transition(
    commands: &mut Commands,
    client_entity: Entity,
    player_entity: Entity,
    target_map: Entity,
) {
    commands.entity(player_entity).insert((
        RigidBodyDisabled,
        DisableRollback,
        MapInstanceId(target_map),
        ChunkTarget::new(target_map, 4),
        Position(Vec3::new(0.0, 30.0, 0.0)),  // map spawn point
        LinearVelocity(Vec3::ZERO),
    ));

    // Room transitions (when rooms are implemented):
    // commands.trigger(RoomEvent { room: old_room, target: RoomTarget::RemoveSender(client_entity) });
    // commands.trigger(RoomEvent { room: old_room, target: RoomTarget::RemoveEntity(player_entity) });
    // commands.trigger(RoomEvent { room: new_room, target: RoomTarget::AddSender(client_entity) });
    // commands.trigger(RoomEvent { room: new_room, target: RoomTarget::AddEntity(player_entity) });

    // Send transition message to client
    // (accessed via separate system or MessageSender on client_entity)
}
```

For **server-initiated transitions** (portals, game events), the server calls `initiate_map_transition` directly without going through `PlayerMapSwitchRequest`. The same function handles both paths.

##### MapTransitionStart Message

Server-to-client message notifying the client to enter its loading state:

```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Reflect, Message)]
pub struct MapTransitionStart {
    pub target: MapSwitchTarget,
}
```

Sent to the specific client via `MessageSender<MapTransitionStart>` on the `ClientOf` entity, same pattern as `VoxelStateSync` ([server/map.rs:339-347](crates/server/src/map.rs#L339-L347)). The client receives it via `MessageReceiver<MapTransitionStart>` and enters `MapTransitionState::Transitioning`.

### Where Each Piece Lives

| Component | Crate | File | Notes |
|-----------|-------|------|-------|
| `PlayerMapSwitchRequest` | protocol | `map.rs` | Next to `VoxelEditRequest` |
| `MapTransitionStart` | protocol | `map.rs` | Server-to-client transition notification |
| `MapSwitchTarget` | protocol | `map.rs` | Enum: `Overworld`, `Homebase` |
| `MapChannel` | protocol | `map.rs` | Next to `VoxelChannel` |
| Registration | protocol | `lib.rs` | In `ProtocolPlugin::build`, after voxel message registration |
| `MapSwitchButton` | ui | `components.rs` | Next to other button markers |
| Button spawn + interaction | ui | `map_switch.rs` | New module |
| `handle_map_switch_requests` | server | `map.rs` | Receives request, spawns homebase if needed, initiates transition |
| `initiate_map_transition` | server | `map.rs` | Shared by client-requested and server-initiated transitions |
| `find_or_spawn_homebase` | server | `map.rs` | Lazy homebase creation |

### Dependency Consideration

The `ui` crate already depends on `protocol` ([ui/Cargo.toml:9](crates/ui/Cargo.toml)) and `lightyear` ([ui/Cargo.toml:8](crates/ui/Cargo.toml)). `MessageSender<PlayerMapSwitchRequest>` works because the existing `ingame_button_interaction` already accesses `Query<Entity, With<Client>>` and triggers disconnect — so accessing `MessageSender` is consistent.

### Resolved Design Decisions (Map Switch)

1. **Toggle button**: Single button that toggles between "Homebase" and "Overworld" based on current `MapInstanceId` on the predicted player entity. No keyboard shortcut.
2. **Dedicated `MapChannel`**: Separate from `VoxelChannel`. Bidirectional for `PlayerMapSwitchRequest` (C→S) and `MapTransitionStart` (S→C).
3. **UI module**: New `ui/src/map_switch.rs` module for button spawn, interaction, and text update systems.
4. **Server-initiated transitions**: Server calls `initiate_map_transition` directly for portals/game events, bypassing `PlayerMapSwitchRequest`. Same function handles both paths.