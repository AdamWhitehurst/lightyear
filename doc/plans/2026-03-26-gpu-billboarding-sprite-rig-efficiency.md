# GPU Billboarding & Sprite Rig Efficiency Implementation Plan

## Overview

Move billboard rotation from CPU `Transform` manipulation to a GPU vertex shader via Bevy's `ExtendedMaterial`/`MaterialExtension` system, then optimize sprite rig rendering by deduplicating mesh/material handles and collapsing the multi-entity bone hierarchy into a single skinned mesh per character.

## Current State Analysis

### Billboard Systems
Two CPU-side billboard systems mutate `Transform` every frame:
- `billboard_rigs_face_camera` (`spawn.rs:251-275`): Y-axis billboard for sprite rigs via `atan2` + parent rotation compensation
- `billboard_face_camera` (`health_bar.rs:67-89`): Y-axis billboard for health bars via `Quat::from_rotation_arc`

Both dirty `Transform` every frame, triggering re-upload to GPU via retained render world change detection.

### Sprite Rig Rendering
Each bone is a separate entity with unique `Handle<Mesh>` + `Handle<StandardMaterial>` (`spawn.rs:125-135`). No batching possible. Each character = 8 entities (root + billboard + 6 visible bones), 6 draw calls.

### Entity Hierarchy (current)
```
CharacterEntity (AnimationPlayer, BoneEntities, Facing)
  └── RigBillboard (Transform -- rotated by CPU billboard system)
        └── root (Name, Transform, BoneZOrder)
              ├── torso (Name, Transform, BoneZOrder, Mesh3d, MeshMaterial3d)
              │     ├── head (...)
              │     ├── arm_l (...)
              │     └── arm_r (...)
              ├── leg_l (...)
              └── leg_r (...)
```

### Key Discoveries
- No custom shaders exist in the project; `assets/shaders/` doesn't exist
- All bone meshes are `Plane3d::new(Vec3::Z, size/2)` -- Z-facing quads
- Z-ordering is baked into `Transform::translation.z` by animation curves (`animation.rs:328`)
- `AnimationTargetId` uses single-name hashing (`animation.rs:232-233`), not hierarchy paths
- `apply_facing_to_rig` sets `JointRoot.scale.x = -1.0` to mirror the bone hierarchy
- Health bar color mutation uses `materials.get_mut(&handle).base_color = ...` (`health_bar.rs:146-164`)
- Bevy 0.18 view uniform fields: `view_from_world`, `clip_from_view`, `clip_from_world`

## Desired End State

All sprite rig bones rendered in **1 draw call per character** via a single skinned mesh. All health bars and sprite rigs use **GPU vertex shader billboarding** -- no per-frame `Transform` writes for camera-facing orientation. Mesh and material handles are shared across characters of the same rig type.

### Verification
- `cargo check-all` passes
- `cargo server` + `cargo client` -- characters render correctly, face camera, animate, flip on facing change
- Health bars render correctly, face camera, update color on damage/invulnerability
- No visual regressions vs. current CPU billboard behavior (except: spherical billboard replaces cylindrical -- sprites no longer foreshorten at camera pitch angles)

## What We're NOT Doing

- Compute shader billboarding (no clean Bevy extension point)
- Texture atlasing (placeholder colors for now; atlas is a future task when real art arrives)
- Array textures for per-bone sprite selection
- Instanced skinned meshes across multiple characters (Bevy handles batching automatically on storage-buffer platforms)
- `bevy_mod_billboard` crate (stalled at Bevy 0.14)

## Implementation Approach

Three sequential phases, each building on the previous:

1. **Billboard Material Extension**: GPU vertex shader replaces CPU billboard systems
2. **Shared Handles (Flyweight)**: Deduplicate mesh/material handles for automatic batching
3. **Single Skinned Mesh**: Collapse bone entities into joint entities + one skinned mesh per character

---

## Phase 1: Billboard Material Extension

### Overview

Create a `BillboardExt` `MaterialExtension` with a custom vertex shader that strips rotation from the model-view matrix, making quads always face the camera. Replace `StandardMaterial` on all sprite rig bones and health bars with `ExtendedMaterial<StandardMaterial, BillboardExt>`. Remove both CPU billboard systems.

### Changes Required

#### 1. Create shader file
**File**: `assets/shaders/billboard.wgsl` (new)

