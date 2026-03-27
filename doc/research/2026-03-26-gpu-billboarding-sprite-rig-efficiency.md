---
date: 2026-03-26T11:01:37-07:00
researcher: Claude
git_commit: ac0c8c45c6dcece0d80674c707487cf1d8bbbf3a
branch: master
repository: bevy-lightyear-template
topic: "GPU billboarding for sprite rigs and health bars"
tags: [research, optimization, billboarding, gpu, shader, sprite-rig, health-bar, vertex-shader, instancing, skinned-mesh, gpu-skinning]
status: complete
last_updated: 2026-03-26
last_updated_by: Claude
last_updated_note: "Resolved all 8 open questions with verified findings from Bevy 0.18 source; updated WGSL to Bevy 0.18 naming conventions; elaborated single skinned mesh approach"
---

# Research: GPU Billboarding for Sprite Rigs and Health Bars

**Date**: 2026-03-26T11:01:37-07:00
**Researcher**: Claude
**Git Commit**: ac0c8c45c6dcece0d80674c707487cf1d8bbbf3a
**Branch**: master
**Repository**: bevy-lightyear-template

## Research Question

Can we move sprite rig and health bar billboarding from CPU-side transform manipulation to a GPU compute/vertex shader? Could this also enable rendering all sprite rig pieces together more efficiently?

## Summary

The project has two CPU-side billboard systems that mutate `Transform` every frame: one for sprite rig bone hierarchies and one for health bars. Moving billboarding to a **vertex shader** via Bevy's `ExtendedMaterial` / `MaterialExtension` system is the practical path -- compute shaders are overkill and don't have a clean extension point in Bevy's transform pipeline. The vertex shader approach eliminates per-frame CPU transform writes, avoids re-upload of dirty transforms to the GPU, and gets multi-camera correctness for free.

For rendering sprite rig pieces more efficiently, the biggest wins come from **shared mesh/material handles** (enabling automatic GPU batching) and **texture atlasing** rather than from changing the billboarding approach itself. A single-draw-call skinned mesh approach is possible but unnecessary at the expected character counts.

---

## Current Implementation

### Sprite Rig Billboard (`sprite_rig/spawn.rs:251-275`)

Entity hierarchy per character:
```
CharacterEntity (SpriteRig, Facing, BoneEntities, AnimationPlayer)
  └─ JointRoot (Transform, marker) [currently named `RigBillboard` in code -- rename in Phase 1]
       ├─ root bone (Mesh3d<Plane3d>, MeshMaterial3d<StandardMaterial>, BoneZOrder)
       │    ├─ child bone ...
       │    └─ child bone ...
       └─ another root bone ...
```

`billboard_rigs_face_camera` runs every frame:
1. Gets camera world position
2. Computes direction from billboard to camera, zeroing Y (cylindrical billboard)
3. Calculates world rotation via `atan2(direction.x, direction.z)`
4. Converts to local space by removing parent's world rotation: `transform.rotation = parent_rotation.inverse() * world_rotation`

`apply_facing_to_rig` (`spawn.rs:299-317`) mirrors the entire bone hierarchy by setting `JointRoot`'s `transform.scale.x` to `1.0` or `-1.0` when `Facing` changes.

### Health Bar Billboard (`render/health_bar.rs:67-89`)

```
CharacterEntity
  └─ HealthBarRoot + Billboard (Transform at Y+5.0)
       ├─ background mesh (z = -0.01)
       └─ HealthBarForeground mesh (z = 0.0)
```

`billboard_face_camera` runs every frame with nearly identical logic but uses `Quat::from_rotation_arc(Vec3::Z, direction.normalize())` instead of `atan2`.

### Rendering Details

- Each bone is a `Plane3d::new(Vec3::Z, size / 2.0)` mesh -- a quad lying in the XY plane with +Z normal
- Material is `StandardMaterial { unlit: true, double_sided: true, cull_mode: None }` with solid placeholder colors
- **No texture atlas, no shared mesh/material handles** between bones
- Bevy's stock PBR pipeline handles all rendering
- No custom shaders exist in the project

### Per-Frame CPU Work

For each character: 1 `GlobalTransform` read (camera) + 1 `GlobalTransform` read (billboard) + 1 `GlobalTransform` read (parent) + 1 `Transform` write (billboard rotation). Plus the `Facing` system on velocity change.

For each health bar: same pattern -- 3 reads + 1 write.

Every `Transform` write dirties the entity in Bevy's retained render world, triggering re-upload to GPU.

---

## GPU Billboarding Approaches

### Approach 1: Vertex Shader Billboard (Recommended)

Strip the model's rotation in the vertex shader and reconstruct a camera-facing orientation. This is the standard technique used by bevy_mod_billboard, Hanabi particles, and virtually every game engine.

#### WGSL Implementation (~20 lines)

**Technique A -- Strip rotation from ModelView matrix:**
```wgsl
var model_view = view.view_from_world * mesh_functions::get_world_from_local(in.instance_index);
// Replace rotation columns with identity (cylindrical: preserve column 1 for Y-lock)
model_view[0] = vec4<f32>(1.0, 0.0, 0.0, model_view[0][3]);
model_view[1] = vec4<f32>(0.0, 1.0, 0.0, model_view[1][3]);
model_view[2] = vec4<f32>(0.0, 0.0, 1.0, model_view[2][3]);
let clip_pos = view.clip_from_view * model_view * vec4<f32>(in.position, 1.0);
```

