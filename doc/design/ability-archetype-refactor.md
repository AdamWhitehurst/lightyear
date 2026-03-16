# Refactoring AbilityDef to Archetype-from-Asset

Migrate the current `AbilityDef` / `AbilityEffect` / `EffectTrigger` enum-based system to a component-composition model where each ability is a RON
file containing a set of reflected ECS components. Effects become small, orthogonal components; triggers become component wrappers with tick/phase
metadata. The existing `ActiveAbility` phase-machine, `AbilityDefs` registry, and lightyear replication stay mostly unchanged.

---

## 1. Current Design (what we're replacing)

```
AbilityDef (monolithic)
├── startup_ticks, active_ticks, recovery_ticks, cooldown_ticks
└── effects: Vec<EffectTrigger>
    ├── EffectTrigger::OnTick { tick, effect: AbilityEffect }
    ├── EffectTrigger::WhileActive(AbilityEffect)
    ├── EffectTrigger::OnHit(AbilityEffect)
    ├── EffectTrigger::OnEnd(AbilityEffect)
    └── EffectTrigger::OnInput { action, effect: AbilityEffect }

AbilityEffect (big enum)
├── Melee { id, target }
├── Projectile { id, speed, lifetime_ticks }
├── SetVelocity { speed, target }
├── Damage { amount, target }
├── ApplyForce { force, frame, target }
├── AreaOfEffect { id, target, radius, duration_ticks }
├── Ability { id, target }
├── Teleport { distance }
├── Shield { absorb }
└── Buff { stat, multiplier, duration_ticks, target }
```

### Problems

- Adding a new effect requires modifying `AbilityEffect`, every match arm in `apply_on_tick_effects`, `apply_on_end_effects`,
  `apply_while_active_effects`, etc.
- `EffectTrigger` nests `AbilityEffect` — can't query for "all entities with a Damage effect" without walking the enum.
- The RON files are expressive but the Rust side is a single monolithic dispatcher.
- Non-ability entities (items, hazards, buffs) can't reuse the same effect primitives.

---

## 2. Target Design

Each RON file declares a flat map of reflected components. The "phase timing" information (`startup_ticks`, etc.) becomes a component. Each effect
becomes its own component type. Triggers become wrapper components that hold a Vec of effect descriptors with tick metadata.

```
ability RON file (component bundle)
├── "AbilityPhases": (startup: 2, active: 4, recovery: 3, cooldown: 30)
├── "OnTickEffects": ([ ... ])    ← replaces EffectTrigger::OnTick
├── "WhileActiveEffects": ([ ... ]) ← replaces EffectTrigger::WhileActive
├── "OnHitEffects": ([ ... ])     ← replaces EffectTrigger::OnHit
├── "OnEndEffects": ([ ... ])     ← replaces EffectTrigger::OnEnd
└── "OnInputEffects": ([ ... ])   ← replaces EffectTrigger::OnInput
```

The runtime spawn system reads the archetype asset and inserts these as real ECS components on the `ActiveAbility` entity — the existing dispatch
systems (`dispatch_effect_markers`, `apply_on_tick_effects`, etc.) then work against concrete component queries instead of walking enum vecs.

---

## 3. Component Definitions

### 3a. Phase Timing (replaces AbilityDef's scalar fields)

