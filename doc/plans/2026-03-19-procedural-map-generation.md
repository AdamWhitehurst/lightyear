# Procedural Map Generation System — Implementation Plan

## Overview

Replace the hardcoded `flat_terrain_voxels` generator with a data-driven procedural generation system. Terrain configuration is authored in `.terrain.ron` files using the same reflect-based component-map format as `.object.ron`. Noise-based heightmaps, biome rules, and world object placement are all driven by which components are present on the map entity (archetype pattern). Per-chunk entity persistence enables procedural objects to survive unload/reload with mutation tracking.

## Current State Analysis

- **Single generator**: `flat_terrain_voxels` (`meshing.rs:66`) — fills `Solid(0)` below y=0, `Air` above. Used by all map types (overworld, homebase, arena) on both server and client.
- **`VoxelGenerator`**: `Arc<dyn Fn(IVec3) -> Vec<WorldVoxel> + Send + Sync>` (`config.rs:9`). Stored in `VoxelMapConfig.generator`.
- **Seed exists but unused**: `VoxelMapConfig.seed` is set (default 999 for overworld) but never passed to any generator.
- **`WorldObjectDef` pattern**: RON → `reflect_loader::deserialize_component_map` → `Vec<Box<dyn PartialReflect>>` → `apply_object_components`. Proven, extensible.
- **One world object**: `tree_circle.object.ron`, spawned by `spawn_test_tree` with hardcoded position.
- **Entity persistence**: Flat `entities.bin` per map for `RespawnPoint` entities only. No per-chunk entity storage.
- **No noise crate** in any `Cargo.toml`.

### Key Discoveries:
- `VoxelMapInstance` constructors (`overworld`, `homebase`, `arena` in `instance.rs:51-103`) all accept a `VoxelGenerator` parameter — the swap point is clean.
- `ChunkGenResult` (`generation.rs:13-19`) has no world object field yet.
- `remove_out_of_range_chunks` (`lifecycle.rs:139-163`) already saves dirty chunks on eviction — same pattern needed for chunk entities.
- Client `generator_for_map` (`client/src/map.rs:584-591`) matches `MapInstanceId` to generator. Client sets `generates_chunks = false`, so this is only used as a fallback — can remain `flat_terrain_voxels`.
- `reflect_loader::deserialize_component_map` (`reflect_loader.rs:56-64`) is the shared deserialization engine — reusable for `TerrainDef` without modification.

## Desired End State

- `.terrain.ron` files define terrain configuration per map type via reflected components.
- `TerrainDefRegistry` resource maps terrain IDs to loaded `TerrainDef` assets.
- `build_generator(entity, seed)` reads `HeightMap`, `MoistureMap`, `BiomeRules` from the map entity and produces a `VoxelGenerator` closure.
- `PlacementRules` drives Poisson-disk-sampled object spawning per chunk.
- Procedurally spawned objects load/unload with their chunks.
- Per-chunk entity files track mutations (destroyed, moved, component overrides) for procedural objects and store non-procedural (player-placed) objects.
- Existing flat terrain behavior is preserved for maps with empty `.terrain.ron` files.

### Verification:
- `cargo check-all` passes.
- `cargo server` generates noise-based overworld terrain visible at runtime.
- `cargo client -c 1` renders the streamed terrain.
- World objects spawn procedurally in correct biomes.
- Objects despawn when their chunk unloads, respawn when it reloads.
- Destroyed objects stay destroyed across chunk unload/reload cycles.

## What We're NOT Doing

- **3D cave/tunnel carving** — deferred to a future `CaveCarver` component.
- **Biome blending** — hard boundaries only; no noise-based edge smoothing.
- **Client-side terrain config** — chunks are streamed from server; client keeps `flat_terrain_voxels` fallback.
- **Material palette asset** — `u8` material indices exist but visual mapping (color/texture per index) is out of scope.
- **Structure generation** (villages, dungeons) — out of scope; `PlacementRules` handles individual objects only.
- **Mobile entity persistence** (NPCs, wildlife) — only static/scenery objects. Mobile entity chunk persistence is a separate future system.

## Implementation Approach

Reuse the `WorldObjectDef` pattern: RON component maps deserialized via `reflect_loader`, stored as `Vec<Box<dyn PartialReflect>>`, applied to entities via `apply_object_components`. Each terrain aspect is an independent ECS component. Generator behavior is determined by component presence (archetype pattern). World object placement returns spawn descriptors from the async generation task; the main thread spawns entities. Per-chunk entity files use the same versioned-envelope + atomic-write pattern as chunk voxel persistence.

---

## Phase 1: Terrain Types & Noise Generation

### Overview
Define the terrain component types and the core noise-driven chunk generation function. No asset loading yet — types are constructed in code for testing.

### Changes Required:

#### 1. Add `noise` dependency
**File**: `crates/voxel_map_engine/Cargo.toml`
**Changes**: Add `noise = "0.9"` to `[dependencies]`.

```toml
[dependencies]
# ... existing deps ...
noise = "0.9"
```

#### 2. Create terrain types module
**File**: `crates/voxel_map_engine/src/terrain.rs` (new)
**Changes**: Define all terrain component types, enums, and the noise construction function.

```rust
use bevy::prelude::*;
use noise::{
    Fbm, HybridMulti, NoiseFn, OpenSimplex, Perlin, RidgedMulti, ScaleBias, SuperSimplex,
    Value, Worley,
};
use serde::{Deserialize, Serialize};

/// Which base noise algorithm to use.
#[derive(Clone, Debug, Serialize, Deserialize, Reflect, Default)]
pub enum NoiseType {
    #[default]
    Perlin,
    OpenSimplex,
    Value,
    Worley,
    SuperSimplex,
}

/// Which fractal layering to apply. `None` uses the raw base noise.
#[derive(Clone, Debug, Serialize, Deserialize, Reflect, Default)]
pub enum FractalType {
    #[default]
    Fbm,
    RidgedMulti,
    HybridMulti,
    None,
}

/// Noise sampling parameters. Embedded in noise-driven components.
#[derive(Clone, Debug, Serialize, Deserialize, Reflect)]
pub struct NoiseDef {
    pub noise_type: NoiseType,
    pub fractal: FractalType,
    pub seed_offset: u32,
    pub octaves: u32,
    pub frequency: f64,
    pub lacunarity: f64,
    pub persistence: f64,
}

/// Drives terrain height. Presence = noise-based terrain. Absence = flat at y=0.
#[derive(Component, Clone, Debug, Serialize, Deserialize, Reflect)]
#[reflect(Component, Serialize, Deserialize)]
pub struct HeightMap {
    pub noise: NoiseDef,
    pub base_height: i32,
    pub amplitude: f64,
}

/// Drives biome moisture sampling. Only meaningful alongside `BiomeRules`.
#[derive(Component, Clone, Debug, Serialize, Deserialize, Reflect)]
#[reflect(Component, Serialize, Deserialize)]
pub struct MoistureMap {
    pub noise: NoiseDef,
}

/// Biome selection rules. Multiple biomes per map.
#[derive(Component, Clone, Debug, Serialize, Deserialize, Reflect)]
#[reflect(Component, Serialize, Deserialize)]
pub struct BiomeRules(pub Vec<BiomeRule>);

/// A single biome's selection criteria and material assignment.
#[derive(Clone, Debug, Serialize, Deserialize, Reflect)]
pub struct BiomeRule {
    pub biome_id: String,
    pub height_range: (f64, f64),
    pub moisture_range: (f64, f64),
    pub surface_material: u8,
    pub subsurface_material: u8,
    pub subsurface_depth: u32,
}
```

