# Lightyear Debugging Lessons

## Single Entity Architecture (lightyear 0.25+)

As of lightyear 0.25, Predicted/Confirmed/Interpolated are merged into a single entity. There is only ONE entity per replicated character on the client. The `Predicted` component is a marker on that same entity.

This project uses lightyear 0.26.4 (from git submodule). The old two-entity architecture does not apply.

## Stuck Axes: Leafwing CentralInputStore Gap

Leafwing-input-manager v0.20 has a bug where `ActionState::update` only processes actions present in `UpdatedActions`. For DualAxis/Axis actions, if no keys are pressed AND `just_released` has expired, `CentralInputStore` has no entry, `VirtualDPad::get_axis_pair` returns `None`, and the action is omitted from `UpdatedActions`. The axis retains its stale value forever.

This is triggered by lightyear rollback's `from_snapshot` restoring a stale ActionState after the `just_released` frame has passed. Fixed in leafwing PR #741 (commit `7f407ac`, post-v0.20).

**Debugging lesson:** When an input value is "stuck," instrument at multiple pipeline stages (raw ButtonInput → CentralInputStore → ActionState) to isolate which layer is stale. The root cause may be in the input framework, not the networking layer.

## Tracy Instrumentation for Lightyear

- `tracy-client` without the `enable` feature compiles `plot!()` to no-ops — no `#[cfg]` guards needed
- Use `tracy_client::Client::running()` + `.message()` for discrete events (not `plot!()`)
- `plot!()` sets a named value — the last call per frame wins. In FixedUpdate (which runs multiple times per frame during rollback), the last replayed tick's value is what shows in Tracy
