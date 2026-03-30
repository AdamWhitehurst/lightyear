# Research Questions: Singleplayer Mode

1. **How is the Lightyear client-server connection established?** Trace the full flow from `ClientNetworkConfig` creation through transport selection, `NetcodeClient` setup, and the `Connect` trigger in both native (`crates/client/`) and web (`crates/web/`) clients.

2. **How does the server initialize and run its game loop?** Document what `ServerPlugins`, `ServerNetworkPlugin`, and the server `main.rs` set up — plugins, resources, schedules, and observers — from startup through accepting connections.

3. **What is the Crossbeam transport and how is it used in tests?** Document how `ClientTransport::Crossbeam` and `ServerTransport::Crossbeam` are configured, how the in-memory IO channels are created and shared, and what test infrastructure exists in `crates/client/tests/` and `crates/server/tests/`.

4. **What is the `ClientState` state machine and how do transitions work?** Document all states, transition triggers, observers, and UI systems in `crates/ui/` that react to connection/disconnection events.

5. **How are server-side gameplay systems structured?** Document the systems in `crates/server/src/gameplay.rs`, `map.rs`, `world_object.rs`, and `persistence.rs` — what plugins they belong to, what run conditions they use, and what resources/events they depend on.

6. **What shared gameplay logic lives in the protocol crate vs. what is server-only or client-only?** Map which systems, plugins, and types from `crates/protocol/` are used by server, client, or both. Document how `SharedGameplayPlugin` is structured.

7. **How does asset loading and `AppState` gating work across server and client?** Document the `TrackedAssets` mechanism, how `AppState::Loading → Ready` transitions, and whether server and client share the same loading path.

8. **What server resources and plugins require headless/`MinimalPlugins` vs. full `DefaultPlugins`?** Document any server code that assumes headless mode or any client code that assumes a windowed renderer.

9. **How does Lightyear support running both client and server in the same Bevy app?** Document any Lightyear APIs, examples, or configuration for combined/host mode — `ClientPlugins` + `ServerPlugins` coexisting, shared world, local client connections. Specifically: does Lightyear provide a local/loopback transport? How does replication behave when both ends are in-process?

10. **How do the native client and web client differ in their setup?** Compare `crates/client/src/main.rs` and `crates/web/src/main.rs` — plugin differences, transport differences, platform-specific code, and shared code paths.

11. **How do Lightyear's replication, prediction, and rollback systems work at runtime?** Document the data flow for replicated components — how server authority, client prediction, rollback correction, and interpolation interact. What happens to these systems when client and server share the same world (zero latency, no packet loss)?