#### 3. Implement noise construction
**File**: `crates/voxel_map_engine/src/terrain.rs` (continued)
**Changes**: `build_noise_fn` returns a `Box<dyn NoiseFn<f64, 2> + Send + Sync>` from a `NoiseDef` + world seed.

The `noise` crate's `NoiseFn` trait is generic over dimensionality. All terrain sampling uses 2D `[f64; 2]` inputs (x, z world coordinates). The function constructs the base noise generator from `NoiseType`, wraps it in a fractal combinator from `FractalType`, and applies `ScaleBias` with `scale = frequency` (the frequency parameter controls the spatial scale of features).

Seed combination: `(seed as u32).wrapping_add(def.seed_offset)` — each noise layer gets a distinct seed derived from the world seed plus the configured offset.

```rust
/// Construct a 2D noise function from a `NoiseDef` and world seed.
pub fn build_noise_fn(
    def: &NoiseDef,
    seed: u64,
) -> Box<dyn NoiseFn<f64, 2> + Send + Sync> {
    let s = (seed as u32).wrapping_add(def.seed_offset);
    match (&def.noise_type, &def.fractal) {
        (NoiseType::Perlin, FractalType::Fbm) => Box::new(
            Fbm::<Perlin>::new(s)
                .set_octaves(def.octaves as usize)
                .set_frequency(def.frequency)
                .set_lacunarity(def.lacunarity)
                .set_persistence(def.persistence),
        ),
        (NoiseType::Perlin, FractalType::RidgedMulti) => Box::new(
            RidgedMulti::<Perlin>::new(s)
                .set_octaves(def.octaves as usize)
                .set_frequency(def.frequency)
                .set_lacunarity(def.lacunarity),
        ),
        // ... other combinations follow the same pattern.
        // Each (NoiseType, FractalType) pair constructs the appropriate type.
        // FractalType::None wraps the raw generator in ScaleBias for frequency control.
    }
}
```

All 20 combinations (5 noise types × 4 fractal types) must be enumerated because `noise` crate types are concrete generics (`Fbm<Perlin>`, `Fbm<OpenSimplex>`, etc.) and cannot be type-erased at the inner level. The `Box<dyn NoiseFn<f64, 2>>` erasure happens at the outer level.

For `FractalType::None`, the raw noise generator is sampled directly. The `frequency` parameter is applied by scaling the input coordinates before sampling (the caller multiplies `world_x * frequency`), not via `ScaleBias` on output.

#### 4. Implement heightmap chunk generation
**File**: `crates/voxel_map_engine/src/terrain.rs` (continued)
**Changes**: `generate_heightmap_chunk` — the core generation function.

```rust
use crate::types::{CHUNK_SIZE, PaddedChunkShape, WorldVoxel};
use ndshape::ConstShape;

/// Generate a chunk with noise-based terrain.
///
/// Pre-computes a 2D heightmap (and optional moisture map) for the chunk's
/// (x, z) footprint, then fills voxels based on height and biome rules.
pub fn generate_heightmap_chunk(
    chunk_pos: IVec3,
    seed: u64,
    height_map: &HeightMap,
    moisture_map: Option<&MoistureMap>,
    biome_rules: Option<&BiomeRules>,
) -> Vec<WorldVoxel> {
    let height_noise = build_noise_fn(&height_map.noise, seed);
    let moisture_noise = moisture_map.map(|m| build_noise_fn(&m.noise, seed));

    // Pre-compute 2D heightmap for the 18×18 padded footprint.
    let padded = 18usize;
    let mut height_cache = [0.0f64; 18 * 18];
    let mut moisture_cache = [0.0f64; 18 * 18];

    for pz in 0..padded {
        for px in 0..padded {
            let world_x = chunk_pos.x * CHUNK_SIZE as i32 + px as i32 - 1;
            let world_z = chunk_pos.z * CHUNK_SIZE as i32 + pz as i32 - 1;
            let idx = pz * padded + px;

            let h = height_noise.get([world_x as f64, world_z as f64]);
            height_cache[idx] = height_map.base_height as f64 + h * height_map.amplitude;

            if let Some(ref mn) = moisture_noise {
                moisture_cache[idx] = mn.get([world_x as f64, world_z as f64]);
            }
        }
    }

    let mut voxels = vec![WorldVoxel::Air; PaddedChunkShape::USIZE];
    for i in 0..PaddedChunkShape::SIZE {
        let [px, py, pz] = PaddedChunkShape::delinearize(i);
        let world_y = chunk_pos.y * CHUNK_SIZE as i32 + py as i32 - 1;
        let idx_2d = pz as usize * padded + px as usize;
        let terrain_height = height_cache[idx_2d];

        if (world_y as f64) <= terrain_height {
            let material = select_material(
                biome_rules,
                &moisture_noise,
                terrain_height,
                moisture_cache[idx_2d],
                world_y as f64,
            );
            voxels[i as usize] = WorldVoxel::Solid(material);
        }
    }
    voxels
}

fn select_material(
    biome_rules: Option<&BiomeRules>,
    moisture_noise: &Option<Box<dyn NoiseFn<f64, 2> + Send + Sync>>,
    terrain_height: f64,
    moisture: f64,
    world_y: f64,
) -> u8 {
    let Some(rules) = biome_rules else { return 0; };
    if moisture_noise.is_none() { return 0; }

    // Normalize height to 0..1 range for biome selection.
    // The raw noise output from Fbm is roughly in [-1, 1], so terrain_height
    // varies around base_height. We normalize based on the noise value itself.
    let biome = select_biome(&rules.0, terrain_height, moisture);
    let depth = (terrain_height - world_y) as u32;
    if depth < biome.subsurface_depth {
        biome.surface_material
    } else {
        biome.subsurface_material
    }
}

/// Select the best-matching biome for the given height and moisture values.
///
/// Returns the first biome whose ranges contain both values.
/// Falls back to the first biome in the list if no ranges match.
fn select_biome(rules: &[BiomeRule], height: f64, moisture: f64) -> &BiomeRule {
    rules
        .iter()
        .find(|b| {
            height >= b.height_range.0
                && height <= b.height_range.1
                && moisture >= b.moisture_range.0
                && moisture <= b.moisture_range.1
        })
        .unwrap_or(&rules[0])
}
```

