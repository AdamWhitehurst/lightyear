# Lightyear Bug: Input State Loss in Serialization Pipeline

## Summary

Lightyear's input snapshot serialization pipeline does not reliably preserve leafwing-input-manager's `JustPressed` transient state. The server's `ActionState` sees `Pressed` but never `JustPressed`, causing any game logic that depends on `just_pressed()` to fail on the server.

## Impact

Any game action gated by `action_state.just_pressed(action)` works on the client (local prediction) but **never fires on the server**. Continuous inputs (`pressed()`, `axis_pair()`) work correctly. This means:

- Ability activations fail on server → client rollback undoes the predicted ability
- Jump inputs are lost if using `just_pressed`
- Any discrete one-shot action gated by `just_pressed` is unreliable

## Reproduction

1. Register a `PlayerActions` enum with button actions via `InputPlugin`
2. On the server, check `action_state.just_pressed(&SomeAction)` in `FixedUpdate`
3. The check never returns `true` on the server, even though the client correctly sends the press

Verified with Tracy instrumentation: `ability2_pressed` shows `1` on the server (the button IS pressed), but `ability2_just_pressed` is always `0`.

## Root Cause Analysis

The `JustPressed` state is lost through multiple reinforcing mechanisms:

### 1. ActionDiff Has No JustPressed Variant

**`lightyear_inputs_leafwing/src/action_diff.rs:16-48`**

The `ActionDiff` enum only has `Pressed` and `Released` variants. There is no `JustPressed` variant. The wire format cannot distinguish between `JustPressed` and `Pressed`.

### 2. ActionDiff::create Collapses JustPressed into Pressed

**`lightyear_inputs_leafwing/src/action_diff.rs:62-75`**

```rust
if button_data_after.state.pressed() && !button_data_before.state.pressed()
```

`pressed()` returns `true` for both `JustPressed` and `Pressed`, so `ActionDiff::create` cannot tell them apart. Furthermore, the diff from `JustPressed` (tick N) to `Pressed` (tick N+1) produces **no diff** because both return `true` for `pressed()`.

### 3. get_snapshots_from_message Calls tick() Between Diffs

**`lightyear_inputs_leafwing/src/input_message.rs:66-78`**

When reconstructing snapshots from the message, `tick()` is called before applying each tick's diffs. This transitions `JustPressed` → `Pressed`, destroying the transient state before the snapshot is stored.

### 4. Server Tick Alignment Is Imperfect

**`lightyear_inputs/src/server.rs:299-305`**

```rust
let tick = timeline.tick();
// ...
if let Some(snapshot) = input_buffer.get_predict(tick) {
    S::from_snapshot(S::State::into_inner(action_state), snapshot);
}
```

Even if the `start_state` in the wire message preserves `JustPressed` for one tick, `update_action_state` must read the buffer at *exactly* that tick. Any tick misalignment (which is common — our Tracy data shows `srv_tick_past_buffer_end` oscillating between -3 and +1) means the server reads a different tick's snapshot where the state has already transitioned to `Pressed`.

### 5. decay_tick Destroys JustPressed in Predictions

**`lightyear_inputs_leafwing/src/input_message.rs:19-21`**

```rust
fn decay_tick(&mut self, tick_duration: Duration) {
    self.tick(Instant::now(), Instant::now() + tick_duration);
}
```

When the server predicts forward from the last known input (via `get_predict`), `decay_tick` calls `tick()` which transitions `JustPressed` → `Pressed`.

## Net Effect

The `start_state` in the wire message *does* preserve `JustPressed` for the first tick of the sequence, and `from_snapshot` does a raw clone that would preserve it. But the combination of:

1. Only one tick in the buffer having `JustPressed`
2. `tick()` immediately clearing it on the next iteration
3. Server tick alignment being imperfect (±1 tick jitter is normal)

makes it **practically impossible** for the server to observe `JustPressed` reliably.

## Workaround

Replace `just_pressed()` with `pressed()` in game logic, using cooldowns or other guards to prevent re-activation while held:

```rust
// Before (broken on server):
if !action_state.just_pressed(action) { continue; }

// After (works on both client and server):
if !action_state.pressed(action) { continue; }
// Cooldown check below prevents re-activation while held
if cooldowns.is_on_cooldown(slot_idx, tick, phases.cooldown) { continue; }
```

## Suggested Fix (Upstream)

The most robust fix would be to ensure the server's `ActionState` correctly reflects `JustPressed` transitions. Options:

1. **Add `JustPressed`/`JustReleased` to `ActionDiff`** — Encode the transition type in the diff format so the server can reconstruct it exactly.

2. **Track transitions in `update_action_state`** — Instead of raw-cloning the snapshot, compare with the previous ActionState and call `press()`/`release()` to let leafwing manage transitions internally.

3. **Document the limitation** — If fixing is complex, document that `just_pressed()`/`just_released()` are not reliable on the receiving side and recommend using `pressed()`/`released()` with custom guards.

---

## Bug 2: Stale Axis Values From Input Buffer Gaps

### Summary

When the server's input buffer has no entry for the current tick, `update_action_state` preserves the previous `ActionState` unchanged. For axis inputs (movement), this causes the axis value to "stick" at its last known value even after the player releases the key.

### Root Cause

**`lightyear_inputs/src/server.rs:302-305`**

```rust
// We only apply the ActionState from the buffer if we have one.
// If we don't (because the input packet is late or lost), we won't do anything.
// This is equivalent to considering that the player will keep playing the last action they played.
if let Some(snapshot) = input_buffer.get_predict(tick) {
    S::from_snapshot(S::State::into_inner(action_state), snapshot);
}
```

When `get_predict(tick)` returns `None`, the ActionState is unchanged. If the last value was `axis_pair(Move) = (1.0, 0.0)`, it stays at `(1.0, 0.0)` indefinitely until a new input arrives.

This is compounded by `history_depth = 1` on the server (`server.rs:339`), which aggressively pops old buffer entries, leaving a very thin window.

### Observed Behavior

Tracy instrumentation showed `srv_tick_past_buffer_end` oscillating between -3 and +1. When it reaches +1, the server has no input for the current tick. The default `packet_redundancy` of 5 provides ~5 ticks of coverage per message, but this is insufficient to prevent occasional gaps.

### Workaround

Increased `packet_redundancy` from 5 to 10 in `InputConfig` to double the buffer coverage.

### Suggested Fix (Upstream)

1. **Decay axis values toward neutral** when no input is available, rather than persisting the last value. Buttons could persist (reasonable for held buttons), but axes should decay toward 0.
2. **Increase default server `history_depth`** beyond 1 to retain more buffer entries.

## References

- `lightyear_inputs_leafwing/src/action_diff.rs` — ActionDiff encoding
- `lightyear_inputs_leafwing/src/input_message.rs` — Snapshot reconstruction, from_snapshot, decay_tick
- `lightyear_inputs/src/server.rs:286-348` — update_action_state
- `lightyear_inputs/src/server.rs:335-339` — history_depth = 1 for non-host-client
- `lightyear_inputs/src/input_message.rs:178-181` — update_buffer with decay_tick
- `lightyear_inputs/src/config.rs:20` — packet_redundancy default (5)