**Technique B -- Reconstruct camera basis from view-projection:**
```wgsl
let camera_right = normalize(vec3<f32>(view.clip_from_world.x.x, view.clip_from_world.y.x, view.clip_from_world.z.x));
let camera_up = normalize(vec3<f32>(view.clip_from_world.x.y, view.clip_from_world.y.y, view.clip_from_world.z.y));
let world_pos = center + camera_right * in.position.x * scale.x + camera_up * in.position.y * scale.y;
```

**Technique C -- Cross-product basis (used by Hanabi):**
```wgsl
let axis_z = normalize(camera_position - entity_position);
let axis_x = normalize(cross(view.view_from_world[1].xyz, axis_z));
let axis_y = cross(axis_z, axis_x);
let world_pos = position + axis_x * vert.x * size.x + axis_y * vert.y * size.y;
```

#### Bevy Integration via ExtendedMaterial

```rust
#[derive(AsBindGroup, Asset, TypePath, Clone)]
pub struct BillboardExt {
    // No extra uniforms needed -- camera data comes from Bevy's view uniform (group 0)
}

impl MaterialExtension for BillboardExt {
    fn vertex_shader() -> ShaderRef {
        "shaders/billboard.wgsl".into()
    }
}

// Register:
app.add_plugins(MaterialPlugin::<ExtendedMaterial<StandardMaterial, BillboardExt>>::default());
```

Entities would use `MeshMaterial3d<ExtendedMaterial<StandardMaterial, BillboardExt>>` instead of `MeshMaterial3d<StandardMaterial>`.

**Batching**: Bevy's automatic instancing works normally with custom `Material` impls. Entities sharing the same `Handle<Mesh>` + `Handle<ExtendedMaterial<...>>` batch into single draw calls.

#### What This Changes

| Aspect | Before (CPU) | After (GPU vertex shader) |
|--------|-------------|--------------------------|
| Billboard rotation | System writes `Transform` every frame | Vertex shader strips rotation; `Transform` unchanged |
| Transform re-upload | Every frame (dirty) | Only on actual position/animation changes |
| Multi-camera | Would need per-camera system | Automatic (view uniform is per-camera) |
| Facing flip | `scale.x = -1.0` on billboard entity | Unchanged -- `scale.x = -1.0` on billboard entity mirrors bone positions via transform propagation; no UV flip needed |
| System scheduling | Must run after transform propagation | Eliminated |
| Bone animation | Unchanged (AnimationPlayer writes bone transforms) | Unchanged |

#### Interaction with Animation System

