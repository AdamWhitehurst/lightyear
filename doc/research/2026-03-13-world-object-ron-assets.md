---
date: 2026-03-13T19:20:14-07:00
researcher: Claude
git_commit: 05da6f6f6c0e6d3f8d447e0715acdecd59c44337
branch: master
repository: bevy-lightyear-template
topic: "How to define, load, and place world object RON assets"
tags: [research, codebase, world-objects, ron, assets, procgen, reflect, manifest]
status: complete
last_updated: 2026-03-13
last_updated_by: Claude
last_updated_note: "Added world object replication patterns and cross-engine comparison"
---

# Research: World Object RON Assets

**Date**: 2026-03-13T19:20:14-07:00
**Researcher**: Claude
**Git Commit**: 05da6f6f6c0e6d3f8d447e0715acdecd59c44337
**Branch**: master
**Repository**: bevy-lightyear-template

## Research Question

How to define, load, and place world object RON assets (buildings, doors, trees, ores, items, characters). How to attach visual assets (.vox models, sprites, sprite rigs). How maps can reference these for procedural generation. How to follow the abilities manifest/hot-reload pattern. How to specify Reflect Components on world objects.

## Summary

The codebase has a well-established pattern for hot-reloadable RON assets via `bevy_common_assets` `RonAssetPlugin`, manifest files for WASM, and `TrackedAssets` for load-gating. The ability system demonstrates every piece needed: per-file assets, a manifest listing all files, folder-based native loading, hot-reload via `AssetEvent`, and conversion from raw asset to game resource. No world object definition system exists yet. The existing entity persistence system (`SavedEntity` / `SavedEntityKind`) currently only handles `RespawnPoint` and would need extension. The project does not use Bevy's reflection system for dynamic component insertion — all `Reflect` derives exist for Lightyear networking requirements. Implementing Reflect-based component attachment on world objects would be new infrastructure.

---

## Detailed Findings

### 1. Established Asset Loading Patterns

The project has two mature asset loading patterns that serve as templates.

#### Pattern A: Individual File Assets with Manifest (AbilityDef)

Each ability is a separate `.ability.ron` file in `assets/abilities/`. The RON deserializes directly into `AbilityDef` (which derives `Asset, TypePath, Serialize, Deserialize, Reflect`).

