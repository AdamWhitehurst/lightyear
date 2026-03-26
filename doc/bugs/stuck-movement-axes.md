---
date: 2026-03-25T17:34:08-07:00
researcher: Claude
git_commit: 4ad1ae0
branch: master
repository: bevy-lightyear-template
topic: "Stuck movement axes after key release"
tags: [bug, lightyear, input, rollback, leafwing]
status: resolved
last_updated: 2026-03-25
last_updated_by: Claude
---

# Bug: Stuck Movement Axes After Key Release

**Date**: 2026-03-25T17:34:08-07:00
**Researcher**: Claude
**Git Commit**: 4ad1ae0
**Branch**: master
**Repository**: bevy-lightyear-template

## User's Prompt

Client's inputs seem to get stuck, where if I press a movement button, sometimes the player keeps moving after I release the move button until I press it again. This seems to be a lightyear networking bug.

## Summary

Movement axes (and potentially other continuous inputs) can get permanently stuck at their last value after the physical key is released. Both client and server `move_input_magnitude` stay at 1.0 until the player presses the same key again. The bug is intermittent.

The AdamWhitehurst lightyear fork (commit 8769bae6) fixed a server-side variant (`get` -> `get_predict` in `update_action_state`), but the client-side `get_action_state` still uses `get`, leaving an analogous gap.

## Investigation

### Input Pipeline Overview (Client)

1. **PreUpdate**: Leafwing `tick_action_state` + `update_action_state` (reads physical input via `CentralInputStore`)
2. **PreUpdate**: Lightyear rollback check -> `run_rollback` (direct `world.run_schedule(FixedMain)` N times) -> `end_rollback` (removes `Rollback`)
3. **RunFixedMainLoop / BeforeFixedMainLoop**: Leafwing `swap_to_fixed_update` + `update_action_state` (re-reads physical input)
4. **RunFixedMainLoop / FixedMain**: `buffer_action_state` -> `get_action_state` -> game systems
5. **PostUpdate**: `prepare_input_message` -> `send_input_messages` -> `clean_buffers`

### Rollback Lifecycle

- `Rollback` component added in `PreUpdate` (`RollbackSystems::Check`), `rollback.rs:305`
- `run_rollback` (`rollback.rs:772-864`) rewinds `LocalTimeline` then calls `world.run_schedule(FixedMain)` N times
- `end_rollback` (`rollback.rs:866-873`) removes `Rollback` component
- All happens in `PreUpdate`, BEFORE `RunFixedMainLoop`
- Normal forward tick runs WITHOUT `Rollback` in the subsequent `RunFixedMainLoop`

### Client `get_action_state` (`client.rs:301-389`)

During rollback (`is_rollback = true`), for local player:
- `buffer_action_state` is **SKIPPED** (`Without<Rollback>` filter, `client.rs:266`)
- `get_action_state` calls `input_buffer.get(tick)` (line 354)
- If `Some(snapshot)`: overwrites ActionState via `from_snapshot` (full clone, `input_message.rs:131-133`)
- If `None` AND entity is remote: applies decay (line 367-386)
- **If `None` AND entity is local: NOTHING HAPPENS. ActionState retains its previous value silently.**

### `get` vs `get_predict` (`input_buffer.rs:280-322`)

- `get(tick)`: returns `None` if tick is outside buffer range
- `get_predict(tick)`: if tick is BEYOND buffer end, returns `get_last()` (last known value)
- The fork fixed the server to use `get_predict` (commit 8769bae6). The client still uses `get`.

### `clean_buffers` (`client.rs:429-449`)

- Runs in `PostUpdate`, every frame
- Pops entries older than `current_tick - HISTORY_DEPTH` (HISTORY_DEPTH = 20)
- Entries below `start_tick` after pop are permanently lost

### LeafwingSnapshot and Dual-State

- `LeafwingSnapshot` wraps a full `ActionState<A>` (line 24-25 of leafwing `input_message.rs`)
- `from_snapshot` does `*state = snapshot.0.clone()` — replaces the entire ActionState including `update_state`, `fixed_update_state`, and `state` fields
- `to_snapshot` clones the entire ActionState
- Leafwing 0.20 maintains dual state: `state` (active), `update_state` (for Update schedule), `fixed_update_state` (for FixedUpdate)
- `swap_to_fixed_update` + `update_action_state` in `BeforeFixedMainLoop` DO re-read physical input after rollback ends

### What the Fork Fix Addressed (Server-Side)

`server.rs:305`: Changed `input_buffer.get(tick)` to `input_buffer.get_predict(tick)`.

When the server doesn't have input for a tick (packet loss, timing), `get` returned `None` -> ActionState unchanged (could retain stale "pressed" from leafwing's `tick_action_state`). `get_predict` returns the last known input instead.

### Disproved: Rollback Feedback Loop

Previously investigated and disproved (see `lightyear-just-pressed-lost.md`):
- `buffer_action_state` uses `Without<Rollback>`, so it doesn't overwrite the buffer during rollback
- Leafwing re-reads physical input in `BeforeFixedMainLoop`, which runs AFTER rollback (rollback is in PreUpdate, BeforeFixedMainLoop is in RunFixedMainLoop)
- Forward ticks correctly reflect physical input

However, the disproval doesn't cover the case where `get_action_state` returns None for some rollback ticks.

## Code References