```wgsl
#import bevy_pbr::{
    mesh_bindings::mesh,
    mesh_functions,
    skinning,
    forward_io::{Vertex, VertexOutput},
    mesh_view_bindings::view,
}

@vertex
fn vertex(vertex_no_morph: Vertex) -> VertexOutput {
    var out: VertexOutput;

    var vertex = vertex_no_morph;

    // Compute world_from_local (skinned or static)
#ifdef SKINNED
    var world_from_local = skinning::skin_model(
        vertex.joint_indices,
        vertex.joint_weights,
        vertex_no_morph.instance_index,
    );
#else
    var world_from_local = mesh_functions::get_world_from_local(
        vertex_no_morph.instance_index,
    );
#endif

    // Billboard: strip rotation from model-view matrix, preserving translation + scale
    var model_view = view.view_from_world * world_from_local;

    // Extract per-axis scale from rotation columns before overwriting
    let scale_x = length(model_view[0].xyz);
    let scale_y = length(model_view[1].xyz);
    let scale_z = length(model_view[2].xyz);

    // Replace rotation with identity, scaled (spherical billboard)
    model_view[0] = vec4<f32>(scale_x, 0.0, 0.0, model_view[0][3]);
    model_view[1] = vec4<f32>(0.0, scale_y, 0.0, model_view[1][3]);
    model_view[2] = vec4<f32>(0.0, 0.0, scale_z, model_view[2][3]);

    let view_pos = model_view * vec4<f32>(vertex.position, 1.0);
    out.position = view.clip_from_view * view_pos;

    // World position for fragment shader (lighting, fog, etc.)
    let world_pos = world_from_local * vec4<f32>(vertex.position, 1.0);
    out.world_position = world_pos;

#ifdef VERTEX_NORMALS
    // Billboard normal: always face camera (view-space +Z = toward camera)
    out.world_normal = normalize(
        (view.world_from_view * vec4<f32>(0.0, 0.0, 1.0, 0.0)).xyz
    );
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex_no_morph.instance_index;
#endif

    return out;
}
```

**Key design decisions:**
- **Spherical billboard** (all 3 columns replaced): sprites always face screen plane, no foreshortening at camera pitch. This is a visual change from the current cylindrical approach -- intentional for 2.5D sprite rendering.
- **Scale preservation**: `length()` of each column extracts the per-axis scale before overwriting with identity. This preserves `scale.x = -1.0` facing flip and any animation-driven scale.
- **SKINNED ifdef**: handled from the start so Phase 3 works without shader changes.
- **Normals**: set to camera-facing direction for correct lighting if materials become lit later.

#### 2. Create billboard material extension
**File**: `crates/render/src/billboard_material.rs` (new)

```rust
use bevy::prelude::*;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::render::render_resource::{AsBindGroup, ShaderRef};

/// Material extension that performs GPU-side spherical billboarding.
///
/// Strips rotation from the model-view matrix in the vertex shader,
/// making quads always face the camera. Handles both skinned and
/// non-skinned meshes.
pub type BillboardMaterial = ExtendedMaterial<StandardMaterial, BillboardExt>;

/// Marker extension for GPU billboard vertex shader. Contains no
/// additional uniforms -- camera data comes from Bevy's view uniform.
#[derive(AsBindGroup, Asset, TypePath, Clone, Default)]
pub struct BillboardExt {}

impl MaterialExtension for BillboardExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/billboard.wgsl".into()
    }
}
```

#### 3. Register plugin
**File**: `crates/render/src/lib.rs`

Add to `RenderPlugin::build`:
```rust
app.add_plugins(
    MaterialPlugin::<BillboardMaterial>::default()
);
```

#### 4. Update sprite rig bone spawning
**File**: `crates/sprite_rig/src/spawn.rs`

In `spawn_bone_hierarchy`, where `MeshMaterial3d<StandardMaterial>` is inserted (`spawn.rs:125-135`), change to `MeshMaterial3d<BillboardMaterial>`. The `StandardMaterial` config (unlit, double-sided, cull_mode: None, base_color) becomes the `.base` field of `BillboardMaterial`.

The `sprite_rig` crate needs to depend on the `render` crate (or the `BillboardMaterial` type needs to live somewhere both can access -- see dependency note below).

