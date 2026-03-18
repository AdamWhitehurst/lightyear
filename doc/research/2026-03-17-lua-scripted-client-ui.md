---
date: 2026-03-17T20:52:02-07:00
researcher: Claude
git_commit: 167fcfb055e012fff5f4201edc97fe266b489c5f
branch: master
repository: bevy-lightyear-template
topic: "Lua-scripted client UI similar to World of Warcraft's addon system"
tags: [research, ui, lua, scripting, addons, bevy-ui, wow]
status: complete
last_updated: 2026-03-17
last_updated_by: Claude
last_updated_note: "Added follow-up research: WASM compatibility, Lua version analysis, bevy_hui details, performance budgets"
---

# Research: Lua-Scripted Client UI Similar to WoW's Addon System

**Date**: 2026-03-17T20:52:02-07:00
**Researcher**: Claude
**Git Commit**: 167fcfb055e012fff5f4201edc97fe266b489c5f
**Branch**: master
**Repository**: bevy-lightyear-template

## Research Question

How to implement a Lua-scripted client UI similar to World of Warcraft's UI addon system in this Bevy 0.18 project.

## Summary

No Lua or scripting integration exists in the project today. The current UI (`crates/ui/`) uses Bevy's built-in `bevy_ui` with hardcoded Rust screens (main menu, connecting, in-game HUD). To build a WoW-style scripted UI, the project would combine **bevy_ui** (native widget primitives) with **mlua** or **bevy_mod_scripting** (Lua runtime) and a custom frame API that mirrors WoW's `CreateFrame()`/event-driven architecture. No turnkey solution exists — this would be a substantial custom system built from existing building blocks.

## Detailed Findings

### Current UI State

The project has a dedicated `crates/ui/` crate using `bevy_ui`:

- [lib.rs](crates/ui/src/lib.rs) — `UiPlugin` with screen setup functions (`setup_main_menu`, `setup_connecting_screen`, `setup_ingame_hud`, `setup_transition_loading_screen`) and button interaction systems
- [components.rs](crates/ui/src/components.rs) — Marker components: `ConnectButton`, `QuitButton`, `MainMenuButton`, `CancelButton`, `MapSwitchButton`
- [state.rs](crates/ui/src/state.rs) — `ClientState` enum (`MainMenu`, `Connecting`, `InGame`) and `MapTransitionState`
- [health_bar.rs](crates/render/src/health_bar.rs) — 3D billboard health bars (mesh-based, not bevy_ui)

No Lua, scripting, plugin/addon, or modding system exists anywhere in the codebase.

---

### WoW UI Architecture (Reference Model)

WoW's addon system is the gold standard for game UI moddability. Key architectural elements:

#### Dual-Layer Definition: XML + Lua
- **XML** defines widget tree structure and layout (frames, textures, font strings)
- **Lua** defines all runtime behavior (event handlers, data processing, frame manipulation)
- Since Patch 1.10.0, pure-Lua addons are viable — many modern addons skip XML entirely, creating frames via `CreateFrame(frameType, name, parent, template)`

#### Frame Hierarchy
```
ScriptObject
  ScriptRegion
    Region
      Frame → Button, CheckButton, EditBox, ScrollFrame, Slider, StatusBar, GameTooltip, Model, Cooldown
      TextureBase → Texture, MaskTexture, Line
      FontInstance → FontString, Font
    AnimatableObject → AnimationGroup → Animation (Alpha, Rotation, Scale, Translation)
```

Key frame types:
- **Frame** — base container. Receives events, has children, has strata/level for draw ordering
- **Button** — clickable frame with normal/highlight/pushed textures
- **FontString** — text rendering (a Region, not a Frame)
- **Texture** — image rendering with atlas/texcoord support
- **StatusBar** — progress bar with min/max values

#### Positioning: Anchor-Based
```lua
frame:SetPoint("TOPLEFT", parentFrame, "BOTTOMLEFT", xOffset, yOffset)
```
Frames anchor relative to other frames/regions. Draw layers within a frame (bottom to top): BACKGROUND, BORDER, ARTWORK, OVERLAY, HIGHLIGHT.

#### Event System
- 202+ event categories (combat, units, player state, UI, inventory, social, quests)
- Registration: `frame:RegisterEvent("EVENT_NAME")`
- Handler: `frame:SetScript("OnEvent", function(self, event, ...) end)`
- Key events: `ADDON_LOADED`, `PLAYER_LOGIN`, `COMBAT_LOG_EVENT_UNFILTERED`, `UNIT_HEALTH`, `BAG_UPDATE`