The animation system writes `Transform::translation`, `rotation`, and `scale` on bone entities via `AnimatableCurve`. The billboard vertex shader would override the *rendering rotation* without touching the ECS `Transform`. This means:
- Bone-relative transforms (parent-child hierarchy) work normally
- The billboard entity no longer needs its rotation written per-frame
- Z-ordering (baked into `Transform::translation.z`) is preserved
- Facing flip via `scale.x` on the billboard entity still works: transform propagation mirrors bone world positions (the X component of each bone's world translation is negated), and the vertex shader uses those already-mirrored positions as quad centers. The shader strips rotation columns (which also strips per-quad scale), but this only affects the quad's own visual orientation -- bone *positions* are already correct. No per-quad UV flip is needed because character art is symmetric (directional art will be handled by the animation system with different poses, not mirrored textures).

### Approach 2: Compute Shader (Not Recommended)

Bevy 0.16+ uses a compute shader internally to transform `MeshInputUniform` into `MeshUniform`. There is no public extension point to inject custom transform logic into this pipeline. You would need to replace `MeshRenderPlugin` internals.

**Verdict**: Overkill. The vertex shader approach is simpler and achieves the same result.

### Approach 3: bevy_mod_billboard Crate

Implements GPU-side vertex shader billboarding (Technique B). **Last release: 0.7.0 (July 2024, Bevy 0.14)**. No Bevy 0.15+ support. The shader is ~30 lines of WGSL and trivially reimplementable via `ExtendedMaterial`.

---

## Rendering Sprite Rig Pieces More Efficiently

### Current Cost Profile

Each character spawns 1+1+N entities (root + billboard + bones). Each visible bone has its own unique `Handle<Mesh>` and `Handle<StandardMaterial>` (created at `spawn.rs:126-134`). This means:
- N draw calls per character (no batching possible with unique handles)
- N mesh uploads, N material uploads
- N transform propagation nodes in the hierarchy

### Optimization 1: Shared Mesh + Material Handles (Flyweight)

Bones with identical `size` produce geometrically identical `Plane3d` meshes. A cache `HashMap<(OrderedFloat<f32>, OrderedFloat<f32>), Handle<Mesh>>` would deduplicate these. Similarly, bones with the same placeholder color (or eventually the same atlas texture) could share material handles.

**Effect**: enables Bevy's automatic batching. All "torso" bones across all characters of the same type batch into one draw call. All "arm" bones batch into another, etc.

This is the **highest-impact, lowest-effort** optimization for sprite rig rendering.

### Optimization 2: Texture Atlas

When real textures replace placeholder colors, pack all bone sprites into one atlas image. All bones sharing the atlas share one `Handle<Image>` -> one material -> one batch per character type (or across all characters if they share the atlas).

Combined with the billboard vertex shader, UV offsets select the correct sub-sprite. This is exactly what Bevy's 2D sprite renderer does internally.

### Optimization 3: Single Skinned Mesh

Collapse the entire multi-entity bone hierarchy into a single mesh with GPU skinning. One draw call per character instead of N.

#### How It Works

The technique applies standard GPU skeletal animation to 2D sprite planes:

1. **Build a single mesh** containing all bone quads (6 visible bones = 6 quads = 24 vertices, 36 indices)
2. **Each vertex stores** joint indices + joint weights (`ATTRIBUTE_JOINT_INDEX`, `ATTRIBUTE_JOINT_WEIGHT`) binding it to its bone
3. **Joint entities** (invisible transform-only entities) form the skeleton hierarchy, driven by `AnimationPlayer`
4. **GPU vertex shader** reads per-joint matrices from a storage buffer, transforms each quad's vertices by its bone's matrix
5. **Billboard vertex shader** then strips the camera-facing rotation (composing with skinning)

#### Bevy's Skinning Infrastructure

Bevy has built-in GPU skinning (`SkinnedMesh` component + `skinning.wgsl`):

```rust
pub struct SkinnedMesh {
    pub inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,  // Vec<Mat4>
    pub joints: Vec<Entity>,  // joint entities with GlobalTransform
}
```

**Pipeline**: `extract_skins` reads each joint entity's `GlobalTransform`, multiplies by inverse bind pose, uploads all joint matrices to a single GPU storage buffer. The vertex shader indexes into this buffer via `mesh[instance_index].current_skin_index + joint_index`.

**Vertex attributes** (locations 6 and 7):
```rust
mesh.with_inserted_attribute(
    Mesh::ATTRIBUTE_JOINT_INDEX,
    vec![[bone_idx as u16, 0, 0, 0]; 4]  // per vertex, [u16; 4]
)
.with_inserted_attribute(
    Mesh::ATTRIBUTE_JOINT_WEIGHT,
    vec![[1.0f32, 0.0, 0.0, 0.0]; 4]     // per vertex, [f32; 4]
)
```

For sprite rig bones (rigid quads, no deformation), each vertex has weight 1.0 on exactly one joint. No blending between joints needed.

**Programmatic construction**: Bevy's `custom_skinned_mesh` example demonstrates building a `SkinnedMesh` entirely from code -- no GLTF required. Joint entities are spawned with `Transform`, mesh is built with `Mesh::new()` + vertex attributes.

**Max joints**: 256 per mesh (configurable via `GlobalSkinnedMeshSettings` in newer Bevy). The humanoid rig has 7 bones -- well within limits.

#### Concrete Mesh Construction for the Humanoid Rig

Current humanoid: 7 bones, 6 visible (root has no slot). Each visible bone becomes a quad in the mesh:

```
Mesh vertices (24 total = 6 quads * 4 vertices):
  Quad 0 (torso):  4 vertices, joint_index=1, UVs -> atlas region for torso
  Quad 1 (arm_l):  4 vertices, joint_index=3, UVs -> atlas region for arm_l
  Quad 2 (leg_l):  4 vertices, joint_index=5, UVs -> atlas region for leg_l
  Quad 3 (leg_r):  4 vertices, joint_index=6, UVs -> atlas region for leg_r
  Quad 4 (arm_r):  4 vertices, joint_index=4, UVs -> atlas region for arm_r
  Quad 5 (head):   4 vertices, joint_index=2, UVs -> atlas region for head

Index buffer (36 total = 6 quads * 6 indices):
  Ordered back-to-front by z_order for correct overlap (painter's algorithm)
```

**Z-ordering**: quads are ordered in the index buffer by z_order (back-to-front). With `AlphaMode::Blend` and depth-write off, later triangles paint over earlier ones. This matches the current z_order values: arm_l (-0.001) behind torso (0.0) behind arm_r (0.001), etc.

Alternatively, per-vertex z-offsets can be baked into positions (matching the current approach where `Transform::translation.z = z_order`), with depth testing enabled. The current rig already uses this pattern.

#### Joint Entity Hierarchy

Joint entities replace the current bone entities but are invisible (no mesh):

```
CharacterEntity (AnimationPlayer, SkinnedMesh)
  └─ JointRoot (scale.x for facing)
       └─ joint_root (Transform)
            ├─ joint_torso (Transform)
            │   ├─ joint_head (Transform)
            │   ├─ joint_arm_l (Transform)
            │   └─ joint_arm_r (Transform)
            ├─ joint_leg_l (Transform)
            └─ joint_leg_r (Transform)
```

The `AnimationPlayer` drives joint `Transform` exactly as it drives bone `Transform` today. `AnimationTargetId::from_names` works the same way. The animation clip building code (`build_clip_from`) needs no changes to its curve generation -- it already targets `Transform` fields by bone name.

#### Skinning + Billboard Vertex Shader Composition

**Critical detail**: `MaterialExtension::vertex_shader()` **replaces** the base material's vertex shader entirely -- it does not compose. This means the custom billboard vertex shader must also handle skinning.

The shader must:
1. Import `bevy_pbr::skinning` and call `skin_model(joint_indices, joint_weights, instance_index)` to get the skinned world-from-local matrix
2. Strip rotation columns from the resulting model-view matrix for billboarding
3. Apply the final position

```wgsl
#import bevy_pbr::skinning

// In vertex main:
#ifdef SKINNED
    var world_from_local = skinning::skin_model(in.joint_indices, in.joint_weights, in.instance_index);
#else
    var world_from_local = mesh_functions::get_world_from_local(in.instance_index);
#endif

// Billboard: strip rotation from model-view, keeping translation + scale
var model_view = view.view_from_world * world_from_local;
model_view[0] = vec4<f32>(1.0, 0.0, 0.0, model_view[0][3]);
model_view[1] = vec4<f32>(0.0, 1.0, 0.0, model_view[1][3]);
model_view[2] = vec4<f32>(0.0, 0.0, 1.0, model_view[2][3]);
out.clip_position = view.clip_from_view * model_view * vec4<f32>(in.position, 1.0);
```

The `SKINNED` shader def is automatically set by Bevy when the entity has a `SkinnedMesh` component.

#### Per-Bone Texture Selection

Two approaches for selecting different textures per quad within a single mesh:

**Option A -- Texture atlas with per-vertex UVs (simpler)**:
Pack all bone sprites into one atlas image. Each quad's 4 vertices have UVs pointing at the correct atlas region. Use `StandardMaterial` with the atlas as `base_color_texture`. No custom fragment shader.

**Option B -- Array texture with per-vertex layer index (cleaner)**:
Use a `texture_2d_array` with one layer per body part. Add a custom vertex attribute for the layer index. Requires an `ExtendedMaterial` fragment shader that samples `texture_sample(tex_array, sampler, uv, layer)`. Bevy has an official [array texture example](https://bevy.org/examples/shaders/array-texture/).

For placeholder colors (current state), neither is needed -- vertex colors or a uniform color per quad suffice. Option A is the natural choice when real art arrives.

#### Instanced Skinned Meshes (Multiple Characters)

Bevy does not instance skinned meshes in the traditional sense (one draw call for N characters). Each character gets its own joint matrix set in the storage buffer and its own draw call. However, since Bevy 0.16 ([PR #16599](https://github.com/bevyengine/bevy/pull/16599)), skinned meshes are **batchable** on storage-buffer platforms -- joint matrices for all characters are packed into one buffer, reducing CPU overhead of bind calls.

Characters sharing the same `Handle<Mesh>` + `Handle<Material>` + `Handle<SkinnedMeshInverseBindposes>` get the best batching. The `many_foxes` example (1000 foxes) showed frame time improvement from 15.5ms to 11.9ms with this optimization.

#### Trade-offs

| Aspect | Per-entity bones (current) | Single skinned mesh |
|--------|---------------------------|-------------------|
| Draw calls per character | N (6 for humanoid) | 1 |
| Entities per character | 1 + 1 + N (8 for humanoid) | 1 mesh + 1 billboard + N joints (8, but joints are lighter -- no Mesh3d/MeshMaterial3d) |
| Transform propagation | N visible entities with mesh data | N joint entities (transform-only, cheaper) |
| GPU upload per frame | N dirty transforms | 1 joint matrix buffer slice (N matrices) |
| Mesh/material sharing | Requires explicit flyweight cache | Natural -- one mesh handle per rig type, one material handle per atlas |
| Animation system | Unchanged | Unchanged (AnimationPlayer targets joint Transform) |
| Adding/removing bones | Insert/remove entity | Rebuild mesh (but infrequent -- rig changes are asset-time, not runtime) |
| Per-bone gameplay queries | Natural (each bone is an entity) | Possible via joint entities (they still exist, just invisible) |

#### Industry Precedent

This is the standard approach used by Spine, DragonBones, and Unity 2D Animation:

- **Spine**: iterates slots in draw order, appends quad vertices into one mesh per skeleton, submits as one draw call per atlas page. Z-ordering via triangle submission order (painter's algorithm).
- **Unity SpriteSkin**: uses CPU or GPU deformation on individual sprites, with dynamic batching combining them. GPU path uses per-object draws.
- **No existing Bevy plugin** implements this for 2D sprite rigs. Would be built from scratch using Bevy's `custom_skinned_mesh` infrastructure.

#### Sources

- [Bevy `custom_skinned_mesh` example](https://github.com/bevyengine/bevy/blob/main/examples/animation/custom_skinned_mesh.rs)
- [Bevy skinning.wgsl source](https://github.com/bevyengine/bevy/blob/main/crates/bevy_pbr/src/render/skinning.wgsl)
- [Bevy array texture example](https://bevy.org/examples/shaders/array-texture/)
- [PR #16599 -- Batch skinned meshes with storage buffers](https://github.com/bevyengine/bevy/pull/16599)
- [MaterialExtension docs](https://docs.rs/bevy/latest/bevy/pbr/trait.MaterialExtension.html)
- [Spine Runtime Skeletons](http://en.esotericsoftware.com/spine-runtime-skeletons)
- [Spine-Unity Rendering](http://en.esotericsoftware.com/spine-unity-rendering)

---

## Performance Analysis: CPU vs GPU Billboarding

### CPU-Side Cost

- ECS iteration: microseconds for ~100 entities (Bevy's archetype iteration is very fast)
- Rotation math: `atan2` + quaternion multiply -- negligible
- **Real cost**: dirtying `Transform` triggers re-upload via retained render world change detection. Every billboard entity's transform is marked modified every frame, even when only the camera moved.

### GPU-Side Cost

- ~3 vector operations per vertex (normalize, cross, multiply) -- negligible on GPU
- Zero CPU cost per frame (vertex shader does the work)
- No transform re-upload when only camera moves

### Crossover Point

| Entity count | Recommendation |
|-------------|---------------|
| < 100 | CPU is trivially fast; architecture and correctness are better reasons to switch |
| 100-1000 | GPU avoids dirtying transforms every frame, measurable benefit |
| 1000+ | GPU strongly recommended |

**For this project**: the argument for GPU billboarding is **architectural**, not performance-critical:
1. Eliminates two per-frame systems (`billboard_rigs_face_camera`, `billboard_face_camera`)
2. Stops dirtying transforms every frame (retained render world benefit)
3. Multi-camera correctness for free
4. Cleaner separation: ECS `Transform` reflects game-logic position, GPU handles visual orientation

---

## Concrete Implementation Path

### Phase 1: Billboard Material Extension

1. Create `crates/render/src/billboard_material.rs`
2. Implement `BillboardExt` as `MaterialExtension` with a vertex shader that strips rotation and reconstructs camera-facing orientation (Technique A or B above)
3. The vertex shader must handle both `SKINNED` and non-skinned paths (ifdef), calling `skinning::skin_model()` when `SKINNED` is defined -- this prepares for Phase 3
4. Register `MaterialPlugin::<ExtendedMaterial<StandardMaterial, BillboardExt>>`
5. Health bars use this material instead of plain `StandardMaterial`
6. Sprite rig bones use this material instead of plain `StandardMaterial`
7. Remove `billboard_rigs_face_camera` and `billboard_face_camera` systems
8. Rename `RigBillboard` component to `JointRoot` (it no longer handles billboarding -- only joint hierarchy parenting and `scale.x` facing flip)

### Phase 2: Shared Handles (Flyweight)

1. Create a `BoneMeshCache` resource mapping bone size -> `Handle<Mesh>`
2. Create a `BoneMaterialCache` resource mapping color/texture -> `Handle<ExtendedMaterial<StandardMaterial, BillboardExt>>`
3. Sprite rig spawning looks up caches instead of creating new assets per bone
4. This enables Bevy's automatic batching across all characters

### Phase 3: Single Skinned Mesh

Replace the per-bone entity hierarchy with a single skinned mesh per character.

1. **Build rig mesh at startup**: for each `SpriteRigAsset`, build a single `Mesh` containing all visible bone quads (24 vertices, 36 indices for humanoid). Each vertex gets `ATTRIBUTE_JOINT_INDEX` and `ATTRIBUTE_JOINT_WEIGHT` binding it to its bone. Quads ordered in index buffer by z_order (back-to-front). Cache as `Handle<Mesh>` per rig type.
2. **Compute inverse bind poses**: for each joint, the inverse bind pose is the inverse of that joint's **global** rest-pose transform (accumulated through the parent chain). For the humanoid, e.g. head's global rest pose = root * torso * head = translate(0, 2.8, 0.003), so its inverse bind pose = translate(0, -2.8, -0.003). Store as `Handle<SkinnedMeshInverseBindposes>` per rig type.
3. **Spawn joint entities**: replace visible bone entities (with Mesh3d/MeshMaterial3d) with invisible joint entities (Transform only). Same parent-child hierarchy under `JointRoot`. Keep `AnimationTargetId` + `AnimatedBy` on joints.
4. **Spawn skinned mesh entity**: single entity with `Mesh3d`, `MeshMaterial3d` (billboard material from Phase 1), `SkinnedMesh { inverse_bindposes, joints }`, `DynamicSkinnedMeshBounds`. Child of character entity (or billboard entity).
5. **Animation system**: unchanged -- `AnimationPlayer` targets joint `Transform` by name, same `build_clip_from` curves.
6. **Billboard shader**: already handles `SKINNED` path from Phase 1. The skinned world transform is computed first, then rotation is stripped for billboarding.
7. **Remove per-bone mesh/material creation** from `spawn_sprite_rigs`. Remove `BoneMeshCache`/`BoneMaterialCache` from Phase 2 (no longer needed -- one mesh per rig type).
8. **Health bars**: remain as separate non-skinned billboard entities (small, simple, not worth merging into the rig mesh).

---

## Interaction with Existing Systems

### Animation System

No change needed across all phases. `AnimationPlayer` continues writing `Transform` on joint entities (formerly bone entities). The vertex shader overrides rendering orientation without touching ECS data. In Phase 3, joint entities replace visible bone entities but retain the same hierarchy, names, and `AnimationTargetId` assignments. `build_clip_from` curve generation is unchanged.

### Facing System

`apply_facing_to_rig` sets `scale.x = -1.0` on the `JointRoot` entity. This mirrors all descendant bone positions via transform propagation -- a bone at local X=+2 gets world X=-2. The vertex shader receives each bone's world position (already mirrored) and uses it as the quad center. The shader strips rotation columns from the model-view matrix (which also strips per-quad scale), but positional mirroring is already baked into the translation component. **No change needed to the facing system.**

The `JointRoot` entity remains necessary as a hierarchy node for positional mirroring. Its rotation is no longer written per-frame (the shader handles that), but `scale.x` still flips on `Facing` change. No per-quad UV flip or `MeshTag` is needed -- character art is symmetric, and directional art will be handled by the animation system (different poses, not mirrored textures).

### Z-Ordering

**Phases 1-2**: Bone z-order is baked into `Transform::translation.z` by the animation system. The billboard vertex shader operates on the model-view matrix, which includes this z offset. Z-ordering is preserved.

**Phase 3**: Z-ordering moves from per-entity `Transform::translation.z` to index buffer ordering within the single mesh. Quads are appended back-to-front by z_order value. With depth-write off and `AlphaMode::Blend`, later triangles paint over earlier ones (painter's algorithm). Alternatively, z-offsets can be baked into vertex positions as they are today, with depth testing enabled. Either approach works; the index-buffer approach is simpler and matches industry standard (Spine).

### Health Bar Color Updates

`update_health_bars` (`health_bar.rs:92-125`) mutates the foreground material's `base_color` based on health percentage. With `ExtendedMaterial`, it would mutate the inner `StandardMaterial`'s `base_color` the same way. No change needed.

---

## Code References

### Current Billboard Systems
- `crates/sprite_rig/src/spawn.rs:251-275` -- `billboard_rigs_face_camera`
- `crates/sprite_rig/src/spawn.rs:278-296` -- `update_facing_from_velocity`
- `crates/sprite_rig/src/spawn.rs:299-317` -- `apply_facing_to_rig`
- `crates/render/src/health_bar.rs:67-89` -- `billboard_face_camera`
- `crates/sprite_rig/src/lib.rs:48-50` -- system scheduling (chained)
- `crates/render/src/lib.rs:37` -- health bar billboard scheduling

### Bone Rendering
- `crates/sprite_rig/src/spawn.rs:126-134` -- per-entity Plane3d + StandardMaterial
- `crates/sprite_rig/src/spawn.rs:82-89` -- JointRoot entity
- `crates/sprite_rig/src/animation.rs:316-345` -- translation curves with baked z-order
- `crates/sprite_rig/src/animation.rs:489-518` -- AnimationTargetId attachment

### Health Bar Rendering
- `crates/render/src/health_bar.rs:19-65` -- spawn_health_bar (unique mesh/material per bar)
- `crates/render/src/health_bar.rs:92-125` -- update_health_bars (color mutation)

## External References

- [Bevy ExtendedMaterial vertex shader blog](https://dev.to/mikeam565/rust-game-dev-log-6-custom-vertex-shading-using-extendedmaterial-4312)
- [Vertex Shaders in Bevy (WilliamR)](https://williamr.dev/posts/bevy-vertex/)
- [Simple Billboarding Vertex Shader (Geeks3D)](https://www.geeks3d.com/20140807/billboarding-vertex-shader-glsl/)
- [bevy_mod_billboard GitHub](https://github.com/kulkalkul/bevy_mod_billboard) (stalled at Bevy 0.14, but useful shader reference)
- [Bevy Hanabi particle billboard (Medium)](https://medium.com/@Sou1gh0st/gpu-particle-research-bevy-hanabi-part-4-63032e045a38)
- [Bevy 0.16 GPU-Driven Rendering](https://bevy.org/news/bevy-0-16/)
- [Bevy Automatic Instancing example](https://bevy.org/examples/shaders/automatic-instancing/)
- [Bevy Material trait docs](https://docs.rs/bevy/latest/bevy/pbr/trait.Material.html)
- [Bevy VisibilityRange example](https://bevy.org/examples/3d-rendering/visibility-range/)
- [Toji: Compute Vertex Data in WebGPU](https://toji.dev/webgpu-best-practices/compute-vertex-data.html)
- [Bevy `custom_skinned_mesh` example](https://github.com/bevyengine/bevy/blob/main/examples/animation/custom_skinned_mesh.rs)
- [Bevy `skinning.wgsl` source](https://github.com/bevyengine/bevy/blob/main/crates/bevy_pbr/src/render/skinning.wgsl)
- [Bevy array texture example](https://bevy.org/examples/shaders/array-texture/)
- [PR #16599 -- Batch skinned meshes with storage buffers](https://github.com/bevyengine/bevy/pull/16599)
- [PR #21256 -- Configurable MAX_JOINTS](https://github.com/bevyengine/bevy/pull/21256)
- [MaterialExtension trait docs](https://docs.rs/bevy/latest/bevy/pbr/trait.MaterialExtension.html)
- [Spine Runtime Skeletons](http://en.esotericsoftware.com/spine-runtime-skeletons)
- [Spine-Unity Rendering](http://en.esotericsoftware.com/spine-unity-rendering)
- [Spine In Depth](https://en.esotericsoftware.com/spine-in-depth)
- [Unity SpriteSkin docs](https://docs.unity3d.com/Packages/com.unity.2d.animation@10.1/manual/SpriteSkin.html)

## Resolved Questions

### 1. Cylindrical vs Spherical Billboard

**Resolved: use spherical (replace all 3 columns).**

Two options in the vertex shader:
- **Spherical** (replace all 3 rotation columns with identity): quad always faces screen plane exactly, no foreshortening. From an isometric camera at ~45 degrees, sprites render fully face-on.
- **Cylindrical** (replace columns 0 and 2, keep column 1): quad preserves camera pitch rotation. From 45-degree camera, sprites tilt toward camera and appear vertically foreshortened.

The current CPU implementation zeros Y in the direction vector, which is cylindrical. However, for 2.5D sprite characters that should always appear at full height regardless of camera angle, **spherical is the correct choice**. This is standard for 2D sprite rendering in 3D space.

The WGSL snippet from the research (replacing all 3 columns) already implements spherical:
```wgsl
model_view[0] = vec4<f32>(1.0, 0.0, 0.0, model_view[0][3]);
model_view[1] = vec4<f32>(0.0, 1.0, 0.0, model_view[1][3]);
model_view[2] = vec4<f32>(0.0, 0.0, 1.0, model_view[2][3]);
```

Note: this is a visual behavior change from the current CPU cylindrical approach. Sprites will no longer foreshorten when the camera looks down. This is likely desirable but should be verified visually.

Source: [Geeks3D Billboard Tutorial](https://www.geeks3d.com/20140807/billboarding-vertex-shader-glsl/)

### 2. Bevy 0.18 ExtendedMaterial API

**Resolved: verified against local Bevy 0.18 source.**

- **Entry point**: `@vertex fn vertex(vertex_no_morph: Vertex) -> VertexOutput` (mesh.wgsl:37)
- **View uniform naming** (Bevy 0.18 uses `destination_from_source` convention):
  - `view.view_from_world_from_world` (view matrix, was `view.view_from_world`)
  - `view.clip_from_view` (projection matrix, was `view.clip_from_view`)
  - `view.clip_from_world` (view-projection matrix, was `view.clip_from_world`)
  - `view.world_position` (camera position)
- **Required imports**:
  ```wgsl
  #import bevy_pbr::{
      mesh_bindings::mesh,
      mesh_functions,
      skinning,
      forward_io::{Vertex, VertexOutput},
      view_transformations::position_world_to_clip,
  }
  ```
- **Skinning**: `skinning::skin_model(vertex.joint_indices, vertex.joint_weights, vertex_no_morph.instance_index)` returns `mat4x4<f32>`
- **Non-skinned**: `mesh_functions::get_world_from_local(vertex_no_morph.instance_index)` returns `mat4x4<f32>`
- **Vertex struct locations**: position @0, normal @1, uv @2, uv_b @3, tangent @4, color @5, joint_indices @6 (`vec4<u32>`), joint_weights @7 (`vec4<f32>`)

The corrected billboard shader snippet for Bevy 0.18:
```wgsl
#ifdef SKINNED
    var world_from_local = skinning::skin_model(vertex.joint_indices, vertex.joint_weights, vertex_no_morph.instance_index);
#else
    var world_from_local = mesh_functions::get_world_from_local(vertex_no_morph.instance_index);
#endif

var model_view = view.view_from_world_from_world * world_from_local;
model_view[0] = vec4<f32>(1.0, 0.0, 0.0, model_view[0][3]);
model_view[1] = vec4<f32>(0.0, 1.0, 0.0, model_view[1][3]);
model_view[2] = vec4<f32>(0.0, 0.0, 1.0, model_view[2][3]);
out.clip_position = view.clip_from_view * model_view * vec4<f32>(vertex.position, 1.0);
```

Source files: `git/bevy/crates/bevy_pbr/src/render/mesh.wgsl`, `skinning.wgsl`, `mesh_functions.wgsl`, `forward_io.wgsl`, `git/bevy/crates/bevy_render/src/view/view.wgsl`

### 3. Health Bar Material Mutation

**Resolved: yes, it works.**

`Assets::<ExtendedMaterial<StandardMaterial, BillboardExt>>::get_mut(&handle)` correctly triggers Bevy's asset change detection. The modified material is re-extracted to the render world and its bind group is recreated via `as_bind_group()`. Modifying `material.base.base_color` propagates to the GPU.

```rust
fn update_health_bars(
    mut materials: ResMut<Assets<ExtendedMaterial<StandardMaterial, BillboardExt>>>,
    // ...
) {
    let mat = materials.get_mut(&handle).expect("material exists");
    mat.base.base_color = new_color;
}
```

Source: [Bevy Discussion #6907](https://github.com/bevyengine/bevy/discussions/6907)

### 4. WebGPU/WebGL Compatibility

**Resolved: works on both, with WebGL 2 constraints.**

- **WebGPU**: full support, no issues
- **WebGL 2**: `ExtendedMaterial` works but with constraints:
  - Uniform struct alignment must be 16 bytes ([PR #18812](https://github.com/bevyengine/bevy/pull/18812) fixed this for Bevy's own examples)
  - No vertex storage buffers (`DownlevelFlags::VERTEX_STORAGE` unavailable)
  - Uniform buffer limit: 16 KiB per binding
  - Skinning uses uniform buffers on WebGL 2: max 256 joints (16384 bytes / 64 bytes per mat4)
  - **Skinned mesh batching is NOT available on WebGL 2** -- each skinned mesh is a separate draw call (storage buffer batching from [PR #16599](https://github.com/bevyengine/bevy/pull/16599) requires storage buffers)

For this project: the humanoid rig has 7 joints, well within the 256 limit. The BillboardExt struct has no custom uniforms (camera data comes from Bevy's view uniform at group 0), so the 16-byte alignment constraint is moot. WebGL 2 will work but each character is a separate draw call (no batching of skinned meshes).

### 5. Skinned Mesh + Billboard Composition

**Resolved: composition works correctly.**

`skinning::skin_model()` returns a `mat4x4<f32>` that is the blended `world_from_model` transform: `joint_global_transform * inverse_bind_pose`. This is a world-space transform. Multiplying by `view.view_from_world_from_world` gives a model-view matrix whose rotation columns can be stripped for billboarding.

The extraction pipeline (in `skin.rs:498`) computes per-joint:
```rust
joint_matrix = joint_transform.affine() * inverse_bindpose
```

The vertex shader blends 4 of these by weight (in our case weight is 1.0 on one joint, 0.0 on others -- rigid binding). The result is the joint's world-from-model matrix. Stripping rotation from the model-view matrix then makes the quad face the camera while preserving the world-space position.

The `SKINNED` shader def is automatically set by Bevy when the entity has a `SkinnedMesh` component. Non-skinned entities (health bars) take the `#else` path using `get_world_from_local`.

### 6. Inverse Bind Pose Computation

**Resolved: must be the inverse of each joint's GLOBAL rest-pose transform, not local.**

The extraction code multiplies `joint_global_transform * inverse_bind_pose` to produce `world_from_model`. This only works if the inverse bind pose encodes the full global rest transform. Using `Mat4::from_translation(-local_position)` would be **incorrect** for child joints.

For the humanoid rig, the computation at startup:

```rust
// Accumulate global rest-pose transforms through the hierarchy
fn compute_global_rest_pose(bone_defs: &[BoneDef]) -> Vec<Mat4> {
    let mut global_transforms = vec![Mat4::IDENTITY; bone_defs.len()];
    for (i, bone) in bone_defs.iter().enumerate() {
        let local = Mat4::from_translation(Vec3::new(
            bone.default_transform.translation.x,
            bone.default_transform.translation.y,
            z_order,  // from slot lookup
        ));
        global_transforms[i] = match bone.parent {
            Some(parent_idx) => global_transforms[parent_idx] * local,
            None => local,
        };
    }
    // Inverse bind poses = inverse of each global rest-pose
    global_transforms.iter().map(|g| g.inverse()).collect()
}
```

Example for the humanoid:
- `root`: global = translate(0, 0, 0), inverse = translate(0, 0, 0)
- `torso`: global = translate(0, 1, 0), inverse = translate(0, -1, 0)
- `head`: global = translate(0, 1, 0) * translate(0, 1.8, 0.003) = translate(0, 2.8, 0.003), inverse = translate(0, -2.8, -0.003)
- `arm_l`: global = translate(0, 1, 0) * translate(-1.2, 0, -0.001) = translate(-1.2, 1, -0.001), inverse = translate(1.2, -1, 0.001)

This is confirmed by the glTF spec and Bevy's glTF loader, which reads `inverseBindMatrices` as global rest-pose inverses.

### 7. Skinned Mesh Entity Parenting

**Resolved: parent the skinned mesh entity to the character root, NOT the billboard.**

In the `SKINNED` vertex shader path, `skin_model()` replaces `get_world_from_local()`. The mesh entity's own `GlobalTransform` (and thus its parent's transform) is **not used for vertex positioning** -- the skinned path bypasses it entirely. Joint entities provide the world-space transforms directly.

This means:
- The `JointRoot` entity's `scale.x = -1` does NOT affect the skinned mesh entity's vertex positions (the skinned path ignores the mesh's model matrix)
- The `scale.x = -1` DOES affect the joint entities (which are children of the billboard), and their `GlobalTransform` feeds into the joint matrix computation
- So facing flip works correctly: joint world positions are mirrored by the billboard's scale, which propagates into the joint matrices, which the vertex shader uses

The skinned mesh entity can be a child of the character root or even a sibling -- its parenting doesn't affect rendering. Parenting to the character root is simplest.

However, `DynamicSkinnedMeshBounds` converts the computed world-space AABB back to entity-space using the mesh entity's `GlobalTransform` inverse (skinning.rs:234-243). If the mesh entity has an unexpected transform, this conversion could produce wrong bounds. Keeping the mesh entity at `Transform::default()` as a child of the character root avoids this.

### 8. DynamicSkinnedMeshBounds

**Resolved: works correctly for flat 2D meshes, should be included.**

`DynamicSkinnedMeshBounds` is a marker component (no data). When present, `update_skinned_mesh_bounds` recomputes the entity's `Aabb` every frame from joint positions. The algorithm:

1. Reads per-joint AABBs from `SkinnedMeshBounds` (precomputed from vertex positions at mesh creation)
2. For each joint: computes `world_from_model = world_from_joint * inverse_bind_pose`
3. Transforms the model-space AABB to world space
4. Accumulates all joint AABBs into one enclosing AABB
5. Converts back to entity-space using the mesh entity's `GlobalTransform` inverse

**For flat meshes**: per-joint AABBs will have zero thickness on the Z axis (all vertices at the same Z for a given joint). `transform_aabb` handles this correctly -- the Arvo (1990) algorithm works with zero-extent axes. Frustum culling tests halfspace intersections, which handles infinitely thin slabs. The sphere radius test uses max extent, which is nonzero as long as any axis has extent.

**Without this component**: the mesh keeps a static bind-pose AABB that never updates. For animated characters whose joints move significantly, this risks incorrect frustum culling (characters disappearing when they shouldn't). **Always include `DynamicSkinnedMeshBounds` on animated skinned meshes.**

One edge case: if the mesh is flat AND viewed perfectly edge-on, the AABB has near-zero projected area. In practice this won't happen with an isometric camera viewing XY-plane quads. No special handling needed.

Source files: `git/bevy/crates/bevy_camera/src/visibility/mod.rs:286`, `git/bevy/crates/bevy_mesh/src/skinning.rs:188-244`
