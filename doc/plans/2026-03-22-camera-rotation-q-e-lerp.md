# Camera Rotation (Q/E with Lerp) Implementation Plan

## Overview

Add 90-degree orbital camera rotation around the player on Q/E press with smooth lerping. Movement and facing become camera-relative by rotating the `ActionState` move axis pair on the client before lightyear captures it — no protocol changes needed.

## Current State Analysis

- Camera: fixed offset `(0, 18, -36)`, snaps to player each frame (`crates/render/src/camera.rs:22-36`)
- Movement: world-space, `Vec3::new(-move_dir.x, 0.0, move_dir.y)` (`crates/protocol/src/character/movement.rs:54`)
- Facing: world-space, `atan2(move_dir.x, -move_dir.y)` (`crates/protocol/src/character/movement.rs:78`)
- Q/E: unbound
- Lighting: fixed position `(8, 16, 8)` (`crates/render/src/camera.rs:18`)
- Billboard systems read camera transform live — adapt automatically
- `Rotation` is predicted with rollback (`crates/protocol/src/lib.rs:190-194`)

### Key Discoveries:
- `ActionState::set_axis_pair()` exists — lightyear's FPS example uses it for programmatic axis mutation
- Leafwing updates `ActionState` in `PreUpdate`
- Lightyear buffers inputs in `FixedPreUpdate` at `InputSystems::BufferClientInputs`
- Existing exponential lerp pattern: `(SPEED * dt).min(1.0)` (`crates/sprite_rig/src/animation.rs:70-88`)

## Desired End State

- Q rotates camera 90° counter-clockwise around player, E rotates clockwise
- Four discrete positions (0°, 90°, 180°, 270°), smooth lerp between them (~0.15s)
- WASD movement is camera-relative (W = toward camera "forward")
- Character faces camera-relative direction
- Lighting follows camera rotation
- Server receives already-rotated input — prediction works identically on both sides
- No changes to `crates/protocol/`

### Verification:
- Press Q/E: camera smoothly orbits to next 90° position
- During/after rotation, WASD moves character relative to camera facing
- Character faces movement direction relative to camera
- Lighting rotates with camera
- Multiplayer: no rollback fighting, server movement matches client

## What We're NOT Doing

- Free-rotation (mouse orbit) — discrete 90° only
- Camera zoom
- Camera tilt adjustment
- Networking camera state
- Modifying the protocol crate

## Implementation Approach

**Input rotation strategy**: A client-only system in `FixedPreUpdate` (before `InputSystems::BufferClientInputs`) reads the camera's current yaw and calls `set_axis_pair` to rotate the move vector. Lightyear then buffers and replicates the already-rotated values. Both client and server run identical `apply_movement` / `update_facing` on the same rotated input.

## Phase 1: Camera Orbit State & Lerped Follow

### Overview
Add `CameraOrbitState` component to the camera entity. Modify `follow_player` to rotate the offset vector by the current (lerped) angle.

### Changes Required:

#### 1. `CameraOrbitState` component
**File**: `crates/render/src/camera.rs`

```rust
/// Orbital camera state for discrete 90° rotation around the player.
#[derive(Component)]
pub struct CameraOrbitState {
    /// Target angle in radians (one of 0, π/2, π, 3π/2)
    pub target_angle: f32,
    /// Current angle in radians (lerps toward target)
    pub current_angle: f32,
}

impl Default for CameraOrbitState {
    fn default() -> Self {
        Self {
            target_angle: 0.0,
            current_angle: 0.0,
        }
    }
}
```

#### 2. Spawn camera with `CameraOrbitState`
**File**: `crates/render/src/camera.rs`
**Change**: Add `CameraOrbitState::default()` to the camera spawn bundle.

```rust
pub(crate) fn setup_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 18.0, -36.0).looking_at(Vec3::ZERO, Dir3::Y),
        CameraOrbitState::default(),
    ));
}
```

#### 3. Modify `follow_player` to use rotated offset
**File**: `crates/render/src/camera.rs`

```rust
const BASE_OFFSET: Vec3 = Vec3::new(0.0, 18.0, -36.0);

pub(crate) fn follow_player(
    player_query: Query<&Position, With<Controlled>>,
    mut camera_query: Query<(&mut Transform, &CameraOrbitState), With<Camera3d>>,
) {
    let Ok(player_pos) = player_query.single() else {
        return;
    };
    let Ok((mut camera_transform, orbit)) = camera_query.single_mut() else {
        return;
    };

    let rotated_offset = Quat::from_rotation_y(orbit.current_angle) * BASE_OFFSET;
    camera_transform.translation = **player_pos + rotated_offset;
    camera_transform.look_at(**player_pos, Dir3::Y);
}
```