**Note on height/moisture normalization**: The `height_range` and `moisture_range` in `BiomeRule` match against the **raw noise output** (roughly [-1, 1] for Fbm) scaled by amplitude and offset by base_height. For the overworld `.terrain.ron`, these ranges are authored in world-height units (e.g., -20.0 to 20.0 for amplitude 20). For moisture, the raw noise range [-1, 1] is used directly. The initial implementation uses raw values; normalization to [0, 1] can be added later if authoring becomes cumbersome.

#### 5. Register module
**File**: `crates/voxel_map_engine/src/lib.rs`
**Changes**: Add `pub mod terrain;` and register types in `VoxelPlugin::build`.

```rust
pub mod terrain;

// In VoxelPlugin::build():
app.register_type::<terrain::HeightMap>();
app.register_type::<terrain::MoistureMap>();
app.register_type::<terrain::BiomeRules>();
app.register_type::<terrain::NoiseDef>();
app.register_type::<terrain::NoiseType>();
app.register_type::<terrain::FractalType>();
app.register_type::<terrain::BiomeRule>();
```

### Success Criteria:

#### Automated Verification:
- [ ] `cargo check-all` passes
- [ ] Unit tests for `build_noise_fn` (deterministic output for same seed)
- [ ] Unit tests for `generate_heightmap_chunk` (non-flat output with HeightMap, flat-equivalent without)
- [ ] Unit tests for `select_biome` (correct biome selection, fallback behavior)

#### Manual Verification:
- [ ] None required — no runtime integration yet.

---

## Phase 2: TerrainDef Asset Pipeline

### Overview
Create the `TerrainDef` asset type and loader, following the same pattern as `WorldObjectDef`. Load `.terrain.ron` files during startup, build `TerrainDefRegistry`.

### Changes Required:

#### 1. Create terrain module in protocol
**File**: `crates/protocol/src/terrain/mod.rs` (new)
**Changes**: Module root with re-exports.

```rust
mod loading;
mod plugin;
mod registry;

pub use plugin::TerrainPlugin;
pub use registry::TerrainDefRegistry;
```

#### 2. TerrainDef type
**File**: `crates/protocol/src/terrain/types.rs` (new)
**Changes**: Asset type, structurally identical to `WorldObjectDef`.

```rust
use bevy::prelude::*;

/// A loaded terrain definition. Component map loaded from `.terrain.ron`.
#[derive(Asset, TypePath)]
pub struct TerrainDef {
    pub components: Vec<Box<dyn PartialReflect>>,
}
```

Implement `Clone` and `Debug` the same way `WorldObjectDef` does — via `reflect_clone()` for Clone, type-path-only for Debug.

#### 3. TerrainDefLoader
**File**: `crates/protocol/src/terrain/loader.rs` (new)
**Changes**: `AssetLoader` implementation, mirrors `WorldObjectLoader`.

```rust
use bevy::asset::{AssetLoader, LoadContext, Reader};
use bevy::reflect::TypeRegistryArc;
use bevy::prelude::*;
use crate::reflect_loader;

pub(super) struct TerrainDefLoader {
    type_registry: TypeRegistryArc,
}

impl FromWorld for TerrainDefLoader {
    fn from_world(world: &mut World) -> Self {
        Self {
            type_registry: world.resource::<AppTypeRegistry>().0.clone(),
        }
    }
}

impl AssetLoader for TerrainDefLoader {
    type Asset = super::types::TerrainDef;
    type Settings = ();
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn extensions(&self) -> &[&str] {
        &["terrain.ron"]
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
        let components = reflect_loader::deserialize_component_map(&bytes, &registry)?;
        Ok(super::types::TerrainDef { components })
    }
}
```

**Empty RON files**: An empty `{ }` RON map deserializes to an empty `Vec<Box<dyn PartialReflect>>`. This is the "homebase" case — no terrain components means flat terrain.

#### 4. TerrainDefRegistry
**File**: `crates/protocol/src/terrain/registry.rs` (new)
**Changes**: Resource holding the loaded terrain definitions.

```rust
use bevy::prelude::*;
use std::collections::HashMap;
use super::types::TerrainDef;

/// Maps terrain definition IDs (e.g., "overworld") to loaded `TerrainDef` assets.
#[derive(Resource)]
pub struct TerrainDefRegistry {
    pub terrains: HashMap<String, TerrainDef>,
}

impl TerrainDefRegistry {
    pub fn get(&self, id: &str) -> Option<&TerrainDef> {
        self.terrains.get(id)
    }
}
```

#### 5. Loading pipeline
**File**: `crates/protocol/src/terrain/loading.rs` (new)
**Changes**: Same dual-path (native/WASM) pattern as `world_object/loading.rs`.

**Native**: `load_folder("terrain")` → `TrackedAssets` → `insert_terrain_defs` once all loaded.
**WASM**: Load `terrain.manifest.ron` → individual file loads → `insert_terrain_defs`.

ID derivation: strip `.terrain.ron` suffix from filename, same as `object_id_from_path`.

#### 6. TerrainPlugin
**File**: `crates/protocol/src/terrain/plugin.rs` (new)
**Changes**: Registers the asset type, loader, type reflections, and loading systems.

```rust
impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<TerrainDef>();
        app.init_asset_loader::<TerrainDefLoader>();
        // Type registrations for terrain components are already done in VoxelPlugin.
        // TerrainPlugin only handles the asset pipeline.
        app.add_systems(Startup, load_terrain_defs);
        app.add_systems(Update,
            insert_terrain_defs.run_if(not(resource_exists::<TerrainDefRegistry>)),
      %% Where are insert_terrain_defs and load_terrain_defs defined?
        );
    }
}
```

#### 7. Register module in protocol
**File**: `crates/protocol/src/lib.rs`
**Changes**: Add `pub mod terrain;` and add `TerrainPlugin` to `ProtocolPlugin` (or whichever plugin aggregates protocol plugins).

#### 8. Create `.terrain.ron` asset files

