---
date: "2026-03-18T20:12:06-07:00"
researcher: Claude
git_commit: b5587b0827283d094a5802818ade472e0c9f0e28
branch: master
repository: bevy-lightyear-template
topic: "Procedural Map Generation System: Techniques, Crate APIs, and Integration with Existing Voxel Engine"
tags: [research, codebase, procedural-generation, noise, voxel-map-engine, world-objects, terrain, biomes]
status: complete
last_updated: "2026-03-18"
last_updated_by: Claude
last_updated_note: "Resolved open questions, corrected 2.5D framing, clarified EntityRef API and WASM support"
---

# Research: Procedural Map Generation System

**Date**: 2026-03-18T20:12:06-07:00
**Researcher**: Claude
**Git Commit**: b5587b0827283d094a5802818ade472e0c9f0e28
**Branch**: master
**Repository**: bevy-lightyear-template

## Research Question

How to implement a procedural generation system for Maps, modelled after Minecraft and/or Veloren's terrain generation system, including spawning world objects (trees, resources, wildlife, settlements). Research proc gen techniques and how we can use them here. This replaces the current hard-coded generator function for each Map type with data-driven asset-loaded configuration data passed to maps on instantiation. Use `noise` crate for noise generation.

## Summary

The codebase already has a clean `VoxelGenerator` abstraction (`Arc<dyn Fn(IVec3) -> Vec<WorldVoxel>>`) in the voxel map engine, but currently only one generator exists: `flat_terrain_voxels` which produces a flat plane at y=0. The existing `WorldObjectDef` pattern — reflect-based RON deserialization of a component map (`HashMap<String, Box<dyn PartialReflect>>`) — provides the ideal pattern for terrain configuration. Each terrain aspect (heightmap, moisture, biomes, object placement) becomes a reflected ECS component, and map definitions are authored as `.terrain.ron` files using the same format as `.object.ron`. The `noise` crate (v0.9.0) provides all needed noise primitives (Perlin, Fbm, RidgedMulti, Select, etc.) with composable combinators. This research covers: the current generation system, Minecraft/Veloren techniques, `noise` crate API, archetype-based config design, world object placement algorithms, and 2.5D-specific simplifications.

## Detailed Findings

### 1. Current Generation System

#### VoxelGenerator Type

`voxel_map_engine/src/config.rs:9`:
```rust
pub type VoxelGenerator = Arc<dyn Fn(IVec3) -> Vec<WorldVoxel> + Send + Sync>;
```

- Input: chunk position in chunk coordinates
- Output: flat `Vec<WorldVoxel>` of size 5832 (18^3 padded: 16^3 + 1 voxel padding per side)
- `Arc` allows cheap cloning for async tasks

#### Single Implementation: flat_terrain_voxels

`voxel_map_engine/src/meshing.rs:66-76`:
```rust
pub fn flat_terrain_voxels(chunk_pos: IVec3) -> Vec<WorldVoxel> {
    let mut voxels = vec![WorldVoxel::Air; PaddedChunkShape::USIZE];
    for i in 0..PaddedChunkShape::SIZE {
        let [_x, y, _z] = PaddedChunkShape::delinearize(i);
        let world_y = chunk_pos.y * CHUNK_SIZE as i32 + y as i32 - 1;
        if world_y <= 0 {
            voxels[i as usize] = WorldVoxel::Solid(0);
        }
    }
    voxels
}
```

No noise, no seed usage. Just `Solid(0)` below y=0, `Air` above.

#### VoxelMapConfig

`voxel_map_engine/src/config.rs:13-26`:

| Field | Type | Purpose |
|---|---|---|
| `seed` | `u64` | Seed for generation (unused by flat_terrain_voxels) |
| `generation_version` | `u32` | Algorithm version for save compat |
| `spawning_distance` | `u32` | Chunk loading radius |
| `bounds` | `Option<IVec3>` | Bounded maps (homebases/arenas) vs unbounded (overworld) |
| `tree_height` | `u32` | Octree height (5=overworld, 3=homebase/arena) |
| `generator` | `VoxelGenerator` | The closure |
| `save_dir` | `Option<PathBuf>` | Persistence directory |
| `generates_chunks` | `bool` | Server=true, client=false |

#### WorldVoxel Type

`voxel_map_engine/src/types.rs:14-18`:
```rust
pub enum WorldVoxel {
    Air,
    Unset,
    Solid(u8),  // u8 = material index
}
```

Only `Solid(0)` is used currently. The `u8` material index enables different block types (stone, dirt, grass, sand, etc.) without any code changes.

#### Chunk Lifecycle

1. `ChunkTarget` entities (players) drive demand
2. `update_chunks` computes desired positions around targets
3. Missing chunks spawn async tasks on `AsyncComputeTaskPool`
4. Each task: try disk load, fallback to `generator(chunk_pos)`
5. Result: voxels meshed via greedy quads (`block_mesh` crate), child entity spawned
6. Out-of-range chunks evicted and despawned

#### How Generators Are Passed

Server (`server/src/map.rs:105-112`):
```rust
let mut config = VoxelMapConfig::new(
    seed, generation_version, 2, None, 5,
    Arc::new(flat_terrain_voxels),
);
```

Client mirrors this but sets `generates_chunks = false`.

#### Current World Object System

- `WorldObjectDef` stores reflected components as `Vec<Box<dyn PartialReflect>>`
- Loaded from `.object.ron` files via manifest + `WorldObjectLoader`
- Only one definition exists: `tree_circle.object.ron` with hardcoded `Position((5.0, 5.0, 5.0))`
- `spawn_test_tree` (gameplay.rs:236) spawns exactly one tree on `AppState::Ready`
- No procedural placement logic exists
- World objects are replicated via Lightyear rooms (server spawns, clients attach visuals)

### 2. Minecraft's Terrain Generation Pipeline

Minecraft uses a multi-phase pipeline:

