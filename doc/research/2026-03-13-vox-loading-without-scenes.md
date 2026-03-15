---
date: 2026-03-13T14:36:44-07:00
researcher: Claude
git_commit: edb759890fc51c2014625ec55ef25e2c2e66ea4f
branch: master
repository: bevy-lightyear-template
topic: "Using bevy_vox_scene without scenes, or skipping it entirely via dot_vox + block-mesh-rs"
tags: [research, vox, dot_vox, block-mesh-rs, meshing, assets]
status: complete
last_updated: 2026-03-13
last_updated_by: Claude
---

# Research: .vox Loading Without Scenes

**Date**: 2026-03-13T14:36:44-07:00
**Researcher**: Claude
**Git Commit**: edb759890fc51c2014625ec55ef25e2c2e66ea4f
**Branch**: master
**Repository**: bevy-lightyear-template

## Research Question

How to use bevy_vox_scene without needing to actually use scenes once loaded — or whether to skip it entirely and use dot_vox + block-mesh-rs directly, given the project already uses block-mesh-rs for terrain.

## Summary

**bevy_vox_scene couples mesh generation to scene spawning** — there is no public API to extract a `Handle<Mesh>` without spawning the scene first. For this project, the better path is **dot_vox + the existing block-mesh-rs pipeline**. The project already has all the infrastructure: `greedy_quads` with `RIGHT_HANDED_Y_UP_CONFIG`, the `Voxel`/`MergeVoxel` trait pattern, and the quad-to-Bevy-Mesh conversion. A .vox model is just a differently-shaped voxel array — the meshing is identical.

## Detailed Findings

### 1. bevy_vox_scene Limitations for This Use Case

**bevy_vox_scene's single asset type is `VoxelScene`** — loading a `.vox` file always produces a scene graph, not a mesh. There is no `VoxelModel` or `VoxelMesh` asset type.

**Mesh generation is coupled to scene spawning.** The crate generates `Mesh` and `StandardMaterial` assets during scene instantiation, not during asset loading. To get mesh handles, you must:
1. Spawn the `VoxelScene` (creates entities with `Mesh3d` + `MeshMaterial3d<StandardMaterial>`)
2. Query spawned entities for their handles
3. Clone the handles for your own use
4. Despawn the scene hierarchy

This "spawn-extract-despawn" pattern is awkward, adds frame latency, and fights the asset pipeline rather than working with it.

**What bevy_vox_scene provides that we don't need:**
- MagicaVoxel scene graph instantiation (groups, layers, transforms)
- 16x16 texture atlas generation for PBR materials (metalness, roughness, emission, transmission)
- MagicaVoxel animation support
- Cloud/volumetric fog materials

**What we actually need:** Parse .vox → greedy-mesh the voxel grid → produce a Bevy `Mesh` with vertex colors.

### 2. dot_vox API (v5.x) — Raw .vox Parsing

`dot_vox` is the actively-maintained pure-Rust .vox parser (used internally by bevy_vox_scene).

```rust
// Load
let data: DotVoxData = dot_vox::load_bytes(&bytes)?;

// Access
data.models    // Vec<Model> — one per model in the file
data.palette   // Vec<Color> — 256 entries, Color { r, g, b, a }
data.materials // Vec<Material> — MagicaVoxel material properties
data.scenes    // Vec<SceneNode> — scene graph (Transform/Group/Shape)
```

**Model struct** — sparse voxel representation:
```rust
pub struct Model {
    pub size: Size,         // Size { x: u32, y: u32, z: u32 }
    pub voxels: Vec<Voxel>, // only occupied voxels
}
pub struct Voxel {
    pub x: u8, pub y: u8, pub z: u8,
    pub i: u8, // palette index (0-254)
}
```