**Dependency consideration**: `render` already depends on `sprite_rig`. Adding `sprite_rig -> render` would create a cycle. Two options:
- **Option A**: Move `BillboardMaterial`/`BillboardExt` into `protocol` crate (both depend on it). Protocol is the shared foundation.
- **Option B**: Pass `Handle<BillboardMaterial>` into `spawn_sprite_rigs` via a resource, so `sprite_rig` doesn't need to know the concrete type. Use a type alias or newtype in `protocol`.

**Chosen: Option A** -- move `billboard_material.rs` to `crates/protocol/src/render/billboard_material.rs`. Protocol already has bevy as a dependency. The `render` crate imports from `protocol`. The `sprite_rig` crate already depends on `protocol`.

**Revised file location**: `crates/protocol/src/render/billboard_material.rs`

Add `render` module to protocol:
- `crates/protocol/src/render/mod.rs` (new): `pub mod billboard_material;`
- `crates/protocol/src/lib.rs`: add `pub mod render;`
- `crates/protocol/Cargo.toml`: ensure `bevy/bevy_pbr` feature is enabled (needed for `MaterialExtension`)

#### 5. Update health bar spawning
**File**: `crates/render/src/health_bar.rs`

In `spawn_health_bar` (`health_bar.rs:19-65`):
- Change `ResMut<Assets<StandardMaterial>>` to `ResMut<Assets<BillboardMaterial>>`
- Wrap `StandardMaterial` in `BillboardMaterial { base: StandardMaterial { ... }, extension: BillboardExt {} }`
- Change `MeshMaterial3d<StandardMaterial>` to `MeshMaterial3d<BillboardMaterial>`

In `set_fg_color` (`health_bar.rs:122-144`):
- Change `ResMut<Assets<StandardMaterial>>` to `ResMut<Assets<BillboardMaterial>>`
- Query `MeshMaterial3d<BillboardMaterial>` instead of `MeshMaterial3d<StandardMaterial>`
- Access `mat.base.base_color` instead of `mat.base_color`

In `update_health_bars` -- no material access, only `Transform` mutation. No changes needed.

#### 6. Remove CPU billboard systems
**File**: `crates/sprite_rig/src/spawn.rs`
- Delete `billboard_rigs_face_camera` system entirely
- Remove from system chain in `lib.rs`

**File**: `crates/render/src/health_bar.rs`
- Delete `billboard_face_camera` system entirely
- Remove from system chain in `lib.rs`

**File**: `crates/render/src/lib.rs`
- Remove `billboard_face_camera` from the system chain

**File**: `crates/sprite_rig/src/lib.rs`
- Remove `billboard_rigs_face_camera` from the system chain

#### 7. Rename RigBillboard to JointRoot
**File**: `crates/sprite_rig/src/spawn.rs`
- Rename `RigBillboard` component to `JointRoot`
- Update all references (spawn, apply_facing_to_rig, etc.)
- The component no longer handles billboarding -- it's purely a hierarchy node for facing flip via `scale.x`

#### 8. Remove Billboard component from health bars
**File**: `crates/render/src/health_bar.rs`
- Remove `Billboard` component definition (no longer needed -- GPU handles it)
- Remove `Billboard` from `HealthBarRoot` entity spawn
- The health bar root entity just needs `HealthBarRoot` marker + `Transform`

### Success Criteria

#### Automated Verification:
- [ ] `cargo check-all`
- [ ] `cargo server` builds and runs
- [ ] `cargo client` builds and runs

#### Manual Verification:
- [ ] Sprite rig characters face camera from all angles
- [ ] Sprite rigs animate correctly (translation, rotation, scale curves)
- [ ] Facing flip works (scale.x = -1.0 mirrors correctly)
- [ ] Health bars face camera
- [ ] Health bar color changes on damage and invulnerability
- [ ] No per-frame transform dirtying on billboard entities (check with Tracy if available)
- [ ] Camera rotation shows sprites always face screen (spherical, not cylindrical)

---

## Phase 2: Shared Mesh + Material Handles (Flyweight)

### Overview

Deduplicate mesh and material handles so bones with identical geometry/color share the same GPU resources. This enables Bevy's automatic batching -- all "torso" bones across all characters batch into one draw call.

### Changes Required

#### 1. Create BoneMeshCache resource
**File**: `crates/sprite_rig/src/spawn.rs`