```rust
/// Tick-based phase durations and cooldown. Loaded from RON.
#[derive(Component, Clone, Debug, Reflect, Serialize, Deserialize, Default)]
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

### 3b. Effect Primitives (replaces AbilityEffect enum variants)

Each former `AbilityEffect` variant becomes its own reflected struct. These are **not** `Component` — they're data items inside trigger Vecs.

```rust
/// Specifies who receives an effect.
#[derive(Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
pub enum EffectTarget {
    #[default]
    Caster,
    Victim,
    OriginalCaster,
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct MeleeEffect {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub target: EffectTarget,
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct ProjectileEffect {
    #[serde(default)]
    pub id: Option<String>,
    pub speed: f32,
    pub lifetime_ticks: u16,
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct SetVelocityEffect {
    pub speed: f32,
    pub target: EffectTarget,
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct DamageEffect {
    pub amount: f32,
    pub target: EffectTarget,
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct ApplyForceEffect {
    pub force: Vec3,
    #[serde(default)]
    pub frame: ForceFrame,
    pub target: EffectTarget,
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct AreaOfEffectEffect {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub target: EffectTarget,
    pub radius: f32,
    #[serde(default)]
    pub duration_ticks: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct SubAbilityEffect {
    pub id: String,
    pub target: EffectTarget,
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct TeleportEffect {
    pub distance: f32,
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct ShieldEffect {
    pub absorb: f32,
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct BuffEffect {
    pub stat: String,
    pub multiplier: f32,
    pub duration_ticks: u16,
    pub target: EffectTarget,
}
```

> **Key decision:** These stay as an enum (`AbilityEffect`) for now. The reflect-based archetype loader doesn't require decomposing the enum — it only
> needs `Reflect + Serialize + Deserialize` on the outer component types. Decomposing into individual structs is a future step if you 
> want fully open-ended extensibility (new effect types without modifying the enum). For the initial refactor, keeping`AbilityEffect` as-is is fine.

### 3c. Trigger Components (replaces EffectTrigger enum + dispatch_effect_markers)

These are real `Component`s that live on the `ActiveAbility` entity. They replace both the `EffectTrigger` enum and the current one-shot marker
components (`OnTickEffects`, `WhileActiveEffects`, etc.) — the same type now serves as both the asset-loaded definition and the runtime marker.

```rust
/// Fires on specific Active-phase tick offsets. Multiple entries can share
/// or differ in tick offset for sequencing.
#[derive(Component, Clone, Debug, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct OnTickEffects(pub Vec<TickEffect>);

#[derive(Clone, Debug, Reflect, Serialize, Deserialize)]
pub struct TickEffect {
    /// Active-phase tick offset (0-indexed). Defaults to 0.
    #[serde(default)]
    pub tick: u16,
    pub effect: AbilityEffect,
}

/// Fires every tick during Active phase.
#[derive(Component, Clone, Debug, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct WhileActiveEffects(pub Vec<AbilityEffect>);

/// Fires when a hitbox/projectile hits a target.
#[derive(Component, Clone, Debug, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct OnHitEffects(pub Vec<AbilityEffect>);

/// Fires once when Active → Recovery transition occurs.
#[derive(Component, Clone, Debug, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct OnEndEffects(pub Vec<AbilityEffect>);

/// Fires during Active phase when the specified input is just-pressed.
#[derive(Component, Clone, Debug, Reflect, Serialize, Deserialize, Default)]
#[reflect(Component, Serialize, Deserialize)]
pub struct OnInputEffects(pub Vec<InputEffect>);

#[derive(Clone, Debug, Reflect, Serialize, Deserialize)]
pub struct InputEffect {
    pub action: PlayerActions,
    pub effect: AbilityEffect,
}
```

---

## 4. RON File Format

### 4a. Before (current AbilityDef RON via bevy_common_assets)

```ron
// dash.ability.ron
(
    startup_ticks: 2,
    active_ticks: 6,
    recovery_ticks: 8,
    cooldown_ticks: 30,
    effects: [
        OnTick(tick: 0, effect: Melee()),
        WhileActive(SetVelocity(speed: 25.0, target: Caster)),
        OnEnd(SetVelocity(speed: 0.0, target: Caster)),
    ],
)
```

### 4b. After (archetype-from-asset RON)

```ron
// dash.ability.ron
{
    "AbilityPhases": (startup: 2, active: 6, recovery: 8, cooldown: 30),
    "OnTickEffects": ([
        (tick: 0, effect: Melee()),
    ]),
    "WhileActiveEffects": ([
        SetVelocity(speed: 25.0, target: Caster),
    ]),
    "OnEndEffects": ([
        SetVelocity(speed: 0.0, target: Caster),
    ]),
}
```

### 4c. More examples

```ron
// fireball.ability.ron
{
    "AbilityPhases": (startup: 3, active: 1, recovery: 5, cooldown: 45),
    "OnTickEffects": ([
        (tick: 0, effect: Projectile(speed: 40.0, lifetime_ticks: 120)),
    ]),
    "OnHitEffects": ([
        Damage(amount: 25.0, target: Victim),
        ApplyForce(force: (0.0, 5.0, 15.0), frame: RelativePosition, target: Victim),
    ]),
}
```

```ron
// combo_slash.ability.ron — multi-tick sequencing
{
    "AbilityPhases": (startup: 1, active: 8, recovery: 4, cooldown: 20),
    "OnTickEffects": ([
        (tick: 0, effect: Melee()),
        (tick: 4, effect: Melee()),
        (tick: 4, effect: AreaOfEffect(radius: 5.0, duration_ticks: Some(2))),
    ]),
    "OnHitEffects": ([
        Damage(amount: 15.0, target: Victim),
    ]),
    "OnInputEffects": ([
        (action: Ability1, effect: Ability(id: "combo_finisher", target: Caster)),
    ]),
}
```

```ron
// shield_bash.ability.ron — sub-ability + buff + shield
{
    "AbilityPhases": (startup: 4, active: 2, recovery: 6, cooldown: 60),
    "OnTickEffects": ([
        (tick: 0, effect: Shield(absorb: 50.0)),
        (tick: 0, effect: Buff(stat: "speed", multiplier: 1.5, duration_ticks: 30, target: Caster)),
        (tick: 0, effect: Melee()),
    ]),
    "OnHitEffects": ([
        Damage(amount: 10.0, target: Victim),
        ApplyForce(force: (0.0, 8.0, 20.0), frame: RelativePosition, target: Victim),
        Ability(id: "stun_debuff", target: Victim),
    ]),
}
```

---

## 5. The Asset and Loader

### 5a. Asset definition

```rust
use bevy::prelude::*;
use bevy::reflect::PartialReflect;

/// A bundle of reflected components loaded from a RON file.
/// Replaces the old `AbilityDef` asset.
#[derive(Asset, TypePath)]
pub struct AbilityAsset {
    pub components: Vec<Box<dyn PartialReflect>>,
}
```

### 5b. Loader with TypeRegistry access

```rust
use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext};
use bevy::prelude::*;
use bevy::reflect::serde::TypedReflectDeserializer;
use bevy::reflect::TypeRegistry;
use serde::de::{self, DeserializeSeed, MapAccess, Visitor};
use std::sync::{Arc, RwLock};

pub struct AbilityAssetLoader {
    registry: Arc<RwLock<TypeRegistry>>,
}

/// Use FromWorld so `init_asset_loader` can construct it after type registration.
impl FromWorld for AbilityAssetLoader {
    fn from_world(world: &mut World) -> Self {
        let registry = world.resource::<AppTypeRegistry>().0.clone();
        Self { registry }
    }
}

impl AssetLoader for AbilityAssetLoader {
    type Asset = AbilityAsset;
    type Settings = ();
    type Error = Box<dyn std::error::Error + Send + Sync>;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        let registry = self.registry.read().unwrap();
        let mut deserializer = ron::de::Deserializer::from_bytes(&bytes)?;
        let components = deserializer.deserialize_map(AbilityVisitor {
            registry: &registry,
        })?;

        Ok(AbilityAsset { components })
    }

    fn extensions(&self) -> &[&str] {
        &["ability.ron"]
    }
}
```

### 5c. The Visitor

```rust
struct AbilityVisitor<'a> {
    registry: &'a TypeRegistry,
}

impl<'a, 'de> Visitor<'de> for AbilityVisitor<'a> {
    type Value = Vec<Box<dyn PartialReflect>>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "a map of component type names to component data")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut components = Vec::new();

        while let Some(type_name) = map.next_key::<String>()? {
            let registration = self
                .registry
                .get_with_short_type_path(&type_name)
                .or_else(|| self.registry.get_with_type_path(&type_name))
                .ok_or_else(|| {
                    de::Error::custom(format!(
                        "Type '{}' not found in registry. Did you call register_type?",
                        type_name
                    ))
                })?;

            let seed = TypedReflectDeserializer::new(registration, self.registry);
            let component = map.next_value_seed(seed)?;
            components.push(component);
        }

        Ok(components)
    }
}
```

---

## 6. AbilityDefs Registry (minimal changes)

The `AbilityDefs` resource changes from `HashMap<AbilityId, AbilityDef>` to `HashMap<AbilityId, Handle<AbilityAsset>>`. Systems that need to read
ability data now go through `Assets<AbilityAsset>` + the type registry.

```rust
#[derive(Resource, Clone, Debug, Default)]
pub struct AbilityDefs {
    pub abilities: HashMap<AbilityId, Handle<AbilityAsset>>,
}
```

However, to avoid reflection overhead on every tick in hot systems like `update_active_abilities` and `dispatch_effect_markers`, we extract the
`AbilityPhases` component eagerly into a parallel lookup:

```rust
/// Pre-extracted phase data for fast lookup without reflection.
#[derive(Resource, Clone, Debug, Default)]
pub struct AbilityPhaseLookup {
    pub phases: HashMap<AbilityId, AbilityPhases>,
}
```

Populated during `insert_ability_defs` / `reload_ability_defs` by reading the `AbilityPhases` component out of each loaded `AbilityAsset`.

---

## 7. Spawning ActiveAbility Entities

### 7a. What changes

Currently `ability_activation` spawns an `ActiveAbility` component and later systems read `AbilityDefs` to walk the `effects` vec. In the new design,
activation also applies the archetype's components onto the `ActiveAbility` entity via reflection.

### 7b. The spawn flow

```
ability_activation (or spawn_sub_ability)
  │
  ├── spawn entity with ActiveAbility { def_id, caster, phase, ... }
  │
  └── insert ApplyAbilityArchetype(ability_id) marker
          │
          ▼
apply_ability_archetypes (new system, runs in PreUpdate after activation)
  │
  ├── look up Handle<AbilityAsset> from AbilityDefs
  ├── get AbilityAsset from Assets<AbilityAsset>
  ├── for each reflected component in asset.components:
  │     ├── get TypeId → TypeRegistration → ReflectComponent
  │     └── reflect_component.apply_or_insert(entity, ...)
  └── remove ApplyAbilityArchetype marker
```

```rust
/// Marker: the entity needs its archetype components applied.
#[derive(Component)]
pub struct ApplyAbilityArchetype(pub AbilityId);

fn apply_ability_archetypes(
    mut commands: Commands,
    query: Query<(Entity, &ApplyAbilityArchetype)>,
    ability_defs: Res<AbilityDefs>,
    assets: Res<Assets<AbilityAsset>>,
    registry: Res<AppTypeRegistry>,
) {
    let registry = registry.read();

    for (entity, marker) in &query {
        let Some(handle) = ability_defs.abilities.get(&marker.0) else {
            warn!("Ability {:?} not found in defs", marker.0);
            continue;
        };
        let Some(asset) = assets.get(handle) else {
            continue; // Not loaded yet — will retry next frame
        };

        for reflected in &asset.components {
            let Some(type_info) = reflected.get_represented_type_info() else {
                continue;
            };
            let type_id = type_info.type_id();
            let Some(registration) = registry.get(type_id) else {
                continue;
            };
            let Some(reflect_component) = registration.data::<ReflectComponent>() else {
                warn!("Type {} missing #[reflect(Component)]", type_info.type_path());
                continue;
            };

            reflect_component.apply_or_insert(
                &mut commands.entity(entity),
                reflected.as_ref(),
                &registry,
            );
        }

        commands.entity(entity).remove::<ApplyAbilityArchetype>();
    }
}
```

### 7c. Updated ability_activation (sketch)

```rust
pub fn ability_activation(
    mut commands: Commands,
    ability_phases: Res<AbilityPhaseLookup>,
    // ... same params as before ...
) {
    // ... cooldown check uses ability_phases.phases.get(&ability_id) ...

    commands.spawn((
        ActiveAbility { def_id: ability_id.clone(), /* ... */ },
        ApplyAbilityArchetype(ability_id.clone()),
        PreSpawned::default_with_salt(salt),
        Name::new("ActiveAbility"),
    ));
}
```

---

## 8. Impact on Existing Systems

### Systems that need NO changes

These systems already work against the trigger-component types (`OnTickEffects`, `WhileActiveEffects`, etc.) via queries. Since the new design uses
the _same component types_ (just populated via reflection instead of `dispatch_effect_markers`), the apply systems are untouched:

- `apply_on_tick_effects` — queries `Query<&OnTickEffects, &ActiveAbility>`
- `apply_while_active_effects` — queries `Query<&WhileActiveEffects, &ActiveAbility>`
- `apply_on_end_effects` — queries `Query<&OnEndEffects, &ActiveAbility>`
- `apply_on_input_effects` — queries `Query<&OnInputEffects, &ActiveAbility>`
- `ability_projectile_spawn`, `handle_ability_projectile_spawn`
- `aoe_hitbox_lifetime`, `ability_bullet_lifetime`
- `expire_buffs`

### Systems that change

| System                              | Change                                                                                                                                                                                                                          |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `dispatch_effect_markers`           | **Simplified or removed.** The archetype loader already inserts `OnTickEffects`, etc. as components. Phase-gated filtering (only fire `OnTick` entries matching current tick offset) moves into `apply_on_tick_effects` itself. |
| `update_active_abilities`           | Reads `AbilityPhases` component from the entity (or `AbilityPhaseLookup` resource) instead of `AbilityDefs.get(&id)`.                                                                                                           |
| `ability_activation`                | Adds `ApplyAbilityArchetype` marker. Cooldown check reads `AbilityPhaseLookup`.                                                                                                                                                 |
| `spawn_sub_ability`                 | Same changes as `ability_activation`.                                                                                                                                                                                           |
| `insert_ability_defs`               | Builds `AbilityDefs` with `Handle<AbilityAsset>` instead of cloned `AbilityDef`. Also populates `AbilityPhaseLookup`.                                                                                                           |
| `reload_ability_defs`               | Same — rebuilds handles + phase lookup.                                                                                                                                                                                         |
| `cleanup_effect_markers_on_removal` | Unchanged — still removes the same component types.                                                                                                                                                                             |

### dispatch_effect_markers simplification

Currently this system re-reads the `AbilityDef` every tick and inserts/removes marker components based on phase. With the archetype approach, the
trigger components are present from spawn. The system simplifies to:

```rust
/// Filter OnTickEffects to only the entries matching the current tick offset.
/// Remove trigger components when leaving Active phase.
fn filter_tick_effects(
    mut commands: Commands,
    timeline: Res<LocalTimeline>,
    query: Query<(Entity, &ActiveAbility, &OnTickEffects)>,
) {
    let tick = timeline.tick();
    for (entity, active, on_tick) in &query {
        if active.phase != AbilityPhase::Active {
            // Trigger components stay on the entity but systems skip non-Active phases.
            // Alternatively, remove them here for clarity.
            continue;
        }
        let offset = (tick - active.phase_start_tick) as u16;
        let matching: Vec<AbilityEffect> = on_tick.0.iter()
            .filter(|t| t.tick == offset)
            .map(|t| t.effect.clone())
            .collect();
        if !matching.is_empty() {
            commands.entity(entity).insert(CurrentTickEffects(matching));
        }
    }
}

/// One-shot component: effects that matched THIS tick. Consumed by apply_on_tick_effects.
#[derive(Component)]
pub struct CurrentTickEffects(pub Vec<AbilityEffect>);
```

This replaces the tick-filtering logic that was inside `dispatch_active_phase_markers`. The `WhileActiveEffects`, `OnHitEffects`, and `OnInputEffects`
need no per-tick filtering — they're always-on during Active phase and the apply systems already handle phase gating.

---

## 9. Type Registration

```rust
impl Plugin for AbilityPlugin {
    fn build(&self, app: &mut App) {
        // Register ALL reflected types FIRST
        app.register_type::<AbilityPhases>()
           .register_type::<OnTickEffects>()
           .register_type::<TickEffect>()
           .register_type::<WhileActiveEffects>()
           .register_type::<OnHitEffects>()
           .register_type::<OnEndEffects>()
           .register_type::<OnInputEffects>()
           .register_type::<InputEffect>()
           .register_type::<AbilityEffect>()
           .register_type::<EffectTarget>()
           .register_type::<ForceFrame>()
           // ... any nested types used in AbilityEffect variants ...
           ;

        // THEN init the loader (uses FromWorld → reads registry)
        app.init_asset::<AbilityAsset>()
           .init_asset_loader::<AbilityAssetLoader>();

        // Resources
        app.init_resource::<AbilityDefs>()
           .init_resource::<AbilityPhaseLookup>();

        // Systems
        app.add_systems(Startup, load_ability_defs)
           .add_systems(PreUpdate, apply_ability_archetypes)
           .add_systems(Update, (
               insert_ability_defs,
               reload_ability_defs,
               filter_tick_effects,
               // ... existing apply systems unchanged ...
           ));
    }
}
```

> **Critical:** `register_type` calls must come before `init_asset_loader`. Bevy calls `FromWorld` during `init_asset_loader`, which clones the
> `Arc<RwLock<TypeRegistry>>`. Types registered after that point will still be visible (they share the Arc), but it's cleaner to register everything
> upfront.

---

## 10. Loading Pipeline (native + WASM)

The existing `load_folder` (native) / manifest (WASM) pattern stays the same, but the asset type changes from `AbilityDef` to `AbilityAsset`. The
`bevy_common_assets` `RonAssetPlugin` is replaced by the custom `AbilityAssetLoader`.

```
Startup
  ├── [native]  load_folder("abilities") → Handle<LoadedFolder>
  └── [wasm]    load manifest → individual loads

Update (insert_ability_defs)
  ├── iterate loaded AbilityAsset handles
  ├── extract AbilityId from filename (unchanged)
  ├── store Handle<AbilityAsset> in AbilityDefs
  └── extract AbilityPhases from each asset → AbilityPhaseLookup

Update (reload_ability_defs)
  └── on AssetEvent<AbilityAsset>::Modified → rebuild AbilityDefs + AbilityPhaseLookup
```

### Extracting AbilityPhases from an AbilityAsset

```rust
fn extract_phases(asset: &AbilityAsset, registry: &TypeRegistry) -> Option<AbilityPhases> {
    let phases_type_id = std::any::TypeId::of::<AbilityPhases>();
    for reflected in &asset.components {
        let Some(info) = reflected.get_represented_type_info() else { continue };
        if info.type_id() == phases_type_id {
            // Convert PartialReflect → concrete type
            let registration = registry.get(phases_type_id)?;
            let reflect_from = registration.data::<ReflectFromReflect>()?;
            let concrete = reflect_from.from_reflect(reflected.as_ref())?;
            return concrete.downcast::<AbilityPhases>().ok().map(|b| *b);
        }
    }
    None
}
```

---

## 11. Migration Checklist

### Phase 1: Define new types alongside old

- [ ] Create `AbilityPhases` component
- [ ] Create `TickEffect`, `InputEffect` structs
- [ ] Refactor `OnTickEffects` to hold `Vec<TickEffect>` (with tick offset)
- [ ] Refactor `OnHitEffects` to hold `Vec<AbilityEffect>` (remove caster/depth — those live on `ActiveAbility`)
- [ ] Refactor `OnInputEffects` to hold `Vec<InputEffect>`
- [ ] Add `Reflect`, `Serialize`, `Deserialize` derives to all trigger components
- [ ] Add `#[reflect(Component, Serialize, Deserialize)]` to all trigger components
- [ ] Register all types

### Phase 2: Asset loader

- [ ] Define `AbilityAsset`
- [ ] Implement `AbilityAssetLoader` with `FromWorld`
- [ ] Implement `AbilityVisitor`
- [ ] Replace `RonAssetPlugin::<AbilityDef>` with custom loader
- [ ] Convert RON files from `( ... )` struct format to `{ "Type": (data) }` map format
- [ ] Update `AbilityDefs` to hold `Handle<AbilityAsset>`
- [ ] Create `AbilityPhaseLookup`, populate during loading

### Phase 3: Spawn pipeline

- [ ] Create `ApplyAbilityArchetype` component
- [ ] Implement `apply_ability_archetypes` system
- [ ] Update `ability_activation` to add `ApplyAbilityArchetype`
- [ ] Update `spawn_sub_ability` to add `ApplyAbilityArchetype`

### Phase 4: Simplify dispatch

- [ ] Simplify or remove `dispatch_effect_markers` — archetype already inserts trigger components
- [ ] Add `filter_tick_effects` system for per-tick offset filtering
- [ ] Add `CurrentTickEffects` one-shot component
- [ ] Update `apply_on_tick_effects` to read `CurrentTickEffects` instead of `OnTickEffects`
- [ ] Ensure `apply_while_active_effects`, `apply_on_end_effects`, `apply_on_input_effects` gate on `active.phase == Active`
- [ ] Remove old `AbilityDef`, `EffectTrigger` types
- [ ] Remove `bevy_common_assets` dependency for abilities (keep for `AbilitySlots` if still using it)

### Phase 5: Verify

- [ ] All existing `.ability.ron` files converted to new format
- [ ] Hot reload works (edit RON → see changes in-game)
- [ ] WASM manifest loading works with new asset type
- [ ] Lightyear prediction/rollback still functions (ActiveAbility replication unchanged)
- [ ] Sub-abilities (Ability effect) spawn and resolve correctly
- [ ] Cooldown checking uses `AbilityPhaseLookup`

---

## 12. Future Extensions

Once the archetype loader is in place, these become straightforward:

- **New effect types**: Add a variant to `AbilityEffect`, implement its apply logic — no loader changes needed. Or, go further and decompose
  `AbilityEffect` into individual reflected structs for fully open-ended extensibility.
- **Non-ability archetypes**: Items, hazards, environmental effects can reuse the same loader and trigger components.
- **Custom components per ability**: RON files can include _any_ registered component, not just the trigger types. Want a
  `Homing(target_tracking: 0.8)` component on a projectile ability? Just register the type and add it to the RON file.
- **Designer tooling**: Since the RON format is a flat map of type names, you can build an editor that enumerates registered component types and lets
  designers compose abilities visually.
- **Conditional effects**: Add new trigger types like `OnHealthBelow { threshold: 0.3, effects: [...] }` as components — the archetype loader picks
  them up automatically.