**File**: `assets/terrain/overworld.terrain.ron` (new)
```ron
{
    "voxel_map_engine::terrain::HeightMap": (
        noise: (noise_type: Perlin, fractal: Fbm, seed_offset: 0,
                octaves: 6, frequency: 0.005, lacunarity: 2.0, persistence: 0.5),
        base_height: 0,
        amplitude: 20.0,
    ),
    "voxel_map_engine::terrain::MoistureMap": (
        noise: (noise_type: Perlin, fractal: Fbm, seed_offset: 100,
                octaves: 4, frequency: 0.008, lacunarity: 2.0, persistence: 0.6),
    ),
    "voxel_map_engine::terrain::BiomeRules": ([
        (biome_id: "grassland", height_range: (-5.0, 5.0), moisture_range: (-0.3, 0.3),
         surface_material: 1, subsurface_material: 2, subsurface_depth: 3),
        (biome_id: "desert", height_range: (-5.0, 5.0), moisture_range: (-1.0, -0.3),
         surface_material: 3, subsurface_material: 3, subsurface_depth: 5),
        (biome_id: "forest", height_range: (-5.0, 10.0), moisture_range: (0.3, 1.0),
         surface_material: 1, subsurface_material: 2, subsurface_depth: 4),
    ]),
}
```

**File**: `assets/terrain/homebase.terrain.ron` (new)
```ron
{
}
```

**File**: `assets/terrain/arena_hills.terrain.ron` (new)
```ron
{
    "voxel_map_engine::terrain::HeightMap": (
        noise: (noise_type: Perlin, fractal: Fbm, seed_offset: 0,
                octaves: 3, frequency: 0.02, lacunarity: 2.0, persistence: 0.4),
        base_height: 0,
        amplitude: 5.0,
    ),
    "voxel_map_engine::terrain::BiomeRules": ([
        (biome_id: "sand", height_range: (-100.0, 100.0), moisture_range: (-1.0, 1.0),
         surface_material: 2, subsurface_material: 2, subsurface_depth: 99),
    ]),
}
```

**File**: `assets/terrain.manifest.ron` (new, for WASM)
```ron
["overworld", "homebase", "arena_hills"]
```

### Success Criteria:

#### Automated Verification:
- [ ] `cargo check-all` passes
- [ ] `.terrain.ron` files parse without errors at startup (verified via `cargo server` log output)

#### Manual Verification:
- [ ] `cargo server` starts and logs "Loaded N terrain definitions"

---

## Phase 3: Wire Terrain Into Map Spawning

### Overview
Connect the `TerrainDefRegistry` to map spawning. Apply terrain components onto map entities, then `build_generator` reads them to produce a `VoxelGenerator`.

### Changes Required:

#### 1. Implement `build_generator`
**File**: `crates/voxel_map_engine/src/terrain.rs`
**Changes**: Add `build_generator` function.

```rust
use crate::config::VoxelGenerator;
use crate::meshing::flat_terrain_voxels;
use std::sync::Arc;

/// Build a `VoxelGenerator` closure from terrain components on a map entity.
///
/// Reads `HeightMap`, `MoistureMap`, and `BiomeRules` from the entity.
/// If no `HeightMap` is present, falls back to `flat_terrain_voxels`.
pub fn build_generator(entity: EntityRef, seed: u64) -> VoxelGenerator {
    let height = entity.get::<HeightMap>().cloned();
    let moisture = entity.get::<MoistureMap>().cloned();
    let biomes = entity.get::<BiomeRules>().cloned();
  %% We should add `debug_assert!`s to enforce expectations like: biome rules must exist if moisture specified, or shouldn't have biomes, mositure if no heightmap

    match height {
        Some(h) => Arc::new(move |chunk_pos| {
            generate_heightmap_chunk(
                chunk_pos,
                seed,
                &h,
                moisture.as_ref(),
                biomes.as_ref(),
            )
        }),
        None => Arc::new(flat_terrain_voxels),
    }
}
```

#### 2. Apply terrain def to map entity
**File**: `crates/server/src/map.rs`
**Changes**: Modify `spawn_overworld` and `spawn_homebase` to:
1. Look up `TerrainDef` from `TerrainDefRegistry`
2. Apply terrain components onto the map entity via `apply_object_components`
3. Build generator from the entity's terrain components

The flow changes from:
```rust
// Before
let config = VoxelMapConfig::new(..., Arc::new(flat_terrain_voxels));
```
To:
```rust
// After
// 1. Spawn entity with placeholder generator
let config = VoxelMapConfig::new(..., Arc::new(flat_terrain_voxels));
let entity = commands.spawn((VoxelMapInstance::new(5), config, ...)).id();
%% Why do we need a placeholder entity with a placeholder config?
%% Should VoxelMapConfig contain the generator at all? Why not make the VoxelGenerator its own component and eliminate the need for `NeedsGeneratorBuild`, because systems can check e.g. `Without<VoxelGenerator>`?

// 2. Apply terrain def components
let terrain_def = terrain_registry.get("overworld").expect("overworld terrain must be loaded");
apply_object_components(&mut commands, entity, terrain_def.components.clone(), type_registry.0.clone());

// 3. In a follow-up system (one frame later), build the real generator
```

**Problem**: `apply_object_components` defers insertion via `commands.queue()`. The entity won't have terrain components until the next command flush. The generator cannot be built in the same system call.

**Solution**: Use a two-phase approach with a marker component:
%% Can we chain the systems and apply commands in-between the two systems somehow to avoid the 1 frame delay?

```rust
/// Marker: this map entity needs its generator built from terrain components.
#[derive(Component)]
pub struct NeedsGeneratorBuild;
```

Phase A (`spawn_overworld`): Spawn with `flat_terrain_voxels` placeholder + `NeedsGeneratorBuild`. Apply terrain def via `apply_object_components`.

Phase B (new system `build_terrain_generators`): Runs in `Update`, queries for `(Entity, &mut VoxelMapConfig, &VoxelMapInstance)` with `With<NeedsGeneratorBuild>`. Reads terrain components from the entity via `EntityRef`, calls `build_generator`, replaces `config.generator`, removes `NeedsGeneratorBuild`.

```rust
fn build_terrain_generators(
    mut commands: Commands,
    world: &World,
    mut query: Query<(Entity, &mut VoxelMapConfig), With<NeedsGeneratorBuild>>,
) {
    for (entity, mut config) in &mut query {
        let entity_ref = world.entity(entity);
        config.generator = build_generator(entity_ref, config.seed);
        commands.entity(entity).remove::<NeedsGeneratorBuild>();
        info!("Built terrain generator for map entity {entity:?}");
    }
}
```

**Timing**: The `NeedsGeneratorBuild` marker ensures no chunks are generated with the placeholder. The `update_chunks` system (which spawns generation tasks) runs after `build_terrain_generators` in the chain, so the first chunk generation uses the real generator. Alternatively, `ensure_pending_chunks` could skip inserting `PendingChunks` while `NeedsGeneratorBuild` is present — this is cleaner:

```rust
// In ensure_pending_chunks: skip entities that still need generator build
fn ensure_pending_chunks(
    mut commands: Commands,
    query: Query<Entity, (With<VoxelMapInstance>, Without<PendingChunks>, Without<NeedsGeneratorBuild>)>,
) { ... }
```