```rust
/// Cache of mesh handles keyed by bone quad dimensions.
/// Enables Bevy's automatic batching for bones with identical geometry.
#[derive(Resource, Default)]
pub struct BoneMeshCache(pub HashMap<(FloatOrd, FloatOrd), Handle<Mesh>>);
```

Insert as resource during `SpriteRigPlugin::build`.

#### 2. Create BoneMaterialCache resource
**File**: `crates/sprite_rig/src/spawn.rs`

```rust
/// Cache of billboard material handles keyed by placeholder color.
/// Enables Bevy's automatic batching for bones with identical appearance.
#[derive(Resource, Default)]
pub struct BoneMaterialCache(pub HashMap<[FloatOrd; 4], Handle<BillboardMaterial>>);
```

#### 3. Update spawn_bone_hierarchy
**File**: `crates/sprite_rig/src/spawn.rs`

Where `meshes.add(Plane3d::new(...))` and `materials.add(StandardMaterial { ... })` are called (`spawn.rs:125-135`):

```rust
let mesh_handle = bone_mesh_cache
    .0
    .entry((FloatOrd(size.x), FloatOrd(size.y)))
    .or_insert_with(|| meshes.add(Plane3d::new(Vec3::Z, size / 2.0)))
    .clone();

let color = placeholder_color_for_bone(bone_name);
let color_key = [
    FloatOrd(color.red), FloatOrd(color.green),
    FloatOrd(color.blue), FloatOrd(color.alpha),
];
let mat_handle = bone_material_cache
    .0
    .entry(color_key)
    .or_insert_with(|| {
        materials.add(BillboardMaterial {
            base: StandardMaterial {
                base_color: color,
                unlit: true,
                double_sided: true,
                cull_mode: None,
                ..default()
            },
            extension: BillboardExt {},
        })
    })
    .clone();
```

### Success Criteria

#### Automated Verification:
- [ ] `cargo check-all`
- [ ] `cargo server` builds and runs
- [ ] `cargo client` builds and runs

#### Manual Verification:
- [ ] Characters render identically to Phase 1
- [ ] Multiple characters of same type visible simultaneously -- verify draw call count decreased (Tracy or Bevy diagnostics)

---

## Phase 3: Single Skinned Mesh

### Overview

Replace the per-bone entity hierarchy (each bone = entity with Mesh3d + MeshMaterial3d) with a single skinned mesh per character. Bone entities become invisible joint entities (Transform only). One draw call per character instead of 6.

### Design

#### Entity Hierarchy (after Phase 3)
```
CharacterEntity (AnimationPlayer, BoneEntities, Facing)
  ├── JointRoot (Transform -- scale.x for facing flip)
  │     └── joint_root (Name, Transform, AnimationTargetId, AnimatedBy)
  │           ├── joint_torso (Name, Transform, AnimationTargetId, AnimatedBy)
  │           │     ├── joint_head (...)
  │           │     ├── joint_arm_l (...)
  │           │     └── joint_arm_r (...)
  │           ├── joint_leg_l (...)
  │           └── joint_leg_r (...)
  └── SkinMeshEntity (Mesh3d, MeshMaterial3d<BillboardMaterial>, SkinnedMesh, DynamicSkinnedMeshBounds)
```

**Key structural decisions:**
- **Skinned mesh entity is a child of CharacterEntity**, not JointRoot. In the `SKINNED` vertex shader path, `skin_model()` returns `world_from_local` computed from joint `GlobalTransform` values -- the mesh entity's own `GlobalTransform` is not used for vertex positioning. Parenting to CharacterEntity with `Transform::default()` keeps the mesh entity at identity, which is important for `DynamicSkinnedMeshBounds` AABB entity-space conversion.
- **Joint entities replace bone entities** but keep the same `Name`, `Transform`, hierarchy, `AnimationTargetId`, `AnimatedBy`. No `Mesh3d`/`MeshMaterial3d` -- just transform nodes.
- **`BoneEntities` HashMap** still maps bone names to joint entity IDs. Animation attachment is unchanged.
- **Facing flip** still works: `JointRoot.scale.x = -1.0` mirrors joint world positions via transform propagation. The vertex shader receives already-mirrored joint matrices.

### Changes Required

#### 1. Build rig mesh at startup
**File**: `crates/sprite_rig/src/spawn.rs` (new function)