**Phase 1 -- Base Terrain Shape**: 3D density functions determine solid vs air. Positive density = solid, negative = air. Uses layered Perlin noise with octaves.

**Phase 2 -- Biome Placement**: Five independent noise parameters per position: Temperature, Humidity, Continentalness, Erosion, Weirdness. Biomes selected by nearest-neighbor lookup in parameter space.

**Phase 3 -- Surface Rules**: Replaces surface stone with biome-appropriate materials (grass, sand, etc.).

**Phase 4 -- Carving**: Perlin Worm caves, noise-based cheese/spaghetti/noodle caves.

**Phase 5 -- Population**: Structures first (villages, strongholds), then features (trees, ores, grass).

**Noise Router**: Core orchestration -- a collection of composable density functions routing noise to terrain aspects. Density functions are composable operations: constants, noise sampling, math (add/mul/min/max), control flow (range_choice), transforms (clamp, spline).

**Key Takeaway**: Since 1.18+, Minecraft's worldgen is fully data-driven via JSON data packs. Every noise definition, density function, biome, and structure is defined in configuration files.

### 3. Veloren's Terrain Generation

Veloren is an open-source Rust voxel RPG with a different approach:

- **Erosion-simulation-based**: Initial heightmap from noise, then 100 iterations of tectonic uplift + fluvial erosion
- **River networks**: Computed from erosion simulation using Planchon-Darboux algorithm
- **Statistical noise normalization**: Pre-sorts noise values for uniform distribution control
- **Biomes**: Derived from continuous climate parameters (temp, humidity, altitude)
- **World simulation**: Sites (villages, ruins, dungeons), trade routes, wildlife spawning based on biome
- **Lazy generation**: Voxels determined only when a player loads the area

**Key Takeaway**: Veloren's approach is much more simulation-heavy and compute-expensive than Minecraft's. For a 2.5D brawler, the Minecraft-style layered noise approach is more appropriate -- simpler, faster, and the composable density function pattern maps well to data-driven configuration.

### 4. The `noise` Rust Crate (v0.9.0)

#### Base Noise Generators (all implement `NoiseFn`)

| Struct | Description |
|---|---|
| `Perlin` | Classic gradient noise, good general-purpose |
| `OpenSimplex` | Higher quality than Perlin (less axis artifacts), slightly slower |
| `Value` | Lower quality, faster |
| `Worley` | Cell/Voronoi noise -- good for cell-like patterns, settlement regions |
| `SuperSimplex` | Improved simplex variant |

#### Fractal Combinators (layer multiple octaves)

| Struct | Description | Use Case |
|---|---|---|
| `Fbm<T>` | Fractal Brownian Motion -- standard layered noise | General terrain, clouds |
| `RidgedMulti<T>` | Like fBm but with `abs()` per octave, creates ridges | Mountain ridges, cliff edges |
| `HybridMulti<T>` | Blend of fBm and multifractal | Natural terrain: smooth valleys, rough peaks |
| `BasicMulti<T>` | Basic multifractal | General purpose |

Configurable parameters: `octaves`, `frequency`, `lacunarity`, `persistence`, `seed`.

#### Modifier/Combinator Functions

| Struct | Purpose |
|---|---|
| `ScaleBias` | `output * scale + bias` |
| `Clamp` | Clamp to [min, max] |
| `Abs` | Absolute value |
| `Terrace` | Staircase curve -- creates flat plateaus |
| `Select` | Choose between two sources based on control noise |
| `Blend` | Weighted blend of two sources |
| `Add`, `Multiply`, `Min`, `Max` | Arithmetic combinators |
| `Turbulence` | Random displacement using internal Perlin noise |
| `TranslatePoint`, `ScalePoint` | Coordinate transforms before sampling |

#### Usage Example

```rust
use noise::{Fbm, Perlin, RidgedMulti, Select, NoiseFn};

let fbm = Fbm::<Perlin>::new(seed)
    .set_octaves(6)
    .set_frequency(0.002)
    .set_lacunarity(2.0)
    .set_persistence(0.5);

let ridged = RidgedMulti::<Perlin>::new(seed + 1)
    .set_octaves(6)
    .set_frequency(0.002);

let terrain = Select::new(fbm, ridged, Perlin::new(seed + 2))
    .set_bounds(0.0, 1.0)
    .set_falloff(0.5);

// Sample at world coordinates (scaled by frequency)
let height: f64 = terrain.get([world_x as f64 * 0.01, world_z as f64 * 0.01]);
```

### 5. Data-Driven Configuration Design (Archetype-Based)

The existing `WorldObjectDef` pattern — a flat map of reflected components loaded from RON via `reflect_loader` — is the blueprint. Instead of a monolithic `TerrainConfig` struct, each terrain aspect is a standalone reflected ECS component. A `.terrain.ron` file is a component map, identical in format to `.object.ron`. Map types are differentiated by which components are present (like ECS archetypes), not by fields in a single struct.

#### Why Archetype-Based

- **Reuses the existing reflect loader** — `deserialize_world_object` already parses `HashMap<String, Box<dyn PartialReflect>>` from RON. No new asset type or loader needed.
- **Authoring is identical** to `.object.ron` — designers learn one format.
- **Composition through presence/absence** — a homebase that omits `HeightMap` and `BiomeRules` gets flat terrain. An arena can have `HeightMap` but no `PlacementRules`. No `Option` fields or feature flags.
- **Extensible** — adding a new terrain feature (e.g., cave carving, river paths) means registering a new component type, not changing a shared struct that breaks all existing configs.

#### Component Types

