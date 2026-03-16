---
date: 2026-03-15T16:51:31-07:00
researcher: Claude
git_commit: d26d64fada86cfb275ca33c569f027154ce52afd
branch: master
repository: bevy-lightyear-template
topic: "Critical analysis of ability-archetype-refactor design"
tags: [research, codebase, ability-system, archetype, reflection, refactor]
status: complete
last_updated: 2026-03-15
last_updated_by: Claude
---

# Research: Critical Analysis of Ability Archetype Refactor

**Date**: 2026-03-15T16:51:31-07:00
**Researcher**: Claude
**Git Commit**: d26d64fada86cfb275ca33c569f027154ce52afd
**Branch**: master
**Repository**: bevy-lightyear-template

## Research Question

Critical evaluation of the ability-archetype-refactor design document. Assess correctness of its assumptions about the current architecture, evaluate API accuracy for Bevy 0.18, analyze pros/cons including expressiveness, 1-frame delays, system integration, constraints, and performance.

## Summary

The design is architecturally sound and solves real problems. However, it contains several incorrect API assumptions for Bevy 0.18, the 1-frame delay concern is avoidable but requires different system scheduling than proposed, and the refactor's scope can be reduced by keeping `AbilityEffect` as an enum (which the doc already acknowledges as a valid path). The biggest wins are queryable trigger data, custom per-ability components, and elimination of the per-tick `AbilityDefs` lookup in dispatch.

---

## Detailed Findings

### 1. Incorrect API Assumptions (Bevy 0.18)