Create `build_rig_mesh(rig: &SpriteRigAsset) -> Mesh` that constructs a single mesh containing all visible bone quads:

```rust
/// Builds a single mesh containing all visible bone quads for a sprite rig.
///
/// Each bone with a slot becomes a quad (4 vertices, 6 indices). Vertices
/// are positioned in model space at the bone's rest-pose position. Each
/// vertex is rigidly bound to its bone's joint via ATTRIBUTE_JOINT_INDEX
/// with weight 1.0.
///
/// Quads are ordered in the index buffer by z_order (back-to-front) for
/// correct overlap via depth testing with z-offsets baked into vertex positions.
fn build_rig_mesh(rig: &SpriteRigAsset) -> Mesh {
    let slot_lookup = build_slot_lookup(rig);
    let sorted_bones = topological_sort_bones(&rig.bones);

    // Build bone index map (bone name -> joint index)
    let bone_index: HashMap<&str, u32> = sorted_bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i as u32))
        .collect();

    // Accumulate global rest-pose transforms through hierarchy
    let global_rest_poses = compute_global_rest_poses(&sorted_bones, &slot_lookup);

    // Collect visible bones (those with slots), sorted by z_order for index buffer ordering
    let mut visible_bones: Vec<_> = sorted_bones
        .iter()
        .filter(|b| slot_lookup.contains_key(b.name.as_str()))
        .collect();
    visible_bones.sort_by(|a, b| {
        let za = slot_lookup[a.name.as_str()].0;
        let zb = slot_lookup[b.name.as_str()].0;
        za.partial_cmp(&zb).unwrap()
    });

    let quad_count = visible_bones.len();
    let mut positions = Vec::with_capacity(quad_count * 4);
    let mut normals = Vec::with_capacity(quad_count * 4);
    let mut uvs = Vec::with_capacity(quad_count * 4);
    let mut joint_indices = Vec::with_capacity(quad_count * 4);
    let mut joint_weights = Vec::with_capacity(quad_count * 4);
    let mut indices = Vec::with_capacity(quad_count * 6);

    for (quad_idx, bone) in visible_bones.iter().enumerate() {
        let (z_order, size) = slot_lookup[bone.name.as_str()];
        let joint_idx = bone_index[bone.name.as_str()];
        let half = size / 2.0;

        // Quad vertices in model space (will be transformed by joint matrix on GPU)
        // Plane3d::new(Vec3::Z, size/2) produces a Z-facing quad in XY plane
        let quad_verts = [
            Vec3::new(-half.x, -half.y, 0.0),
            Vec3::new( half.x, -half.y, 0.0),
            Vec3::new( half.x,  half.y, 0.0),
            Vec3::new(-half.x,  half.y, 0.0),
        ];

        let base_vertex = (quad_idx * 4) as u32;
        for v in &quad_verts {
            positions.push(v.to_array());
            normals.push([0.0, 0.0, 1.0]);
            uvs.push([
                if v.x > 0.0 { 1.0 } else { 0.0 },
                if v.y > 0.0 { 0.0 } else { 1.0 },
            ]);
            // Rigid binding: weight 1.0 on this bone's joint only
            joint_indices.push([joint_idx as u16, 0, 0, 0]);
            joint_weights.push([1.0f32, 0.0, 0.0, 0.0]);
        }

        // Two triangles per quad
        indices.extend_from_slice(&[
            base_vertex, base_vertex + 1, base_vertex + 2,
            base_vertex, base_vertex + 2, base_vertex + 3,
        ]);
    }

    let mut mesh = Mesh::new(
        bevy::render::mesh::PrimitiveTopology::TriangleList,
        bevy::render::render_resource::RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, joint_indices);
    mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, joint_weights);
    mesh.insert_indices(bevy::render::mesh::Indices::U32(indices));
    mesh.with_generated_skinned_mesh_bounds()
}
```

**Z-ordering approach**: Vertex positions are in bone-local space (centered at origin). The joint's rest-pose transform includes the z_order in its translation.z. When the GPU applies `joint_global_transform * inverse_bind_pose`, the z_order offset is preserved in world space. Depth testing handles overlap. This matches the current approach where `Transform::translation.z = z_order`.

#### 2. Compute inverse bind poses
**File**: `crates/sprite_rig/src/spawn.rs` (new function)