```rust
/// Noise sampling parameters. Embedded in noise-driven components.
#[derive(Clone, Serialize, Deserialize, Reflect)]
pub struct NoiseDef {
    pub noise_type: NoiseType,       // Perlin, OpenSimplex, Value, Worley
    pub fractal: FractalType,        // Fbm, RidgedMulti, HybridMulti, None
    pub seed_offset: u32,
    pub octaves: u32,
    pub frequency: f64,
    pub lacunarity: f64,
    pub persistence: f64,
}

/// Drives terrain height. Presence = noise-based terrain. Absence = flat.
#[derive(Component, Clone, Serialize, Deserialize, Reflect)]
#[reflect(Component, Serialize, Deserialize)]
pub struct HeightMap {
    pub noise: NoiseDef,
    pub base_height: i32,
    pub amplitude: f64,
}

/// Drives biome moisture sampling. Only useful alongside BiomeRules.
#[derive(Component, Clone, Serialize, Deserialize, Reflect)]
#[reflect(Component, Serialize, Deserialize)]
pub struct MoistureMap {
    pub noise: NoiseDef,
}

/// Biome selection rules. Vec because multiple biomes per map.
#[derive(Component, Clone, Serialize, Deserialize, Reflect)]
#[reflect(Component, Serialize, Deserialize)]
pub struct BiomeRules(pub Vec<BiomeRule>);

#[derive(Clone, Serialize, Deserialize, Reflect)]
pub struct BiomeRule {
    pub biome_id: String,
    pub height_range: (f64, f64),
    pub moisture_range: (f64, f64),
    pub surface_material: u8,
    pub subsurface_material: u8,
    pub subsurface_depth: u32,
}

/// Object placement rules. Vec because multiple object types per map.
#[derive(Component, Clone, Serialize, Deserialize, Reflect)]
#[reflect(Component, Serialize, Deserialize)]
pub struct PlacementRules(pub Vec<PlacementRule>);

#[derive(Clone, Serialize, Deserialize, Reflect)]
pub struct PlacementRule {
    pub object_id: String,
    pub allowed_biomes: Vec<String>,
    pub density: f64,
    pub min_spacing: f64,
    pub slope_max: Option<f64>,
}

```

`DefaultMaterial` is removed. It's unnecessary complexity — the generator always has a sensible fallback: `Solid(0)`. If a map needs a specific material without biome rules, a `HeightMap` with `amplitude: 0.0` produces flat terrain at `base_height` with the surface material coming from a single-entry `BiomeRules`. This keeps the component set smaller and avoids the ambiguity of "what if neither `HeightMap` nor `DefaultMaterial` is present?" — the answer is simply: no `HeightMap` = flat terrain at y=0 with `Solid(0)`, same as current `flat_terrain_voxels`. If `BiomeRules` is present, the matched biome's material is used; if absent, `Solid(0)`.

Note: `NoiseDef`, `BiomeRule`, and `PlacementRule` are plain data structs (not components). Only the top-level types that appear as keys in RON need `#[derive(Component)]` and `#[reflect(Component)]`.

#### RON Examples

Overworld — full-featured terrain:
```ron
// assets/terrain/overworld.terrain.ron
{
    "terrain::HeightMap": (
        noise: (noise_type: Perlin, fractal: Fbm, seed_offset: 0,
                octaves: 6, frequency: 0.005, lacunarity: 2.0, persistence: 0.5),
        base_height: 0,
        amplitude: 20.0,
    ),
    "terrain::MoistureMap": (
        noise: (noise_type: Perlin, fractal: Fbm, seed_offset: 100,
                octaves: 4, frequency: 0.008, lacunarity: 2.0, persistence: 0.6),
    ),
    "terrain::BiomeRules": ([
        (biome_id: "grassland", height_range: (0.3, 0.6), moisture_range: (0.3, 0.7),
         surface_material: 1, subsurface_material: 2, subsurface_depth: 3),
        (biome_id: "desert", height_range: (0.3, 0.6), moisture_range: (0.0, 0.3),
         surface_material: 3, subsurface_material: 3, subsurface_depth: 5),
        (biome_id: "forest", height_range: (0.35, 0.65), moisture_range: (0.7, 1.0),
         surface_material: 1, subsurface_material: 2, subsurface_depth: 4),
    ]),
    "terrain::PlacementRules": ([
        (object_id: "tree_circle", allowed_biomes: ["grassland", "forest"],
         density: 0.3, min_spacing: 4.0, slope_max: Some(0.3)),
    ]),
}
```

Homebase — flat, no noise (empty config = `flat_terrain_voxels` with `Solid(0)`):
```ron
// assets/terrain/homebase.terrain.ron
{
}
```

Arena — heightmap terrain, single material via single-entry biome rules, no object placement:
```ron
// assets/terrain/arena_hills.terrain.ron
{
    "terrain::HeightMap": (
        noise: (noise_type: Perlin, fractal: Fbm, seed_offset: 0,
                octaves: 3, frequency: 0.02, lacunarity: 2.0, persistence: 0.4),
        base_height: 0,
        amplitude: 5.0,
    ),
    "terrain::BiomeRules": ([
        (biome_id: "sand", height_range: (-1.0, 1.0), moisture_range: (0.0, 1.0),
         surface_material: 2, subsurface_material: 2, subsurface_depth: 99),
    ]),
}
```

#### Integration with Existing Asset Pipeline

The `WorldObjectDef` asset type and loader (`reflect_loader.rs` + `world_object/loading.rs`) already:
1. Parses `HashMap<String, Box<dyn PartialReflect>>` from RON
2. Stores components as `Vec<Box<dyn PartialReflect>>`
3. Inserts components onto entities via `apply_object_components`

Terrain configs follow the same path. A `TerrainDef` is structurally identical to `WorldObjectDef`:

```rust
/// A loaded terrain definition. Structurally identical to WorldObjectDef.
#[derive(Asset, TypePath)]
pub struct TerrainDef {
    pub components: Vec<Box<dyn PartialReflect>>,
}
```

Loaded with the same `reflect_loader::load_reflect_map` function, registered as `.terrain.ron`. At map spawn time, components are inserted onto the map entity via `apply_object_components`, then the generator is built from whatever components are present:

