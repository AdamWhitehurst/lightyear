# Ability Archetype-from-Asset Refactor

## Overview

Migrate the ability system from a monolithic `AbilityDef`/`EffectTrigger` enum-based model to a
component-composition model where each `.ability.ron` file is a flat map of reflected ECS components.
The existing `ActiveAbility` phase machine, system chain, and Lightyear replication stay mostly
unchanged. Effects remain as an `AbilityEffect` enum (decomposition to individual structs is a future
step).

**Design doc**: `doc/design/ability-archetype-refactor.md`
**Research**: `doc/research/2026-03-15-ability-archetype-refactor-analysis.md`

## Current State Analysis

- `AbilityDef` is a monolithic struct with phase timing + `Vec<EffectTrigger>` (`ability.rs:180-187`)
- `EffectTrigger` nests `AbilityEffect` — can't query for "all entities with a Damage effect"
  without walking the enum (`ability.rs:157-177`)
- `dispatch_effect_markers` re-reads `AbilityDefs` every tick, walks the effects vec, and
  inserts/removes marker components (`ability.rs:793-912`)
- 14 `.ability.ron` files use `bevy_common_assets` `RonAssetPlugin<AbilityDef>` for loading
- The `world_object` module already implements the exact reflect-based RON loading pattern we need
  (`world_object/loader.rs`, `world_object/types.rs`, `world_object/spawn.rs`)
- 31 integration tests in `ability_systems.rs` construct `AbilityDef` directly

### Key Discoveries:

- `OnHitEffects` has `caster`, `original_caster`, `depth` fields needed by hit detection — cannot
  simply become `Vec<AbilityEffect>`. Need a separate archetype type and keep the current struct for
  hitbox/projectile entities.
- Apply systems don't check phase (implicit via dispatch) — need explicit phase gating with
  persistent archetype components.
- World object loader uses `TypeRegistrationDeserializer` (full type paths). We should use the same
  approach for consistency — RON files use full type paths like `"protocol::ability::AbilityPhases"`.
- `#![enable(implicit_some)]` works with the custom deserializer (RON extension persists through
  nested visitors).

## Desired End State

Each `.ability.ron` file is a flat `{ "TypePath": (data) }` map of reflected components. At ability
activation, these components are applied inline via `commands.queue` + `ReflectComponent::insert`.
`apply_on_tick_effects` filters `OnTickEffects` directly by tick offset (no intermediate dispatch
component). `OnHitEffects` construction is inlined into `update_active_abilities`. Apply systems gate
on phase explicitly.

### Verification:

- `cargo test-all` — all 31+ existing tests pass, plus new integration test
- `cargo server` + `cargo client` — abilities work in-game (activation, phases, hitboxes,
  projectiles, combos, buffs, shields, teleport)
- Hot reload works: edit a RON file → see changes in-game
- Sub-ability chains work at any depth (up to 4)
- Prediction/rollback unaffected

## What We're NOT Doing