#### Data Flow
- **Pull (polling)**: `UnitHealth("player")`, `C_Spell.GetSpellInfo(spellID)`, etc.
- **Push (events)**: Game fires events on state changes; addons react
- **Combat log**: `COMBAT_LOG_EVENT_UNFILTERED` → `CombatLogGetCurrentEventInfo()` — richest data source

#### Addon Loading
1. WoW scans `Interface/AddOns/` for folders with matching `.toc` files
2. TOC declares metadata, dependencies, load order, SavedVariables
3. Files load top-to-bottom per TOC listing
4. Dependencies override alphabetical order
5. Lifecycle events: `ADDON_LOADED` → `PLAYER_LOGIN` → `PLAYER_ENTERING_WORLD`
6. SavedVariables persist to disk on logout (account-wide or per-character)

#### Security/Sandboxing (Taint System)
- Execution starts **secure**, becomes **tainted** when touching addon code/data
- Taint propagates — anything written by tainted execution is also tainted
- **Protected functions** (targeting, casting spells) only callable from secure execution
- No `io`, `os`, `debug` libraries — no filesystem/network/process access
- Safe hooking: `hooksecurefunc()` post-hooks without tainting originals

#### Key Design Patterns
- Event-driven invisible frames for data processing
- `hooksecurefunc()` for extending Blizzard functions
- Ace3 library ecosystem (AceAddon, AceEvent, AceDB, AceGUI)
- Secure template inheritance for combat-functional UI
- Load-on-demand modules for memory optimization