This approach is cleaner because it naturally gates chunk generation behind terrain setup completion, without requiring system ordering guarantees.

#### 3. Modify `spawn_overworld`
**File**: `crates/server/src/map.rs`
**Changes**: Add `TerrainDefRegistry` and `AppTypeRegistry` parameters. Look up "overworld" terrain def. Apply components. Add `NeedsGeneratorBuild`.

```rust
pub fn spawn_overworld(
    mut commands: Commands,
    mut registry: ResMut<MapRegistry>,
    save_path: Res<WorldSavePath>,
    terrain_registry: Res<TerrainDefRegistry>,
    type_registry: Res<AppTypeRegistry>,
) {
    // ... existing seed/version loading ...

    let mut config = VoxelMapConfig::new(
        seed, generation_version, 2, None, 5,
        Arc::new(flat_terrain_voxels), // placeholder
    );
    config.save_dir = Some(map_dir);

    let map = commands
        .spawn((
            VoxelMapInstance::new(5),
            config,
            Transform::default(),
            MapInstanceId::Overworld,
            NeedsGeneratorBuild,
        ))
        .id();

    // Apply terrain components from def
    if let Some(terrain_def) = terrain_registry.get("overworld") {
        apply_object_components(
            &mut commands, map,
            terrain_def.components.clone(),
            type_registry.0.clone(),
        );
    }

    commands.insert_resource(OverworldMap(map));
    registry.insert(MapInstanceId::Overworld, map);
}
```

#### 4. Modify `spawn_homebase`
**File**: `crates/server/src/map.rs`
**Changes**: Same pattern — look up "homebase" terrain def, apply, add `NeedsGeneratorBuild`.

#### 5. Add `build_terrain_generators` system
**File**: `crates/server/src/map.rs` (or a new `crates/server/src/terrain.rs`)
**Changes**: System that builds generators from terrain components, as described above. Register in `ServerMapPlugin`.

#### 6. Gate `ensure_pending_chunks`
**File**: `crates/voxel_map_engine/src/lifecycle.rs`
**Changes**: Add `Without<NeedsGeneratorBuild>` filter to the query in `ensure_pending_chunks`, so maps don't start generating chunks until their generator is built.

The `NeedsGeneratorBuild` component is defined in `voxel_map_engine` (not server) so the lifecycle system can reference it without a server dependency.

#### 7. Delete existing save data
Existing saved chunks were generated with `flat_terrain_voxels`. After changing the generator, old chunks would mismatch new procedural terrain.

**Approach**: Bump `GENERATION_VERSION` from 0 to 1. In `spawn_chunk_gen_task`, when loading from disk, compare the saved chunk's generation version against the config's. If mismatched, regenerate instead of using the saved data.

This requires adding `generation_version` to `ChunkData` (it's already in `VoxelMapConfig` but not persisted per-chunk). Alternatively, simpler: bump `CHUNK_SAVE_VERSION` in `persistence.rs`, which causes all old chunks to fail version validation and be regenerated. This is the simplest approach and appropriate since the generation algorithm fundamentally changed.

**File**: `crates/voxel_map_engine/src/persistence.rs`
**Changes**: Bump `CHUNK_SAVE_VERSION` from 1 to 2.

### Success Criteria:

#### Automated Verification:
- [ ] `cargo check-all` passes
- [ ] Existing tests pass (may need updates for new system parameters)

#### Manual Verification:
- [ ] `cargo server` generates non-flat overworld terrain
- [ ] `cargo client -c 1` renders the noise-based terrain
- [ ] Homebase remains flat (empty terrain def)
- [ ] Server log shows "Built terrain generator for map entity"
- [ ] Multiple materials visible on terrain surface (different biomes)

---

## Phase 4: World Object Placement

### Overview
Add `PlacementRules` component, implement Poisson disk sampling for object positions, extend `ChunkGenResult` to carry spawn descriptors, and spawn/despawn objects with their chunks.

### Changes Required:

#### 1. Add `fast_poisson` dependency
**File**: `crates/voxel_map_engine/Cargo.toml`
**Changes**: Add `fast_poisson = "0.5"`.

Note: `fast_poisson` does not compile to WASM, but object placement only runs server-side. The voxel_map_engine crate is used by the client too, so the placement code must be behind a `#[cfg(not(target_arch = "wasm32"))]` gate or in a separate module that the client doesn't compile. Since `generates_chunks = false` on the client, the placement code is never called there — but the crate still must compile for WASM.

**Alternative**: Use `poisson_diskus` which is pure Rust and WASM-compatible. Or implement Bridson's algorithm directly (it's ~50 lines).

**Decision**: Implement Bridson's algorithm directly in `terrain.rs`. It's simple, has no external dependency, and avoids WASM compilation issues entirely.

#### 2. PlacementRules component
**File**: `crates/voxel_map_engine/src/terrain.rs`
**Changes**: Add types.

```rust
/// Rules for procedural object placement. Multiple object types per map.
#[derive(Component, Clone, Debug, Serialize, Deserialize, Reflect)]
#[reflect(Component, Serialize, Deserialize)]
pub struct PlacementRules(pub Vec<PlacementRule>);

/// A single object placement rule.
#[derive(Clone, Debug, Serialize, Deserialize, Reflect)]
pub struct PlacementRule {
    /// World object definition ID (matches WorldObjectId).
    pub object_id: String,
    /// Biomes where this object may spawn.
    pub allowed_biomes: Vec<String>,
    /// Probability of accepting a candidate position (0.0–1.0).
    pub density: f64,
    /// Minimum distance between instances of this object type.
    pub min_spacing: f64,
}
```

Register `PlacementRules` and `PlacementRule` in `VoxelPlugin::build`.

#### 3. WorldObjectSpawn descriptor
**File**: `crates/voxel_map_engine/src/terrain.rs`
**Changes**: Add spawn descriptor returned from generation.

```rust
/// Descriptor for a world object to spawn at a specific position.
/// Produced by the generator, consumed by the main thread to spawn entities.
#[derive(Clone, Debug)]
pub struct WorldObjectSpawn {
    /// World object definition ID.
    pub object_id: String,
    /// World-space position.
    pub position: Vec3,
    /// Deterministic ID for this procedural spawn. Derived from
    /// `hash(seed, chunk_pos, spawn_index)`.
    pub procedural_id: u64,
}
```

#### 4. Extend ChunkGenResult
**File**: `crates/voxel_map_engine/src/generation.rs`
**Changes**: Add `world_objects` field.

```rust
pub struct ChunkGenResult {
    pub position: IVec3,
    pub mesh: Option<Mesh>,
    pub voxels: Vec<WorldVoxel>,
    pub from_disk: bool,
    pub world_objects: Vec<WorldObjectSpawn>,
}
```

