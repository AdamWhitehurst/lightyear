---
date: 2026-03-13T19:16:07-07:00
researcher: Claude
git_commit: 05da6f6f6c0e6d3f8d447e0715acdecd59c44337
branch: master
repository: bevy-lightyear-template
topic: "Loading .vox models with dot_vox: manifest-based asset loading, hot-reload, vertex colors, colliders, LOD"
tags: [research, vox, dot_vox, asset-loading, colliders, lod, vertex-colors, manifest]
status: complete
last_updated: 2026-03-13
last_updated_by: Claude
---

# Research: .vox Model Loading and Rendering Pipeline

**Date**: 2026-03-13T19:16:07-07:00
**Researcher**: Claude
**Git Commit**: 05da6f6f6c0e6d3f8d447e0715acdecd59c44337
**Branch**: master
**Repository**: bevy-lightyear-template

## Research Question

How to use `dot_vox` to load .vox model files and render in-game, following the abilities manifest-based asset loading pattern for web client support, with hot-reload and physics collider integration.

## Summary

The project has all the infrastructure needed. The pipeline is: `dot_vox` parses .vox bytes → rasterize sparse voxels into a padded dense array (with Z-up→Y-up remap) → `greedy_quads` (same as terrain) → Bevy `Mesh` with vertex colors from palette → `Collider::trimesh_from_mesh` for physics. A manifest file (`models.manifest.ron`) lists all model paths so WASM clients can load them without `load_folder`. Hot-reload comes free from implementing Bevy's `AssetLoader` trait.

## Detailed Findings

### 1. Asset Loading: Manifest Pattern (from Abilities System)

The abilities system establishes the pattern for manifest-based loading with WASM support:

**Native (server/client)** — [ability.rs:436-443](crates/protocol/src/ability.rs#L436-L443):
- Uses `asset_server.load_folder("abilities")` to discover all `.ability.ron` files
- Tracks the `LoadedFolder` handle via `TrackedAssets`

**WASM** — [ability.rs:447-454](crates/protocol/src/ability.rs#L447-L454):
- Loads `abilities.manifest.ron` — a JSON array of ability names: `["barrier","blink_strike","dash",...]`
- Parses the manifest to construct individual asset paths: `abilities/{name}.ability.ron`
- Loads each individually and tracks all handles via `TrackedAssets`

**Manifest file** — [abilities.manifest.ron](assets/abilities.manifest.ron):
```ron
["barrier","blink_strike","dash","dive_kick","fireball","ground_pound","punch","punch2","punch3","shield_bash","shockwave","speed_burst","teleport_burst","uppercut"]
```

**TrackedAssets gate**> — [app_state.rs](crates/protocol/src/app_state.rs):
- All handles are added to `TrackedAssets`
- `check_assets_loaded` polls `asset_server.is_loaded_with_dependencies` for each
- Transitions `AppState::Loading → AppState::Ready` when all are loaded

**Hot-reload** — [ability.rs:539-564](crates/protocol/src/ability.rs#L539-L564):
- Native: watches `AssetEvent<AbilityDef>::Modified` via the `LoadedFolder`
- WASM: watches `AssetEvent<AbilityDef>::Modified` on tracked handles
- Both rebuild the `AbilityDefs` resource on change

**For .vox models, replicate this pattern:**
- Manifest file: `models.manifest.ron` listing relative paths under `models/`
- Native: `load_folder("models")` or load individually
- WASM: parse manifest, load each `.vox` by path
- Track all handles, gate on `AppState::Loading`

### 2. Custom AssetLoader for .vox Files

Implement `AssetLoader` to integrate with Bevy's asset pipeline (hot-reload, tracked loading):

```rust
#[derive(Asset, TypePath)]
pub struct VoxModelAsset {
    pub mesh: Handle<Mesh>,
    pub size: UVec3,    // model dimensions in voxels
}

struct VoxModelLoader;

impl AssetLoader for VoxModelLoader {
    type Asset = VoxModelAsset;
    type Settings = ();
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn extensions(&self) -> &[&str] { &["vox"] }

    async fn load(
        &self,
        reader: &mut dyn bevy::asset::io::Reader,
        _settings: &(),
        load_context: &mut bevy::asset::LoadContext<'_>,
    ) -> Result<VoxModelAsset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let data = dot_vox::load_bytes(&bytes)
            .map_err(|e| format!("dot_vox parse error: {e}"))?;
        let model = &data.models[0];
        let mesh = mesh_vox_model(model, &data.palette);
        let mesh_handle = load_context.add_labeled_asset("mesh".to_string(), mesh);
        Ok(VoxModelAsset {
            mesh: mesh_handle,
            size: UVec3::new(model.size.x, model.size.z, model.size.y), // Z-up → Y-up
        })
    }
}
```

Using `load_context.add_labeled_asset` for the mesh makes it a sub-asset of the `.vox` file, which means:
- Hot-reload propagates automatically when the `.vox` file changes
- The mesh handle is `"path/to/model.vox#mesh"`

### 3. Coordinate Remapping: dot_vox Z-up → Bevy Y-up

Both MagicaVoxel and Bevy use right-handed coordinate systems. Only axis relabeling is needed — no handedness change, no winding order flip, no normal correction.

| MagicaVoxel (Z-up) | Bevy (Y-up) |
|---------------------|-------------|
| X (right) | X |
| Y (forward) | Z |
| Z (up) | Y |

**Rasterization with remap and centering:**
```rust
// Padded size for greedy_quads (1-voxel border on each side)
let sx = model.size.x + 2;
let sy = model.size.z + 2; // Z-up → Y-up
let sz = model.size.y + 2; // Y-forward → Z-forward

// Center offset: shift so model origin is at center
let cx = model.size.x as f32 / 2.0;
let cy = model.size.z as f32 / 2.0;
let cz = model.size.y as f32 / 2.0;

let shape = RuntimeShape::<u32, 3>::new([sx, sy, sz]);
let mut voxels = vec![VoxModelVoxel::Empty; shape.size() as usize];

for v in &model.voxels {
    let idx = shape.linearize([
        v.x as u32 + 1,           // +1 for padding
        v.z as u32 + 1,           // Z→Y (up axis)
        v.y as u32 + 1,           // Y→Z (forward axis)
    ]);
    voxels[idx as usize] = VoxModelVoxel::Filled(v.i);
}
```

After meshing, offset vertex positions by `(-cx, -cy, -cz)` to center the model at origin.

**Interaction with block-mesh-rs:** `RIGHT_HANDED_Y_UP_CONFIG` handles face normals, UVs, and winding for Y-up space. The voxel data must already be in Y-up coordinates when passed to `greedy_quads` — the config does not perform any axis remapping itself.

### 4. Vertex Colors from Palette

Bevy's `StandardMaterial` supports vertex colors automatically via `Mesh::ATTRIBUTE_COLOR`.

**Format:** `Float32x4` (linear RGBA), one per vertex, same length as position array.

**No special material config needed.** The PBR shader conditionally compiles vertex color support via `#ifdef VERTEX_COLORS`, set automatically when the mesh has `ATTRIBUTE_COLOR`. Vertex colors multiply with `StandardMaterial::base_color` (default `WHITE`, so no modification). Lighting, shadows, and all PBR effects apply normally.

**Palette lookup during mesh generation:**
```rust
// dot_vox palette: Vec<dot_vox::Color> where Color { r, g, b, a }
// Quad's merge_value = palette index (u8)

fn palette_to_linear(color: &dot_vox::Color) -> [f32; 4] {
    // dot_vox palette is sRGB — must linearize for Bevy
    [
        srgb_to_linear(color.r),
        srgb_to_linear(color.g),
        srgb_to_linear(color.b),
        color.a as f32 / 255.0,
    ]
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}
```

During quad-to-mesh conversion, look up the quad's `merge_value` (palette index) in the precomputed linear palette array. Each quad has 4 vertices that all get the same color.

**Gotchas:**
- Palette values are **sRGB** — must linearize before inserting as `ATTRIBUTE_COLOR`
- Alpha < 1.0 requires `AlphaMode` on the material
- `base_color` defaults to `WHITE` which is correct — don't change it

### 5. Collider Generation

The project already has the pattern at [map.rs:186-219](crates/protocol/src/map.rs#L186-L219):

```rust
let collider = Collider::trimesh_from_mesh(mesh);
commands.entity(entity).insert((
    collider,
    RigidBody::Static,
    terrain_collision_layers(),
));
```

**For .vox models, use the same `Collider::trimesh_from_mesh` approach.** The greedy-meshed output from `mesh_vox_model` is already a valid Bevy `Mesh` with positions and indices — `trimesh_from_mesh` extracts these to build a Parry trimesh collider.

**World objects need their own collision layer:**
```rust
pub fn world_object_collision_layers() -> CollisionLayers {
    CollisionLayers::new(GameLayer::Terrain, [GameLayer::Character])
    // Or a new GameLayer::WorldObject if different behavior is needed
}
```

**Reactive attachment pattern:** Follow the existing `attach_chunk_colliders` system — query `(Added<Mesh3d>, With<WorldObject>)` and insert colliders reactively. This keeps collider generation decoupled from spawning.

**Available Avian3D methods** (from [git/avian](git/avian/src/collision/collider/parry/mod.rs)):
| Method | Use case |
|--------|----------|
| `Collider::trimesh_from_mesh` | Exact shape — best for static world objects |
| `Collider::convex_hull_from_mesh` | Simplified — good for physics-only objects |
| `Collider::voxelized_trimesh_from_mesh` | Voxelized — could match aesthetic |

`trimesh_from_mesh` is the right default for static decorations (trees, rocks, buildings).

### 6. LOD (Level of Detail)

**Downsampling:** Collapse 2x2x2 voxel blocks into 1 voxel per LOD level.

```rust
fn downsample_2x(voxels: &[VoxModelVoxel], size: UVec3) -> (Vec<VoxModelVoxel>, UVec3) {
    let new_size = size / 2;
    let mut result = vec![VoxModelVoxel::Empty; (new_size.x * new_size.y * new_size.z) as usize];
    for z in 0..new_size.z {
        for y in 0..new_size.y {
            for x in 0..new_size.x {
                // Gather 2x2x2 neighborhood, pick majority color
                let mut counts: HashMap<u8, u8> = HashMap::new();
                for dz in 0..2 { for dy in 0..2 { for dx in 0..2 {
                    let src = voxels[linearize(x*2+dx, y*2+dy, z*2+dz, size)];
                    if let VoxModelVoxel::Filled(i) = src { *counts.entry(i).or_default() += 1; }
                }}}
                if let Some((&color, _)) = counts.iter().max_by_key(|(_, &c)| c) {
                    result[linearize(x, y, z, new_size)] = VoxModelVoxel::Filled(color);
                }
            }
        }
    }
    (result, new_size)
}
```

**Color merge strategy:** Majority vote (mode) — preserves the original palette, best for stylized voxel art. Average blurs colors and creates values not in the palette.

**Bevy integration via `VisibilityRange`:**
```rust
// LOD 0: full res, visible 0..30m
commands.spawn((
    Mesh3d(lod0_mesh),
    VisibilityRange { start_margin: 0.0..0.0, end_margin: 30.0..35.0, use_aabb: false },
));
// LOD 1: half res, visible 30..60m
commands.spawn((
    Mesh3d(lod1_mesh),
    VisibilityRange { start_margin: 30.0..35.0, end_margin: 60.0..65.0, use_aabb: false },
));
```

`VisibilityRange` uses screen-space dithering for crossfade — no alpha blending overhead. Evaluated per-view.

**Practical approach:** Generate 2-3 LOD meshes at asset load time (in the `AssetLoader`), store as labeled sub-assets (`"mesh_lod0"`, `"mesh_lod1"`). Spawning code attaches `VisibilityRange` to each.

### 7. Hot-Reload

Implementing `AssetLoader` gives hot-reload for free on native builds (Bevy's file watcher detects changes). When a `.vox` file is modified:

1. Bevy reloads the asset via the `AssetLoader`
2. `AssetEvent::Modified` fires for the `VoxModelAsset`
3. Sub-asset mesh handles update automatically (same `Handle<Mesh>`)
4. Entities using `Mesh3d(handle)` render the updated mesh next frame
5. Colliders need reactive re-attachment (via `Changed<Mesh3d>` query, same as chunks)

WASM does not have file watching, but the asset system still supports programmatic reload.

## Code References

- [ability.rs:436-454](crates/protocol/src/ability.rs#L436-L454) — Manifest-based asset loading (native vs WASM)
- [ability.rs:539-564](crates/protocol/src/ability.rs#L539-L564) — Hot-reload system for abilities
- [app_state.rs](crates/protocol/src/app_state.rs) — `TrackedAssets` and `AppState` gate
- [abilities.manifest.ron](assets/abilities.manifest.ron) — Manifest file format
- [meshing.rs:10-61](crates/voxel_map_engine/src/meshing.rs#L10-L61) — Greedy quads mesh pipeline
- [types.rs:110-133](crates/voxel_map_engine/src/types.rs#L110-L133) — `Voxel`/`MergeVoxel` trait impls
- [map.rs:186-219](crates/protocol/src/map.rs#L186-L219) — Reactive collider attachment from mesh
- [hit_detection.rs:16-52](crates/protocol/src/hit_detection.rs#L16-L52) — Collision layers
- [physics.rs](crates/protocol/src/physics.rs) — `MapCollisionHooks` for map isolation

## External References

- [dot_vox docs.rs](https://docs.rs/dot_vox/latest/dot_vox/) — `.vox` parser API
- [dot_vox Voxel struct](https://docs.rs/dot_vox/latest/dot_vox/struct.Voxel.html) — confirms right-handed Z-up
- [block-mesh-rs GitHub](https://github.com/bonsairobo/block-mesh-rs) — `RIGHT_HANDED_Y_UP_CONFIG` source
- [Bevy vertex_colors.rs example](https://github.com/bevyengine/bevy/blob/main/examples/3d/vertex_colors.rs)
- [Bevy VisibilityRange docs](https://docs.rs/bevy/latest/bevy/render/view/visibility/struct.VisibilityRange.html)
- [Bevy visibility_range.rs example](https://github.com/bevyengine/bevy/blob/latest/examples/3d/visibility_range.rs)
- [ephtracy/voxel-model coordinate conversion](https://github.com/ephtracy/voxel-model/pull/33/files) — confirms `(x, z, y)` for Y-up
- [MagicaVoxel .vox format spec](https://github.com/ephtracy/voxel-model/blob/master/MagicaVoxel-file-format-vox.txt)

## Related Research

- [doc/research/2026-03-13-vox-loading-without-scenes.md](doc/research/2026-03-13-vox-loading-without-scenes.md) — Earlier research on dot_vox vs bevy_vox_scene
- [doc/research/2026-03-12-streaming-ron-assets-to-web-clients.md](doc/research/2026-03-12-streaming-ron-assets-to-web-clients.md) — Web asset streaming patterns

## Design Decisions

1. **One model per .vox file.** The `AssetLoader` loads `models[0]` only. Multi-model scene graph support is not needed — artists export one model per file.
2. **No colliders on distant LOD levels.** Only the highest-detail LOD gets a collider. Distant decorations beyond interaction range skip colliders entirely to save physics cost.
