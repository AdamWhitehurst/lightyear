---
date: 2026-03-21T22:41:46-07:00
researcher: Claude
git_commit: d1a4e91fd06ede05f3955f6f2d2fdcc26c33208a
branch: bevy-lightyear-template-2
repository: bevy-lightyear-template-2
topic: "Why chunks around 0,0 on overworld don't load but homebase ones do"
tags: [research, codebase, chunk-loading, client-buffer, map-transition]
status: complete
last_updated: 2026-03-21
last_updated_by: Claude
---

# Research: Why Overworld Chunks Around 0,0 Don't Load But Homebase Ones Do

**Date**: 2026-03-21T22:41:46-07:00
**Researcher**: Claude
**Git Commit**: d1a4e91fd06ede05f3955f6f2d2fdcc26c33208a
**Branch**: bevy-lightyear-template-2
**Repository**: bevy-lightyear-template-2

## Research Question

Why do chunks around 0,0 on the overworld fail to load while homebase chunks load correctly?

## Summary

The root cause is in the staged (uncommitted) changes to `crates/client/src/map.rs`. The `ChunkDataSyncBuffer` was modified to track which map entity its buffered data belongs to, with logic to clear stale data on map transitions. However, this clear logic triggers on **first initialization** (`None != Some(overworld)`), discarding chunks that were legitimately buffered during the window between the server sending chunk data and the client's predicted player entity receiving a `ChunkTicket`.

The server marks these discarded chunks as "sent" in `ClientChunkVisibility.sent_chunks`, so they are never re-sent. The result is permanent gaps in the overworld terrain near the player's spawn point.

Homebase is unaffected because the player already has a `ChunkTicket` (and thus `buffer.map_entity` is already set) when transitioning, so the buffer clear logic correctly handles the old→new map transition.

## Detailed Findings

### The Chunk Delivery Pipeline

1. **Server** generates chunks around players via the ticket/propagator system (`lifecycle.rs:112-159`)
2. **Server** pushes `ChunkDataSync` messages to clients via `push_chunks_to_clients` (`server/src/map.rs:654-749`), tracking sent chunks in `ClientChunkVisibility.sent_chunks`
3. **Client** receives messages in `handle_chunk_data_sync` (`client/src/map.rs:136-234`), buffering them if the predicted player isn't ready yet

### The Buffer Clear Bug

In the staged changes to `client/src/map.rs:165-176`, the buffer clear logic is:

```rust
if buffer.map_entity != Some(chunk_ticket.map_entity) {
    if !buffer.chunks.is_empty() {
        buffer.chunks.clear();  // BUG: discards valid chunks on first init
    }
    buffer.map_entity = Some(chunk_ticket.map_entity);
}
```

**Timeline on initial connection:**

1. Server spawns player with `ChunkTicket::player(overworld)` and `ClientChunkVisibility::default()` (`gameplay.rs:317-318`)
2. Server's `push_chunks_to_clients` begins sending `ChunkDataSync` messages, marking each in `visibility.sent_chunks`
3. Client receives messages but predicted player has no `ChunkTicket` yet → messages buffered in `ChunkDataSyncBuffer.chunks`
4. Client's `attach_chunk_ticket_to_player` adds `ChunkTicket` to predicted player (`client/src/map.rs:116-131`)
5. Next `handle_chunk_data_sync` call: `buffer.map_entity` is `None`, `chunk_ticket.map_entity` is `Some(overworld)` → **condition is true** → **buffer cleared**
6. Server has already marked those chunks as sent → **never re-sent**
7. Chunks nearest to player (generated first, buffered first) are **permanently missing**

### Why Homebase Works

During a map transition to homebase:
- The player already has a `ChunkTicket` (pointing to overworld)
- `buffer.map_entity` is already `Some(overworld_entity)`
- When the ticket changes to homebase, `Some(overworld) != Some(homebase)` correctly clears stale overworld data
- New homebase chunks arrive after the buffer is initialized with the correct map entity
- No data loss occurs

### The Committed Code (Pre-Change)

The committed version (`buffer.0.drain(..).chain(incoming)`) simply drains all buffered chunks without any map validation. All buffered chunks are processed when the player appears — no data loss.

### Server-Side Change (Unstaged)

The `tracked_map` addition to `ClientChunkVisibility` (`server/src/map.rs:668-672`) has the same `None != Some(...)` pattern but is harmless: `sent_chunks` and `sent_columns` are empty by default, so clearing them on first access is a no-op.

### How Many Chunks Are Lost

The number of lost chunks depends on the delay between server sending and client predicted player getting a `ChunkTicket`. Typically 1-5 frames. At 32 chunks generated per frame (`MAX_TASKS_PER_FRAME`), sorted by level (nearest first), the **closest** chunks to the player are the ones lost. This matches the screenshots showing gaps near the player's position while distant chunks load fine.

## Code References

- `crates/client/src/map.rs:165-176` — Buffer clear logic (staged change, the bug)
- `crates/client/src/map.rs:152-161` — Buffering when no predicted player
- `crates/server/src/map.rs:654-749` — `push_chunks_to_clients` (sends chunks, tracks in `sent_chunks`)
- `crates/server/src/map.rs:641-650` — `ClientChunkVisibility` (tracks what's been sent)
- `crates/server/src/gameplay.rs:317-318` — Player spawn with `ChunkTicket` + `ClientChunkVisibility`
- `crates/voxel_map_engine/src/lifecycle.rs:279-311` — `spawn_missing_chunks` (sorts by level, nearest first)

## Architecture Documentation

### Overworld vs Homebase Differences

| Property | Overworld | Homebase |
|---|---|---|
| bounds | `None` (unbounded) | `Some(IVec3(4,4,4))` |
| tree_height | 5 | 3 |
| terrain | HeightMap (amplitude=40, Perlin Fbm) | Flat (no HeightMap) |
| generates_chunks (server) | true | true |
| generates_chunks (client) | false | false |
| Total chunks (radius 10) | ~7056 (441 cols × 16 Y) | ~343 (bounded) |

### Chunk Channel

`ChunkChannel` uses `UnorderedReliable` (`protocol/src/lib.rs:112-116`), so messages are guaranteed to arrive but may arrive out of order. Message loss at the network level is not the issue.

## Open Questions

1. Should the server track whether a client has acknowledged receipt of chunks, rather than assuming "sent = received"?
2. Should `push_chunks_to_clients` re-check periodically whether the client actually has the chunk data, or implement a protocol-level ACK?