- Decomposing `AbilityEffect` enum into individual reflected structs (future extensibility step)
- Registering trigger components for Lightyear replication (not needed — they're local)
- Changing `ActiveAbility` or its replication/prediction setup
- Modifying hit detection systems (`process_hitbox_hits`, `process_projectile_hits`)
- Supporting both old and new RON formats simultaneously (clean cut-over)

## Implementation Approach

Reuse the existing `world_object` loader infrastructure by extracting the shared
`ComponentMapVisitor`/`ComponentMapDeserializer` to a shared module. Apply archetype components
inline at spawn time (no deferred `ApplyAbilityArchetype` marker) to avoid frame delays with
sub-ability chains. Keep `OnHitEffects` struct unchanged for hitbox entities; introduce
`OnHitEffectDefs` as the archetype component.

No `CurrentTickEffects` intermediate — `apply_on_tick_effects` queries `&OnTickEffects` +
`&ActiveAbility` and filters by `(tick - phase_start_tick) as u16` directly. No
`AbilityPhaseLookup` resource — `update_active_abilities` queries `&AbilityPhases` from the entity
(it's an archetype component); cooldown checks in `ability_activation` use `extract_phases` from the
`AbilityAsset`. This eliminates a duplicate source of truth and the sync concern during hot-reload.

---

## Phase 1: Define New Types and Update Existing Ones

### Overview

Add `AbilityPhases`, `TickEffect`, `InputEffect`, `OnHitEffectDefs`, and `AbilityAsset`. Add
`Reflect`/`Serialize`/`Deserialize` derives to trigger components. Register all types.

### Changes Required:

#### 1. New types in `ability.rs`

**File**: `crates/protocol/src/ability.rs`

Add after the `AbilityDef` definition:

```rust
/// Tick-based phase durations and cooldown. Loaded from RON archetype.
#[derive(Component, Clone, Debug, PartialEq, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct AbilityPhases {
    pub startup: u16,
    pub active: u16,
    pub recovery: u16,
    pub cooldown: u16,
}

impl AbilityPhases {
    pub fn phase_duration(&self, phase: &AbilityPhase) -> u16 {
        match phase {
            AbilityPhase::Startup => self.startup,
            AbilityPhase::Active => self.active,
            AbilityPhase::Recovery => self.recovery,
        }
    }
}
```

#### 2. Update trigger component types

**File**: `crates/protocol/src/ability.rs`

Refactor `OnTickEffects` to hold tick offsets. `apply_on_tick_effects` filters by
`(tick - active.phase_start_tick) as u16` directly — no `CurrentTickEffects` one-shot needed:

```rust
/// Active-phase tick effect with offset metadata.
#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct TickEffect {
    /// Active-phase tick offset (0-indexed). Defaults to 0.
    #[serde(default)]
    pub tick: u16,
    pub effect: AbilityEffect,
}

/// Archetype component: all tick-triggered effects with their offsets.
/// Persists on the ActiveAbility entity for the ability's lifetime.
/// Apply systems filter by current tick offset directly — no intermediate dispatch component.
#[derive(Component, Clone, Debug, PartialEq, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct OnTickEffects(pub Vec<TickEffect>);
```

Refactor `OnInputEffects` to use a struct:

```rust
/// Input-triggered effect with action metadata.
#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct InputEffect {
    pub action: PlayerActions,
    pub effect: AbilityEffect,
}

/// Archetype component: input-triggered effects during Active phase.
#[derive(Component, Clone, Debug, PartialEq, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct OnInputEffects(pub Vec<InputEffect>);
```

Add `OnHitEffectDefs` (archetype component for hit effects, without caster/depth):

```rust
/// Archetype component: effects applied when a hitbox/projectile hits a target.
/// Does NOT contain caster/depth — those come from ActiveAbility at dispatch time.
#[derive(Component, Clone, Debug, PartialEq, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct OnHitEffectDefs(pub Vec<AbilityEffect>);
```

Add derives to existing trigger components:

```rust
/// Archetype component: effects that fire every tick during Active phase.
#[derive(Component, Clone, Debug, PartialEq, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct WhileActiveEffects(pub Vec<AbilityEffect>);

/// Archetype component: effects that fire when Active → Recovery.
#[derive(Component, Clone, Debug, PartialEq, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct OnEndEffects(pub Vec<AbilityEffect>);
```

Keep `OnHitEffects` unchanged (for hitbox/projectile entities):

```rust
/// Runtime component on hitbox/projectile entities. Carries caster context for hit detection.
#[derive(Component, Clone, Debug)]
pub struct OnHitEffects {
    pub effects: Vec<AbilityEffect>,
    pub caster: Entity,
    pub original_caster: Entity,
    pub depth: u8,
}
```

#### 3. AbilityAsset type

**File**: `crates/protocol/src/ability.rs`

```rust
use bevy::reflect::PartialReflect;

/// A bundle of reflected components loaded from a `.ability.ron` file.
/// Replaces `AbilityDef` as the asset type.
#[derive(Asset, TypePath)]
pub struct AbilityAsset {
    pub components: Vec<Box<dyn PartialReflect>>,
}

impl Clone for AbilityAsset {
    fn clone(&self) -> Self {
        Self {
            components: self
                .components
                .iter()
                .map(|c| {
                    c.reflect_clone()
                        .expect("ability component must be cloneable")
                        .into_partial_reflect()
                })
                .collect(),
        }
    }
}

impl fmt::Debug for AbilityAsset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AbilityAsset")
            .field(
                "components",
                &self
                    .components
                    .iter()
                    .map(|c| c.reflect_type_path())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}
```

#### 4. `extract_phases` helper

**File**: `crates/protocol/src/ability.rs`

Extracts `AbilityPhases` from an `AbilityAsset` via reflection. Used by `ability_activation` for
cooldown checks (before the entity exists):

```rust
/// Extract AbilityPhases from an AbilityAsset's reflected components.
fn extract_phases(asset: &AbilityAsset) -> Option<&AbilityPhases> {
    let target_id = std::any::TypeId::of::<AbilityPhases>();
    for reflected in &asset.components {
        let Some(info) = reflected.get_represented_type_info() else { continue };
        if info.type_id() == target_id {
            return reflected.as_any().downcast_ref::<AbilityPhases>();
        }
    }
    None
}
```

#### 5. Type registration

**File**: `crates/protocol/src/ability.rs`, in `AbilityPlugin::build`

Add before the asset loader init:

```rust
app.register_type::<AbilityPhases>()
   .register_type::<OnTickEffects>()
   .register_type::<TickEffect>()
   .register_type::<WhileActiveEffects>()
   .register_type::<OnHitEffectDefs>()
   .register_type::<OnEndEffects>()
   .register_type::<OnInputEffects>()
   .register_type::<InputEffect>()
   .register_type::<AbilityEffect>()
   .register_type::<EffectTarget>()
   .register_type::<ForceFrame>()
   .register_type::<PlayerActions>();
```

### Success Criteria:

#### Automated Verification:
- [x] `cargo check-all` compiles

---

## Phase 2: Asset Loader and Loading Pipeline

### Overview

Extract the shared reflect-based RON deserializer from `world_object/loader.rs`, create
`AbilityAssetLoader`, update `AbilityDefs` to hold handles.

### Changes Required:

#### 1. Extract shared deserializer

**File**: `crates/protocol/src/reflect_loader.rs` (new)

Extract `ComponentMapDeserializer` and `ComponentMapVisitor` from `world_object/loader.rs` into a
shared module. Both `WorldObjectLoader` and `AbilityAssetLoader` will use it.

```rust
use bevy::prelude::*;
use bevy::reflect::serde::{TypeRegistrationDeserializer, TypedReflectDeserializer};
use bevy::reflect::{PartialReflect, ReflectFromReflect, TypeRegistry};
use serde::de::{DeserializeSeed, Deserializer, MapAccess, Visitor};
use std::fmt;

pub struct ComponentMapDeserializer<'a> {
    pub registry: &'a TypeRegistry,
}

impl<'a, 'de> DeserializeSeed<'de> for ComponentMapDeserializer<'a> {
    type Value = Vec<Box<dyn PartialReflect>>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(ComponentMapVisitor {
            registry: self.registry,
        })
    }
}

struct ComponentMapVisitor<'a> {
    registry: &'a TypeRegistry,
}

impl<'a, 'de> Visitor<'de> for ComponentMapVisitor<'a> {
    type Value = Vec<Box<dyn PartialReflect>>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "a map of component type paths to component data")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut components = Vec::new();
        while let Some(registration) =
            map.next_key_seed(TypeRegistrationDeserializer::new(self.registry))?
        {
            let value =
                map.next_value_seed(TypedReflectDeserializer::new(registration, self.registry))?;
            let value = self
                .registry
                .get(registration.type_id())
                .and_then(|tr| tr.data::<ReflectFromReflect>())
                .and_then(|fr| fr.from_reflect(value.as_partial_reflect()))
                .map(PartialReflect::into_partial_reflect)
                .unwrap_or(value);
            components.push(value);
        }
        Ok(components)
    }
}

/// Deserialize a `Vec<Box<dyn PartialReflect>>` from RON bytes using a flat
/// `{ "type::Path": (data) }` map format.
pub fn deserialize_component_map(
    bytes: &[u8],
    registry: &TypeRegistry,
) -> Result<Vec<Box<dyn PartialReflect>>, ReflectLoadError> {
    let mut deserializer = ron::de::Deserializer::from_bytes(bytes)?;
    let components = ComponentMapDeserializer { registry }.deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(components)
}

#[derive(Debug)]
pub enum ReflectLoadError {
    Io(std::io::Error),
    Ron(ron::error::SpannedError),
}

// Display, Error, From impls (same as WorldObjectLoadError)
```

#### 2. Update world_object/loader.rs to use shared module

**File**: `crates/protocol/src/world_object/loader.rs`

Replace inline `ComponentMapDeserializer`/`ComponentMapVisitor` with import from
`crate::reflect_loader`. The `deserialize_world_object` function becomes:

```rust
pub fn deserialize_world_object(
    bytes: &[u8],
    registry: &TypeRegistry,
) -> Result<WorldObjectDef, WorldObjectLoadError> {
    let components = crate::reflect_loader::deserialize_component_map(bytes, registry)?;
    Ok(WorldObjectDef { components })
}
```

Update `WorldObjectLoadError` to impl `From<ReflectLoadError>`.

#### 3. AbilityAssetLoader

**File**: `crates/protocol/src/ability.rs`

```rust
use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext};
use bevy::reflect::TypeRegistryArc;

struct AbilityAssetLoader {
    type_registry: TypeRegistryArc,
}

impl FromWorld for AbilityAssetLoader {
    fn from_world(world: &mut World) -> Self {
        Self {
            type_registry: world.resource::<AppTypeRegistry>().0.clone(),
        }
    }
}

impl AssetLoader for AbilityAssetLoader {
    type Asset = AbilityAsset;
    type Settings = ();
    type Error = crate::reflect_loader::ReflectLoadError;

    fn extensions(&self) -> &[&str] {
        &["ability.ron"]
    }

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let registry = self.type_registry.read();
        let components = crate::reflect_loader::deserialize_component_map(&bytes, &registry)?;
        Ok(AbilityAsset { components })
    }
}
```

#### 4. Update AbilityDefs

**File**: `crates/protocol/src/ability.rs`

Change `AbilityDefs` to hold handles:

```rust
#[derive(Resource, Clone, Debug, Default)]
pub struct AbilityDefs {
    pub abilities: HashMap<AbilityId, Handle<AbilityAsset>>,
}

impl AbilityDefs {
    pub fn get(&self, id: &AbilityId) -> Option<&Handle<AbilityAsset>> {
        self.abilities.get(id)
    }
}
```

#### 5. Update AbilityPlugin

**File**: `crates/protocol/src/ability.rs`, in `AbilityPlugin::build`

Replace `RonAssetPlugin::<AbilityDef>` with:

```rust
app.init_asset::<AbilityAsset>()
   .init_asset_loader::<AbilityAssetLoader>();
```

#### 6. Update native `insert_ability_defs`

**File**: `crates/protocol/src/ability.rs`

`load_ability_defs` remains unchanged (already handles `TrackedAssets` to gate `AppState::Ready`).

Update `insert_ability_defs` to store handles:

```rust
#[cfg(not(target_arch = "wasm32"))]
fn insert_ability_defs(
    mut commands: Commands,
    folder_handle: Option<Res<AbilityFolderHandle>>,
    loaded_folders: Res<Assets<LoadedFolder>>,
    ability_assets: Res<Assets<AbilityAsset>>,
    asset_server: Res<AssetServer>,
    existing: Option<Res<AbilityDefs>>,
) {
    if existing.is_some() {
        trace!("AbilityDefs already inserted");
        return;
    }
    let Some(folder_handle) = folder_handle else {
        trace!("ability folder handle not yet loaded");
        return;
    };
    let Some(folder) = loaded_folders.get(&folder_handle.0) else {
        trace!("ability folder not yet available in Assets<LoadedFolder>");
        return;
    };

    let mut abilities = HashMap::new();
    for handle in &folder.handles {
        let Some(path) = asset_server.get_path(handle.id()) else {
            trace!("asset handle {:?} has no path yet", handle.id());
            continue;
        };
        let Some(name) = path.path().file_name().and_then(|n| n.to_str()) else {
            trace!("skipping asset with non-UTF8 filename");
            continue;
        };
        if !name.ends_with(".ability.ron") { continue }
        let Some(id) = ability_id_from_path(&path) else {
            trace!("could not extract ability id from path: {:?}", path);
            continue;
        };
        let typed = handle.clone().typed::<AbilityAsset>();
        abilities.insert(id, typed);
    }

    info!("Loaded {} ability definitions", abilities.len());
    commands.insert_resource(AbilityDefs { abilities });
}
```

#### 7. Update WASM `trigger_individual_ability_loads`

**File**: `crates/protocol/src/ability.rs`

Change `Handle<AbilityDef>` to `Handle<AbilityAsset>`:

```rust
#[cfg(target_arch = "wasm32")]
fn trigger_individual_ability_loads(
    manifest_handle: Option<Res<AbilityManifestHandle>>,
    manifest_assets: Res<Assets<AbilityManifest>>,
    pending: Option<Res<PendingAbilityHandles>>,
    mut tracked: ResMut<crate::app_state::TrackedAssets>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    if pending.is_some() {
        trace!("PendingAbilityHandles already exists");
        return;
    }
    let Some(manifest_handle) = manifest_handle else {
        trace!("manifest handle not yet loaded");
        return;
    };
    let Some(manifest) = manifest_assets.get(&manifest_handle.0) else {
        trace!("manifest asset not yet available");
        return;
    };
    let handles: Vec<Handle<AbilityAsset>> = manifest
        .0
        .iter()
        .map(|id| {
            let h: Handle<AbilityAsset> = asset_server.load(format!("abilities/{id}.ability.ron"));
            tracked.add(h.clone());
            h
        })
        .collect();
    commands.insert_resource(PendingAbilityHandles(handles));
}
```

Update `PendingAbilityHandles` to hold `Vec<Handle<AbilityAsset>>`.

#### 8. Update WASM `insert_ability_defs`

**File**: `crates/protocol/src/ability.rs`

```rust
#[cfg(target_arch = "wasm32")]
fn insert_ability_defs(
    mut commands: Commands,
    pending: Option<Res<PendingAbilityHandles>>,
    ability_assets: Res<Assets<AbilityAsset>>,
    asset_server: Res<AssetServer>,
    existing: Option<Res<AbilityDefs>>,
) {
    if existing.is_some() {
        trace!("AbilityDefs already inserted");
        return;
    }
    let Some(pending) = pending else {
        trace!("PendingAbilityHandles not yet available");
        return;
    };
    let abilities: HashMap<AbilityId, Handle<AbilityAsset>> = pending
        .0
        .iter()
        .filter_map(|handle| {
            // Verify asset is loaded before including
            ability_assets.get(handle)?;
            let path = asset_server.get_path(handle.id())?;
            let id = ability_id_from_path(&path)?;
            Some((id, handle.clone()))
        })
        .collect();
    if abilities.len() != pending.0.len() {
        trace!("not all ability assets loaded yet ({}/{})", abilities.len(), pending.0.len());
        return;
    }
    info!("Loaded {} ability definitions", abilities.len());
    commands.insert_resource(AbilityDefs { abilities });
}
```

#### 9. Update native `reload_ability_defs`

**File**: `crates/protocol/src/ability.rs`

```rust
#[cfg(not(target_arch = "wasm32"))]
fn reload_ability_defs(
    mut commands: Commands,
    folder_handle: Option<Res<AbilityFolderHandle>>,
    loaded_folders: Res<Assets<LoadedFolder>>,
    ability_assets: Res<Assets<AbilityAsset>>,
    asset_server: Res<AssetServer>,
    mut events: MessageReader<AssetEvent<AbilityAsset>>,
) {
    let Some(folder_handle) = folder_handle else {
        events.clear();
        return;
    };
    let has_changes = events
        .read()
        .any(|e| matches!(e, AssetEvent::Modified { .. }));
    if !has_changes {
        return;
    }
    let Some(folder) = loaded_folders.get(&folder_handle.0) else {
        warn!("ability assets changed but LoadedFolder not available");
        return;
    };

    let mut abilities = HashMap::new();
    for handle in &folder.handles {
        let Some(path) = asset_server.get_path(handle.id()) else { continue };
        let Some(name) = path.path().file_name().and_then(|n| n.to_str()) else { continue };
        if !name.ends_with(".ability.ron") { continue }
        let Some(id) = ability_id_from_path(&path) else { continue };
        let typed = handle.clone().typed::<AbilityAsset>();
        abilities.insert(id, typed);
    }

    info!("Hot-reloaded {} ability definitions", abilities.len());
    commands.insert_resource(AbilityDefs { abilities });
}
```

#### 10. Update WASM `reload_ability_defs`

**File**: `crates/protocol/src/ability.rs`

```rust
#[cfg(target_arch = "wasm32")]
fn reload_ability_defs(
    mut commands: Commands,
    pending: Option<Res<PendingAbilityHandles>>,
    ability_assets: Res<Assets<AbilityAsset>>,
    asset_server: Res<AssetServer>,
    mut events: MessageReader<AssetEvent<AbilityAsset>>,
) {
    let Some(pending) = pending else {
        events.clear();
        return;
    };
    let has_changes = events
        .read()
        .any(|e| matches!(e, AssetEvent::Modified { .. }));
    if !has_changes {
        return;
    }
    let abilities: HashMap<AbilityId, Handle<AbilityAsset>> = pending
        .0
        .iter()
        .filter_map(|handle| {
            ability_assets.get(handle)?;
            let path = asset_server.get_path(handle.id())?;
            let id = ability_id_from_path(&path)?;
            Some((id, handle.clone()))
        })
        .collect();
    if abilities.len() != pending.0.len() {
        trace!("not all ability assets loaded yet for reload ({}/{})", abilities.len(), pending.0.len());
        return;
    }
    info!("Hot-reloaded {} ability definitions", abilities.len());
    commands.insert_resource(AbilityDefs { abilities });
}
```

#### 11. Update `lib.rs` module declaration

**File**: `crates/protocol/src/lib.rs`

Add `pub mod reflect_loader;` and remove the `bevy_common_assets` dependency for abilities (keep for
`AbilitySlots` and `AbilityManifest` if still used).

### Success Criteria:

#### Automated Verification:
- [x] `cargo check-all` compiles
- [x] Existing world_object loader tests pass: `cargo test -p protocol world_object`
- [x] Deserialization smoke test passes (add a `#[test]` in `reflect_loader.rs` or
  `ability_systems.rs` that deserializes a minimal RON `{ "protocol::ability::AbilityPhases":
  (startup: 1, active: 2, recovery: 3, cooldown: 4) }` and verifies the result — confirms newtype
  tuple struct syntax `([...])` works with `TypedReflectDeserializer`)

---

## Phase 3: Convert RON Files

### Overview

Convert all 14 `.ability.ron` files from the struct `(...)` format to the map `{ "Type": (data) }`
format. Update `AbilityManifest` for WASM if needed.

### Changes Required:

#### 1. RON format conversion

Each file changes from:
```ron
#![enable(implicit_some)]
(
    startup_ticks: 4,
    active_ticks: 20,
    recovery_ticks: 0,
    cooldown_ticks: 16,
    effects: [
        OnTick(effect: Melee()),
        OnHit(Damage(amount: 5.0, target: Victim)),
        OnHit(ApplyForce(force: (0.0, 0.9, 2.85), frame: RelativePosition, target: Victim)),
        OnInput(action: Ability1, effect: Ability(id: "punch2", target: Caster)),
    ],
)
```

To:
```ron
#![enable(implicit_some)]
{
    "protocol::ability::AbilityPhases": (startup: 4, active: 20, recovery: 0, cooldown: 16),
    "protocol::ability::OnTickEffects": ([(tick: 0, effect: Melee())]),
    "protocol::ability::OnHitEffectDefs": ([
        Damage(amount: 5.0, target: Victim),
        ApplyForce(force: (0.0, 0.9, 2.85), frame: RelativePosition, target: Victim),
    ]),
    "protocol::ability::OnInputEffects": ([
        (action: Ability1, effect: Ability(id: "punch2", target: Caster)),
    ]),
}
```

#### 2. Full conversion list

All 14 files in `assets/abilities/`:
- `barrier.ability.ron`
- `blink_strike.ability.ron`
- `dash.ability.ron`
- `dive_kick.ability.ron`
- `fireball.ability.ron`
- `ground_pound.ability.ron`
- `punch.ability.ron`
- `punch2.ability.ron`
- `punch3.ability.ron`
- `shield_bash.ability.ron`
- `shockwave.ability.ron`
- `speed_burst.ability.ron`
- `teleport_burst.ability.ron`
- `uppercut.ability.ron`

Each conversion follows the same mechanical pattern: extract phase timing into `AbilityPhases`,
group effects by trigger type into the corresponding component.

### Success Criteria:

#### Automated Verification:
- [x] `cargo check-all` compiles
- [x] `cargo server` starts without errors (assets load)

#### Manual Verification:
- [ ] Activate at least one ability in-game (`cargo server` + `cargo client`) to confirm RON
  deserialization, archetype application, and effect dispatch all work end-to-end (assets may load
  lazily or fail silently at activation time — server startup alone is insufficient)

---

## Phase 4: Update Spawn, Dispatch, and Apply Systems

### Overview

Apply archetype components inline at spawn time. Inline `OnHitEffects` construction into
`update_active_abilities` (no separate dispatch system). Add phase gating and direct tick-offset
filtering to apply systems.

### Changes Required:

#### 1. Inline archetype application helper

**File**: `crates/protocol/src/ability.rs`

Uses the same `commands.queue` + `ReflectComponent::insert` pattern as `world_object/spawn.rs`:

```rust
/// Insert all reflected components from an AbilityAsset onto an entity.
fn apply_ability_archetype(
    commands: &mut Commands,
    entity: Entity,
    asset: &AbilityAsset,
    registry: TypeRegistryArc,
) {
    let components: Vec<Box<dyn PartialReflect>> = asset
        .components
        .iter()
        .map(|c| {
            c.reflect_clone()
                .expect("ability component must be cloneable")
                .into_partial_reflect()
        })
        .collect();

    commands.queue(move |world: &mut World| {
        let registry = registry.read();
        let mut entity_mut = world.entity_mut(entity);
        for component in &components {
            let type_path = component.reflect_type_path();
            let Some(registration) = registry.get_with_type_path(type_path) else {
                warn!("Ability component type not registered: {type_path}");
                continue;
            };
            let Some(reflect_component) = registration.data::<ReflectComponent>() else {
                warn!("Type missing #[reflect(Component)]: {type_path}");
                continue;
            };
            reflect_component.insert(&mut entity_mut, component.as_ref(), &registry);
        }
    });
}
```

#### 2. Update `ability_activation`

**File**: `crates/protocol/src/ability.rs`

Add `Assets<AbilityAsset>` and `Res<AppTypeRegistry>` parameters. Use `extract_phases` from the
asset for cooldown check. Use `let entity_id = commands.spawn(...).id()` to avoid borrow checker
issues (the `EntityCommands` temporary drops immediately):

```rust
pub fn ability_activation(
    mut commands: Commands,
    ability_defs: Res<AbilityDefs>,
    ability_assets: Res<Assets<AbilityAsset>>,
    registry: Res<AppTypeRegistry>,
    default_slots: Res<DefaultAbilitySlots>,
    timeline: Res<LocalTimeline>,
    mut query: Query<(
        Entity,
        &ActionState<PlayerActions>,
        Option<&AbilitySlots>,
        &mut AbilityCooldowns,
        &PlayerId,
    )>,
    server_query: Query<&ControlledBy>,
) {
    let tick = timeline.tick();

    for (entity, action_state, slots_opt, mut cooldowns, player_id) in &mut query {
        let slots = slots_opt.unwrap_or(&default_slots.0);
        for (slot_idx, action) in ABILITY_ACTIONS.iter().enumerate() {
            if !action_state.just_pressed(action) { continue }
            let Some(ref ability_id) = slots.0[slot_idx] else { continue };
            let Some(handle) = ability_defs.get(ability_id) else {
                warn!("Ability {:?} not found in defs", ability_id);
                continue;
            };
            let Some(asset) = ability_assets.get(handle) else {
                warn!("Ability {:?} asset not loaded", ability_id);
                continue;
            };
            let Some(phases) = extract_phases(asset) else {
                warn!("Ability {:?} missing AbilityPhases component", ability_id);
                continue;
            };
            if cooldowns.is_on_cooldown(slot_idx, tick, phases.cooldown) { continue }

            cooldowns.last_used[slot_idx] = Some(tick);
            let salt = (player_id.0.to_bits()) << 32 | (slot_idx as u64) << 16 | 0u64;

            let entity_id = commands.spawn((
                ActiveAbility {
                    def_id: ability_id.clone(),
                    caster: entity,
                    original_caster: entity,
                    target: entity,
                    phase: AbilityPhase::Startup,
                    phase_start_tick: tick,
                    ability_slot: slot_idx as u8,
                    depth: 0,
                },
                PreSpawned::default_with_salt(salt),
                Name::new("ActiveAbility"),
            )).id();

            // Apply archetype components inline
            apply_ability_archetype(
                &mut commands,
                entity_id,
                asset,
                registry.0.clone(),
            );

            if let Ok(controlled_by) = server_query.get(entity) {
                commands.entity(entity_id).insert((
                    Replicate::to_clients(NetworkTarget::All),
                    PredictionTarget::to_clients(NetworkTarget::All),
                    *controlled_by,
                ));
            }
        }
    }
}
```

#### 3. Update `spawn_sub_ability`

**File**: `crates/protocol/src/ability.rs`

Add `ability_assets: &Assets<AbilityAsset>` and `registry: &TypeRegistryArc` parameters. Use
`let entity_id = commands.spawn(...).id()` to avoid borrow checker issues:

```rust
pub(crate) fn spawn_sub_ability(
    commands: &mut Commands,
    ability_defs: &AbilityDefs,
    ability_assets: &Assets<AbilityAsset>,
    registry: &TypeRegistryArc,
    id: &str,
    target_entity: Entity,
    original_caster: Entity,
    parent_slot: u8,
    parent_depth: u8,
    tick: Tick,
    server_query: &Query<&ControlledBy>,
    player_id_query: &Query<&PlayerId>,
) {
    // ... existing depth check and validation ...

    let ability_id = AbilityId(id.to_string());
    let Some(handle) = ability_defs.get(&ability_id) else {
        warn!("Sub-ability {:?} not found in defs", id);
        return;
    };

    // ... existing PlayerId check, salt computation ...

    let entity_id = commands.spawn((
        // ... ActiveAbility, PreSpawned, Name ...
    )).id();

    // Apply archetype components inline
    if let Some(asset) = ability_assets.get(handle) {
        apply_ability_archetype(commands, entity_id, asset, registry.clone());
    }

    // ... existing replication setup using commands.entity(entity_id) ...
}
```

#### 4. Update `update_active_abilities`

**File**: `crates/protocol/src/ability.rs`

Query `&AbilityPhases` directly from the entity (it's an archetype component). Inline
`OnHitEffects` construction on first Active tick:

```rust
pub fn update_active_abilities(
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    mut query: Query<(
        Entity,
        &mut ActiveAbility,
        &AbilityPhases,
        Option<&OnHitEffectDefs>,
    )>,
) {
    let tick = timeline.tick();
    for (entity, mut active, phases, on_hit_defs) in &mut query {
        let prev_phase = active.phase;
        advance_ability_phase(&mut commands, entity, &mut active, phases, tick);

        // Construct OnHitEffects on first Active tick
        if active.phase == AbilityPhase::Active && prev_phase != AbilityPhase::Active {
            if let Some(defs) = on_hit_defs {
                if !defs.0.is_empty() {
                    commands.entity(entity).insert(OnHitEffects {
                        effects: defs.0.clone(),
                        caster: active.caster,
                        original_caster: active.original_caster,
                        depth: active.depth,
                    });
                }
            }
        }

        // Clean up OnHitEffects when leaving Active phase
        if active.phase != AbilityPhase::Active && prev_phase == AbilityPhase::Active {
            commands.entity(entity).remove::<OnHitEffects>();
        }
    }
}
```

Update `advance_ability_phase` to take `&AbilityPhases` instead of `&AbilityDef`.

#### 5. Remove `dispatch_effect_markers` system

**File**: `crates/protocol/src/ability.rs`

Delete `dispatch_effect_markers` entirely. Its responsibilities are now handled by:
- `OnHitEffects` construction: inlined into `update_active_abilities` (§4)
- `OnTickEffects` filtering: done directly in `apply_on_tick_effects` (§6)
- Marker cleanup: handled by `update_active_abilities` and `cleanup_effect_markers_on_removal`

Remove from system ordering in `lib.rs`.

#### 6. Update `apply_on_tick_effects`

**File**: `crates/protocol/src/ability.rs`

Filter `OnTickEffects` by tick offset directly — no intermediate `CurrentTickEffects`:

```rust
pub fn apply_on_tick_effects(
    mut commands: Commands,
    ability_defs: Res<AbilityDefs>,
    ability_assets: Res<Assets<AbilityAsset>>,
    registry: Res<AppTypeRegistry>,
    timeline: Res<LocalTimeline>,
    server_query: Query<&ControlledBy>,
    player_id_query: Query<&PlayerId>,
    query: Query<(
        Entity,
        &OnTickEffects,
        &ActiveAbility,
        Option<&OnHitEffects>,
    )>,
    mut caster_query: Query<(&mut Position, &Rotation, &MapInstanceId)>,
) {
    let tick = timeline.tick();

    for (entity, on_tick, active, on_hit) in &query {
        if active.phase != AbilityPhase::Active { continue }

        let active_offset = (tick - active.phase_start_tick) as u16;
        for tick_effect in &on_tick.0 {
            if tick_effect.tick != active_offset { continue }
            match &tick_effect.effect {
                // ... same match logic as before, using tick_effect.effect ...
                // spawn_sub_ability calls gain ability_assets.as_ref() and &registry.0 params
            }
        }
    }
}
```

#### 7. Add phase gating to apply systems

**File**: `crates/protocol/src/ability.rs`

`apply_while_active_effects` — add phase check:

```rust
for (effects, active) in &query {
    if active.phase != AbilityPhase::Active { continue }
    // ... existing logic ...
}
```

`apply_on_end_effects` — check Recovery phase and first tick:

```rust
for (entity, effects, active) in &query {
    if active.phase != AbilityPhase::Recovery || active.phase_start_tick != tick { continue }
    // ... existing logic ...
    // DO NOT remove OnEndEffects (it's a persistent archetype component)
}
```

Remove the `commands.entity(entity).remove::<OnEndEffects>()` line — the component is archetype
data and persists.

`apply_on_input_effects` — add phase check, update tuple access for `InputEffect`:

```rust
for (_entity, effects, active) in &query {
    if active.phase != AbilityPhase::Active { continue }
    for input_effect in &effects.0 {
        if !action_state.just_pressed(&input_effect.action) { continue }
        match &input_effect.effect {
            // ... existing logic ...
        }
    }
}
```

#### 8. Update apply systems that call `spawn_sub_ability`

All apply systems that call `spawn_sub_ability` (`apply_on_tick_effects`, `apply_on_end_effects`,
`apply_on_input_effects`) gain `Res<Assets<AbilityAsset>>` and `Res<AppTypeRegistry>` parameters.
Pass `ability_assets.as_ref()` and `&registry.0` to `spawn_sub_ability`.

In `hit_detection.rs`, the call chain is `process_hitbox_hits` / `process_projectile_hits` →
`apply_on_hit_effects` → `spawn_sub_ability`. All three function signatures need updating:

- `process_hitbox_hits`: add `ability_assets: Res<Assets<AbilityAsset>>`,
  `registry: Res<AppTypeRegistry>` system params; pass to `apply_on_hit_effects`
- `process_projectile_hits`: same additions; pass to `apply_on_hit_effects`
- `apply_on_hit_effects`: add `ability_assets: &Assets<AbilityAsset>`,
  `registry: &TypeRegistryArc` params; pass to `spawn_sub_ability`

#### 9. Update `cleanup_effect_markers_on_removal`

**File**: `crates/protocol/src/ability.rs`

Add `OnHitEffectDefs` to the cleanup list:

```rust
pub fn cleanup_effect_markers_on_removal(
    trigger: On<Remove, ActiveAbility>,
    mut commands: Commands,
) {
    if let Ok(mut cmd) = commands.get_entity(trigger.entity) {
        cmd.try_remove::<OnTickEffects>();
        cmd.try_remove::<WhileActiveEffects>();
        cmd.try_remove::<OnHitEffects>();
        cmd.try_remove::<OnHitEffectDefs>();
        cmd.try_remove::<OnEndEffects>();
        cmd.try_remove::<OnInputEffects>();
        cmd.try_remove::<ProjectileSpawnEffect>();
        cmd.try_remove::<AbilityPhases>();
    }
}
```

#### 10. Update system ordering

**File**: `crates/protocol/src/lib.rs`

Remove `dispatch_effect_markers` from the system chain. The chain becomes:

```rust
(
    ability::ability_activation,
    ability::update_active_abilities,
    ability::apply_on_tick_effects,
    ability::apply_while_active_effects,
    ability::apply_on_end_effects,
    ability::apply_on_input_effects,
    ability::ability_projectile_spawn,
)
    .chain(),
```

### Success Criteria:

#### Automated Verification:
- [x] `cargo check-all` compiles
- [x] `cargo server` starts, loads abilities, no errors in log

#### Manual Verification:
- [ ] Abilities activate and progress through phases in-game
- [ ] Melee hitboxes spawn and deal damage
- [ ] Projectiles fire and hit targets
- [ ] Dash moves the character (WhileActive SetVelocity)
- [ ] Combo chaining works (OnInput → sub-ability)
- [ ] Shields, buffs, teleport work
- [ ] Hot reload: edit a RON file → see changes

---

## Phase 5: Tests and Integration Test

### Overview

Migrate existing test helpers to work with the new types using native builders (no intermediate
bridge). Add an end-to-end integration test that loads an `AbilityAsset` from RON bytes, applies it
to an entity, and verifies the components.

### Changes Required:

#### 1. Native test builder

**File**: `crates/protocol/tests/ability_systems.rs`

Build `AbilityAsset` directly — no `def_to_asset` bridge, no two-phase migration. `AbilityDef` and
`EffectTrigger` are removed in this phase (or Phase 6 at latest):

```rust
/// Build an AbilityAsset and AbilityPhases from explicit effect lists.
fn test_ability(
    phases: AbilityPhases,
    on_tick: Vec<TickEffect>,
    while_active: Vec<AbilityEffect>,
    on_hit: Vec<AbilityEffect>,
    on_end: Vec<AbilityEffect>,
    on_input: Vec<InputEffect>,
) -> (AbilityPhases, AbilityAsset) {
    let mut components: Vec<Box<dyn PartialReflect>> = vec![
        Box::new(phases.clone()),
    ];
    if !on_tick.is_empty() { components.push(Box::new(OnTickEffects(on_tick))); }
    if !while_active.is_empty() { components.push(Box::new(WhileActiveEffects(while_active))); }
    if !on_hit.is_empty() { components.push(Box::new(OnHitEffectDefs(on_hit))); }
    if !on_end.is_empty() { components.push(Box::new(OnEndEffects(on_end))); }
    if !on_input.is_empty() { components.push(Box::new(OnInputEffects(on_input))); }
    (phases, AbilityAsset { components })
}
```

#### 2. Rewrite `test_defs()` using `test_ability`

Convert every entry in `test_defs()` from `AbilityDef` + `EffectTrigger` to `test_ability(...)`.
Returns `Vec<(AbilityId, AbilityPhases, AbilityAsset)>`.

#### 3. Update `test_app()`

```rust
fn test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.init_resource::<ComponentRegistry>();
    app.world_mut().register_component::<Server>();
    app.world_mut().register_component::<PreSpawnedReceiver>();

    // Register reflect types (required for archetype application)
    app.register_type::<AbilityPhases>()
       .register_type::<OnTickEffects>()
       .register_type::<TickEffect>()
       .register_type::<WhileActiveEffects>()
       .register_type::<OnHitEffectDefs>()
       .register_type::<OnEndEffects>()
       .register_type::<OnInputEffects>()
       .register_type::<InputEffect>()
       .register_type::<AbilityEffect>()
       .register_type::<EffectTarget>()
       .register_type::<ForceFrame>()
       .register_type::<PlayerActions>();

    let defs = test_defs();
    let mut ability_handles = HashMap::new();
    let mut assets = Assets::<AbilityAsset>::default();

    for (id, _phases, asset) in defs {
        let handle = assets.add(asset);
        ability_handles.insert(id, handle);
    }

    app.insert_resource(AbilityDefs { abilities: ability_handles });
    app.insert_resource(assets);
    app.insert_resource(DefaultAbilitySlots::default());

    app.add_systems(
        Update,
        (
            ability::ability_activation,
            ability::update_active_abilities,
            ability::apply_on_tick_effects,
            ability::apply_while_active_effects,
            ability::apply_on_end_effects,
            ability::apply_on_input_effects,
            ability::ability_projectile_spawn,
        )
            .chain(),
    );
    app.add_systems(Update, ability::ability_bullet_lifetime);
    app
}
```

#### 4. Update `insert_test_ability` helper

```rust
fn insert_test_ability(app: &mut App, id: &str, phases: AbilityPhases, asset: AbilityAsset) {
    let handle = app.world_mut().resource_mut::<Assets<AbilityAsset>>().add(asset);
    let ability_id = AbilityId(id.to_string());
    app.world_mut()
        .resource_mut::<AbilityDefs>()
        .abilities
        .insert(ability_id, handle);
}
```

#### 5. New integration test: end-to-end archetype loading

**File**: `crates/protocol/tests/ability_systems.rs`

```rust
#[test]
fn archetype_loads_from_ron_bytes() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.register_type::<AbilityPhases>()
       .register_type::<OnTickEffects>()
       .register_type::<TickEffect>()
       .register_type::<WhileActiveEffects>()
       .register_type::<OnHitEffectDefs>()
       .register_type::<OnEndEffects>()
       .register_type::<OnInputEffects>()
       .register_type::<InputEffect>()
       .register_type::<AbilityEffect>()
       .register_type::<EffectTarget>()
       .register_type::<ForceFrame>();

    let registry = app.world().resource::<AppTypeRegistry>().0.read();

    let ron_bytes = br#"
    {
        "protocol::ability::AbilityPhases": (startup: 4, active: 20, recovery: 0, cooldown: 16),
        "protocol::ability::OnTickEffects": ([(tick: 0, effect: Melee())]),
        "protocol::ability::OnHitEffectDefs": ([
            Damage(amount: 5.0, target: Victim),
        ]),
        "protocol::ability::WhileActiveEffects": ([
            SetVelocity(speed: 15.0, target: Caster),
        ]),
    }
    "#;

    let components =
        protocol::reflect_loader::deserialize_component_map(ron_bytes, &registry).unwrap();
    drop(registry);

    // Verify we got 4 components
    assert_eq!(components.len(), 4);

    // Apply to an entity and verify
    let entity = app.world_mut().spawn_empty().id();
    let registry_arc = app.world().resource::<AppTypeRegistry>().0.clone();
    {
        let registry = registry_arc.read();
        let mut entity_mut = app.world_mut().entity_mut(entity);
        for component in &components {
            let type_path = component.reflect_type_path();
            let registration = registry.get_with_type_path(type_path).unwrap();
            let reflect_component = registration.data::<ReflectComponent>().unwrap();
            reflect_component.insert(&mut entity_mut, component.as_ref(), &registry);
        }
    }

    // Check AbilityPhases
    let phases = app.world().get::<AbilityPhases>(entity).unwrap();
    assert_eq!(phases.startup, 4);
    assert_eq!(phases.active, 20);
    assert_eq!(phases.recovery, 0);
    assert_eq!(phases.cooldown, 16);

    // Check OnTickEffects
    let on_tick = app.world().get::<OnTickEffects>(entity).unwrap();
    assert_eq!(on_tick.0.len(), 1);
    assert_eq!(on_tick.0[0].tick, 0);

    // Check WhileActiveEffects
    let while_active = app.world().get::<WhileActiveEffects>(entity).unwrap();
    assert_eq!(while_active.0.len(), 1);

    // Check OnHitEffectDefs
    let on_hit = app.world().get::<OnHitEffectDefs>(entity).unwrap();
    assert_eq!(on_hit.0.len(), 1);
}
```

#### 6. New integration test: full ability lifecycle with archetype

```rust
#[test]
fn archetype_ability_full_lifecycle() {
    let mut app = test_app();
    let world = app.world_mut();
    insert_timeline(world, 0);
    let character = spawn_character(world);

    // Tick 0: press ability
    world
        .get_mut::<ActionState<PlayerActions>>(character)
        .unwrap()
        .press(&PlayerActions::Ability1);
    app.update();

    // Should have spawned ActiveAbility with archetype components
    let (ability_entity, active) = find_active_ability(app.world_mut()).unwrap();
    assert_eq!(active.phase, AbilityPhase::Startup);

    // Verify archetype components are present
    assert!(app.world().get::<OnTickEffects>(ability_entity).is_some());
    assert!(app.world().get::<OnHitEffectDefs>(ability_entity).is_some());
    assert!(app.world().get::<OnInputEffects>(ability_entity).is_some());

    // Advance through startup to active phase
    for _ in 0..4 {
        advance_timeline(app.world_mut(), 1);
        app.update();
    }

    let (_, active) = find_active_ability(app.world_mut()).unwrap();
    assert_eq!(active.phase, AbilityPhase::Active);

    // MeleeHitbox should have spawned on first Active tick
    let hitbox_count = app
        .world_mut()
        .query::<&MeleeHitbox>()
        .iter(app.world())
        .count();
    assert_eq!(hitbox_count, 1, "melee hitbox should spawn on first Active tick");
}
```

### Success Criteria:

#### Automated Verification:
- [ ] All tests pass: `cargo test-all`
- [ ] New integration tests pass

---

## Phase 6: Cleanup

### Overview

Remove `AbilityDef`, `EffectTrigger`, `bevy_common_assets` usage for abilities, and
`collect_abilities_from_folder`.

### Changes Required:

#### 1. Remove old types

**File**: `crates/protocol/src/ability.rs`

Remove:
- `AbilityDef` struct and its `impl` block
- `EffectTrigger` enum
- `collect_abilities_from_folder` function
- `RonAssetPlugin::<AbilityDef>` from `AbilityPlugin::build` (already replaced in Phase 2)

#### 2. Remove `bevy_common_assets` for abilities

**File**: `crates/protocol/Cargo.toml`

If `bevy_common_assets` is only used for `AbilityDef` and `AbilitySlots`/`AbilityManifest`, keep it
for the latter. If `AbilitySlots` and `AbilityManifest` can also be converted, remove the
dependency entirely.

#### 3. Update README if needed

Check if README documents the `.ability.ron` format and update to reflect the new map format.

### Success Criteria:

#### Automated Verification:
- [ ] All tests pass: `cargo test-all`
- [ ] `cargo check-all` compiles with no dead code warnings for removed types
- [ ] `cargo server` + `cargo client` work

#### Manual Verification:
- [ ] All abilities function in-game
- [ ] Hot reload still works
- [ ] No regression in prediction/rollback

---

## Testing Strategy

### Unit Tests:
- `archetype_loads_from_ron_bytes` — deserialize RON, verify component count and data
- Deserialization smoke test in Phase 2 — verify newtype tuple struct `([...])` syntax

### Integration Tests:
- `archetype_ability_full_lifecycle` — spawn ability, verify archetype components present, advance
  through phases, verify effects fire
- All 31 existing tests adapted to native `test_ability` builders (one migration in Phase 5)

### Manual Testing Steps:
1. `cargo server` + `cargo client` — join, activate each ability type
2. Edit `dash.ability.ron` speed value while running → verify hot reload
3. Verify combo chain (punch → punch2 → punch3) still works
4. Verify fireball projectile spawns and hits targets
5. Verify shield_bash gives shield + deals damage

## Performance Considerations

- Spawn path adds ~5 reflection lookups per ability activation (negligible at dozens/second)
- No separate dispatch system — tick filtering is inline in apply systems
- `update_active_abilities` queries `&AbilityPhases` from entity (zero-cost ECS query vs HashMap)
- Archetype fragmentation: ~10 distinct archetypes with 14 abilities — trivial for Bevy

## Migration Notes

- RON files change format in a single cut-over (Phase 3)
- Tests migrate directly to native `test_ability` builders (Phase 5) — no intermediate bridge
- `AbilityDef`/`EffectTrigger` removed in Phase 6
- `bevy_common_assets` kept for `AbilitySlots`/`AbilityManifest` unless also converted

## References

- Design doc: `doc/design/ability-archetype-refactor.md`
- Research: `doc/research/2026-03-15-ability-archetype-refactor-analysis.md`
- World object loader pattern: `crates/protocol/src/world_object/loader.rs`
- World object spawn pattern: `crates/protocol/src/world_object/spawn.rs`
- Current ability system: `crates/protocol/src/ability.rs`
- Current tests: `crates/protocol/tests/ability_systems.rs`
- System ordering: `crates/protocol/src/lib.rs:311-349`
