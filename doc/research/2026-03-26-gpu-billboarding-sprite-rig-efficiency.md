---
date: 2026-03-26T11:01:37-07:00
researcher: Claude
git_commit: ac0c8c45c6dcece0d80674c707487cf1d8bbbf3a
branch: master
repository: bevy-lightyear-template
topic: "GPU billboarding for sprite rigs and health bars"
tags: [research, optimization, billboarding, gpu, shader, sprite-rig, health-bar, vertex-shader, instancing]
status: complete
last_updated: 2026-03-26
last_updated_by: Claude
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
  └─ RigBillboard (Transform, marker)
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

`apply_facing_to_rig` (`spawn.rs:299-317`) mirrors the entire bone hierarchy by setting `RigBillboard`'s `transform.scale.x` to `1.0` or `-1.0` when `Facing` changes.

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
var model_view = view.view * mesh_functions::get_world_from_local(in.instance_index);
// Replace rotation columns with identity (cylindrical: preserve column 1 for Y-lock)
model_view[0] = vec4<f32>(1.0, 0.0, 0.0, model_view[0][3]);
model_view[1] = vec4<f32>(0.0, 1.0, 0.0, model_view[1][3]);
model_view[2] = vec4<f32>(0.0, 0.0, 1.0, model_view[2][3]);
let clip_pos = view.projection * model_view * vec4<f32>(in.position, 1.0);
```

**Technique B -- Reconstruct camera basis from view-projection:**
```wgsl
let camera_right = normalize(vec3<f32>(view.view_proj.x.x, view.view_proj.y.x, view.view_proj.z.x));
let camera_up = normalize(vec3<f32>(view.view_proj.x.y, view.view_proj.y.y, view.view_proj.z.y));
let world_pos = center + camera_right * in.position.x * scale.x + camera_up * in.position.y * scale.y;
```

**Technique C -- Cross-product basis (used by Hanabi):**
```wgsl
let axis_z = normalize(camera_position - entity_position);
let axis_x = normalize(cross(view.view[1].xyz, axis_z));
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
| Facing flip | `scale.x = -1.0` on billboard entity | Could use negative scale on entity, or a shader uniform |
| System scheduling | Must run after transform propagation | Eliminated |
| Bone animation | Unchanged (AnimationPlayer writes bone transforms) | Unchanged |

#### Interaction with Animation System

The animation system writes `Transform::translation`, `rotation`, and `scale` on bone entities via `AnimatableCurve`. The billboard vertex shader would override the *rendering rotation* without touching the ECS `Transform`. This means:
- Bone-relative transforms (parent-child hierarchy) work normally
- The billboard entity no longer needs its rotation written per-frame
- Z-ordering (baked into `Transform::translation.z`) is preserved
- Facing flip via `scale.x` still works (the vertex shader sees the final model matrix including scale)

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

### Optimization 3: Single Skinned Mesh (Not Recommended Yet)

Build one mesh containing all bone quads with per-vertex bone indices. Upload bone transforms as a joint matrix buffer. Vertex shader applies skinning + billboarding. **One draw call per character.**

This is standard GPU skeletal animation applied to 2D sprite planes. Bevy already has `SkinnedMesh` + `joint_matrices` infrastructure in `mesh.wgsl`.

**Trade-offs**:

| Aspect | Per-entity bones (current) | Single skinned mesh |
|--------|---------------------------|-------------------|
| Draw calls per character | N | 1 |
| Transform overhead | N ECS entities in hierarchy | 1 buffer upload |
| Implementation complexity | Simple ECS | Custom mesh builder + skinning setup |
| Flexibility | Easy add/remove bones | Fixed topology, rebuild mesh |
| Bevy integration | Native | Requires manual mesh construction or GLTF tooling |

At the expected character counts (<50 on screen), the per-entity approach with shared handles + batching is sufficient. The skinned mesh approach would matter at 100+ characters.