```rust
fn build_generator(entity: EntityRef, seed: u64) -> VoxelGenerator {
    // EntityRef::get::<T>() returns Option<&T> — None if the component isn't present.
    // This is the standard Bevy API for optional component access on a read-only entity ref.
    // .cloned() converts Option<&T> → Option<T> so the data can be moved into the closure.
    let height = entity.get::<HeightMap>().cloned();
    let moisture = entity.get::<MoistureMap>().cloned();
    let biomes = entity.get::<BiomeRules>().cloned();

    Arc::new(move |chunk_pos| {
        match &height {
            Some(h) => generate_heightmap_chunk(
                chunk_pos, seed, h, moisture.as_ref(), biomes.as_ref(),
            ),
            None => flat_terrain_voxels(chunk_pos), // no HeightMap = flat at y=0, Solid(0)
        }
    })
}
```

Composition through presence: no `HeightMap` = flat terrain at y=0 with `Solid(0)`. No `BiomeRules` = `Solid(0)`. No `PlacementRules` = no procedural objects. Each concern is independent.

### 6. World Object Placement Techniques

#### Poisson Disk Sampling (Primary Technique)

Produces natural-looking distributions with guaranteed minimum spacing. Bridson's algorithm runs in O(N):

1. Place initial random sample
2. For each active sample, generate k candidates in annular region [r, 2r]
3. Accept if no existing sample within distance r
4. Mark inactive if all k candidates rejected

**Rust crates**: `fast_poisson` (iterator-based, simplest API) and `poisson_diskus` (arbitrary dimensions). Note: `fast_poisson` does NOT compile to WASM due to its `kiddo` (K-D tree) dependency having platform-specific code; `poisson_diskus` is pure-math Rust and does compile to WASM. However, object placement only runs server-side (server spawns objects, clients receive via Lightyear replication), so WASM support is not a constraint for crate selection.

#### Density-Modulated Placement

1. Generate density map from noise (0.0-1.0)
2. Run Poisson disk sampling for candidate positions
3. Accept each candidate with probability proportional to density value
4. Result: naturally spaced objects that cluster in high-density areas

#### Per-Chunk Deterministic Generation

For chunk-based worlds, object placement must be deterministic per chunk:

1. Seed a per-chunk RNG from `hash(world_seed, chunk_pos)`
2. Generate candidates within chunk bounds
3. Check spacing against objects in neighboring chunks (need to sample adjacent chunks too)
4. Store placed objects as part of chunk generation result

#### Constraint-Based Placement

Beyond spacing:
- **Biome constraints**: trees in forest/grassland, cacti in desert
- **Slope constraints**: no objects on steep terrain
- **Altitude constraints**: no trees above treeline
- **Exclusion zones**: no objects inside structure footprints

#### Object Placement Rule

See `PlacementRule` in the archetype component types (section 5). The `PlacementRules` component on a map entity holds a `Vec<PlacementRule>`, each specifying an object type, allowed biomes, density, spacing, and constraints. Density can optionally be modulated by a `NoiseDef` for spatial variation.

### 7. Terrain Generation Scope

The world uses full 3D voxels (the voxel engine is genuinely 3D), and world objects can exist at any 3D position. The generation approach aligns closely with Minecraft and Veloren rather than diverging from them:

**Heightmap-based surface generation (same as Minecraft/Veloren):**
- Both Minecraft and Veloren generate surface terrain by sampling 2D noise at (x, z) to produce a height value, then filling voxels below that height. Our approach is the same.
- Biome selection is also 2D in both games — Minecraft samples temperature/humidity/continentalness at (x, z) intervals; Veloren uses 2D climate maps. Our approach matches.
- Object placement (trees, structures) in both games determines (x, z) positions on the 2D surface, then places at terrain height. Our approach matches.

**What we skip for now:**
- **3D cave/tunnel generation**: Minecraft uses Perlin worm carvers and cheese/spaghetti/noodle caves. These could be added later as a `CaveCarver` component on the terrain archetype. Caves could also be separate instanced maps rather than carved into the overworld.
- **3D biome variation**: Minecraft 1.18+ added Y-axis biome variation for deep caves (deep dark, lush caves). Not needed unless underground biomes are added.

**What is identical to Minecraft/Veloren:**
- Noise configuration and layering
- Poisson disk sampling for object placement
- Data-driven config pattern
- Per-chunk deterministic generation
- 2D surface sampling for height, biomes, and object positions

**Terrain generation function:**

For each voxel in a chunk, the generator needs:
1. Compute world x,z coordinates
2. Sample heightmap noise at (x, z)
3. Sample moisture noise at (x, z) for biome selection
4. Determine biome from height + moisture thresholds
5. Fill voxels: surface material from biome config, subsurface material below, air above

```rust
fn generate_heightmap_chunk(
    chunk_pos: IVec3,
    seed: u64,
    height_map: &HeightMap,
    moisture_map: Option<&MoistureMap>,
    biome_rules: Option<&BiomeRules>,
) -> Vec<WorldVoxel> {
    let height_noise = build_noise_fn(&height_map.noise, seed);
    let moisture_noise = moisture_map.map(|m| build_noise_fn(&m.noise, seed));

    let mut voxels = vec![WorldVoxel::Air; PaddedChunkShape::USIZE];
    for i in 0..PaddedChunkShape::SIZE {
        let [x, y, z] = PaddedChunkShape::delinearize(i);
        let world_x = chunk_pos.x * CHUNK_SIZE as i32 + x as i32 - 1;
        let world_y = chunk_pos.y * CHUNK_SIZE as i32 + y as i32 - 1;
        let world_z = chunk_pos.z * CHUNK_SIZE as i32 + z as i32 - 1;

        let height = height_map.base_height as f64
            + height_noise.get([world_x as f64, world_z as f64]) * height_map.amplitude;

        if world_y as f64 <= height {
            let material = match (biome_rules, &moisture_noise) {
                (Some(rules), Some(mn)) => {
                    let moisture = mn.get([world_x as f64, world_z as f64]);
                    let biome = select_biome(&rules.0, height, moisture);
                    let depth = (height - world_y as f64) as u32;
                    if depth < biome.subsurface_depth {
                        biome.surface_material
                    } else {
                        biome.subsurface_material
                    }
                }
                _ => 0, // no biome rules = Solid(0)
            };
            voxels[i as usize] = WorldVoxel::Solid(material);
        }
    }
    voxels
}
```