#### 4. Lerp system for orbit angle
**File**: `crates/render/src/camera.rs`

```rust
const ORBIT_LERP_SPEED: f32 = 20.0;

/// Lerps camera orbit angle toward the target using frame-rate-independent exponential approach.
fn update_camera_orbit(time: Res<Time>, mut query: Query<&mut CameraOrbitState>) {
    let dt = time.delta_secs();
    let lerp_factor = (ORBIT_LERP_SPEED * dt).min(1.0);

    for mut orbit in &mut query {
        let diff = orbit.target_angle - orbit.current_angle;
        if diff.abs() > 0.001 {
            orbit.current_angle += diff * lerp_factor;
        } else {
            orbit.current_angle = orbit.target_angle;
        }
    }
}
```

#### 5. Register `update_camera_orbit`
**File**: `crates/render/src/lib.rs`
**Change**: Add `update_camera_orbit` to `Update`, before `follow_player`.

```rust
app.add_systems(
    Update,
    (
        camera::update_camera_orbit,
        camera::follow_player,
        health_bar::billboard_face_camera,
        health_bar::update_health_bars,
    )
        .chain(),
);
```

Note: `.chain()` ensures orbit update runs before follow_player. The other systems also benefit from ordering (billboard reads camera transform set by follow_player).

### Success Criteria:

#### Automated Verification:
- [x] `cargo check-all`

#### Manual Verification:
- [ ] Camera spawns at default position (unchanged from current behavior)
- [ ] No visual difference yet (orbit angle is 0)

---

## Phase 2: Q/E Input Handling

### Overview
Handle Q/E keypresses to cycle the camera's target angle by ±90°. Uses `ButtonInput<KeyCode>` directly since this is client-only — no need to add to `PlayerActions`.

### Changes Required:

#### 1. Q/E input system
**File**: `crates/render/src/camera.rs`

```rust
/// Handles Q/E input to rotate camera orbit by 90° increments.
fn handle_camera_rotation_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut query: Query<&mut CameraOrbitState>,
) {
    let Ok(mut orbit) = query.single_mut() else {
        return;
    };

    if keys.just_pressed(KeyCode::KeyQ) {
        orbit.target_angle += std::f32::consts::FRAC_PI_2;
    }
    if keys.just_pressed(KeyCode::KeyE) {
        orbit.target_angle -= std::f32::consts::FRAC_PI_2;
    }
}
```

#### 2. Register the system
**File**: `crates/render/src/lib.rs`
**Change**: Add `handle_camera_rotation_input` before `update_camera_orbit` in the chain.

```rust
app.add_systems(
    Update,
    (
        camera::handle_camera_rotation_input,
        camera::update_camera_orbit,
        camera::follow_player,
        health_bar::billboard_face_camera,
        health_bar::update_health_bars,
    )
        .chain(),
);
```

### Success Criteria:

#### Automated Verification:
- [x] `cargo check-all`