```rust
/// Computes the global rest-pose transform for each joint, accumulated
/// through the parent chain. Returns one Mat4 per bone in sorted order.
fn compute_global_rest_poses(
    sorted_bones: &[&BoneDef],
    slot_lookup: &HashMap<&str, (f32, Vec2)>,
) -> Vec<Mat4> {
    let bone_index: HashMap<&str, usize> = sorted_bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();

    let mut global_transforms = vec![Mat4::IDENTITY; sorted_bones.len()];

    for (i, bone) in sorted_bones.iter().enumerate() {
        let z = slot_lookup
            .get(bone.name.as_str())
            .map(|(z, _)| *z)
            .unwrap_or(0.0);
        let local = Mat4::from_scale_rotation_translation(
            Vec3::new(
                bone.default_transform.scale.x,
                bone.default_transform.scale.y,
                1.0,
            ),
            Quat::from_rotation_z(bone.default_transform.rotation.to_radians()),
            Vec3::new(
                bone.default_transform.translation.x,
                bone.default_transform.translation.y,
                z,
            ),
        );
        global_transforms[i] = match &bone.parent {
            Some(parent_name) => {
                let parent_idx = bone_index[parent_name.as_str()];
                global_transforms[parent_idx] * local
            }
            None => local,
        };
    }

    global_transforms
}

/// Computes inverse bind poses from global rest-pose transforms.
fn compute_inverse_bind_poses(global_rest_poses: &[Mat4]) -> Vec<Mat4> {
    global_rest_poses.iter().map(|g| g.inverse()).collect()
}
```

**Why global, not local**: The extraction code computes `joint_global_transform * inverse_bind_pose` to get `world_from_model`. This only produces correct results if the inverse bind pose encodes the full accumulated global rest transform. Using just the local transform's inverse would be incorrect for child joints -- their global rest position includes parent translations.

**Example for humanoid rig:**
| Joint | Global Rest Pose | Inverse Bind Pose |
|-------|-----------------|-------------------|
| root | T(0, 0, 0) | T(0, 0, 0) |
| torso | T(0, 1, 0) | T(0, -1, 0) |
| head | T(0, 2.8, 0.003) | T(0, -2.8, -0.003) |
| arm_l | T(-1.2, 1, -0.001) | T(1.2, -1, 0.001) |
| arm_r | T(1.2, 1, 0.001) | T(-1.2, -1, -0.001) |
| leg_l | T(-0.5, -1, -0.002) | T(0.5, 1, 0.002) |
| leg_r | T(0.5, -1, 0.002) | T(-0.5, 1, -0.002) |

#### 3. Cache rig meshes and bind poses per rig type
**File**: `crates/sprite_rig/src/spawn.rs`

```rust
/// Cached GPU assets for a rig type, shared across all characters using that rig.
pub struct RigMeshAssets {
    pub mesh: Handle<Mesh>,
    pub inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
    pub material: Handle<BillboardMaterial>,
}

/// Cache of rig mesh assets keyed by rig asset handle ID.
#[derive(Resource, Default)]
pub struct RigMeshCache(pub HashMap<AssetId<SpriteRigAsset>, RigMeshAssets>);
```

This replaces `BoneMeshCache` and `BoneMaterialCache` from Phase 2. One mesh handle + one bind pose handle + one material handle per rig type. All characters of the same type share them.

#### 4. Rewrite spawn_sprite_rigs
**File**: `crates/sprite_rig/src/spawn.rs`

The spawning system changes from creating visible bone entities to creating invisible joint entities + one skinned mesh entity.