### 8. Map Type Differentiation via Component Presence

The archetype-based approach naturally differentiates map types by which components their `.terrain.ron` includes:

| Map Type | HeightMap | MoistureMap | BiomeRules | PlacementRules | Result |
|---|---|---|---|---|---|
| **Overworld** | yes | yes | yes | yes | Full noise terrain, biomes, object spawning |
| **Homebase** | -- | -- | -- | -- | Flat terrain at y=0, `Solid(0)`, player-editable |
| **Arena (flat)** | -- | -- | -- | -- | Flat terrain at y=0, `Solid(0)` |
| **Arena (hills)** | yes | -- | 1-entry | -- | Noise terrain, single material, no objects |
| **Arena (biome)** | yes | yes | yes | -- | Noise terrain, biomes, no object spawning |

The `VoxelMapInstance` bundle constructors already accept a `VoxelGenerator`. The change:
1. Load `TerrainDef` assets during startup (same loader as `WorldObjectDef`)
2. At map spawn time, apply the terrain def's components onto the map entity
3. Build a `VoxelGenerator` closure from whatever components are present on the entity
4. Pass it to `VoxelMapConfig::new()`

Future terrain features (cave carving, river paths, structure footprints) are added by registering new component types and handling their presence in the generator builder — existing `.terrain.ron` files that omit the new component are unaffected.

### 9. Per-Chunk Entity Persistence

#### Current State

The existing entity persistence system saves all entities for a map in a single flat `entities.bin` file. Only `RespawnPoint` entities are persisted (via `SavedEntityKind::RespawnPoint`). There is no spatial partitioning, no entity-chunk association, and no concept of entities belonging to specific chunks. This works for global map-level entities (respawn points) but cannot support procedurally spawned world objects that load/unload with their chunks.

#### Design: Position-Based Chunk Association (Minecraft Model)

Following Minecraft's 1.17+ architecture, entity persistence is spatially partitioned by chunk:

**Core principle**: An entity belongs to whichever chunk its current position falls in. When it moves across a chunk boundary, it is re-indexed to the new chunk. On save, it is written to the new chunk's entity file. The old chunk retains no record of it.

**On-disk format**: Per-chunk entity files stored alongside terrain data:

```
worlds/overworld/
  terrain/
    chunk_0_0_0.bin           # voxel data (existing)
  entities/
    chunk_0_0_0.entities.bin  # entities in this chunk
    chunk_1_0_2.entities.bin
```

```rust
/// A single persisted entity within a chunk.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ChunkEntity {
    /// Deterministic ID for procedurally spawned objects. `None` for player-placed
    /// or non-procedural entities.
    pub procedural_id: Option<u64>,
    /// The world object definition ID (e.g., "tree_circle").
    pub object_id: WorldObjectId,
    /// Current world-space position.
    pub position: Vec3,
    /// Whether this object has been destroyed (pending respawn or permanent).
    pub destroyed: bool,
    /// Component overrides — only components that differ from the `WorldObjectDef`
    /// baseline. Same format as `.object.ron` files: type path → RON data.
    /// On load, the baseline is reconstructed from the def, then these overrides
    /// are applied via `apply_object_components`.
    ///
    /// Empty map = no modifications from baseline.
    pub component_overrides: HashMap<String, String>,
}

/// Versioned file envelope (same pattern as existing ChunkFileEnvelope).
#[derive(Serialize, Deserialize)]
struct ChunkEntityEnvelope {
    version: u32,
    entities: Vec<ChunkEntity>,
}
```

No bespoke mutation types. Changed state is captured by serializing the actual ECS components that differ from the `WorldObjectDef` baseline, using the same `HashMap<type_path, ron_data>` format as `.object.ron` files. This means:

- **Same format everywhere** — `.object.ron` files, terrain defs, and entity persistence all use `HashMap<String, String>` keyed by type path
- **New mutable properties require zero persistence code changes** — register the component with reflect, and it serializes automatically
- **`apply_object_components` is reused** for both initial spawn (from `.object.ron`) and restore (from `component_overrides`)
- **Destroyed objects** use the `destroyed` bool — simpler than component-based "destroyed" state since a destroyed object has no entity to hold components

#### Chunk Load Sequence

When a chunk loads (in `poll_chunk_tasks` after generation completes):

1. **Generate procedural objects**: The generator returns `Vec<WorldObjectSpawn>` for this chunk (from `PlacementRules` + Poisson disk sampling with seed `hash(map_seed, chunk_pos)`)
2. **Load entity file**: Read `entities/chunk_X_Y_Z.entities.bin` if it exists
3. **Merge**:
   - For each procedural spawn, check if a `ChunkEntity` with matching `procedural_id` exists in the entity file
     - If `destroyed = true` → skip spawning
     - If `component_overrides` is non-empty → spawn from def baseline, then apply overrides via `apply_object_components`
     - If the entity file has a different position → use the entity file's position (it was moved)
   - No matching entry → spawn at procedural position with default state from `WorldObjectDef`
   - Entity file entries with `procedural_id = None` → non-procedural entities (player-placed), spawn from def + all stored components
4. **Spawn ECS entities**: Call `spawn_world_object` for each, tag with `ChunkEntityRef { chunk_pos }` component

#### Chunk Unload Sequence

When a chunk unloads (in `despawn_out_of_range_chunks`):

1. **Collect entities**: Query all entities with `ChunkEntityRef { chunk_pos }` matching the unloading chunk
2. **Serialize**: Build `Vec<ChunkEntity>` from their current state
3. **Write**: Save to `entities/chunk_X_Y_Z.entities.bin` (bincode + atomic rename, same as existing pattern)
4. **Despawn**: Remove the ECS entities