Update all construction sites to include `world_objects: Vec::new()` (disk-loaded chunks and fresh generation without placement rules both produce empty vectors).

#### 5. Implement Poisson disk sampling
**File**: `crates/voxel_map_engine/src/terrain.rs`
**Changes**: Bridson's algorithm for 2D point generation within a chunk footprint.

```rust
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Deterministic per-chunk RNG seed.
fn chunk_rng_seed(world_seed: u64, chunk_pos: IVec3) -> u64 {
    let mut hasher = DefaultHasher::new();
    world_seed.hash(&mut hasher);
    chunk_pos.hash(&mut hasher);
    hasher.finish()
}

/// Generate world object spawns for a chunk using Poisson disk sampling.
pub fn generate_chunk_objects(
    chunk_pos: IVec3,
    seed: u64,
    placement_rules: &PlacementRules,
    height_cache: &[f64; 18 * 18],
    biome_ids: &[&str; 18 * 18],  // pre-computed biome ID per (x,z)
) -> Vec<WorldObjectSpawn> {
    let mut spawns = Vec::new();
    let chunk_seed = chunk_rng_seed(seed, chunk_pos);

    for (rule_idx, rule) in placement_rules.0.iter().enumerate() {
        let rule_seed = chunk_seed.wrapping_add(rule_idx as u64);
        let candidates = poisson_disk_2d(
            rule.min_spacing,
            CHUNK_SIZE as f64,
            CHUNK_SIZE as f64,
            rule_seed,
        );

        for (i, (lx, lz)) in candidates.into_iter().enumerate() {
            // Compute world position
            let world_x = chunk_pos.x as f64 * CHUNK_SIZE as f64 + lx;
            let world_z = chunk_pos.z as f64 * CHUNK_SIZE as f64 + lz;

            // Check biome at this position
            let px = (lx + 1.0) as usize; // offset for padding
            let pz = (lz + 1.0) as usize;
            let idx_2d = pz * 18 + px;
            if idx_2d >= 18 * 18 { continue; }
            let biome = biome_ids[idx_2d];
            if !rule.allowed_biomes.iter().any(|b| b == biome) { continue; }

            // Density check
            let accept_hash = chunk_rng_seed(rule_seed, IVec3::new(i as i32, 0, 0));
            let accept_prob = (accept_hash % 10000) as f64 / 10000.0;
            if accept_prob > rule.density { continue; }

            // Height at this position
            let height = height_cache[idx_2d];

            let proc_id = chunk_rng_seed(
                seed,
                IVec3::new(chunk_pos.x, rule_idx as i32, i as i32),
            );

            spawns.push(WorldObjectSpawn {
                object_id: rule.object_id.clone(),
                position: Vec3::new(world_x as f32, height as f32, world_z as f32),
                procedural_id: proc_id,
            });
        }
    }
    spawns
}
```

The `poisson_disk_2d` function implements Bridson's algorithm:

```rust
/// Bridson's Poisson disk sampling in 2D.
/// Returns a list of (x, z) points within [0, width) × [0, height) with
/// minimum spacing `r` between any two points.
fn poisson_disk_2d(r: f64, width: f64, height: f64, seed: u64) -> Vec<(f64, f64)> {
    // ... ~50 lines implementing the standard algorithm ...
    // Uses a simple LCG seeded from `seed` for determinism.
  %% Elaborate on the actual code for this
}
```

#### 6. Integrate placement into generator
**File**: `crates/voxel_map_engine/src/terrain.rs`
**Changes**: Modify `build_generator` to capture `PlacementRules` and return both voxels and object spawns. This requires changing the `VoxelGenerator` type signature.

**Problem**: `VoxelGenerator` currently returns `Vec<WorldVoxel>`. To include world object spawns, it needs to return `(Vec<WorldVoxel>, Vec<WorldObjectSpawn>)`.

**File**: `crates/voxel_map_engine/src/config.rs`
**Changes**: Update type alias.

```rust
pub type VoxelGenerator = Arc<
    dyn Fn(IVec3) -> (Vec<WorldVoxel>, Vec<WorldObjectSpawn>) + Send + Sync
>;
```

Update `flat_terrain_voxels` to return `(voxels, Vec::new())`.

Update all call sites: `generate_chunk` in `generation.rs`, any tests.

#### 7. Spawn objects when chunk loads
**File**: `crates/voxel_map_engine/src/lifecycle.rs`
**Changes**: In `handle_completed_chunk` (called from `poll_chunk_tasks`), after inserting chunk data and spawning the mesh entity, emit the `WorldObjectSpawn` list so the server can spawn the entities.

**Approach**: Store `world_objects` on the `VoxelChunk` component or emit them as events. Events are cleaner:

```rust
/// Emitted when a chunk completes generation with world object spawn requests.
#[derive(Event)]
pub struct ChunkObjectsReady {
    pub map_entity: Entity,
    pub chunk_pos: IVec3,
    pub objects: Vec<WorldObjectSpawn>,
}
```

The server listens for `ChunkObjectsReady` events and calls `spawn_world_object` for each.

#### 8. Despawn objects when chunk unloads
**File**: `crates/voxel_map_engine/src/lifecycle.rs`
**Changes**: In `despawn_out_of_range_chunks`, before despawning the `VoxelChunk` entity, also despawn all entities tagged with `ChunkEntityRef { chunk_pos, map_entity }`.

```rust
/// Tags an entity as belonging to a specific chunk. Despawned when the chunk unloads.
#[derive(Component)]
pub struct ChunkEntityRef {
    pub chunk_pos: IVec3,
    pub map_entity: Entity,
}
```

#### 9. Server-side spawn handler
**File**: `crates/server/src/world_object.rs` (or new file)
**Changes**: System that reads `ChunkObjectsReady` events and spawns world objects.

```rust
fn spawn_chunk_world_objects(
    mut commands: Commands,
    mut events: MessageReader<ChunkObjectsReady>,
    defs: Res<WorldObjectDefRegistry>,
    type_registry: Res<AppTypeRegistry>,
    // ... other params for spawn_world_object ...
) {
    for event in events.read() {
        for obj in &event.objects {
            let id = WorldObjectId(obj.object_id.clone());
            let Some(def) = defs.get(&id) else {
                warn!("Unknown world object for placement: {}", obj.object_id);
                continue;
            };
            let entity = spawn_world_object(&mut commands, id, def, ...);
            // Override position from placement
            commands.entity(entity).insert(Position(obj.position));
            // Tag with chunk ref for lifecycle management
            commands.entity(entity).insert(ChunkEntityRef {
                chunk_pos: event.chunk_pos,
                map_entity: event.map_entity,
            });
        }
    }
}
```

#### 10. Update overworld `.terrain.ron` with placement rules
**File**: `assets/terrain/overworld.terrain.ron`
**Changes**: Add `PlacementRules`.