```rust
fn spawn_sprite_rigs(
    mut commands: Commands,
    query: Query<(Entity, &SpriteRig), Added<SpriteRig>>,
    rig_assets: Res<Assets<SpriteRigAsset>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<BillboardMaterial>>,
    mut bindpose_assets: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut rig_mesh_cache: ResMut<RigMeshCache>,
) {
    for (entity, sprite_rig) in &query {
        let rig = rig_assets.get(&sprite_rig.0).expect("rig asset loaded");
        let slot_lookup = build_slot_lookup(rig);
        let sorted_bones = topological_sort_bones(&rig.bones);

        // Get or create cached mesh assets for this rig type
        let rig_assets_entry = rig_mesh_cache
            .0
            .entry(sprite_rig.0.id())
            .or_insert_with(|| {
                let mesh = build_rig_mesh(rig);
                let global_rest_poses = compute_global_rest_poses(
                    &sorted_bones.iter().collect::<Vec<_>>(),
                    &slot_lookup,
                );
                let inverse_bindposes = compute_inverse_bind_poses(&global_rest_poses);
                RigMeshAssets {
                    mesh: meshes.add(mesh),
                    inverse_bindposes: bindpose_assets.add(
                        SkinnedMeshInverseBindposes::from(inverse_bindposes)
                    ),
                    material: materials.add(BillboardMaterial {
                        base: StandardMaterial {
                            base_color: Color::WHITE,
                            unlit: true,
                            double_sided: true,
                            cull_mode: None,
                            ..default()
                        },
                        extension: BillboardExt {},
                    }),
                }
            });
        let mesh_handle = rig_assets_entry.mesh.clone();
        let bindposes_handle = rig_assets_entry.inverse_bindposes.clone();
        let material_handle = rig_assets_entry.material.clone();

        // Spawn JointRoot (facing flip node)
        let joint_root_id = commands
            .spawn((Name::new("JointRoot"), JointRoot, Transform::default()))
            .id();
        commands.entity(entity).add_child(joint_root_id);

        // Spawn joint entities (transform-only, no mesh)
        let mut bone_map = HashMap::new();
        let mut joint_entities = Vec::with_capacity(sorted_bones.len());

        for bone in &sorted_bones {
            let transform = bone_transform_from_def(
                &bone.default_transform,
                slot_lookup.get(bone.name.as_str()),
            );
            let joint_id = commands
                .spawn((Name::new(bone.name.clone()), transform))
                .id();

            let parent_id = match &bone.parent {
                Some(parent_name) => bone_map[parent_name.as_str()],
                None => joint_root_id,
            };
            commands.entity(parent_id).add_child(joint_id);

            bone_map.insert(bone.name.clone(), joint_id);
            joint_entities.push(joint_id);
        }

        // Spawn skinned mesh entity (child of character, NOT of JointRoot)
        let skin_mesh_id = commands
            .spawn((
                Name::new("SkinMesh"),
                Mesh3d(mesh_handle),
                MeshMaterial3d(material_handle),
                SkinnedMesh {
                    inverse_bindposes: bindposes_handle,
                    joints: joint_entities,
                },
                DynamicSkinnedMeshBounds,
                Transform::default(),
            ))
            .id();
        commands.entity(entity).add_child(skin_mesh_id);

        commands.entity(entity).insert(BoneEntities(bone_map));
    }
}
```

**Critical: joint_entities ordering** must match the inverse bind pose array ordering and the `ATTRIBUTE_JOINT_INDEX` values in the mesh. All three use the topologically sorted bone order.

#### 5. Update bone_transform_from_def signature
**File**: `crates/sprite_rig/src/spawn.rs`

The function currently takes `slot_lookup` data inline. Adjust to accept `Option<&(f32, Vec2)>` for the slot info (z_order and size are only needed for z in the transform; size is no longer used per-entity since the mesh is pre-built):

```rust
fn bone_transform_from_def(
    def: &BoneTransform2d,
    slot_info: Option<&(f32, Vec2)>,
) -> Transform {
    let z = slot_info.map(|(z, _)| *z).unwrap_or(0.0);
    Transform {
        translation: Vec3::new(def.translation.x, def.translation.y, z),
        rotation: Quat::from_rotation_z(def.rotation.to_radians()),
        scale: Vec3::new(def.scale.x, def.scale.y, 1.0),
    }
}
```

#### 6. Update animation z_order handling
**File**: `crates/sprite_rig/src/animation.rs`

**No changes needed.** Animation curves already bake z_order into `Transform::translation.z`. Joint entities receive the same `Transform` writes as bone entities did. The skinning system reads joint `GlobalTransform` values, which include the z_order offsets. The vertex shader preserves these via `joint_global_transform * inverse_bind_pose`.

#### 7. Remove per-bone mesh/material creation
**File**: `crates/sprite_rig/src/spawn.rs`