Coordinate system: right-handed, Z-up (needs remapping to Bevy's Y-up).

### 3. Existing block-mesh-rs Pipeline in This Project

The project already has the complete greedy-quads-to-Bevy-Mesh pipeline at [meshing.rs](crates/voxel_map_engine/src/meshing.rs).

**Current flow for terrain** ([meshing.rs:10-61](crates/voxel_map_engine/src/meshing.rs#L10-L61)):
1. Receive `&[WorldVoxel]` — flat array of 18³ padded voxels
2. Call `greedy_quads(voxels, &PaddedChunkShape{}, [0;3], [17;3], &faces, &mut buffer)`
3. Iterate `buffer.quads.groups` × `faces` → generate positions, normals, UVs, indices
4. Return `Mesh` with `TriangleList` topology

**Trait implementations** ([types.rs:110-133](crates/voxel_map_engine/src/types.rs#L110-L133)):
- `Voxel` trait: `Air`/`Unset` → `Empty`, `Solid(_)` → `Opaque`
- `MergeVoxel` trait: merge value = the `u8` material ID (same-material voxels merge into larger quads)

**The `greedy_quads` function is already generic** over `T: MergeVoxel` and `S: Shape<3>`. Only the wrapper `mesh_chunk_greedy` is hardcoded to 18³ / `WorldVoxel`.

### 4. Meshing .vox Models with the Existing Pipeline

A .vox model is structurally identical to a voxel chunk — just a different size. The steps:

**Step 1: Define a voxel type for .vox data**
```rust
#[derive(Clone, Copy, Default, PartialEq)]
enum VoxModelVoxel {
    #[default]
    Empty,
    Filled(u8), // palette index
}

impl Voxel for VoxModelVoxel {
    fn get_visibility(&self) -> VoxelVisibility {
        match self {
            Self::Empty => VoxelVisibility::Empty,
            Self::Filled(_) => VoxelVisibility::Opaque,
        }
    }
}

impl MergeVoxel for VoxModelVoxel {
    type MergeValue = u8;
    type MergeValueFacingNeighbour = u8;
    fn merge_value(&self) -> Self::MergeValue {
        match self { Self::Filled(i) => *i, _ => 0 }
    }
    fn merge_value_facing_neighbour(&self) -> Self::MergeValueFacingNeighbour {
        self.merge_value()
    }
}
```

**Step 2: Rasterize sparse voxels into padded dense array**
```rust
// dot_vox Model.size = (sx, sy, sz), Model.voxels = sparse
// Padded size = (sx+2, sy+2, sz+2)
// Use ndshape::RuntimeShape or ConstShape for indexing
// Scatter: for v in model.voxels { array[linearize(v.x+1, v.z+1, v.y+1)] = Filled(v.i) }
// Note: remap Z-up → Y-up during scatter
```

**Step 3: Call greedy_quads** — identical to terrain, just different shape dimensions.

**Step 4: Add vertex colors from palette**
During quad-to-mesh conversion, look up the quad's merge value (palette index) in `DotVoxData.palette` to get RGBA. Add as `Mesh::ATTRIBUTE_COLOR` (per-vertex `[f32; 4]`).

### 5. Comparison: dot_vox Direct vs bevy_vox_scene

| Aspect | dot_vox + block-mesh-rs | bevy_vox_scene |
|--------|------------------------|----------------|
| Extra dependency | `dot_vox` only (tiny) | `bevy_vox_scene` + `dot_vox` |
| Mesh control | Full — same pipeline as terrain | None — coupled to scene spawning |
| Coloring | Vertex colors (simple) | Texture atlas (PBR) |
| Hot-reload | Custom `AssetLoader` needed | Built-in via Bevy asset system |
| Scene graph | Must handle manually if needed | Automatic |
| Material fidelity | Color only (no emission/glass) | Full MagicaVoxel PBR |
| Colliders | `trimesh_from_mesh` on the output | Same, but after scene spawn |
| Code reuse | Reuses existing meshing.rs patterns | Separate system |
| Bevy version coupling | None (dot_vox is Bevy-agnostic) | Must match Bevy version |

### 6. Custom AssetLoader for .vox → Mesh

To make .vox files hot-reloadable Bevy assets without bevy_vox_scene, implement a custom `AssetLoader`:

```rust
/// The loaded asset — contains mesh + palette for coloring
#[derive(Asset, TypePath)]
pub struct VoxModelAsset {
    pub mesh: Mesh,
    pub palette: Vec<[f32; 4]>, // RGBA normalized
    pub size: UVec3,
}

/// AssetLoader implementation
struct VoxModelLoader;

impl AssetLoader for VoxModelLoader {
    type Asset = VoxModelAsset;
    type Settings = ();
    type Error = /* ... */;

    fn extensions(&self) -> &[&str] { &["vox"] }

    async fn load(reader, _settings, _ctx) -> Result<VoxModelAsset, _> {
        let bytes = read_all(reader).await?;
        let data = dot_vox::load_bytes(&bytes)?;
        let model = &data.models[0]; // or parameterize
        let mesh = mesh_vox_model(model, &data.palette);
        Ok(VoxModelAsset { mesh, palette, size })
    }
}
```

This integrates with Bevy's file watcher for hot-reload and with `TrackedAssets` for the loading gate.

### 7. Handling Multi-Model .vox Files

MagicaVoxel files can contain multiple models arranged in a scene graph. Two approaches:

**Simple (recommended for world objects):** Load only `models[0]`. Most world objects (trees, ores, doors) are single models. The artist exports one model per .vox file.

**Scene graph (if needed):** Parse `data.scenes` to get transforms and model indices. Each `SceneNode::Shape` references model indices, each `SceneNode::Transform` provides position/rotation. Build a flat list of `(model_index, transform)` pairs by walking the tree. This is only needed for complex multi-part objects (buildings with separate door/window models).

## Code References

- [crates/voxel_map_engine/src/meshing.rs](crates/voxel_map_engine/src/meshing.rs) — `mesh_chunk_greedy`, `flat_terrain_voxels`, quad-to-Mesh conversion
- [crates/voxel_map_engine/src/types.rs:110-133](crates/voxel_map_engine/src/types.rs#L110-L133) — `Voxel`/`MergeVoxel` trait impls for `WorldVoxel`
- [git/block-mesh-rs/src/greedy.rs](git/block-mesh-rs/src/greedy.rs) — `greedy_quads` generic algorithm
- [git/block-mesh-rs/src/lib.rs](git/block-mesh-rs/src/lib.rs) — `Voxel` trait, `VoxelVisibility`
- [git/block-mesh-rs/src/geometry/face.rs](git/block-mesh-rs/src/geometry/face.rs) — `OrientedBlockFace` mesh generation

## External References

- [dot_vox docs.rs](https://docs.rs/dot_vox/latest/dot_vox/) — Full API documentation
- [dot_vox GitHub](https://github.com/dust-engine/dot_vox) — Source code
- [bevy_vox_scene GitHub](https://github.com/Utsira/bevy_vox_scene) — For reference if PBR materials are ever needed
- [bevy_vox_scene mesh.rs](https://github.com/oliver-dew/bevy_vox_scene/blob/main/src/model/mesh.rs) — Their greedy quads implementation for comparison

## Resolved Questions

All questions below have been researched in detail. See [2026-03-13-vox-model-loading-rendering-pipeline.md](2026-03-13-vox-model-loading-rendering-pipeline.md) for full findings.

1. **Vertex color material**: Works automatically. Insert `Mesh::ATTRIBUTE_COLOR` as `Float32x4` (linear RGBA). PBR shader enables vertex colors via `#ifdef VERTEX_COLORS` when the attribute is present. No special `StandardMaterial` config needed — `base_color: WHITE` (default) passes vertex colors through. Lighting, shadows, all PBR effects apply. **Gotcha**: dot_vox palette is sRGB — must linearize before inserting.

2. **Coordinate remapping**: Both systems are right-handed — only axis relabeling needed, no handedness change. Remap: `bevy_pos = (vox.x, vox.z, vox.y)`. Do this during rasterization (before passing to `greedy_quads`). `RIGHT_HANDED_Y_UP_CONFIG` handles everything else (normals, UVs, winding) — it expects Y-up data.

3. **Model centering**: Centered at origin. Offset vertex positions by `(-size.x/2, -size.z/2, -size.y/2)` after meshing (in Bevy coordinates).

4. **LOD**: Downsample 2x2x2 → 1 voxel per LOD level using majority-vote color merge (preserves palette). Mesh each LOD with greedy quads. Use Bevy's `VisibilityRange` component for distance-based dithered crossfade between LOD levels. Generate LOD meshes at asset load time as labeled sub-assets.