**Sources**: [Warcraft Wiki - XML UI](https://warcraft.wiki.gg/wiki/XML_user_interface), [Widget API](https://warcraft.wiki.gg/wiki/Widget_API), [Events](https://warcraft.wiki.gg/wiki/Events), [Secure Execution](https://warcraft.wiki.gg/wiki/Secure_Execution_and_Tainting), [TOC Format](https://warcraft.wiki.gg/wiki/TOC_format), [AddOn Loading](https://warcraft.wiki.gg/wiki/AddOn_loading_process)

---

### Lua-in-Rust Crate Ecosystem

#### mlua (recommended foundation)
| | |
|---|---|
| Version | 0.11.6 (Jan 2026) |
| Downloads | ~236k/month |
| GitHub | [mlua-rs/mlua](https://github.com/mlua-rs/mlua) |

- Supports Lua 5.1–5.5, LuaJIT, Luau
- Safe API, async/await, serde integration, UserData trait, sandboxing
- `Send` feature flag for Send+Sync
- WASM support via `wasm32-unknown-emscripten`
- No Bevy-specific abstractions — you build the bridge

#### bevy_mod_scripting (most batteries-included)
| | |
|---|---|
| Version | 0.19.0 (Jan 2026, Bevy 0.18) |
| GitHub | [makspll/bevy_mod_scripting](https://github.com/makspll/bevy_mod_scripting) |
| Docs | [makspll.github.io/bevy_mod_scripting](https://makspll.github.io/bevy_mod_scripting/) |

- Languages: Lua (5.1–5.4, LuaJIT, Luau), Rhai
- Reflection-based auto-bindings for any `Reflect`-implementing type
- `ReflectReference` handles for ECS access through `WorldAccessGuard`
- Hot reloading, event-driven callbacks, parallel script execution
- **No WASM support**, self-described WIP, API instability expected

#### bevy_scriptum (simpler alternative)
| | |
|---|---|
| Version | 0.11.0 (Mar 2026, Bevy 0.18) |
| GitHub | [jarkonik/bevy_scriptum](https://github.com/jarkonik/bevy_scriptum) |

- LuaJIT, Rhai, Ruby (Linux/macOS only)
- Script callbacks ARE Bevy systems — natural ECS integration
- Promise-based async API, hot reloading
- Less feature-rich than bevy_mod_scripting

#### Others
- **piccolo** (pure-Rust Lua VM): Experimental, missing stdlib, not production-ready
- **rlua**: Archived Sep 2025, now thin wrapper around mlua — use mlua directly

---

### Bevy UI Ecosystem

#### bevy_ui (built-in, Bevy 0.18)
- Retained-mode, flexbox/grid layout, ECS-native (every UI element is an entity)
- New in 0.18: `Popover`, `MenuPopup`, `RadioButton`/`RadioGroup`, `AutoDirectionalNavigation`, `Val`/`Color` animation, `bevy_feathers` styled widgets
- **Limitations**: No text input widget yet, verbose Rust boilerplate, no declarative templating (BSN not landed)

#### bevy_hui (XML templates for bevy_ui)
- Version 0.6, Bevy 0.18 compatible
- Define UI in XML files mapping to `bevy_ui` components
- Hot reload, event bindings (`on_press="{action}"`), property bindings
- Closest existing thing to "declarative UI definition" for Bevy
- [GitHub](https://github.com/Lommix/bevy_hui)

#### bevy_egui
- Immediate-mode via egui, good for tools/debug, not suited for game UI
- Not ECS-native, "tool-like" aesthetic

#### bevy_lunex
- Retained layout engine, fully ECS-native, supports 2D and 3D UI
- [GitHub](https://github.com/bytestring-net/bevy_lunex)

---

### Architectural Patterns from Other Games

| Game | UI Architecture | Key Takeaway |
|---|---|---|
| **WoW** | XML layout + Lua behavior, frame hierarchy, event-driven | Gold standard for addon moddability; event system is the critical API |
| **Garry's Mod** | Lua `Derma` wrapping C++ VGUI panels | Thin Lua wrapper over native panel primitives works well |
| **Roblox** | Luau instance tree (`ScreenGui` > `Frame` > `TextButton`), property-based | Property-based instance model with event connections is accessible |

**Common patterns across all three:**
1. Retained object tree (not immediate-mode)
2. Property-based configuration on objects
3. Event-driven behavior (scripts respond to interactions)
4. Thin wrapper over native engine primitives
5. Client-side only (query game state, don't mutate authoritative state)

---

### ECS-Scripting Bridge Challenges

1. **Ownership/borrowing**: Lua expects free references; Rust's borrow checker prevents this. ECS data lives in archetypes — can't hand raw pointers to scripts.
2. **Query construction**: Bevy queries are compile-time typed; scripts need runtime-dynamic queries via reflection.
3. **Concurrency**: Bevy runs systems in parallel; scripts need runtime access tracking (bevy_mod_scripting's `AccessMap` approach).

**Patterns used in practice:**
- **Reflection-based access** (bevy_mod_scripting): `ReflectReference` handles, `WorldAccessGuard` validation
- **System-as-callback** (bevy_scriptum): Each script callback is a Bevy system
- **Entity ID indirection**: Scripts store `Entity` IDs, not direct references; mutations through commands

**Performance**: Lua cross-language calls ~10x slower than WASM. Reflection adds overhead. For UI (not hot-path), generally acceptable.

---

### Potential Architecture for This Project

A WoW-inspired Lua UI system for this Bevy project would have these layers:

```
┌─────────────────────────────────────────┐
│           Lua Addon Scripts             │  ← User/modder code
│  (CreateFrame, RegisterEvent, etc.)     │
├─────────────────────────────────────────┤
│         Lua Frame API                   │  ← Custom API layer
│  (Frame types, event dispatch,          │
│   anchor system, draw layers)           │
├─────────────────────────────────────────┤
│     Lua Runtime (mlua / bevy_mod_*)     │  ← Lua VM + ECS bridge
├─────────────────────────────────────────┤
│         bevy_ui Primitives              │  ← Native widget rendering
│  (Node, Text, Image, Button, etc.)     │
├─────────────────────────────────────────┤
│         Bevy ECS                        │  ← Engine
└─────────────────────────────────────────┘
```

**Key decisions to make:**
1. **mlua directly vs bevy_mod_scripting** — Control vs convenience tradeoff
2. **Layout definition** — Pure Lua (like modern WoW addons) vs XML+Lua (classic WoW) vs custom format
3. **API surface** — What game state to expose (health, abilities, combat log, inventory, etc.)
4. **Sandboxing** — What to restrict (no filesystem, no network, limited ECS mutation)
5. **Addon discovery** — TOC-like manifest files? Directory scanning?
6. **Persistence** — SavedVariables equivalent for addon settings
7. **Hot reloading** — Essential for addon development iteration

## Code References

- `crates/ui/src/lib.rs` — Current UI plugin (would be extended or replaced)
- `crates/ui/src/components.rs` — Current UI marker components
- `crates/ui/src/state.rs` — Client state machine
- `crates/render/src/health_bar.rs` — Billboard health bars (3D UI, separate concern)

## Historical Context (from doc/)

- `doc/research/2025-11-28-ui-crate-and-client-state.md` — Original UI crate design research
- `doc/plans/2025-11-28-ui-crate-and-client-state.md` — UI crate implementation plan
- `doc/research/2026-02-16-health-respawn-billboard-ui.md` — Health/respawn billboard UI research

## Resolved Questions

1. **Scope**: Full WoW-style addon system with third-party modding support.
2. **WASM**: MUST be supported. See WASM Compatibility Analysis below — this is the critical constraint.
3. **Performance budget**: See Performance Budget Analysis below.
4. **bevy_hui**: Yes, use bevy_hui as the layout/template layer. See bevy_hui Deep Dive below.
5. **Lua version**: See Lua Version WASM Analysis below. Lua 5.4 is the only viable option.
6. **Existing UI migration**: Current hardcoded screens will be rewritten in Lua.

---

## Follow-up Research 2026-03-17T21:00:00-07:00

### CRITICAL: WASM Compatibility Blocker

**mlua targets `wasm32-unknown-emscripten`. Bevy targets `wasm32-unknown-unknown`. These are incompatible.**

- mlua (and its vendored Lua C builds via `lua-src-rs`) only supports `wasm32-unknown-emscripten` for WASM
- Bevy does **not** support `wasm32-unknown-emscripten` ([Bevy issue #4150](https://github.com/bevyengine/bevy/issues/4150))
- `wasm32-unknown-unknown` has no libc, and Lua's C implementation requires libc
- `wasm32-wasip1` is also unsupported (wasi-libc doesn't fully support Lua's requirements)
- [bevy_mod_scripting WASM issue #166](https://github.com/makspll/bevy_mod_scripting/issues/166) tracks this exact problem — still open

**Potential paths forward:**

| Option | Viability | Notes |
|---|---|---|
| **Pure-Rust Lua interpreter** | Uncertain | piccolo exists but is missing most stdlib, not production-ready |
| **Feature-gate Lua out of WASM** | Partial | Web client would have no addon support — defeats the purpose |
| **rilua fork of bevy_mod_scripting** | Unverified | `danielsreichenbach/bevy_mod_scripting` branch `feature/rilua-backend` reportedly has WASM-compatible Lua, not merged upstream |
| **Alternative scripting language** | Viable | Rhai is pure Rust, works on `wasm32-unknown-unknown`, supported by bevy_mod_scripting and bevy_scriptum |
| **Compile Lua to WASM separately** | Complex | Use Emscripten to compile Lua VM to a WASM module, load it as a "WASM-in-WASM" sub-module. Technically possible but extremely complex |
| **Wait for piccolo maturity** | Long-term | Pure Rust Lua VM would solve the problem cleanly, but piccolo's last release was Jun 2024 |

**This is the single biggest technical risk for the Lua requirement.** If WASM support is non-negotiable, using Rhai instead of Lua (or a hybrid approach) may be the pragmatic choice until a pure-Rust Lua VM matures.

---

### Lua Version WASM Analysis

| Version | WASM Support | Notes |
|---|---|---|
| **Lua 5.4** | `wasm32-unknown-emscripten` only | Plain C, compiles via Emscripten. **Not compatible with Bevy's WASM target.** |
| **LuaJIT** | **No WASM support at all** | JIT generates native machine code at runtime (impossible in WASM). Interpreter is hand-coded assembler for x86/ARM — no WASM backend. |
| **Luau** | `wasm32-unknown-emscripten` only | C++, compiles via Emscripten. Same target mismatch as Lua 5.4. Has built-in sandboxing and type annotations. |

**Conclusion**: If Lua is the language, **Lua 5.4** is the only reasonable choice (LuaJIT is ruled out entirely). But all C/C++ Lua implementations share the same Bevy WASM target incompatibility. A pure-Rust implementation is needed for `wasm32-unknown-unknown` compatibility.

---

### bevy_hui Deep Dive

**Architecture**: Custom Bevy asset type + runtime ECS spawner.
1. `.html` template files loaded via `AssetServer` as custom assets
2. `nom`-based parser converts XML-like markup to internal AST
3. Spawning system converts AST to Bevy UI entities (`Node`, `Button`, `UiImage`, `Text`)
4. Hot-reload via Bevy's `AssetEvent::Modified` (filesystem watching)

**WASM compatibility**: Not officially tested, but **likely works**. All dependencies are WASM-friendly (`bevy`, `nom`, `thiserror`, `owo-colors`). Hot-reload won't function on WASM (no filesystem watching), but templates load fine as static bundled assets via Trunk's `copy-dir`. No hard blockers identified.

**Template format** (HTML-like, attributes map to Bevy style fields):
```xml
<template>
    <property name="press"></property>
    <property name="primary">#000</property>

    <button
        hover:background="#333"
        pressed:background="#0aa"
        background="{primary}"
        height="80px"
        width="100%"
        on_press="{press}"
        border="2px"
        border_radius="5px"
        justify_content="center"
        align_items="center"
    >
        <slot />
    </button>
</template>
```

**Event binding**: Register named Rust systems, reference them in templates:
```rust
fn setup(mut html_funcs: ResMut<HtmlFunctions>) {
    html_funcs.register("greet", greet);
}

fn greet(In(entity): In<Entity>, tags: Query<&Tags>, mut cmd: Commands) {
    // entity = the element that fired the event
}
```

Supported events: `on_spawn`, `on_press`, `on_enter`, `on_exit`, `on_change`.
Tag data passing: `tag:key="value"` attributes accessible via `Tags` component.

**No Lua integration** — purely Rust callbacks. To use bevy_hui + Lua together, the Lua layer would register bevy_hui event handlers that dispatch into Lua scripts.

**Key limitations**:
- Single root node per template
- Recursive imports cause memory issues (no cycle detection)
- Child template reload can break parent (re-save parent fixes)
- Manual Bevy style changes get overwritten by bevy_hui

**Dependencies**: `bevy 0.18`, `nom 7.1.3`, `thiserror`, `owo-colors`, optional `bevy_picking 0.18`. No conflicts with mlua.

---

### Performance Budget Analysis

**How games typically schedule Lua UI execution:**

| Pattern | Used By | Description |
|---|---|---|
| **Event-only (reactive)** | WoW `OnEvent`, Factorio | Most efficient — scripts fire only on state changes |
| **Per-frame (`OnUpdate`)** | WoW `OnUpdate` | Runs every rendered frame. At 60fps = 60 calls/sec. Community considers this "excessive" for most uses |
| **Fixed tick rate** | Factorio (60Hz), Roblox Heartbeat (60Hz) | Decoupled from render rate, predictable budget |
| **Time-budgeted** | Roblox parallel Luau | Coroutines suspend/resume across frames |

**WoW's approach**: All addon Lua runs on the main thread, between frame renders. No per-addon time budget enforced. Addon authors self-throttle. In a real 3-minute raid fight, total addon overhead measured at ~23ms/sec (~0.38ms/frame at 60fps, ~2% loss). WeakAuras (heaviest common addon) consumes ~24% of addon CPU. Anything >1ms/sec per addon is worth investigating.

**Raw call overhead**:
- `lua_pcall()` (what mlua uses): ~130ns per call
- At 1ms budget: ~7,700 protected calls per frame
- At 2ms budget: ~15,000 protected calls per frame
- Real functions with table access, string ops etc. consume far more than empty calls

**Recommended approach for this project**: Event-driven primary execution (like WoW's `OnEvent`) with optional per-frame `OnUpdate` for animations/transitions. Budget 1-2ms/frame for all Lua work. This comfortably supports hundreds of non-trivial script invocations per frame.

**Runaway prevention via mlua**:
```rust
// Instruction counting hook
lua.set_hook(HookTriggers::every_nth_instruction(10_000), |_lua, _debug| {
    // Check elapsed time, return Err to abort
    Ok(VmState::Continue)
});

// Memory limit
lua.set_memory_limit(16 * 1024 * 1024); // 16 MB per addon
```

Instruction hooks at every ~10K instructions add minimal overhead (vs every-instruction which costs 10-20%). Memory limits are essentially free.

**Sources**: [WoW OnUpdate Performance](https://authors.curseforge.com/forums/world-of-warcraft/general-chat/lua-code-discussion/225689-newbie-tip-for-onupdate-performance), [Factorio Performance](https://stable.wiki.factorio.com/Tutorial:Diagnosing_performance_issues), [mlua docs](https://docs.rs/mlua/latest/mlua/struct.Lua.html), [TIC-80 sethook overhead](https://github.com/nesbox/TIC-80/issues/1629)

---

## Open Questions

1. **WASM Lua blocker**: Is the `wasm32-unknown-emscripten` vs `wasm32-unknown-unknown` target mismatch a dealbreaker? Should we investigate Rhai as an alternative, or pursue a pure-Rust Lua interpreter?
2. **rilua fork**: Is the `danielsreichenbach/bevy_mod_scripting` rilua-backend branch viable? Needs investigation.
3. **Hybrid approach**: Could the addon system support both Rhai (works everywhere) and Lua (native-only), with addons choosing their language?
4. **bevy_hui + Lua glue**: What does the integration layer look like? bevy_hui handles templates/layout, Lua handles behavior — how do they communicate through the ECS?