- `git/lightyear/lightyear_inputs/src/client.rs:261-298` — `buffer_action_state` (skipped during rollback)
- `git/lightyear/lightyear_inputs/src/client.rs:301-389` — `get_action_state` (uses `get` not `get_predict`)
- `git/lightyear/lightyear_inputs/src/client.rs:429-449` — `clean_buffers` (pops old entries)
- `git/lightyear/lightyear_inputs/src/input_buffer.rs:280-322` — `get` vs `get_predict`
- `git/lightyear/lightyear_inputs/src/server.rs:305` — Server uses `get_predict` (fork fix)
- `git/lightyear/lightyear_inputs_leafwing/src/input_message.rs:131-133` — `from_snapshot` (full ActionState clone)
- `git/lightyear/lightyear_prediction/src/rollback.rs:772-864` — `run_rollback`
- `leafwing-input-manager-0.20.0/src/plugin.rs:173-182` — `update_action_state` in BeforeFixedMainLoop
- `leafwing-input-manager-0.20.0/src/user_input/virtual_axial.rs:437-458` — `VirtualDPad::get_axis_pair`
- `crates/client/src/diagnostics.rs:18,31-35` — `plot_client_input_state` runs in FixedUpdate, plots ALL ActionState entities

## Hypotheses

### Hypothesis 1: `get_action_state` silent None during rollback

**Hypothesis:** During client-side rollback, `input_buffer.get(tick)` returns `None` for the local player at one or more rollback ticks (buffer trimmed by `clean_buffers`, or tick out of range). The local player falls through both branches in `get_action_state` — ActionState silently retains the previous tick's value (or the confirmed state restored at rollback start). If the retained value is "pressed," the axis stays stuck.

**Prediction:** If this hypothesis is correct, adding a `warn!` trace at the fallthrough point in `get_action_state` (after line 387, when `is_local && is_rollback && buffer returned None`) will fire when the stuckness manifests.

**Test:** Add diagnostic `warn!` in `get_action_state` for the local-player-during-rollback-buffer-miss case. Run `cargo client`, trigger the bug, check logs.

**Decision:** Rejected — buffer has data for all rollback ticks. The stale value is IN the buffer, not missing.

### Hypothesis 2: Leafwing update_action_state fails to zero axes after rollback

**Hypothesis:** After rollback restores a stale ActionState (via `from_snapshot`), leafwing's `update_action_state` in BeforeFixedMainLoop fails to correct the axis because `CentralInputStore` has no entry for released keys.

**Prediction:** The `before_fixed` diagnostic (running after `update_action_state`) will show non-zero axis with all raw WASD keys false.

**Test:** Added `debug_post_leafwing_update` system in BeforeFixedMainLoop that logs axis pair AND raw `ButtonInput<KeyCode>` state.

**Result:** CONFIRMED. Logs show `pair=(0.00,1.00) raw_W=false raw_S=false raw_A=false raw_D=false` — leafwing says axis is (0,1) but Bevy says no keys are pressed.

**Decision:** Validated — this is the root cause.

## Root Cause

**Leafwing's `CentralInputStore` population has a gap for released keys.**

In `keyboard.rs:40-51`, `KeyCode::compute` only adds:
1. Currently pressed keys (`get_pressed` → `true`)
2. Just-released keys (`get_just_released` → `false`)

After `clear_central_input_store` clears the store each frame, a key that was released on a PREVIOUS frame has NO entry. `CentralInputStore::pressed(&key)` returns `None`.

In `VirtualDPad::get_axis_pair` (`virtual_axial.rs:437-458`):
- `self.up.get_value(input_store, gamepad)` returns `None` (key not in store)
- If ALL four directions return `None`, `get_axis_pair` returns `None`
- `process_actions` skips the action (not inserted into `updated_actions`)
- `ActionState::update` never touches the axis → **retains its previous value**

**The trigger sequence:**
1. Player presses W → axis = (0, 1). CentralInputStore has W=true.
2. Player releases W → `just_released` fires → store has W=false → axis correctly set to (0, 0).
3. Next frame: W is neither pressed nor just_released → store has NO entry for W.
4. In that same frame or any subsequent frame, rollback's `from_snapshot` replaces ActionState with a snapshot where axis = (0, 1).
5. BeforeFixedMainLoop: `update_action_state` reads CentralInputStore. No WASD entries → `get_axis_pair` returns `None` → Move action omitted → axis stays (0, 1). **STUCK.**
6. All subsequent frames: same — no WASD entries, axis never corrected.
7. Player presses W again → store has W=true → axis updates → stuckness resolves.

## Fixes

### Option A: Fix in leafwing (upstream)

`ActionState::update` should release/zero any action NOT present in `updated_actions`. Currently it only processes actions that ARE present. Adding an "else release" pass would fix the gap.

### Option B: Fix in CentralInputStore population

`KeyCode::compute` should store `false` for all keys that are in the InputMap but not currently pressed, not just `just_released` keys. This ensures `get_value` returns `Some(0.0)` instead of `None`.

### Option C: Fix in lightyear (workaround)

After `from_snapshot` restores ActionState during rollback, call `set_update_state_from_state()` and `set_fixed_update_state_from_state()` to sync the dual-state fields. However, this doesn't fix the core issue — the next `update_action_state` still won't zero the axis.

### Option D: Game-level workaround

In BeforeFixedMainLoop (after `update_action_state`), add a system that explicitly reads `ButtonInput<KeyCode>` and zeros the Move axis when no WASD keys are pressed. This bypasses the CentralInputStore gap entirely.

### Recommended: Option D (immediate) + Option A (upstream PR)

Option D is the fastest path to fix the game. Option A is the correct long-term fix for leafwing.

### Resolution

The fix already existed upstream as leafwing-input-manager PR #741 (commit `7f407ac`), merged after the v0.20 release. Added leafwing-input-manager as a local git submodule at HEAD (which includes the fix) and patched `Cargo.toml` with `[patch.crates-io]` so both the game and lightyear resolve to the local copy. Bug no longer reproduces.