#### Cross-Chunk Movement

A system runs each tick (or each N ticks for performance) checking if any chunk-associated entity has moved to a different chunk:

```rust
/// Re-indexes entities that have crossed chunk boundaries.
fn reindex_chunk_entities(
    mut commands: Commands,
    query: Query<(Entity, &Position, &mut ChunkEntityRef), Changed<Position>>,
) {
    for (entity, position, mut chunk_ref) in &mut query {
        let new_chunk = voxel_to_chunk_pos(position.0.as_ivec3());
        if new_chunk != chunk_ref.chunk_pos {
            chunk_ref.chunk_pos = new_chunk;
        }
    }
}
```

When the old chunk unloads, this entity is no longer in its entity list. When the new chunk unloads, the entity is saved there. Position is truth — no special migration logic needed.

#### Entity Types and Their Persistence Behavior

| Entity Type | Procedural? | Moves? | Persistence Strategy |
|---|---|---|---|
| **Scenery** (trees, rocks) | Yes | No | Regenerate from seed; mutation overlay for destroyed/modified |
| **Resource nodes** | Yes | No | Same as scenery; mutation tracks harvest state |
| **Wildlife/NPCs** | Yes (spawner) | Yes | Per-chunk entity file at current position; despawn rules prevent drift |
| **Player-placed objects** | No | No | Per-chunk entity file; `procedural_id = None` |
| **Respawn points** | No | No | Keep existing flat `entities.bin` (map-global, small count) |

#### Relationship to Existing `entities.bin`

The existing flat `entities.bin` per-map system continues to handle **map-global entities** that should always be loaded regardless of chunk state (respawn points, map metadata entities). The new per-chunk entity system handles **spatially-bound entities** that should load/unload with terrain.

The two systems are orthogonal:
- `entities.bin`: loaded once at map spawn, saved on map save. Entities have `MapSaveTarget` component.
- `entities/chunk_X_Y_Z.entities.bin`: loaded/saved with chunk lifecycle. Entities have `ChunkEntityRef` component.

#### Integration Points

Existing code that needs changes:

- **`ChunkGenResult`** (`voxel_map_engine/src/generation.rs`): Add `world_objects: Vec<WorldObjectSpawn>` field
- **`poll_chunk_tasks`** (`voxel_map_engine/src/lifecycle.rs`): After inserting chunk voxels, load entity file and spawn world objects
- **`despawn_out_of_range_chunks`** (`voxel_map_engine/src/lifecycle.rs`): Before despawning chunk, collect and save associated entities
- **`save_dirty_chunks_debounced`** (`server/src/map.rs`): Also save dirty chunk entities (entities that have been modified since load)
- **`save_world_on_shutdown`** (`server/src/map.rs`): Save all loaded chunk entities
- **`spawn_chunk_gen_task`** (`voxel_map_engine/src/generation.rs`): Generator closure also computes world object spawn positions from `PlacementRules`

New code:

- `ChunkEntityRef` component: `{ chunk_pos: IVec3, map_entity: Entity }`
- `reindex_chunk_entities` system: re-indexes on position change
- `save_chunk_entities` / `load_chunk_entities` functions (parallel to existing `save_chunk` / `load_chunk`)
- Entity file path function: `chunk_entity_file_path(map_dir, chunk_pos) -> PathBuf`

## Code References

### Voxel Map Engine
- `crates/voxel_map_engine/src/config.rs:9` -- `VoxelGenerator` type alias
- `crates/voxel_map_engine/src/config.rs:13-26` -- `VoxelMapConfig` struct
- `crates/voxel_map_engine/src/meshing.rs:66-76` -- `flat_terrain_voxels` (only generator)
- `crates/voxel_map_engine/src/generation.rs:29-63` -- `spawn_chunk_gen_task` (async generation)
- `crates/voxel_map_engine/src/generation.rs:65-74` -- `generate_chunk` (calls generator)
- `crates/voxel_map_engine/src/instance.rs:51-103` -- Bundle constructors (overworld/homebase/arena)
- `crates/voxel_map_engine/src/types.rs:14-18` -- `WorldVoxel` enum (Air/Unset/Solid(u8))
- `crates/voxel_map_engine/src/lifecycle.rs:62-93` -- `update_chunks` (demand-driven loading)

### Server Map
- `crates/server/src/map.rs:93-130` -- `spawn_overworld` (creates config with `flat_terrain_voxels`)
- `crates/server/src/map.rs:780-800` -- Homebase map creation (also `flat_terrain_voxels`)
- `crates/server/src/gameplay.rs:236-259` -- `spawn_test_tree` (hardcoded single tree spawn)

### Client Map
- `crates/client/src/map.rs:584-591` -- `generator_for_map` (matches MapInstanceId to generator)

### World Object System
- `crates/protocol/src/world_object/types.rs:46-49` -- `WorldObjectDef` struct
- `crates/protocol/src/world_object/types.rs:10-11` -- `WorldObjectId`
- `crates/protocol/src/world_object/spawn.rs:8-38` -- `apply_object_components`
- `crates/server/src/world_object.rs:21-51` -- `spawn_world_object`
- `crates/client/src/world_object.rs:30-65` -- `on_world_object_replicated`
- `assets/objects/tree_circle.object.ron` -- Only world object definition

### Asset Loading Pattern
- `crates/protocol/src/ability/loading.rs` -- Established manifest+folder loading pattern
- `crates/protocol/src/world_object/loading.rs` -- World object loading pipeline
- `crates/protocol/src/reflect_loader.rs` -- Shared reflect-based RON deserialization
- `crates/protocol/src/app_state.rs` -- `TrackedAssets` and `AppState` transitions