```ron
    "voxel_map_engine::terrain::PlacementRules": ([
        (object_id: "tree_circle", allowed_biomes: ["grassland", "forest"],
         density: 0.3, min_spacing: 4.0),
    ]),
```

#### 11. Remove `spawn_test_tree`
**File**: `crates/server/src/gameplay.rs`
**Changes**: Delete `spawn_test_tree` and its registration. Trees are now spawned procedurally.

### Success Criteria:

#### Automated Verification:
- [ ] `cargo check-all` passes
- [ ] Unit tests for Poisson disk sampling (minimum spacing guarantee, determinism)
- [ ] Unit tests for `generate_chunk_objects` (correct biome filtering, density modulation)

#### Manual Verification:
- [ ] `cargo server` + `cargo client -c 1`: trees spawn across the overworld in grassland/forest biomes
- [ ] Trees do not spawn in desert biome areas
- [ ] Trees maintain minimum spacing
- [ ] Trees despawn when walking away (chunk unloads)
- [ ] Trees reappear when returning (chunk reloads, same positions)

---

## Phase 5: Per-Chunk Entity Persistence

### Overview
Persist chunk-associated entities (procedural objects with mutations, player-placed objects) to per-chunk entity files. On chunk load, merge procedural regeneration with persisted state.

### Changes Required:

#### 1. Per-chunk entity file format
**File**: `crates/voxel_map_engine/src/entity_persistence.rs` (new)
**Changes**: Types and save/load functions.

```rust
use bevy::math::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const CHUNK_ENTITY_VERSION: u32 = 1;

/// A single persisted entity within a chunk.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChunkEntity {
    /// Deterministic ID for procedurally spawned objects. `None` for player-placed.
    pub procedural_id: Option<u64>,
    /// The world object definition ID.
    pub object_id: String,
    /// Current world-space position.
    pub position: Vec3,
    /// Whether this object has been destroyed.
    pub destroyed: bool,
    /// Component overrides from baseline definition.
    /// Key: type path, Value: RON-serialized component data.
    pub component_overrides: HashMap<String, String>,
}

#[derive(Serialize, Deserialize)]
struct ChunkEntityEnvelope {
    version: u32,
    entities: Vec<ChunkEntity>,
}

pub fn chunk_entity_file_path(map_dir: &Path, chunk_pos: IVec3) -> PathBuf {
    map_dir
        .join("entities")
        .join(format!("chunk_{}_{}_{}.entities.bin", chunk_pos.x, chunk_pos.y, chunk_pos.z))
}

pub fn save_chunk_entities(
    map_dir: &Path,
    chunk_pos: IVec3,
    entities: &[ChunkEntity],
) -> Result<(), String> {
    // Same atomic write pattern as chunk voxel persistence.
    // Only write if entities is non-empty; delete file if empty.
}

pub fn load_chunk_entities(
    map_dir: &Path,
    chunk_pos: IVec3,
) -> Result<Vec<ChunkEntity>, String> {
    // Returns empty Vec if file doesn't exist.
}
```

#### 2. Extend chunk load sequence
**File**: `crates/voxel_map_engine/src/lifecycle.rs` (or `generation.rs`)
**Changes**: After chunk generation completes and `ChunkObjectsReady` is emitted, the server-side handler merges procedural spawns with persisted entity data.

The merge logic runs in the server's `spawn_chunk_world_objects` system:

```rust
fn spawn_chunk_world_objects(
    mut commands: Commands,
    mut events: MessageReader<ChunkObjectsReady>,
    defs: Res<WorldObjectDefRegistry>,
    map_query: Query<&VoxelMapConfig>,
    // ...
) {
    for event in events.read() {
        let save_dir = map_query
            .get(event.map_entity)
            .ok()
            .and_then(|c| c.save_dir.as_ref());

        // Load persisted entities for this chunk
        let persisted = save_dir
            .map(|dir| load_chunk_entities(dir, event.chunk_pos).unwrap_or_default())
            .unwrap_or_default();

        for obj in &event.objects {
            // Check if persisted data overrides this procedural spawn
            let persisted_entry = persisted.iter().find(|e| {
                e.procedural_id == Some(obj.procedural_id)
            });

            match persisted_entry {
                Some(entry) if entry.destroyed => continue, // skip destroyed
                Some(entry) => {
                    // Spawn with overridden position/components
                    spawn_with_overrides(&mut commands, entry, &defs, ...);
                }
                None => {
                    // Spawn fresh from definition
                    spawn_from_def(&mut commands, obj, &defs, ...);
                }
            }
        }

        // Spawn non-procedural entities (player-placed)
        for entry in &persisted {
            if entry.procedural_id.is_none() && !entry.destroyed {
                spawn_with_overrides(&mut commands, entry, &defs, ...);
            }
        }
    }
}
```

#### 3. Save entities on chunk unload
**File**: `crates/server/src/map.rs` (or dedicated system)
**Changes**: Before a chunk is evicted, collect all entities with matching `ChunkEntityRef`, serialize them, and write to the chunk entity file.

```rust
fn save_chunk_entities_on_unload(
    mut commands: Commands,
    removed_chunks: RemovedComponents<VoxelChunk>,
    chunk_entity_query: Query<(Entity, &ChunkEntityRef, &WorldObjectId, &Position, ...)>,
    map_query: Query<&VoxelMapConfig>,
) {
    // Listen for VoxelChunk entity despawns (via RemovedComponents)
    // OR hook into the existing remove_out_of_range_chunks flow.
}
```

**Better approach**: Emit a `ChunkUnloading` event from `despawn_out_of_range_chunks` before despawning. The server listens and saves entities.

```rust
/// Emitted before a chunk entity is despawned due to being out of range.
#[derive(Event)]
pub struct ChunkUnloading {
    pub map_entity: Entity,
    pub chunk_pos: IVec3,
}
```

Server handler:
```rust
fn persist_chunk_entities_on_unload(
    mut events: MessageReader<ChunkUnloading>,
    chunk_entity_query: Query<(&ChunkEntityRef, &WorldObjectId, &Position, Option<&Health>, ...)>,
    map_query: Query<&VoxelMapConfig>,
    type_registry: Res<AppTypeRegistry>,
) {
    for event in events.read() {
        let config = map_query.get(event.map_entity).ok();
        let Some(save_dir) = config.and_then(|c| c.save_dir.as_ref()) else { continue; };

        let entities: Vec<ChunkEntity> = chunk_entity_query
            .iter()
            .filter(|(cr, ..)| cr.chunk_pos == event.chunk_pos && cr.map_entity == event.map_entity)
            .map(|(_, obj_id, pos, ..)| {
                // Serialize component state that differs from the WorldObjectDef baseline.
                // Compare current entity components against the def's default components.
                // Only serialize differences as component_overrides.
                ChunkEntity {
                    procedural_id: /* from ProceduralSpawnId component */,
                    object_id: obj_id.0.clone(),
                    position: pos.0,
                    destroyed: false,
                    component_overrides: /* diff against baseline */,
                }
            })
            .collect();

        if let Err(e) = save_chunk_entities(save_dir, event.chunk_pos, &entities) {
            error!("Failed to save chunk entities at {}: {e}", event.chunk_pos);
        }
    }
}
```