- Remove `placeholder_color_for_bone` function (or keep for debug if desired -- but it's no longer used for individual bone entities)
- Remove `BoneMeshCache` and `BoneMaterialCache` from Phase 2 (replaced by `RigMeshCache`)
- Remove per-bone `Mesh3d`/`MeshMaterial3d` insertion from spawn code

#### 8. Update apply_facing_to_rig
**File**: `crates/sprite_rig/src/spawn.rs`

No changes needed. It already finds the `JointRoot` child and sets `scale.x`. Joint entities are children of `JointRoot`, so their `GlobalTransform` values reflect the flip. The skinning system reads these flipped `GlobalTransform` values.

#### 9. Add bevy_mesh dependency for SkinnedMesh types
**File**: `crates/sprite_rig/Cargo.toml`

Ensure `bevy` features include what's needed for `SkinnedMesh`, `SkinnedMeshInverseBindposes`, `DynamicSkinnedMeshBounds`. These are in `bevy_mesh` and `bevy_camera` respectively. With `bevy` as a workspace dependency, these should be available through re-exports:
- `bevy::mesh::skinning::SkinnedMesh`
- `bevy::mesh::skinning::SkinnedMeshInverseBindposes`

Verify the exact import paths at implementation time.

### Success Criteria

#### Automated Verification:
- [ ] `cargo check-all`
- [ ] `cargo server` builds and runs
- [ ] `cargo client` builds and runs

#### Manual Verification:
- [ ] Characters render correctly with single skinned mesh (all bone quads visible, correct sizes)
- [ ] Z-ordering between bone quads is correct (arms behind/in front of torso, head in front)
- [ ] Animation plays correctly (idle, locomotion blend, ability animations)
- [ ] Facing flip works (character mirrors when moving left vs right)
- [ ] Billboard facing works (character always faces camera from all angles)
- [ ] Multiple characters render simultaneously without visual artifacts
- [ ] Health bars still work correctly (separate from skinned mesh system)
- [ ] New characters spawning mid-game render correctly

---

## Testing Strategy

### Unit Tests
- `compute_global_rest_poses`: verify accumulated transforms for known humanoid rig
- `compute_inverse_bind_poses`: verify `global * inverse == identity` for each joint
- `build_rig_mesh`: verify vertex count (6 quads * 4 = 24), index count (6 * 6 = 36), joint index correctness

### Manual Testing Steps
1. Start server + client, verify characters render and animate
2. Rotate camera 360 degrees -- sprites always face screen
3. Move character left and right -- facing flip mirrors correctly
4. Spawn multiple characters -- all render correctly
5. Take damage -- health bars update and face camera
6. Use abilities -- ability animations play correctly
7. Look at characters from steep angles -- spherical billboard shows no foreshortening (visual change from current cylindrical)

## Performance Considerations

- **Phase 1**: Eliminates 2 per-frame systems + per-entity `Transform` dirtying. Measurable benefit for retained render world.
- **Phase 2**: Reduces draw calls from N-per-bone to N-per-unique-size. Significant for many characters.
- **Phase 3**: 1 draw call per character. Joint entities are lighter than mesh-bearing entities (no Mesh3d/MeshMaterial3d components, no mesh/material extraction).
- **WebGL 2**: Skinned mesh batching not available (requires storage buffers). Each character is a separate draw call. Still benefits from fewer entities and no CPU billboard.

## Migration Notes

- **Visual change**: Spherical billboard replaces cylindrical. Sprites no longer foreshorten at camera pitch angles. This is intentional for 2.5D sprite rendering but should be verified visually.
- **BoneZOrder component**: Can be removed after Phase 3 since z_order is baked into both animation curves and mesh vertex positions via inverse bind poses. Keep it through Phase 1-2 for compatibility.
- **Placeholder colors**: Phase 3 uses a single white material. Per-bone debug coloring is lost. If needed for debugging, use vertex colors (add `ATTRIBUTE_COLOR` to the mesh with per-quad colors).

## References

- Research document: `doc/research/2026-03-26-gpu-billboarding-sprite-rig-efficiency.md`
- Bevy `custom_skinned_mesh` example: `git/bevy/examples/animation/custom_skinned_mesh.rs`
- Bevy skinning shader: `git/bevy/crates/bevy_pbr/src/render/skinning.wgsl`
- Bevy mesh vertex shader: `git/bevy/crates/bevy_pbr/src/render/mesh.wgsl`
- Bevy `ExtendedMaterial`: `git/bevy/crates/bevy_pbr/src/extended_material.rs`
- Current sprite rig spawn: `crates/sprite_rig/src/spawn.rs`
- Current health bar: `crates/render/src/health_bar.rs`