### Entity Persistence (Existing)
- `crates/protocol/src/map/persistence.rs:1-19` -- `SavedEntity`, `SavedEntityKind`, `MapSaveTarget`
- `crates/server/src/persistence.rs:67-107` -- `save_entities` / `load_entities` (flat `entities.bin` per map)
- `crates/server/src/persistence.rs:12-65` -- `MapMeta`, `save_map_meta`, `load_map_meta`
- `crates/server/src/persistence.rs:21-28` -- `WorldSavePath` resource
- `crates/server/src/persistence.rs:31-36` -- `map_save_dir` (per-map directory resolution)
- `crates/server/src/map.rs:247-287` -- `collect_and_save_entities` (groups by MapInstanceId)
- `crates/server/src/map.rs:290-321` -- `load_map_entities` / `load_startup_entities`
- `crates/voxel_map_engine/src/persistence.rs` -- Chunk persistence (save/load/delete per chunk file)
- `crates/voxel_map_engine/src/lifecycle.rs:139-163` -- `remove_out_of_range_chunks` (saves dirty chunks on eviction)

## Architecture Documentation

### Current Generation Architecture

```
VoxelMapConfig.generator (Arc<dyn Fn(IVec3) -> Vec<WorldVoxel>>)
    │
    ├── Server: Arc::new(flat_terrain_voxels)
    │     └── Used by: spawn_overworld(), homebase/arena creation
    │
    └── Client: Arc::new(flat_terrain_voxels) [generates_chunks=false]
          └── Used by: generator_for_map() during map transitions
```

### Proposed Generation Architecture (Archetype-Based)

```
.terrain.ron (component map, same format as .object.ron)
    │
    ├── "terrain::HeightMap"      ─┐
    ├── "terrain::MoistureMap"     │  Presence/absence determines
    ├── "terrain::BiomeRules"      │  terrain behavior (archetype)
    └── "terrain::PlacementRules" ─┘
         │
         ▼  apply_object_components() inserts onto map entity
Map Entity
    ├── VoxelMapInstance
    ├── VoxelMapConfig
    ├── HeightMap?          ← present = noise terrain; absent = flat at y=0
    ├── MoistureMap?        ← present = moisture sampling for biome selection
    ├── BiomeRules?         ← present = biome-driven materials; absent = Solid(0)
    └── PlacementRules?     ← present = procedural object spawning
         │
         ▼  build_generator(entity, seed) reads components
VoxelMapConfig.generator (same Arc<dyn Fn> interface)
```

### Asset Loading Integration

```
assets/terrain/overworld.terrain.ron     ← full archetype (all components)
assets/terrain/homebase.terrain.ron      ← empty (flat terrain fallback)
assets/terrain/arena_hills.terrain.ron   ← HeightMap + BiomeRules(1-entry)
    │
    ▼
reflect_loader::load_reflect_map()   ← same loader as WorldObjectDef
    │
    ▼
TerrainDef { components: Vec<Box<dyn PartialReflect>> }
    │
    ▼
Startup: load_terrain_defs()
    ├── load_folder("terrain") or manifest
    └── add handles to TrackedAssets
    │
    ▼
TerrainDefRegistry (Resource: HashMap<String, Handle<TerrainDef>>)
    │
    ▼
spawn_overworld() / spawn_homebase() / spawn_arena()
    ├── Look up TerrainDef by name
    ├── apply_object_components() to insert terrain components
    ├── build_generator(entity, seed) reads components → VoxelGenerator
    └── VoxelMapConfig::new(..., generator)
```

## External Resources

