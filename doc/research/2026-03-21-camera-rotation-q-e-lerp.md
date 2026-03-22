---
date: 2026-03-21T23:02:50-07:00
researcher: Claude
git_commit: 63fffb9e91e9117563b3d7e60efc21365eebe5d2
branch: bevy-lightyear-template-2
repository: bevy-lightyear-template-2
topic: "90-degree camera rotation around the player when pressing Q or E, with lerping"
tags: [research, codebase, camera, input, rotation, lerp]
status: complete
last_updated: 2026-03-21
last_updated_by: Claude
---

# Research: 90-Degree Camera Rotation Around Player (Q/E with Lerp)

**Date**: 2026-03-21T23:02:50-07:00
**Researcher**: Claude
**Git Commit**: 63fffb9e91e9117563b3d7e60efc21365eebe5d2
**Branch**: bevy-lightyear-template-2
**Repository**: bevy-lightyear-template-2

## Research Question

How does the current camera system work, and what needs to change to support 90-degree camera rotation around the player when pressing Q or E, with smooth lerping?

## Summary

The camera is a fixed-offset follower with no rotation state. Movement is mapped directly to world axes, not camera-relative. Implementing orbital Q/E rotation requires: (1) a rotation state resource/component, (2) lerped rotation updates on Q/E press, (3) rotating the camera offset vector, and (4) making player movement camera-relative. Several downstream systems (billboard facing, voxel raycasting) already derive their orientation from the camera transform, so they will adapt automatically.

## Detailed Findings

### Current Camera System

`crates/render/src/camera.rs` — the entire camera implementation:

- **`setup_camera`** (line 5): Spawns `Camera3d` at `(0, 18, -36)` looking at origin.
- **`follow_player`** (line 22): Queries the `Controlled` player's `Position`, adds a hardcoded `Vec3::new(0.0, 18.0, -36.0)` offset, calls `look_at` toward the player. No smoothing, no rotation state.

Registered in `crates/render/src/lib.rs:27-35`: `setup_camera` on `Startup`, `follow_player` on `Update`.

### Current Input System

- `PlayerActions` enum at `crates/protocol/src/lib.rs:55-64` defines: `Move` (DualAxis), `Jump`, `PlaceVoxel`, `RemoveVoxel`, `Ability1-4`.
- `InputMap` constructed at `crates/client/src/gameplay.rs:39-48`: WASD→Move, Space→Jump, Mouse buttons→voxel, Digits→abilities.
- **Q and E are completely unbound** — available for camera rotation.

### Movement System (Camera-Dependent)

`crates/protocol/src/character/movement.rs`:

- **`apply_movement`** (line 10): Reads `PlayerActions::Move` axis pair, maps directly to world-space: `Vec3::new(-move_dir.x, 0.0, move_dir.y)`. The camera orientation is **not consulted**.
- **`update_facing`** (line 70): Sets character `Rotation` from `atan2(move_dir.x, -move_dir.y)` — also world-space, not camera-relative.

**This means**: if the camera rotates 90 degrees, pressing W would still move the character in the same world direction, not "forward" relative to the new camera angle. Movement must be transformed by the camera's yaw.

### Downstream Camera-Dependent Systems

These systems read the camera's `Transform`/`GlobalTransform` directly and will **automatically adapt** to camera rotation:

1. **`billboard_rigs_face_camera`** (`crates/sprite_rig/src/spawn.rs:251`): Rotates `RigBillboard` entities to face `Camera3d` using atan2 Y-rotation. Reads camera `GlobalTransform` each frame.

2. **`billboard_face_camera`** (`crates/render/src/health_bar.rs:67`): Rotates health bar billboards to face camera. Uses `Quat::from_rotation_arc` from camera direction.

3. **`camera_ray`** (`crates/client/src/map.rs`): Projects cursor through `Camera3d` for voxel raycasting. Uses `camera.viewport_to_world()`.

All three derive orientation from the camera's live transform — no hardcoded angles.

### Existing Lerp Patterns in the Codebase

The only custom lerp is in `crates/sprite_rig/src/animation.rs:547-596`:

```rust
const BLEND_LERP_SPEED: f32 = 10.0;
let lerp_factor = (BLEND_LERP_SPEED * dt).min(1.0);
*current += (target - *current) * lerp_factor;
```

Frame-rate-independent exponential approach. This same pattern could be used for camera angle lerping. For quaternion rotation, use `Quat::slerp` with an equivalent factor.

### Protocol/Networking Consideration

Camera rotation is **client-only** (visual). It doesn't need to be networked or predicted. However, `apply_movement` in the `protocol` crate (shared between client and server) currently ignores camera orientation. To make movement camera-relative:

- Either pass a camera yaw angle into `apply_movement` (adding it to the shared function signature), or
- Transform the input axis pair before it reaches the `ActionState` / before calling `apply_movement` on the client, while the server continues to use raw input.

The latter approach (client transforms input before sending) keeps the protocol crate unchanged but means the server receives already-rotated movement vectors. This is the simpler path since `PlayerActions::Move` is a `DualAxis` — the client can rotate the vector before it's consumed.

## Code References

- `crates/render/src/camera.rs:5-9` — Camera spawn with hardcoded offset
- `crates/render/src/camera.rs:22-36` — `follow_player` system
- `crates/render/src/lib.rs:27-35` — Camera system registration
- `crates/protocol/src/character/movement.rs:10-66` — `apply_movement` (world-space)
- `crates/protocol/src/character/movement.rs:70-81` — `update_facing` (world-space)
- `crates/protocol/src/lib.rs:55-64` — `PlayerActions` enum
- `crates/client/src/gameplay.rs:39-48` — `InputMap` construction
- `crates/sprite_rig/src/spawn.rs:251` — `billboard_rigs_face_camera`
- `crates/render/src/health_bar.rs:67` — `billboard_face_camera`
- `crates/client/src/map.rs:368` — `camera_ray` usage
- `crates/sprite_rig/src/animation.rs:547-596` — Existing exponential lerp pattern

## Architecture Documentation

### Camera Offset Geometry

Current offset `(0, 18, -36)` places the camera behind and above the player in an isometric-like view. The `look_at` call makes the camera point at the player. This offset defines one of four potential 90° positions:

| Direction | Offset Vector |
|-----------|---------------|
| South (current) | `(0, 18, -36)` |
| West (90° CW) | `(36, 18, 0)` |
| North (180°) | `(0, 18, 36)` |
| East (90° CCW) | `(-36, 18, 0)` |

These are rotations of the base offset around the Y-axis by 0°, 90°, 180°, 270°.

### System Execution Order

The `follow_player` system runs every frame in `Update`. Billboard systems also run in `Update` (sprite_rig is chained). The camera position is set before billboards read it within the same frame, assuming ordering — though currently no explicit ordering constraint is declared between `RenderPlugin` and `SpriteRigPlugin` systems.

## Design Decisions

1. **Movement rotation**: Client-side. Rotate the input vector by camera yaw on the client before it enters `apply_movement`. Protocol unchanged — the server receives already-rotated movement vectors.
2. **Facing direction**: Yes — `update_facing` accounts for camera rotation so the character faces their camera-relative movement direction.
3. **Lerp speed**: 20.0 (exponential approach, converges in ~0.15s).
4. **Rotation mode**: Four discrete positions only (0°, 90°, 180°, 270°). Q/E cycles between them on `just_pressed`.