#### 4. ProceduralSpawnId component
**File**: `crates/voxel_map_engine/src/terrain.rs`
**Changes**: Component to track the deterministic ID of procedurally spawned objects.

```rust
/// Marks an entity as procedurally spawned with a deterministic ID.
/// Used to match persisted mutation data against regenerated spawns.
#[derive(Component, Clone, Debug)]
pub struct ProceduralSpawnId(pub u64);
```

Inserted by `spawn_chunk_world_objects` when spawning procedural objects.

#### 5. Destroyed object tracking
When a world object is destroyed (e.g., tree chopped down), the system must:
1. Mark it destroyed in the ECS (remove from world or set a `Destroyed` marker)
2. When the chunk unloads, serialize it as `destroyed: true`
3. When the chunk reloads, the merge step skips spawning it

The `Destroyed` marker approach:
```rust
#[derive(Component)]
pub struct DestroyedObject;
```

When an object's health reaches 0, insert `DestroyedObject` and despawn visuals. On chunk unload, entities with `DestroyedObject` are serialized with `destroyed: true`. On chunk load, procedural spawns with matching `procedural_id` and `destroyed: true` are skipped.
%% Do we need `DestroyedObject` or `destroyed: bool` at all? Can't we just save the `Health` component which will imply "dead" by having a value of `0`?

#### 6. Component override diffing
For the initial implementation, only persist `destroyed` state and `position` changes. Full component override diffing (serialize any changed component as RON via reflection) is complex and can be added later. The `component_overrides` field stays as `HashMap<String, String>` but is initially always empty.
%% We DO want to persist serializable components, not just destroyed and position. Why is this complex? Can't we just insert the saved component values and overwrite whatever spawn-default they had? 

#### 7. Save chunk entities during world save
**File**: `crates/server/src/map.rs`
**Changes**: In `save_dirty_chunks_debounced` and `save_world_on_shutdown`, also save entities for chunks that have dirty entity state. Add a `dirty_chunk_entities: HashSet<IVec3>` to `VoxelMapInstance` (or a separate tracking component).
%% How do we know what chunk entities are dirty? This section needs elaboration

#### 8. Integrate with existing shutdown save
**File**: `crates/server/src/map.rs`
**Changes**: `save_world_on_shutdown` iterates all loaded chunks' entities and saves them.
## Elaborate

### Success Criteria:

#### Automated Verification:
- [ ] `cargo check-all` passes
- [ ] Unit tests for `save_chunk_entities` / `load_chunk_entities` (round-trip)
- [ ] Unit tests for merge logic (procedural + persisted, destroyed skipping)

#### Manual Verification:
- [ ] Destroy a tree, walk away (chunk unloads), walk back — tree stays destroyed
- [ ] Trees spawn at same positions on chunk reload (deterministic)
- [ ] Server restart: overworld terrain regenerates, persisted entity state (destroyed trees) preserved
- [ ] New trees appear in newly explored chunks

---

## Testing Strategy

### Unit Tests:
- `build_noise_fn`: deterministic output for same seed, different output for different seeds
- `generate_heightmap_chunk`: non-flat terrain with HeightMap, flat terrain without
- `select_biome`: correct biome selection across height/moisture ranges, fallback to first
- `poisson_disk_2d`: minimum spacing guarantee, determinism, coverage
- `generate_chunk_objects`: biome filtering, density acceptance, deterministic IDs
- `save/load_chunk_entities`: round-trip serialization, empty file handling, version validation
- Merge logic: procedural + persisted, destroyed skipping, override application

### Integration Tests:
- Full pipeline: terrain def loaded → generator built → chunk generated → objects spawned
- Chunk load/unload cycle preserves entity state
- Multiple map types with different terrain defs

### Manual Testing Steps:
1. Start server, connect client — verify non-flat terrain renders
2. Walk around — terrain generates continuously, different biomes visible
3. Trees/objects appear in appropriate biomes with natural spacing
4. Walk far away from trees, return — same trees at same positions
5. Destroy a tree, walk away, return — tree remains destroyed
6. Restart server — terrain regenerates identically, destroyed trees stay destroyed
7. Check homebase — still flat terrain
8. Check that server logs show terrain def loading, generator building, object spawning

## Performance Considerations

- **Per-chunk 2D noise cache**: Pre-compute 18×18 heightmap and moisture map before iterating the 3D voxel array. Avoids redundant noise evaluation per Y level. Stack-allocated `[f64; 324]`.
- **Rate-limited task spawning**: Existing `MAX_TASKS_PER_FRAME = 32` cap applies to all generation tasks.
- **Noise evaluation cost**: `Fbm<Perlin>` with 6 octaves is the most expensive component. At 324 samples per chunk (18×18 footprint), this is ~2000 noise evaluations per chunk (324 × 6 octaves). Benchmarking required to verify this stays under the async task budget.
- **Poisson disk sampling**: O(N) per chunk where N is the number of placed objects. With `min_spacing = 4.0` on a 16×16 chunk, maximum ~16 objects per rule per chunk. Negligible cost.
- **Entity file I/O**: Per-chunk entity files are small (typically < 1KB). Atomic writes are fast. No compression needed.
- **Memory**: `ChunkEntityRef` is 20 bytes per entity. For 1000 loaded world objects across all chunks, this is 20KB.

## Migration Notes

- **Existing saved worlds**: Bumping `CHUNK_SAVE_VERSION` causes all old chunks to be regenerated with the new noise-based generator. Entity persistence files don't exist yet, so no migration needed there.
- **`spawn_test_tree` removal**: The single hardcoded tree at (5, 5, 5) is replaced by procedural placement. The `tree_circle.object.ron` definition is unchanged.
- **VoxelGenerator signature change** (Phase 4): Changes from `Fn(IVec3) -> Vec<WorldVoxel>` to `Fn(IVec3) -> (Vec<WorldVoxel>, Vec<WorldObjectSpawn>)`. All existing generator closures must be updated. `flat_terrain_voxels` returns `(voxels, Vec::new())`.

## References

- Research: `doc/research/2026-03-18-procedural-map-generation.md`
- Voxel map engine plan: `doc/plans/2026-02-28-voxel-map-engine.md`
- World object RON assets: `doc/plans/2026-03-14-world-object-ron-assets.md`
- Map persistence: `doc/plans/2026-03-09-map-as-directory-saving.md`
