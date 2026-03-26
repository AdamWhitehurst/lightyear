# Voxel Engine Debugging Lessons

## ChunkWorkBudget Requires Reset Each Frame

`ChunkWorkBudget` stores an `Instant` and a duration. `has_time()` checks elapsed time since that instant. If `reset()` is never called, the budget permanently expires after ~4ms from creation.

`update_chunks` (gated on `ChunkGenerationEnabled`) is the primary reset site. Any system gated behind `ChunkGenerationEnabled` that touches the budget will not run on clients. The remesh pipeline (`spawn_remesh_tasks`, `poll_remesh_tasks`) runs unconditionally but checks `budget.has_time()` — so a missing reset silently starves it.

**Pattern:** When gating a system that owns shared per-frame state (budgets, counters), ensure the state is still maintained for ungated consumers. Use `run_if(not(condition))` for a fallback reset.