The design doc targets `ReflectComponent::apply_or_insert` ([Section 7b](../design/ability-archetype-refactor.md#L521)). **This method does not exist in Bevy 0.18.**

Available methods on `ReflectComponent` in Bevy 0.18:

| Method | Use case |
|--------|----------|
| `insert(&self, entity: &mut EntityWorldMut, component: &dyn PartialReflect, registry: &TypeRegistry)` | Direct world access, simple insert |
| `apply(&self, entity: impl Into<EntityMut>, component: &dyn PartialReflect)` | Apply to existing component (panics if missing) |
| `apply_or_insert_mapped(...)` | Requires `EntityMapper` + `RelationshipHookMode` — for scene deserialization, not general use |

For deferred insertion via `Commands`, Bevy 0.18 provides `ReflectCommandExt`:

```rust
commands.entity(entity).insert_reflect(boxed_reflect_value);
```

**Impact on the design**: The `apply_ability_archetypes` system must use either:
- **`insert_reflect` on `EntityCommands`** (deferred, works with Commands) — simplest path
- **`ReflectComponent::insert`** with exclusive world access — requires the system to take `&mut World`

The design's use of `commands.entity(entity)` + `reflect_component.apply_or_insert` is not valid. Replace with `commands.entity(entity).insert_reflect(reflected.clone_value())` which requires the component data to be `Box<dyn PartialReflect>` (already the case since `AbilityAsset.components` stores `Vec<Box<dyn PartialReflect>>`).

### 2. The 1-Frame Delay Problem

The design proposes `apply_ability_archetypes` as a separate system in **PreUpdate** ([Section 7b](../design/ability-archetype-refactor.md#L475)), triggered by an `ApplyAbilityArchetype` marker component. This creates frame-delay problems:

- **Primary abilities**: Spawned in FixedUpdate by `ability_activation`, archetype applied next frame in PreUpdate. Masked by `startup_ticks >= 1` but breaks with `startup_ticks == 0`.
- **Sub-abilities**: Spawned by apply systems (`apply_on_tick_effects`, etc.) which run *after* any archetype-application system in the chain. Each link in a chain (A → B → C → D) adds one tick of delay before the final ability has its trigger components.

Moving the system into FixedUpdate (after `ability_activation`) fixes primary abilities but not sub-abilities — they're spawned later in the chain and still miss the archetype system.

**Solution**: Eliminate the `ApplyAbilityArchetype` marker and deferred `apply_ability_archetypes` system entirely. Instead, apply archetype components inline at spawn time — both `ability_activation` and `spawn_sub_ability` call a shared `spawn_ability_with_archetype` function that inserts trigger components via `insert_reflect` as part of entity creation. This makes chain depth irrelevant: every ability has its components from the tick it's spawned. See **Section 10** for full analysis.

### 3. Current Architecture vs Design Doc Claims

The design doc's characterization of the current system ([Section 1](../design/ability-archetype-refactor.md#L9-L31)) is **accurate**:

- `AbilityDef` is a monolithic struct with phase timing + `Vec<EffectTrigger>` ([ability.rs:180-187](../../crates/protocol/src/ability.rs#L180-L187))
- `AbilityEffect` is a big enum with 10 variants ([ability.rs:84-150](../../crates/protocol/src/ability.rs#L84-L150))
- `EffectTrigger` nests `AbilityEffect` ([ability.rs:157-177](../../crates/protocol/src/ability.rs#L157-L177))
- `dispatch_effect_markers` re-reads `AbilityDefs` every tick and walks the effects vec ([ability.rs:793-912](../../crates/protocol/src/ability.rs#L793-L912))

**One correction**: The design doc implies trigger components are inserted/removed as one-shot markers. In reality:
- `OnTickEffects` and `OnEndEffects` are truly one-shot (inserted then removed same tick after apply)
- `WhileActiveEffects`, `OnHitEffects`, `OnInputEffects` are persistent during Active phase (re-inserted every Active tick by dispatch, removed when leaving Active)

This distinction matters: in the archetype approach, persistent triggers are already on the entity. The `dispatch_effect_markers` simplification is correct — persistent triggers no longer need per-tick reinsertion.

### 4. Expressiveness Gains

#### 4a. Custom per-ability components
The biggest win. Any registered component can appear in RON files. Examples:
- `Homing(tracking_factor: 0.8)` on projectile abilities
- `AirControl(factor: 0.5)` on aerial abilities
- `Interruptible(true)` / `SuperArmor(threshold: 20.0)` for stagger mechanics
- `AnimationOverride("slam")` for custom ability animations

Currently impossible without adding fields to `AbilityDef` or new `AbilityEffect` variants.

#### 4b. Queryable trigger data
Systems can query `Query<&OnHitEffects>` directly to find all entities with hit effects — useful for UI, debugging, or cross-system interaction (e.g., "does any active ability have a Shield effect?"). Currently requires walking `AbilityDefs` + `effects` vec.

#### 4c. Non-ability entities reusing effects
Hazards, items, environmental triggers can compose the same trigger components without going through `AbilityDef`. A lava floor could have `OnHitEffects([Damage(...)])` without being an "ability."

#### 4d. New trigger types without dispatch changes
Adding `OnHealthBelowEffects { threshold, effects }` requires: define the component, register it, write the apply system. The loader picks it up automatically from RON. No changes to `dispatch_effect_markers`.

### 5. Constraints Introduced

#### 5a. Type registration ordering
All reflected types must be registered before `init_asset_loader`. This is **not a hard constraint** in practice — the types share the `Arc<RwLock<TypeRegistry>>`, so types registered later are still visible. But it's good practice.

#### 5b. RON format migration
14 ability files change from struct `(...)` to map `{...}` format. This is a one-time mechanical change, but it breaks backward compatibility. Parallel support is possible but not worth the complexity.

`#![enable(implicit_some)]` continues to work — the extension is parsed once when `ron::Deserializer::from_bytes()` creates the parser, and the flag persists through all nested `DeserializeSeed`/`Visitor` calls including the custom `ComponentMapVisitor`. As a belt-and-suspenders measure, add `#[serde(default)]` to component fields where sensible — this provides resilience if components are deserialized outside the RON file context (e.g., in tests).

#### 5c. Test infrastructure changes
All 1551 lines of tests construct `AbilityDef` manually ([ability_systems.rs:13-152](../../crates/protocol/tests/ability_systems.rs#L13-L152)). They'd need to either:
- Construct `AbilityAsset` manually (building `Vec<Box<dyn PartialReflect>>` — verbose but straightforward)
- Add a helper that converts `AbilityDef` → `AbilityAsset` for backward compatibility in tests
- Load actual RON files in tests (requires full asset server setup — heavier)

**Recommendation**: Keep a `AbilityDef::into_asset(registry: &TypeRegistry) -> AbilityAsset` conversion for tests. This minimizes test churn.

#### 5d. Reflection overhead at spawn time
Each ability spawn performs N reflection lookups + N `insert_reflect` calls (where N = number of components in the archetype, typically 3-5). This is negligible for ability spawns (dozens per second at most).

#### 5e. AbilityEffect enum retained
The design doc explicitly keeps `AbilityEffect` as an enum ([Section 3b note](../design/ability-archetype-refactor.md#L176-L178)). This means adding new effects still requires modifying the enum + every match arm. The full decomposition (individual effect structs) is deferred. This is a pragmatic choice — the enum approach keeps the apply systems simple.

### 6. Performance Analysis

#### 6a. What gets faster
- **`dispatch_effect_markers` eliminated/simplified**: Currently reads `AbilityDefs` resource + walks `Vec<EffectTrigger>` every tick for every active ability. The archetype approach replaces this with a simple query + tick-offset filter for `OnTickEffects`. `WhileActiveEffects`, `OnHitEffects`, `OnInputEffects` need no dispatch at all.
- **Phase duration lookup**: `AbilityPhaseLookup` replaces `AbilityDefs.get(id).phase_duration()`. Same HashMap lookup, but avoids pulling in the entire `AbilityDef` struct.

#### 6b. What gets slower
- **Spawn path**: Reflection-based component insertion vs. direct `commands.spawn((...))`. Additional ~5 HashMap lookups + `FromReflect` conversions per spawn. **Negligible** — ability spawns are infrequent.
- **Archetype fragmentation**: Different abilities produce different component combinations, leading to more ECS archetypes. Bevy handles this well, but extremely diverse abilities could fragment the archetype table. With 14 abilities and ~5 trigger component types, this is ~10 distinct archetypes — trivial.

#### 6c. Net effect
Positive. The per-tick dispatch hot path gets simpler (component queries vs HashMap + Vec walk). Spawn path adds minor overhead but runs infrequently.

### 7. Integration With Other Systems

#### 7a. Hit detection ([hit_detection.rs](../../crates/protocol/src/hit_detection.rs))
Currently reads `OnHitEffects` from hitbox/projectile entities. The archetype refactor changes nothing here — `OnHitEffects` is still a component, still propagated to hitboxes via `.insert(on_hit.clone())`. The component gains `Serialize`/`Deserialize`/`Reflect` derives but this is additive.

#### 7b. Lightyear replication
`ActiveAbility` is the only ability component registered for prediction ([lib.rs:246-248](../../crates/protocol/src/lib.rs#L246-L248)). The trigger components (`OnTickEffects`, etc.) are **not replicated** — they're local markers. This stays the same. The archetype approach doesn't affect replication.

However: since trigger components will now carry `Reflect + Serialize + Deserialize`, they *could* be registered for replication in the future if needed. This is a free option, not a requirement.

#### 7c. Animation system ([animset.rs](crates/sprite_rig/src/animset.rs))
References `AbilityId` for animation mapping. Unaffected by the refactor. In the future, a custom `AbilityAnimation("slash")` component in the RON file could drive animations more flexibly.

### 8. Recommended Changes to the Design

#### 8a. Fix `apply_or_insert` → `insert_reflect`
Replace the `apply_ability_archetypes` system body with:
```rust
for reflected in &asset.components {
    commands.entity(entity).insert_reflect(reflected.clone_value());
}
```

#### 8b. Inline archetype application instead of a deferred system
Eliminate `ApplyAbilityArchetype` marker and `apply_ability_archetypes` system entirely. Instead, have `ability_activation` and `spawn_sub_ability` insert archetype components inline at spawn time via `insert_reflect`. This avoids frame delays at any sub-ability chain depth. See **Section 10** for full analysis.

#### 8c. Retain backward compatibility during migration
Consider a migration period where both `AbilityDef` and `AbilityAsset` are supported. The `insert_ability_defs` system could check for either type. This allows incremental RON file conversion.

### 9. Test Impact Assessment

| Test area | Impact | Migration effort |
|-----------|--------|-----------------|
| `test_defs()` manual construction | Must build `AbilityAsset` instead of `AbilityDef` | Medium — need reflection registry in tests |
| Phase transition tests | Minimal — `AbilityPhases` replaces inline fields | Low |
| Effect dispatch tests | Moderate — verify archetype insertion replaces dispatch | Medium |
| Hit detection tests | None — `OnHitEffects` component unchanged | None |
| Sub-ability tests | Low — `spawn_sub_ability` uses `spawn_ability_with_archetype` | Low |
| Force frame tests | None — downstream of trigger components | None |

## Code References

- [ability.rs:180:187](../../crates/protocol/src/ability.rs:180) — `AbilityDef` struct

- [ability.rs:84-150](../../crates/protocol/src/ability.rs#L84-L150) — `AbilityEffect` enum
- [ability.rs:157-177](../../crates/protocol/src/ability.rs#L157-L177) — `EffectTrigger` enum
- [ability.rs:236-246](../../crates/protocol/src/ability.rs#L236-L246) — `ActiveAbility` component
- [ability.rs:313-337](../../crates/protocol/src/ability.rs#L313-L337) — Trigger marker components
- [ability.rs:793-912](../../crates/protocol/src/ability.rs#L793-L912) — `dispatch_effect_markers` + helpers
- [ability.rs:680-741](../../crates/protocol/src/ability.rs#L680-L741) — `ability_activation`
- [ability.rs:941-994](../../crates/protocol/src/ability.rs#L941-L994) — `spawn_sub_ability`
- [ability.rs:406-433](../../crates/protocol/src/ability.rs#L406-L433) — `AbilityPlugin::build`
- [ability.rs:743-790](../../crates/protocol/src/ability.rs#L743-L790) — `update_active_abilities` + phase advancement
- [lib.rs:245-253](../../crates/protocol/src/lib.rs#L245-L253) — Lightyear ability component registrations
- [lib.rs:311-349](../../crates/protocol/src/lib.rs#L311-L349) — System ordering in `SharedGameplayPlugin`
- [ability_systems.rs:13-152](../../crates/protocol/tests/ability_systems.rs#L13-L152) — Test ability definitions

## Architecture Documentation

The ability system follows a tick-based phase machine pattern:
1. **Activation** → spawn `ActiveAbility` entity at `Startup` phase
2. **Phase advancement** → tick-counted transitions: Startup → Active → Recovery → despawn
3. **Effect dispatch** → per-tick marker component insertion based on phase + tick offset
4. **Effect application** → dedicated systems per trigger type consume markers and execute game logic
5. **Cleanup** → observer strips markers on `ActiveAbility` removal

System chain runs in `FixedUpdate`, strictly ordered, gated on `AppState::Ready`.

## 10. Inline Archetype Application: Eliminating Frame Delays at Any Chain Depth

### The Problem

The design document proposes an `ApplyAbilityArchetype` marker component and a dedicated `apply_ability_archetypes` system that runs at a fixed point in the FixedUpdate chain. This creates a structural timing problem for sub-abilities:

```
ability_activation → [apply_deferred] → apply_ability_archetypes → update_active_abilities
    → dispatch → apply_on_tick_effects → apply_on_end_effects → apply_on_input_effects
```

Primary abilities (from `ability_activation`) are spawned before `apply_ability_archetypes` runs, so they get their archetype components on the same tick. But sub-abilities are spawned by `apply_on_tick_effects`, `apply_on_end_effects`, or `apply_on_input_effects` — all of which run **after** `apply_ability_archetypes`. The sub-ability entity doesn't exist when the archetype system runs.

This means sub-ability archetype components aren't applied until the next tick. For chains (A spawns B spawns C spawns D), each link adds one tick of delay. A 4-deep chain would take 4 extra ticks before the final ability has its trigger components.

With all current abilities having `startup_ticks >= 1`, the delay is masked — triggers don't fire during Startup phase. But it's a latent correctness bug and would become visible with `startup_ticks: 0` or with tight timing requirements in combo systems.

### The Solution: Inline Archetype Application

Instead of a marker + deferred system, make archetype application part of the spawn function itself. Both `ability_activation` and `spawn_sub_ability` insert archetype components at the point of spawning:

```rust
/// Spawns an ActiveAbility entity with its archetype components applied inline.
fn spawn_ability_with_archetype(
    commands: &mut Commands,
    ability_id: &AbilityId,
    active: ActiveAbility,
    salt: u64,
    ability_defs: &AbilityDefs,
    assets: &Assets<AbilityAsset>,
) -> Option<Entity> {
    let handle = ability_defs.abilities.get(ability_id)?;
    let asset = assets.get(handle)?;

    let mut entity_commands = commands.spawn((
        active,
        PreSpawned::default_with_salt(salt),
        Name::new("ActiveAbility"),
    ));

    for reflected in &asset.components {
        entity_commands.insert_reflect(reflected.clone_value());
    }

    Some(entity_commands.id())
}
```

### Why This Works at Any Chain Depth

`spawn_sub_ability` is a function called from within apply systems (`apply_on_tick_effects`, `apply_on_end_effects`, `apply_on_input_effects`). When ability A's effect spawns ability B, `spawn_ability_with_archetype` queues B's archetype components via `Commands::insert_reflect` alongside the `ActiveAbility` spawn. B's entity materializes at the next `apply_deferred` point.

Chains still take 1 tick per link — this is inherent to the system chain ordering, not an artifact of the archetype approach:

```
Tick N:
  update_active_abilities  — A is Active
  apply_on_tick_effects    — A spawns B via Commands (B gets archetype components inline)

Tick N+1:
  update_active_abilities  — B exists, advances from Startup → Active (if startup_ticks == 0)
  apply_on_tick_effects    — B's effects fire, B spawns C via Commands (C gets archetype inline)

Tick N+2:
  update_active_abilities  — C advances to Active
  apply_on_tick_effects    — C's effects fire
```

Each link activates 1 tick after being spawned. This is the **same timing as the current system** — `dispatch_effect_markers` and the apply systems also can't process an entity that was just spawned via `Commands` earlier in the chain. The inline approach adds no extra delay.

With `startup_ticks == 0`: B enters Active on tick N+1 (the first tick `update_active_abilities` sees it), and its effects fire on that same tick. This works because B's trigger components are already present from spawn — unlike the marker approach where `apply_ability_archetypes` might not have run yet.

The key insight: archetype application is **co-located with entity creation**, so trigger components are always present before the entity's first Active tick regardless of chain depth.

### What This Eliminates

- `ApplyAbilityArchetype` marker component — not needed
- `apply_ability_archetypes` system — not needed
- The `apply_deferred` scheduling concern between activation and archetype application
- The edge case where `startup_ticks == 0` causes missed effects (which the marker approach fails on)
- The retry-next-frame logic for assets not yet loaded (if the asset isn't loaded, the spawn fails immediately — same as the current system's behavior when `AbilityDefs` doesn't contain the ID)

### System Parameter Changes

The apply systems that call `spawn_sub_ability` gain two additional parameters:

| System | Current params (relevant) | Added params |
|--------|--------------------------|--------------|
| `apply_on_tick_effects` | `Res<AbilityDefs>` | `Res<Assets<AbilityAsset>>`, `Res<AppTypeRegistry>` |
| `apply_on_end_effects` | `Res<AbilityDefs>` | `Res<Assets<AbilityAsset>>`, `Res<AppTypeRegistry>` |
| `apply_on_input_effects` | `Res<AbilityDefs>` | `Res<Assets<AbilityAsset>>`, `Res<AppTypeRegistry>` |
| `ability_activation` | `Res<AbilityDefs>` | `Res<Assets<AbilityAsset>>`, `Res<AppTypeRegistry>` |

Note: `Res<AppTypeRegistry>` is needed because `insert_reflect` resolves the concrete component type internally through the registry. These are read-only resource accesses and don't affect system parallelism.

### Simplified System Chain

The FixedUpdate chain becomes:

```
ability_activation → update_active_abilities → filter_tick_effects
    → apply_on_tick_effects → apply_while_active_effects
    → apply_on_end_effects → apply_on_input_effects → ability_projectile_spawn
```

No `apply_ability_archetypes` step, no extra `apply_deferred` insertion. The chain is shorter and the data flow is simpler: spawn sites are self-contained.

### Impact on Tests

Test helper functions that construct abilities would call the same `spawn_ability_with_archetype` function, ensuring test and production code follow the same path. Tests that currently insert `AbilityDef` into `AbilityDefs` would instead insert `Handle<AbilityAsset>` and the corresponding `AbilityAsset` into `Assets<AbilityAsset>`. The `AppTypeRegistry` resource must be present in test apps (added via `app.register_type::<T>()` calls).

## 11. Replication, Prediction, and Determinism

### Current Replication Model

Only `ActiveAbility` itself is registered for prediction with entity mapping ([lib.rs:246-248](../../crates/protocol/src/lib.rs#L246-L248)). The trigger marker components (`OnTickEffects`, `WhileActiveEffects`, `OnHitEffects`, `OnEndEffects`, `OnInputEffects`) are **never replicated** — they're local transient markers inserted by `dispatch_effect_markers` and consumed by apply systems within the same or next tick. Related predicted components:

| Component | Prediction | Map Entities | Notes |
|-----------|-----------|-------------|-------|
| `ActiveAbility` | Yes | Yes (`caster`, `original_caster`, `target`) | Core replicated state |
| `AbilityCooldowns` | Yes | No | Per-character cooldown tracking |
| `ActiveShield` | Yes | No | Shield absorption remaining |
| `ActiveBuffs` | Yes | No | Active buff list with expiry ticks |
| `AbilitySlots` | No (replicate-only) | No | Slot assignments, synced once |
| `AbilityProjectileSpawn` | No (replicate-only) | No | Transient spawn marker |

Ability entities use `PreSpawned::default_with_salt(salt)` for deterministic entity matching between client and server. Salt is computed from `PlayerId`, slot index, depth, and ability ID — ensuring both sides produce the same hash for the same ability activation.

### What the Archetype Refactor Changes for Replication

**Nothing changes for `ActiveAbility` replication.** The `ActiveAbility` component — the only predicted component on ability entities — is unaffected. It's still spawned with the same fields, same `PreSpawned` salt, same `Replicate`/`PredictionTarget` setup.

**Trigger components remain non-replicated.** In the current system they're inserted by `dispatch_effect_markers` reading `AbilityDefs`. In the new system they're inserted by `spawn_ability_with_archetype` reading `AbilityAsset`. Either way, they exist only locally and are never sent over the network.

**New archetype components (custom per-ability components) are also non-replicated by default.** Any component added via `insert_reflect` from the archetype is local unless explicitly registered with `register_component::<T>().add_prediction()`. This is correct — these are definition data, not mutable state.

### Determinism Analysis

The ability system achieves determinism through two mechanisms:

1. **Identical system execution**: All ability systems run in `FixedUpdate` on both server and client with the same inputs (`ActionState`, `LocalTimeline` tick). The systems are deterministic given the same state.

2. **PreSpawned matching**: Both sides compute the same salt → same entity hash → Lightyear matches the client's predicted entity with the server's authoritative one.

The archetype refactor preserves both mechanisms:

- **System determinism**: `spawn_ability_with_archetype` reads from `Assets<AbilityAsset>` which is loaded from the same RON files on both server and client. Given the same `AbilityId`, both sides insert identical trigger components. The apply systems then process them identically.

- **PreSpawned salt**: Unchanged. Salt computation doesn't depend on archetype data.

- **Rollback correctness**: During rollback, Lightyear re-runs `FixedMain` for each missed tick. The ability systems re-execute, re-spawning abilities and re-inserting trigger components via the same `spawn_ability_with_archetype` path. Since trigger components aren't registered for prediction, Lightyear doesn't touch them during rollback preparation — they're purely derived from the replay of game logic.

### Potential Risks

#### Risk 1: Asset loading race on client

If `AbilityAsset` handles aren't loaded when `spawn_ability_with_archetype` runs, the spawn fails (`assets.get(handle)` returns `None`). This is the same failure mode as the current system when `AbilityDefs` doesn't contain an ID — the ability simply doesn't activate. Both server and client load assets during startup before `AppState::Ready`, so this shouldn't happen in practice.

However, during **rollback replay**, `Assets<AbilityAsset>` is a `Res` (resource) — it's not rolled back. It reflects the current frame's state, which should have all assets loaded. No issue here.

#### Risk 2: Reflection ordering non-determinism

`AbilityAsset.components` is a `Vec<Box<dyn PartialReflect>>` populated by the RON map deserializer. RON maps preserve insertion order (they use `Map` which iterates in definition order). Both server and client parse the same RON file, so the Vec ordering is identical. Component insertion order doesn't affect ECS behavior — Bevy archetypes are defined by the set of component types, not their insertion order.

#### Risk 3: Trigger components surviving rollback incorrectly

During rollback:
1. Lightyear rewinds `LocalTimeline` and restores predicted components (`ActiveAbility`, etc.) to confirmed state
2. Trigger components (`OnTickEffects`, etc.) are **not** restored — they aren't registered for prediction, so Lightyear leaves them as-is
3. `FixedMain` replays from the rollback tick — the ability systems re-execute `dispatch_effect_markers` (or in the new design, trigger components are already present from spawn)

In the **current system**, stale trigger markers from pre-rollback might linger. But `dispatch_effect_markers` unconditionally re-inserts/removes them based on current phase, so stale markers get overwritten on the first replay tick. The observer `cleanup_effect_markers_on_removal` also fires if `ActiveAbility` is removed during rollback.

In the **new system** with inline archetype application: trigger components are inserted at spawn time. During rollback replay, if `ActiveAbility` was restored (entity still exists), trigger components from the pre-rollback timeline persist — but they're the same data (loaded from the same asset), so this is correct. If the entity was despawned during rollback prep and re-spawned during replay, `spawn_ability_with_archetype` re-inserts them fresh.

**One subtlety**: If rollback determines an ability entity should NOT exist at the rollback tick (server didn't spawn it), Lightyear despawns the predicted entity entirely (PreSpawned entities spawned after the rollback tick are despawned before replay — see `lightyear_prediction/src/rollback.rs:451-473`). The trigger components go away with the entity. If the ability SHOULD be re-spawned during replay, the game systems will call `spawn_ability_with_archetype` again, producing a fresh entity with correct trigger components.

#### Risk 4: `insert_reflect` and `FromReflect` non-determinism

`insert_reflect` internally uses `FromReflect` to convert `Box<dyn PartialReflect>` into the concrete component type. `FromReflect` is derived — it's a deterministic field-by-field construction. No floating-point operations, no platform-dependent behavior. No risk here.

### Future Opportunity: Registering Trigger Components for Prediction

The archetype refactor adds `Reflect + Serialize + Deserialize` to trigger components. This means they **could** be registered for prediction in the future:

```rust
app.register_component::<OnHitEffects>().add_prediction();
```

This would let the server authoritatively correct trigger data if it diverges (e.g., a hot-reloaded ability definition on the server but not the client). Currently unnecessary since both sides load the same assets, but it's a free option the refactor enables.

### Summary

The archetype refactor is **neutral to positive** for replication and determinism:
- No changes to what's replicated or predicted
- Trigger component insertion is deterministic (same asset → same components)
- Rollback replay re-executes game systems which re-derive trigger state correctly
- `PreSpawned` salt computation is unchanged
- Future option to replicate trigger data if needed

## Related Research

- [2026-02-07-ability-system-architecture.md](doc/research/2026-02-07-ability-system-architecture.md)
- [2026-02-20-ability-effect-primitives-implementation-analysis.md](doc/research/2026-02-20-ability-effect-primitives-implementation-analysis.md)
- [2026-02-21-ability-effect-primitives-lightyear-hierarchy.md](doc/research/2026-02-21-ability-effect-primitives-lightyear-hierarchy.md)
- [2026-02-22-remaining-ability-effect-primitives.md](doc/research/2026-02-22-remaining-ability-effect-primitives.md)

## External References

- [ReflectComponent docs (Bevy 0.18)](https://docs.rs/bevy/latest/bevy/ecs/reflect/struct.ReflectComponent.html)
- [ReflectCommandExt trait docs](https://docs.rs/bevy/latest/bevy/ecs/reflect/trait.ReflectCommandExt.html)
- [TypedReflectDeserializer docs](https://docs.rs/bevy/latest/bevy/reflect/serde/struct.TypedReflectDeserializer.html)

## Resolved Questions

1. **`#![enable(implicit_some)]`** — **Works with the custom deserializer.** The extension is parsed once when `ron::Deserializer::from_bytes()` creates the parser, and persists through all nested `DeserializeSeed`/`Visitor` calls. No changes to `Option` field handling needed. Add `#[serde(default)]` to component fields as a belt-and-suspenders measure for test contexts.

2. **`DynamicScene` as alternative** — No. The custom loader is the right approach for single-entity archetypes.

3. **Test strategy** — Build the `AbilityDef::into_asset(registry: &TypeRegistry) -> AbilityAsset` bridge. Minimizes test churn while ensuring tests exercise the same spawn path as production.

4. **Bevy version** — Confirmed as 0.18. All API references in this document target Bevy 0.18.