### Optimization 4: Distance-Based LOD for Rigs

Replace multi-bone rigs with a single billboard sprite at distance:
- Near: 1+1+N entities, full animation
- Far: 1 entity, single textured quad, billboard shader

Use Bevy's built-in `VisibilityRange` component for distance-based switching.

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
3. Register `MaterialPlugin::<ExtendedMaterial<StandardMaterial, BillboardExt>>`
4. Health bars use this material instead of plain `StandardMaterial`
5. Sprite rig bones use this material instead of plain `StandardMaterial`
6. Remove `billboard_rigs_face_camera` and `billboard_face_camera` systems
7. Keep `Facing` flip via `scale.x` (the vertex shader respects the model matrix scale)

### Phase 2: Shared Handles (Flyweight)

1. Create a `BoneMeshCache` resource mapping bone size -> `Handle<Mesh>`
2. Create a `BoneMaterialCache` resource mapping color/texture -> `Handle<ExtendedMaterial<StandardMaterial, BillboardExt>>`
3. Sprite rig spawning looks up caches instead of creating new assets per bone
4. This enables Bevy's automatic batching across all characters

### Phase 3: Texture Atlas (When Real Art Arrives)

1. Pack bone sprites into atlas images per character type
2. All bones share one material (same atlas texture)
3. Per-bone UV coordinates select sub-sprite
4. One draw call per character type for all bone quads

### Phase 4: Distance LOD (Optional)

1. At distance, replace multi-entity rig with single billboard quad
2. Use `VisibilityRange` for smooth crossfade
3. Dramatic entity count reduction for distant characters

---

## Interaction with Existing Systems

### Animation System

No change needed. `AnimationPlayer` continues writing `Transform` on bone entities. The vertex shader overrides rendering orientation without touching ECS data. Bone hierarchy (parent-child transforms) works the same.

### Facing System

`apply_facing_to_rig` sets `scale.x = -1.0` on the `RigBillboard` entity. The billboard vertex shader receives the final model matrix (after transform propagation), which includes this scale flip. The shader's rotation-stripping preserves scale. **No change needed.**

However, with GPU billboarding, the `RigBillboard` intermediate entity becomes optional for rotation purposes. It could be kept purely for the `Facing` scale flip, or the flip could be moved to a shader uniform / `MeshTag` per-instance data.

### Z-Ordering

Bone z-order is baked into `Transform::translation.z` by the animation system. The billboard vertex shader operates on the model-view matrix, which includes this z offset. **Z-ordering is preserved.**

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
- `crates/sprite_rig/src/spawn.rs:82-89` -- RigBillboard entity
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

## Open Questions

1. **Facing flip mechanism**: Keep the `RigBillboard` entity with `scale.x` flip, or move facing to a shader uniform/`MeshTag`? The `RigBillboard` entity is still useful as a hierarchy node even if its rotation is no longer written.
2. **Cylindrical vs spherical**: Current implementation zeros Y for cylindrical billboard. The vertex shader should do the same (preserve column 1 of model-view matrix). Need to confirm the WGSL snippet handles this correctly for the isometric camera angle.
3. **bevy 0.18 ExtendedMaterial API**: The `MaterialExtension` trait and `ExtendedMaterial` wrapper should be stable, but verify the vertex shader entry point name and available imports (`mesh_functions::get_world_from_local`, `view.view_proj`) against 0.18's `bevy_pbr` shader source.
4. **Health bar material mutation**: With `ExtendedMaterial<StandardMaterial, BillboardExt>`, can `base_color` still be mutated at runtime for health percentage? Should work via `Assets<ExtendedMaterial<...>>::get_mut()`, but needs verification.
5. **WebGPU/WebGL compatibility**: Vertex shader billboarding works on all backends. The `ExtendedMaterial` approach should work on WebGPU. WebGL 2 compatibility of `MaterialExtension` vertex shaders needs testing.