### Minecraft Terrain Generation
- [How Minecraft Terrain Generation Works](https://cybrancee.com/blog/how-minecraft-terrain-generation-works/)
- [World generation -- Minecraft Wiki](https://minecraft.wiki/w/World_generation)
- [Density function -- Minecraft Wiki](https://minecraft.wiki/w/Density_function)
- [Noise router -- Minecraft Wiki](https://minecraft.wiki/w/Noise_router)
- [The World Generation of Minecraft - Alan Zucconi](https://www.alanzucconi.com/2022/06/05/minecraft-world-generation/)

### Veloren
- [Veloren Book: World Generation](https://book.veloren.net/players/world-generation.html)
- [veloren_world::sim::erosion docs](https://docs.veloren.net/veloren_world/sim/erosion/)
- [Veloren GitLab](https://gitlab.com/veloren/veloren)

### noise Crate
- [noise crate docs.rs](https://docs.rs/noise)
- [noise-rs GitHub](https://github.com/Razaekel/noise-rs)
- [Fbm docs](https://docs.rs/noise/latest/noise/struct.Fbm.html)
- [RidgedMulti docs](https://docs.rs/noise/latest/noise/struct.RidgedMulti.html)

### Procedural Generation Techniques
- [Red Blob Games: Making maps with noise](https://www.redblobgames.com/maps/terrain-from-noise/)
- [Procedural Terrain Generation in Bevy](https://kcstuff.com/blog/procedural-generation-bevy)
- [Poisson Disk Sampling explanation](https://gameidea.org/2023/12/27/poisson-disk-sampling/)
- [Bridson's Poisson Disk Sampling paper](https://www.cs.ubc.ca/~rbridson/docs/bridson-siggraph07-poissondisk.pdf)
- [Vagabond -- Forest Generation](https://pvigier.github.io/2019/06/09/vagabond-forest-generation.html)
- [fast_poisson Rust crate](https://lib.rs/crates/fast_poisson)

### 2.5D Terrain
- [Procedural Terrain Generation In Rust: A Comprehensive Guide](https://peerdh.com/blogs/programming-insights/procedural-terrain-generation-in-rust-a-comprehensive-guide)
- [Red Blob Games: Island shaping functions](https://www.redblobgames.com/maps/terrain-from-noise/islands.html)

## Historical Context (from doc/)

- `doc/plans/2026-02-28-voxel-map-engine.md` -- Voxel Map Engine plan (current engine architecture)
- `doc/plans/2026-03-14-world-object-ron-assets.md` -- World Object RON assets plan
- `doc/research/2026-03-13-world-object-ron-assets.md` -- World Object system research
- `doc/plans/2026-03-07-map-instance-physics-isolation-and-switching.md` -- Map instance physics isolation
- `doc/research/2026-03-09-minecraft-style-map-directory-saving.md` -- Minecraft-style saving research
- `doc/research/2026-03-11-minecraft-world-sync-protocol.md` -- Minecraft world sync protocol research

## Resolved Questions

1. **World object spawn persistence**: Procedurally generated world objects are **not persisted** — they are deterministically regenerated from `hash(seed, chunk_pos)` each time a chunk loads. Only **mutation state** needs persistence.

   **Identity**: Each procedurally spawned object gets a deterministic UUID from `hash(seed, chunk_pos, spawn_index)`. This UUID is stable across regenerations — the same seed + chunk + index always produces the same UUID, so mutations can be re-applied after regeneration.

   **Mutation overlay**: A per-chunk `HashMap<Uuid, ObjectMutation>` stored alongside chunk data:
   ```rust
   pub enum ObjectMutation {
       Destroyed { respawn_at: Option<Tick> },
       Moved { position: Vec3 },
   }
   ```

   On chunk load: regenerate base objects from seed → apply mutation overlay (skip destroyed, relocate moved).

   **Cross-chunk movement — Minecraft's approach**: Minecraft does **not** tie entities to their spawn chunk. Position is truth: an entity belongs to whichever chunk its current `Pos` coordinates fall in. When an entity crosses a chunk boundary, it is re-indexed into the new chunk's section in memory (`EntitySectionStorage`). On save, the entity is written to the entity region file for its **current** chunk — not its original spawn chunk. The old chunk retains no record of the entity.

   Since 1.17, Minecraft stores entities in **separate region files** (`entities/r.X.Z.mca`) from terrain data. Each entity has a UUID for identity. When a chunk unloads, all entities currently in that chunk are serialized to disk at their current positions. When the chunk reloads, they resume from where they were. There is no teleport-back-to-spawn mechanic.

   Minecraft also uses **despawning rules** to prevent entities from drifting far: hostile mobs instantly despawn beyond 128 blocks from any player. Only persistent mobs (named, tamed, or certain types like villagers) survive indefinitely and are saved at their current position.

   **Implications for this project**: The mutation overlay should **not** track moved objects by spawn chunk. Instead, adopt Minecraft's model:
   - World objects are stored per-chunk based on **current position**, not spawn position
   - When a world object moves across a chunk boundary, it is re-associated with the new chunk
   - On chunk unload, all world objects currently in that chunk are persisted to disk
   - On chunk load, procedural base objects are regenerated from seed, then persisted objects (which may have moved in from other chunks, or be modified versions of procedurally-spawned objects) are loaded from the chunk's entity save file
   - A `ProceduralSpawnId` (deterministic UUID from `hash(seed, original_chunk, spawn_index)`) distinguishes procedural objects from player-placed ones. During chunk load, if a procedural object's UUID matches a persisted mutation, the persisted state wins (e.g., destroyed = don't spawn, moved-away = don't spawn here, the destination chunk's save has the moved entity)
   - Scenery objects (trees, rocks) don't move and don't need cross-chunk tracking — they stay where they spawned and are simply regenerated or skipped based on the mutation overlay
   - Mobile entities (NPCs, wildlife) should use a separate entity persistence system modelled on Minecraft's per-chunk entity storage, not the procedural regeneration system

2. **Noise function caching**: **Per-chunk, not per-map.** Before iterating the 3D voxel array, pre-compute a 2D `[f64; 18*18]` heightmap (and moisture map if present) for the chunk's (x, z) footprint. The inner voxel loop indexes into the cached array instead of re-evaluating noise for each Y level. The cache is allocated on the stack (or thread-local), used during chunk generation, and discarded after — no persistent storage needed.

   Minecraft does the same: `NoiseChunk` pre-computes and caches noise values for the chunk being generated, then discards them. Per-map caching would require storing the entire world's heightmap, which is unbounded for the overworld. Per-chunk is the right granularity because each chunk is generated independently on the async task pool.

3. **World object spawn timing**: The generator closure runs on `AsyncComputeTaskPool` and cannot spawn ECS entities directly. Instead, the generator returns **voxel data + a list of object spawn descriptors** (position, object_id). The main-thread system that polls `ChunkGenResult` spawns the entities. When a chunk is despawned (out of range), associated world objects are also despawned — achieving the "not simulated while unloaded" goal. This means `ChunkGenResult` gains a new field:


    ```rust
    pub struct ChunkGenResult {
        pub position: IVec3,
        pub mesh: Option<Mesh>,
        pub voxels: Vec<WorldVoxel>,
        pub from_disk: bool,
        pub world_objects: Vec<WorldObjectSpawn>,  // new
    }

    pub struct WorldObjectSpawn {
        pub object_id: WorldObjectId,
        pub position: Vec3,
    }
    ```

    The polling system spawns entities for each `WorldObjectSpawn` and tags them with the chunk position so they can be despawned with their chunk.

4. **Material palette**: Implemented as a **separate asset** (e.g., `materials.palette.ron`). Maps `u8` material indices to visual properties (color, texture path). Independent of terrain config — the palette is shared across all maps.

5. **Biome blending**: **Hard boundaries for now.** No noise-based blending at biome edges. Material index is determined solely by which biome's parameter ranges the (height, moisture) values fall into. Blending can be added later as a post-processing step.

6. **Client-side generation**: The server generates all chunks and sends them to clients as serialized data. The client only renders received chunks. This is exactly what the current codebase already does — `generates_chunks = false` on the client, chunks are streamed via `ChunkDataSync`. No client-side terrain config needed. The client's `generator_for_map` is only used as a fallback for local prediction/testing and can remain as `flat_terrain_voxels` or be updated to match the server config if client-side chunk prediction is desired later.