#### Manual Verification:
- [ ] Q press: camera smoothly rotates 90° CCW around player
- [ ] E press: camera smoothly rotates 90° CW around player
- [ ] Multiple presses queue correctly (pressing Q twice → 180°)
- [ ] WASD movement still world-space (not yet camera-relative — that's Phase 3)

---

## Phase 3: Camera-Relative Movement Input

### Overview
Client-only system that rotates the `ActionState<PlayerActions>::Move` axis pair by the camera's current yaw angle. Runs in `FixedPreUpdate` before lightyear buffers the input, so both client and server receive the rotated values.

### Changes Required:

#### 1. Make `CameraOrbitState` public
**File**: `crates/render/src/camera.rs`
**Change**: `CameraOrbitState` is already `pub`. Ensure `crates/render/src/lib.rs` re-exports it.

**File**: `crates/render/src/lib.rs`
**Change**: Add `pub use camera::CameraOrbitState;` at the top.

#### 2. Input rotation system
**File**: `crates/client/src/gameplay.rs`

```rust
use render::CameraOrbitState;

/// Rotates the move input axis pair by camera yaw so movement is camera-relative.
/// Runs before lightyear buffers inputs, so the server receives already-rotated values.
fn rotate_movement_input(
    camera_query: Query<&CameraOrbitState>,
    mut player_query: Query<&mut ActionState<PlayerActions>, With<Predicted>>,
) {
    let Ok(orbit) = camera_query.single() else {
        return;
    };
    if orbit.current_angle.abs() < 0.001 {
        return;
    }

    for mut action_state in &mut player_query {
        let move_input = action_state.axis_pair(&PlayerActions::Move);
        if move_input == Vec2::ZERO {
            continue;
        }
        let (sin, cos) = orbit.current_angle.sin_cos();
        let rotated = Vec2::new(
            move_input.x * cos - move_input.y * sin,
            move_input.x * sin + move_input.y * cos,
        );
        action_state.set_axis_pair(&PlayerActions::Move, rotated);
    }
}
```

#### 3. Register with correct ordering
**File**: `crates/client/src/gameplay.rs`

```rust
use lightyear::prelude::InputSystems;

// In ClientGameplayPlugin::build:
app.add_systems(
    FixedPreUpdate,
    rotate_movement_input.before(InputSystems::BufferClientInputs),
);
```

### Success Criteria:

#### Automated Verification:
- [x] `cargo check-all`

#### Manual Verification:
- [ ] Default camera (0° rotation): WASD unchanged
- [ ] After Q press (90° CCW): W moves character to the right relative to world, but "forward" relative to camera
- [ ] After two Q presses (180°): W moves character in +Z world direction (toward camera's new "forward")
- [ ] Character faces movement direction correctly (update_facing uses the rotated input)
- [ ] Multiplayer: start server + client, rotate camera, move — no excessive rollbacks
- [ ] During lerp transition: movement smoothly transitions between orientations

---

## Phase 4: Lighting Follows Camera

### Overview
Rotate the light position around the player to match camera rotation.

### Changes Required:

#### 1. Add marker component and modify lighting setup
**File**: `crates/render/src/camera.rs`

```rust
/// Marker for the main scene light that follows camera rotation.
#[derive(Component)]
pub struct MainLight;

const BASE_LIGHT_OFFSET: Vec3 = Vec3::new(8.0, 16.0, 8.0);

pub(crate) fn setup_lighting(mut commands: Commands) {
    commands.spawn((
        PointLight {
            shadows_enabled: true,
            ..default()
        },
        Transform::from_translation(BASE_LIGHT_OFFSET),
        MainLight,
    ));
}
```

#### 2. Light follow system
**File**: `crates/render/src/camera.rs`

```rust
/// Updates light position to follow camera rotation around the player.
fn update_light_position(
    player_query: Query<&Position, With<Controlled>>,
    camera_query: Query<&CameraOrbitState>,
    mut light_query: Query<&mut Transform, With<MainLight>>,
) {
    let Ok(player_pos) = player_query.single() else {
        return;
    };
    let Ok(orbit) = camera_query.single() else {
        return;
    };
    let Ok(mut light_transform) = light_query.single_mut() else {
        return;
    };

    let rotated_offset = Quat::from_rotation_y(orbit.current_angle) * BASE_LIGHT_OFFSET;
    light_transform.translation = **player_pos + rotated_offset;
}
```

#### 3. Register the system
**File**: `crates/render/src/lib.rs`
**Change**: Add `camera::update_light_position` after `camera::follow_player` in the chain.

### Success Criteria:

#### Automated Verification:
- [x] `cargo check-all`

#### Manual Verification:
- [ ] Light rotates with camera — shadows shift consistently with view angle
- [ ] No shadow popping or flickering during rotation

---

## Testing Strategy

### Manual Testing Steps:
1. `cargo server` then `cargo client` — verify default camera position unchanged
2. Press Q — camera orbits 90° CCW smoothly (~0.15s)
3. Press E — camera orbits 90° CW smoothly
4. Press Q four times — full 360°, returns to original position
5. Hold W during rotation — character movement transitions smoothly from one orientation to another
6. After rotation, verify WASD feels correct relative to camera
7. Verify character facing matches movement direction
8. Start second client — verify multiplayer prediction is clean (no jittering)
9. Check shadows rotate with camera

## Performance Considerations

- One additional `sin_cos` call per frame per predicted player entity — negligible
- Lerp update is a single multiply-add per frame — negligible
- No new allocations, no new queries beyond what's needed

## References

- Research: `doc/research/2026-03-21-camera-rotation-q-e-lerp.md`
- Camera system: `crates/render/src/camera.rs`
- Movement: `crates/protocol/src/character/movement.rs`
- Client gameplay: `crates/client/src/gameplay.rs`
- Lightyear FPS example (set_axis_pair pattern): `git/lightyear/examples/fps/src/client.rs:46-48`
- Existing lerp pattern: `crates/sprite_rig/src/animation.rs:70-88`