**Native loading** ([ability.rs:435-444](crates/protocol/src/ability.rs#L435-L444)): Uses `asset_server.load_folder("abilities")` which returns a `Handle<LoadedFolder>`. All handles within the folder are tracked.

**WASM loading** ([ability.rs:446-485](crates/protocol/src/ability.rs#L446-L485)): `load_folder` doesn't work on WASM. Instead:
1. A manifest file `abilities.manifest.ron` (a `Vec<String>` of ability IDs) is loaded first
2. Once the manifest is loaded, individual `{id}.ability.ron` files are loaded via `asset_server.load(format!("abilities/{id}.ability.ron"))`
3. Each individual handle is added to `TrackedAssets`

**Asset type** ([ability.rs:180-193](crates/protocol/src/ability.rs#L180-L193)):
```rust
#[derive(Clone, Debug, Serialize, Deserialize, Asset, TypePath, Reflect)]
#[type_path = "protocol::ability"]
pub struct AbilityDef { /* fields */ }
```

**Manifest type** ([ability.rs:373-375](crates/protocol/src/ability.rs#L373-L375)):
```rust
#[derive(Deserialize, Asset, TypePath)]
struct AbilityManifest(Vec<String>);
```

**Aggregation resource** ([ability.rs:195-198](crates/protocol/src/ability.rs#L195-L198)):
```rust
#[derive(Resource, Clone, Debug)]
pub struct AbilityDefs {
    pub abilities: HashMap<AbilityId, AbilityDef>,
}
```

**Hot-reload** ([ability.rs:539-600](crates/protocol/src/ability.rs#L539-L600)): Listens for `AssetEvent::Modified` via `MessageReader<AssetEvent<AbilityDef>>`. On any modification, re-collects all abilities and overwrites the `AbilityDefs` resource.

**Plugin registration** ([ability.rs:406-432](crates/protocol/src/ability.rs#L406-L432)):
```rust
app.add_plugins(RonAssetPlugin::<AbilityDef>::new(&["ability.ron"]));
// WASM only:
app.add_plugins(RonAssetPlugin::<AbilityManifest>::new(&["abilities.manifest.ron"]));
```

**ID extraction from path** ([ability.rs:602-605](crates/protocol/src/ability.rs#L602-L605)):
```rust
fn ability_id_from_path(path: &AssetPath) -> Option<AbilityId> {
    let name = path.path().file_name()?.to_str()?;
    Some(AbilityId(name.strip_suffix(".ability.ron")?.to_string()))
}
```

#### Pattern B: Single File Asset (AbilitySlots)

`default.ability_slots.ron` is a single file that deserializes directly into `AbilitySlots`.

**Loading** ([ability.rs:630-638](crates/protocol/src/ability.rs#L630-L638)):
```rust
fn load_default_ability_slots(mut commands: Commands, asset_server: Res<AssetServer>, mut tracked: ResMut<TrackedAssets>) {
    let handle = asset_server.load::<AbilitySlots>("default.ability_slots.ron");
    tracked.add(handle.clone());
    commands.insert_resource(DefaultAbilitySlotsHandle(handle));
}
```

**Sync (insert + hot-reload combined)** ([ability.rs:640-667](crates/protocol/src/ability.rs#L640-L667)): A single system handles both initial insert and hot-reload by listening for `AssetEvent::LoadedWithDependencies` and `AssetEvent::Modified`.

#### Pattern C: String-Path Cross-References (Sprite Rig)

The sprite rig system demonstrates how assets can reference other assets via string paths:

- `SpriteAnimSetAsset.rig` holds `"rigs/humanoid.rig.ron"` — resolved to a handle via `asset_server.load` at runtime
- `SpriteAnimSetAsset.locomotion.entries[].clip` holds paths like `"anims/humanoid/idle.anim.ron"`
- `SpriteAnimSetAsset.ability_animations` maps ability ID strings to clip paths

These cross-references are resolved lazily in `Update` systems, not during asset loading.

### 2. Load-Gating Infrastructure

**TrackedAssets** ([app_state.rs:12-19](crates/protocol/src/app_state.rs#L12-L19)):
```rust
#[derive(Resource, Default)]
pub struct TrackedAssets(Vec<UntypedHandle>);
```

**AppState gate** ([app_state.rs:34-48](crates/protocol/src/app_state.rs#L34-L48)): `check_assets_loaded` runs in `Update` while in `AppState::Loading`. Transitions to `AppState::Ready` when all tracked handles report `is_loaded_with_dependencies`.

All asset types (abilities, ability slots, rig files, animsets) add their handles to `TrackedAssets` during `Startup`.

### 3. Current World Object Infrastructure

#### What Exists

**RespawnPoint** ([protocol/src/lib.rs:86-88](crates/protocol/src/lib.rs#L86-L88)):
```rust
#[derive(Component, Clone, Debug)]
#[require(MapSaveTarget)]
pub struct RespawnPoint;
```

This is the only "world object" type. It has a position (via `Transform`) and a `MapInstanceId`. The `#[require(MapSaveTarget)]` ensures it's included in persistence.

**SavedEntity** ([protocol/src/map.rs:172-182](crates/protocol/src/map.rs#L172-L182)):
```rust
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum SavedEntityKind {
    RespawnPoint,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SavedEntity {
    pub kind: SavedEntityKind,
    pub position: Vec3,
}
```

Currently only `RespawnPoint` is a valid `SavedEntityKind`. The persistence system (`entities.bin`) saves/loads `Vec<SavedEntity>` per map.

**Dummy target** ([server/src/gameplay.rs:64-80](crates/server/src/gameplay.rs#L64-L80)): A single NPC-like entity spawned at hardcoded position on the overworld at startup. No data-driven definition.

#### What Does NOT Exist

- No world object definition type or prefab system
- No `.vox` loading code (files exist in `assets/models/` but are unused)
- No object placement in terrain generation
- No `VoxelGenerator` decoration/feature pass
- No mechanism for `WorldVoxel` to reference external entities (it only carries `Solid(u8)` material index)

### 4. Existing .vox Model Files

Files exist but are unused by any code:
```
assets/models/trees/tree_circle.vox, tree_square.vox, tree_tall.vox
assets/models/bushes/bush_circle.vox, bush_rectangle.vox, bush_square.vox
assets/models/tools.vox, food.vox, environment.vox, buildings.vox, house.vox
```

No `dot_vox` or `bevy_vox_scene` crate is in any `Cargo.toml`. The [vox loading research](doc/research/2026-03-13-vox-loading-without-scenes.md) recommends `dot_vox` + existing `block-mesh-rs` pipeline.

### 5. Reflect Components — Current State

**Current usage**: All `Reflect` derives in the project exist for Lightyear networking, not for dynamic component insertion. There is:
- No `#[reflect(Component)]` usage
- No `ReflectComponent` or `ReflectDefault` usage
- No `app.register_type::<T>()` calls
- No `TypeRegistry` lookups or dynamic component insertion
- No Bevy scene files (`.scn.ron`)

**Types that derive Reflect** (relevant subset):

| Type | Location | Also Component? |
|------|----------|----------------|
| `PlayerId` | protocol/src/lib.rs:66 | Yes |
| `CharacterType` | protocol/src/lib.rs:77-83 | Yes |
| `MapInstanceId` | protocol/src/map.rs:13 | Yes |
| `AbilityId` | protocol/src/ability.rs:41 | No |
| `AbilityDef` | protocol/src/ability.rs:180 | No (Asset) |
| `AbilityProjectileSpawn` | protocol/src/ability.rs:357 | Yes |
| `WorldVoxel` | voxel_map_engine/src/types.rs:13 | No |

### 6. Terrain Generation — No Object Placement

The only terrain generator is `flat_terrain_voxels` ([meshing.rs:66-76](crates/voxel_map_engine/src/meshing.rs#L66-L76)) — fills everything at/below y=0 with `Solid(0)`. There is no noise, no biome system, no feature placement pass.

The `VoxelGenerator` type ([config.rs:9](crates/voxel_map_engine/src/config.rs#L9)):
```rust
pub type VoxelGenerator = Arc<dyn Fn(IVec3) -> Vec<WorldVoxel> + Send + Sync>;
```

It only produces voxel data. There is no callback or return channel for "also spawn these entities at these positions." Object placement would need to be a separate system that runs after/alongside chunk generation.

### 7. Sprite Rig — How Visual Assets are Referenced

The sprite rig demonstrates the pattern for linking a character type to its visual representation:

1. `CharacterType` component is added to an entity ([protocol/src/lib.rs:77-83](crates/protocol/src/lib.rs#L77-L83))
2. `resolve_character_rig` observer fires on `Added<CharacterType>`, looks up `RigRegistry` (which maps `CharacterType` -> rig handle + animset handle), inserts `SpriteRig` and `AnimSetRef` components ([sprite_rig/src/spawn.rs:40-64](crates/sprite_rig/src/spawn.rs#L40-L64))
3. `spawn_sprite_rigs` fires on `Added<SpriteRig>`, reads the rig asset, spawns the bone hierarchy as child entities ([sprite_rig/src/spawn.rs:67-102](crates/sprite_rig/src/spawn.rs#L67-L102))

The key pattern: the **component** (`CharacterType`) triggers a system that reads the **asset** (rig file) and spawns the **visual representation** (bone entity hierarchy).

---

## Architecture Documentation

### Asset Pattern Summary

| Concern | Abilities Pattern | Ability Slots Pattern |
|---------|------------------|----------------------|
| File structure | One file per definition | Single file |
| Extension | `.ability.ron` | `.ability_slots.ron` |
| Native loading | `load_folder("abilities")` | `asset_server.load("default.ability_slots.ron")` |
| WASM loading | Manifest → individual loads | Same as native |
| Aggregation | `AbilityDefs` HashMap resource | `DefaultAbilitySlots` resource |
| Hot-reload | `AssetEvent::Modified` → rebuild resource | `AssetEvent::Modified` → overwrite resource |
| Load-gating | Handle(s) added to `TrackedAssets` | Handle added to `TrackedAssets` |

### Asset Cross-Reference Pattern (Sprite Rig)

```
SpriteAnimSetAsset (.animset.ron)
  ├── rig: "rigs/humanoid.rig.ron"          → asset_server.load at runtime
  ├── locomotion[].clip: "anims/.../X.anim.ron" → asset_server.load at runtime
  └── ability_animations: {"id": "path"}    → asset_server.load at runtime

Resolution: Lazy in Update systems, not during asset loading.
String paths are relative to assets/ root.
```

### Persistence Pattern

```
Server: entities with MapSaveTarget + recognized SavedEntityKind
  → collect_and_save_entities → bincode serialize → entities.bin per map
  → load_entities at map spawn → reconstruct ECS entities

Currently limited to RespawnPoint (position only).
```

---

## Code References

- [crates/protocol/src/ability.rs:180-198](crates/protocol/src/ability.rs#L180-L198) — AbilityDef asset type, AbilityDefs resource
- [crates/protocol/src/ability.rs:373-375](crates/protocol/src/ability.rs#L373-L375) — AbilityManifest type
- [crates/protocol/src/ability.rs:406-432](crates/protocol/src/ability.rs#L406-L432) — AbilityPlugin (plugin registration, system scheduling)
- [crates/protocol/src/ability.rs:435-600](crates/protocol/src/ability.rs#L435-L600) — Load, insert, reload systems (native + WASM)
- [crates/protocol/src/ability.rs:602-605](crates/protocol/src/ability.rs#L602-L605) — ID extraction from asset path
- [crates/protocol/src/ability.rs:630-667](crates/protocol/src/ability.rs#L630-L667) — DefaultAbilitySlots load + sync
- [crates/protocol/src/app_state.rs](crates/protocol/src/app_state.rs) — TrackedAssets, AppState, load gate
- [crates/protocol/src/map.rs:167-182](crates/protocol/src/map.rs#L167-L182) — MapSaveTarget, SavedEntity, SavedEntityKind
- [crates/protocol/src/lib.rs:86-88](crates/protocol/src/lib.rs#L86-L88) — RespawnPoint component
- [crates/server/src/persistence.rs](crates/server/src/persistence.rs) — MapMeta, entity save/load to disk
- [crates/server/src/gameplay.rs:64-80](crates/server/src/gameplay.rs#L64-L80) — Dummy target (only non-respawn world content)
- [crates/sprite_rig/src/asset.rs](crates/sprite_rig/src/asset.rs) — SpriteRigAsset, SpriteAnimAsset, SpriteAnimSetAsset
- [crates/sprite_rig/src/spawn.rs:40-64](crates/sprite_rig/src/spawn.rs#L40-L64) — resolve_character_rig (CharacterType → rig)
- [crates/sprite_rig/src/lib.rs:21-55](crates/sprite_rig/src/lib.rs#L21-L55) — SpriteRigPlugin registration
- [crates/voxel_map_engine/src/config.rs:9](crates/voxel_map_engine/src/config.rs#L9) — VoxelGenerator type
- [crates/voxel_map_engine/src/meshing.rs:66-76](crates/voxel_map_engine/src/meshing.rs#L66-L76) — flat_terrain_voxels
- [assets/abilities.manifest.ron](assets/abilities.manifest.ron) — Manifest file example
- [assets/abilities/fireball.ability.ron](assets/abilities/fireball.ability.ron) — Individual ability RON example
- [assets/default.ability_slots.ron](assets/default.ability_slots.ron) — Single-file asset RON example
- [assets/rigs/humanoid.rig.ron](assets/rigs/humanoid.rig.ron) — Sprite rig RON with cross-references
- [assets/anims/humanoid/humanoid.animset.ron](assets/anims/humanoid/humanoid.animset.ron) — Animset with path references

## Related Research

- [2026-02-07-ability-system-architecture.md](2026-02-07-ability-system-architecture.md) — Ability system design and implementation
- [2026-02-25-ability-slots-hot-reload-asset.md](2026-02-25-ability-slots-hot-reload-asset.md) — Hot-reloadable AbilitySlots asset pattern
- [2026-03-12-streaming-ron-assets-to-web-clients.md](2026-03-12-streaming-ron-assets-to-web-clients.md) — WASM asset loading approaches
- [2026-03-13-vox-loading-without-scenes.md](2026-03-13-vox-loading-without-scenes.md) — dot_vox + block-mesh-rs pipeline
- [2026-03-09-minecraft-style-map-directory-saving.md](2026-03-09-minecraft-style-map-directory-saving.md) — Map persistence

## Resolved Questions

### 1. Reflect Component Insertion — Custom AssetLoader with TypedReflectDeserializer

Use Bevy's reflection system with a custom `AssetLoader` that deserializes the component map directly from RON at load time — no intermediate representation (`ron::Value`, `String`, etc.) needed.

**Approach**: A custom `AssetLoader` implements `FromWorld` to grab `AppTypeRegistry`, then uses `TypedReflectDeserializer` (which implements `DeserializeSeed`) inside a custom `Visitor::visit_map` to deserialize each component value based on the type path key. This is the same approach Bevy's `SceneLoader` / `SceneMapDeserializer` uses for `.scn.ron` files.

**Cannot use `RonAssetPlugin`** — it uses plain `serde::Deserialize` with no `TypeRegistry` access.

**Requirements for every component type used in RON definitions**:
- `#[derive(Component, Reflect, Default)]`
- `#[reflect(Component, Default)]`
- `app.register_type::<T>()` in plugin setup

**Precedent**: Bevy's `DynamicScene`, `bevy_proto`, and Blenvy all use this exact pattern — components keyed by type path string, deserialized via `TypeRegistry`.

See [Resolved Question 9](#8-component-map-deserialization--custom-assetloader-with-typedreflectdeserializer) for full implementation details.

### 2. Object Placement in Procgen — Deferred

Out of scope for now. Focus is on defining, loading, and spawning world objects. Procgen placement will be a separate system built on top.

### 3. Scope — Unified `WorldObjectDef`

**Use a single unified `WorldObjectDef` type** with a `category` field. This is the consensus pattern across the Bevy ecosystem (bevy_proto, Blenvy, DynamicScene all use flat bags of components, not typed hierarchies).

**Trade-offs researched**:

| | Unified `WorldObjectDef` | Separate `TreeDef`, `OreDef`, `NpcDef`... |
|---|---|---|
| Asset infrastructure | Single loader, manifest, registry, spawn system | One of each per type — N types = N× boilerplate |
| Adding categories | Add enum variant | New type + loader + registry + spawn system |
| ECS alignment | Entities differentiated by components (correct) | Semantics encoded in definition type (fights ECS) |
| Type safety | Runtime validation of required components | Compile-time enforcement of required fields |
| Cross-category objects | Natural (chest that's also harvestable) | Must exist in multiple definitions |
| Ecosystem precedent | All major blueprint systems use this | None found |

**Structure** (see [Resolved Question 9](#8-component-map-deserialization--custom-assetloader-with-typedreflectdeserializer) for full details):
```rust
#[derive(Asset, TypePath)]
pub struct WorldObjectDef {
    pub category: ObjectCategory,
    pub visual: VisualKind,
    pub collider: Option<ColliderConstructor>,
    /// Deserialized at load time via TypedReflectDeserializer.
    pub components: Vec<Box<dyn PartialReflect>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ObjectCategory { Scenery, Interactive, ResourceNode, Item, Npc }
```

Category-specific invariants (e.g., "NPCs must have a BehaviorTree") enforced via validation pass at load time, not the type system.

### 4. Networking — Both Server and Client Load

World object definitions are loaded by both server and client, following the ability system pattern. Server needs the data for physics, collision, and gameplay logic. Client needs it for rendering and visual asset resolution.

### 5. Visual Asset Variants — Enum

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum VisualKind {
    Vox(String),        // e.g., "models/trees/tree_circle.vox"
    SpriteRig(String),  // e.g., "rigs/humanoid.rig.ron"
    Sprite(String),     // e.g., "sprites/tree.png"
    None,               // server-only or invisible objects
}
```

Visual assets are resolved lazily at spawn time via `asset_server.load`, following the sprite rig cross-reference pattern.

## Resolved Questions (Follow-up)

### 6. Collision Definition — Use `ColliderConstructor`

**`ColliderConstructor`** from avian3d is the ideal type. It is already:
- A `Component` with `Reflect`
- `Serialize`/`Deserialize` (enabled via the `serialize` feature, which `protocol` already enables)
- `#[reflect(Component, Debug, PartialEq, Default)]`
- An enum with all standard shapes plus mesh-based constructors

**Relevant variants for world objects**:

| Variant | Use Case |
|---|---|
| `Sphere { radius }` | Simple round objects |
| `Cuboid { x_length, y_length, z_length }` | Boxes, walls, buildings |
| `Capsule { radius, height }` | NPCs, characters |
| `Cylinder { radius, height }` | Tree trunks, pillars |
| `TrimeshFromMesh` | Exact collision from visual mesh |
| `ConvexHullFromMesh` | Simplified convex collision from mesh |
| `ConvexDecompositionFromMesh` | Complex concave objects |
| `Compound(Vec<(Position, Rotation, ColliderConstructor)>)` | Multi-part colliders |

**How it works**: When `ColliderConstructor` is inserted as a component on an entity, avian3d automatically generates the `Collider` from it and removes the constructor component. The `*FromMesh` variants require a `Mesh3d` on the same entity.

**Usage in `WorldObjectDef`**: Since `ColliderConstructor` is already `Serialize + Deserialize`, it can be a direct field — no need for a custom `CollisionDef`. See [Resolved Question 9](#8-component-map-deserialization--custom-assetloader-with-typedreflectdeserializer) for the full struct and RON example.

**Note**: The `*FromMesh` variants (e.g., `TrimeshFromMesh`) require the mesh to be loaded first. Since .vox meshes are generated at asset load time, these variants would work if the mesh handle is inserted before `ColliderConstructor`. For server-side (no rendering), explicit shapes (`Cuboid`, `Cylinder`, etc.) are more appropriate since the server may not load visual meshes.

### 7. Server-Side Colliders — Load .vox for Collider Generation

The server should load .vox files for collider generation even though it doesn't render. The `VoxModelAsset` (from the vox loading pipeline) produces a `Mesh` which can be used with `Collider::trimesh_from_mesh` or the `TrimeshFromMesh` / `ConvexHullFromMesh` constructor variants. Both server and client load the same world object definitions and the same .vox assets.

### 8. Component Map Deserialization — Custom AssetLoader with TypedReflectDeserializer

**`ron::Value` and `String` intermediaries are unnecessary.** A custom `AssetLoader` can deserialize the component map directly from RON using Bevy's `TypedReflectDeserializer`, which implements `DeserializeSeed`. No intermediate representation needed.

**How it works**: Bevy's `SceneMapDeserializer` already implements this exact pattern for `.scn.ron` files. The approach:

1. Implement `FromWorld` on the loader to grab `AppTypeRegistry` (an `Arc<RwLock<TypeRegistry>>` — cheap to clone)
2. In `load()`, create a `ron::de::Deserializer` from bytes
3. Use a custom `DeserializeSeed` with a `Visitor::visit_map` that:
   - Reads each key as a type path string
   - Looks up `TypeRegistration` via `registry.get_with_type_path(type_path)`
   - Calls `map.next_value_seed(TypedReflectDeserializer::new(registration, registry))`
   - Collects results into `Vec<Box<dyn PartialReflect>>`

**Cannot use `RonAssetPlugin`** — it uses plain `serde::Deserialize` with no `TypeRegistry` access. A custom `AssetLoader` is required (same approach Bevy's own `SceneLoader` uses).

**Reference implementation**: `SceneMapDeserializer` in `bevy_scene::serde` ([source](https://github.com/bevyengine/bevy/blob/main/crates/bevy_scene/src/serde.rs)).

**Custom AssetLoader skeleton**:

```rust
#[derive(Asset, TypePath)]
pub struct WorldObjectDef {
    pub category: ObjectCategory,
    pub visual: VisualKind,
    pub collider: Option<ColliderConstructor>,
    pub components: Vec<Box<dyn PartialReflect>>,
}

struct WorldObjectLoader {
    type_registry: TypeRegistryArc,
}

impl FromWorld for WorldObjectLoader {
    fn from_world(world: &mut World) -> Self {
        Self {
            type_registry: world.resource::<AppTypeRegistry>().0.clone(),
        }
    }
}

impl AssetLoader for WorldObjectLoader {
    type Asset = WorldObjectDef;
    type Settings = ();
    type Error = /* ... */;

    fn extensions(&self) -> &[&str] { &["object.ron"] }

    async fn load(&self, reader: &mut dyn Reader, _: &(), _: &mut LoadContext<'_>) -> Result<Self::Asset, _> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let registry = self.type_registry.read();
        // Custom deserialization using TypedReflectDeserializer for the components map
        // while using normal serde for category, visual, collider fields
        // ...
    }
}
```

**Custom Visitor for the components map**:

```rust
struct ComponentMapVisitor<'a> {
    registry: &'a TypeRegistry,
}

impl<'a, 'de> Visitor<'de> for ComponentMapVisitor<'a> {
    type Value = Vec<Box<dyn PartialReflect>>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "a map of component type paths to component data")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where M: MapAccess<'de> {
        let mut components = Vec::new();
        while let Some(type_path) = map.next_key::<String>()? {
            let registration = self.registry
                .get_with_short_type_path(&type_path)
                .or_else(|| self.registry.get_with_type_path(&type_path))
                .ok_or_else(|| de::Error::custom(format!("unregistered type: {type_path}")))?;
            let seed = TypedReflectDeserializer::new(registration, self.registry);
            let value = map.next_value_seed(seed)?;
            components.push(value);
        }
        Ok(components)
    }
}
```

**RON file format** (no quoting, no intermediate representation):

```ron
(
    category: Scenery,
    visual: Vox("models/trees/tree_circle.vox"),
    collider: Some(Cylinder(radius: 0.5, height: 3.0)),
    components: {
        "game::Harvestable": (
            resource: Wood,
            amount: 10,
        ),
        "game::Health": (
            current: 50,
            max: 50,
        ),
    },
)
```

This is the cleanest approach — mirrors `.scn.ron` ergonomics, no `ron::Value` roundtrip issues, no quoted strings, full enum support.

### 9. World Object Replication — Server/Client Spawn Synchronization

#### The Established Pattern in This Codebase

The project already implements the standard pattern for server-authoritative entity spawning with client-side visual attachment. The flow:

1. **Server spawns** entity with `Replicate::to_clients(NetworkTarget::All)` + a marker/type component (e.g., `CharacterType::Humanoid`)
2. **Lightyear replicates** only registered components to clients. Client entity receives `Replicated` marker (and `Predicted`/`Interpolated` if configured)
3. **Client systems** detect `Added<Replicated>` / `Added<Predicted>` and attach client-only components (meshes, sprite rigs, health bars)
4. **Unregistered components** (Mesh3d, materials, SpriteRig) are never sent over the network — they exist only on whichever side inserts them

**Concrete example — CharacterType → SpriteRig pipeline** ([sprite_rig/src/spawn.rs:40-64](crates/sprite_rig/src/spawn.rs#L40-L64)):
- `CharacterType` is replicated (registered with `add_prediction()`)
- Client's `resolve_character_rig` runs on `Added<CharacterType>` with `Or<(With<Predicted>, With<Replicated>, With<Interpolated>)>` filter
- Looks up `RigRegistry` by `CharacterType` → inserts `SpriteRig` + `AnimSetRef` (client-only)
- `spawn_sprite_rigs` fires on `Added<SpriteRig>` → spawns bone hierarchy (client-only)

**Component distribution for characters**:

| Component | Server | Client Confirmed | Client Predicted |
|---|---|---|---|
| `CharacterMarker` | spawned | replicated | predicted (auto-copied) |
| `CharacterType` | spawned | replicated | predicted (auto-copied) |
| `Position`, `Rotation` | spawned | replicated | predicted + rollback |
| `Health` | spawned | replicated | predicted (auto-copied) |
| `CharacterPhysicsBundle` | spawned | — | inserted by client |
| `SpriteRig`, `Facing` | — | inserted by sprite_rig | inserted by sprite_rig |
| `Mesh3d`, materials | — | inserted by render | inserted by render |
| `Replicate`, `PredictionTarget` | spawned | — | — |
| `ControlledBy` | spawned (players only) | — | — |

#### How This Applies to World Objects

World objects follow the same pattern but are simpler — most don't need prediction (trees don't roll back):

**Server spawns**:
```rust
commands.spawn((
    WorldObjectId("oak_tree".into()),   // registered, replicated
    Position(Vec3::new(50.0, 0.0, 30.0)),
    Rotation::default(),
    MapInstanceId::Overworld,
    Replicate::to_clients(NetworkTarget::All),
    // No PredictionTarget — static objects don't need prediction
    // ColliderConstructor inserted from WorldObjectDef
));
```

**Client reacts** (observer on `Added<Replicated>` or `Added<WorldObjectId>`):
```rust
fn on_world_object_replicated(
    query: Query<(Entity, &WorldObjectId), Added<Replicated>>,
    world_defs: Res<WorldObjectDefs>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
) {
    for (entity, obj_id) in &query {
        let def = world_defs.get(obj_id);
        // Attach visual (client-only)
        match &def.visual {
            VisualKind::Vox(path) => {
                let handle = asset_server.load::<VoxModelAsset>(path);
                commands.entity(entity).insert(VoxModelHandle(handle));
            }
            VisualKind::SpriteRig(path) => { /* ... */ }
            _ => {}
        }
        // Attach reflected components from def (shared — physics, gameplay)
        for component in &def.components {
            commands.entity(entity).insert_reflect(component.clone_value());
        }
    }
}
```

**Key difference from characters**: World objects don't need `PredictionTarget` (no client-side prediction). They appear as interpolated-only entities. The `WorldObjectId` component is the equivalent of `CharacterType` — the replicated type identifier that clients use to look up the visual definition.

#### Shared vs Side-Specific Components

| Component Source | Where Inserted | Replicated? |
|---|---|---|
| `WorldObjectId` | Server spawn | Yes (registered) |
| `Position`, `Rotation` | Server spawn | Yes (registered) |
| `MapInstanceId` | Server spawn | Yes (registered) |
| `ColliderConstructor` | Server spawn (from def) | No — both sides insert from def |
| Reflected gameplay components | Both sides from def | Depends on registration |
| `Mesh3d`, materials, `SpriteRig` | Client only | No |
| `Replicate` | Server only | N/A |

#### Static Objects Without Prediction

For world objects that never move (trees, buildings, ores), omit `PredictionTarget`. Lightyear will replicate the entity but clients won't create a predicted copy — the entity exists only as a confirmed entity with `Replicated` marker. This is simpler and cheaper.

For world objects that can change state (e.g., ore being mined, door opening), register the state component with `add_prediction()` if client-side prediction is desired, or just `register_component()` for server-authoritative-only state.

#### Interest Management for Large Worlds

The project already uses lightyear `Room`s for map-based visibility ([server/src/map.rs:323-339](crates/server/src/map.rs#L323-L339)). World objects on a map should be added to the same room as their map entity. This ensures clients only receive world objects for maps they're currently on.

For within-map spatial interest management (only replicate nearby trees), lightyear's `NetworkVisibility` with distance-based filtering would be needed. This is not yet implemented.

### 10. How Other Games Handle This

#### Unity DOTS Netcode — Ghost Spawning

Unity uses a "Ghost" system with classification ([docs](https://docs.unity3d.com/Packages/com.unity.netcode@1.0/manual/ghost-spawning.html)):
- Server registers **ghost prefabs** with `GhostAuthoringComponent`
- Each ghost has a **ghost type** (effectively a prefab ID) derived from its component composition
- Client receives unknown ghost type → spawns the matching prefab locally
- Classification: **Interpolated** (trees, buildings — delayed, no prediction) or **Predicted** (player characters)
- Server and client must agree on available prefabs at compile time
- **Pre-spawned ghosts** in subscenes use deterministic position-hash IDs

This is the closest analog to the `WorldObjectId` → `WorldObjectDef` pattern.

#### Unreal Engine — Actor Class as Prefab ID

Unreal's approach ([wiki](https://unrealcommunity.wiki/replication-vyrv8r37)):
- Server spawns a dynamic actor; the **UClass** (actor class) is serialized alongside location/rotation
- Client receives the class reference and instantiates the same class — meshes, materials, collision are all part of the class definition
- Only **replicated properties** (marked with `UPROPERTY(Replicated)`) traverse the network
- Static level actors are referenced by path name — no spawn message needed
- Visual components exist on both sides automatically because they're part of the class

The UClass is equivalent to our `WorldObjectId` + `WorldObjectDef` lookup.

#### Bevy Replicon — Observer Pattern

[Bevy Replicon](https://docs.rs/bevy_replicon/latest/bevy_replicon/) uses the same observer pattern ([blog](https://www.hankruiger.com/posts/adding-networked-multiplayer-to-my-game-with-bevy-replicon/)):
```rust
app.observe(on_world_object_added);

fn on_world_object_added(
    trigger: Trigger<OnAdd, WorldObject>,
    query: Query<&WorldObject>,
    mut commands: Commands,
) {
    let entity = trigger.target();
    let obj_type = query.get(entity).unwrap();
    commands.entity(entity).insert(visual_bundle_for(obj_type));
}
```

#### Entity Factory Pattern (Bevy + Lightyear)

A Lightyear user documents an `EntityFactory` trait pattern ([blog](https://vladbat00.github.io/blog/000-spawning-entities/)):
- `insert_shared_components()` — runs on both server and client (gameplay data)
- `insert_client_components()` — client-only (meshes, materials, animations)
- `insert_components()` — orchestrates both paths

#### Cross-Engine Consensus

All engines converge on the same architecture:

| Concept | Unity DOTS | Unreal | Bevy+Lightyear |
|---|---|---|---|
| Type identifier | Ghost type (prefab hash) | UClass | Replicated marker component |
| Prefab definition | Ghost prefab | Actor class | Asset (RON) + marker lookup |
| Client visual attachment | Ghost classification system | Automatic (class-based) | Observer on `Added<Marker>` |
| Non-replicated components | `[GhostField]` opt-in | `UPROPERTY(Replicated)` opt-in | `register_component` opt-in |
| Static vs predicted | Interpolated vs Predicted ghost | Simulated proxy vs autonomous proxy | No `PredictionTarget` vs with |

### External References

- [Lightyear GitHub](https://github.com/cBournhonesque/lightyear)
- [Lightyear Book - Examples](https://cbournhonesque.github.io/lightyear/book/examples/title.html)
- [Vlad's Blog - Entities, Components and Multiplayer](https://vladbat00.github.io/blog/000-spawning-entities/)
- [Unity Netcode Ghost Spawning](https://docs.unity3d.com/Packages/com.unity.netcode@1.0/manual/ghost-spawning.html)
- [Unreal Community Wiki - Replication](https://unrealcommunity.wiki/replication-vyrv8r37)
- [Bevy Replicon docs](https://docs.rs/bevy_replicon/latest/bevy_replicon/)
- [Han Ruiger - Adding Multiplayer with Bevy Replicon](https://www.hankruiger.com/posts/adding-networked-multiplayer-to-my-game-with-bevy-replicon/)

## Open Questions

None remaining. All design questions have been resolved.
