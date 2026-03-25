---
name: debugger
description: Systematic hypothesis-driven debugger for diagnosing root causes. Use when the user reports a bug, unexpected behavior, or runtime issue and wants structured investigation. Follows scientific method — propose hypothesis, instrument code to test it, validate or reject, iterate until root cause is found, then fix and clean up.
---

# Debugger

Systematic root-cause debugging via hypothesis testing. Never guess-and-check blindly — form a theory, instrument it, prove or disprove, repeat.

## Invocation

Accept a bug description from the user. If insufficient, ask:
- What is the expected behavior?
- What is the actual behavior?
- When did it last work / what changed?

## Process

### Phase 1: Gather Evidence

Before hypothesizing, collect raw facts:

1. Read the relevant source code
2. Check `git log --oneline -10` and `git diff` for recent changes
3. Check build output / runtime logs if available
4. Identify the specific system, component, or code path involved

Present a brief **Situation Report**: what the code does, what changed recently, what the symptoms point to.

### Phase 2: Hypothesize

Propose a **single root cause hypothesis** with:

```
**Hypothesis:** [What you believe is wrong and why]
**Prediction:** [If this hypothesis is correct, then instrumenting X will show Y]
**Test:** [Exact diagnostics to add — trace!, warn!, info_span!, event!]
```

Get user confirmation before instrumenting. Keep hypotheses specific and falsifiable.

### Phase 3: Instrument

Add **minimal, targeted diagnostics** to validate or invalidate the hypothesis:

- `trace!("description: {:?}", variable)` — for flow/data inspection
- `warn!("unexpected state: {:?}", value)` — for conditions that shouldn't happen
- `info_span!("section_name")` — for tracy profiling / timing
- `tracing::event!(Level::INFO, ?value, "checkpoint")` — for structured events

Rules:
- Tag all diagnostic additions with `// DEBUG` comment so they're easy to find and remove
- Keep diagnostics minimal — only what's needed to test the current hypothesis
- Never add diagnostics that change behavior (no `.unwrap()` additions, no control flow changes)

### Phase 4: Test

Run the application and examine output:
- `cargo server` / `cargo client` as appropriate
- Examine logs for the diagnostic output
- Compare actual output against the prediction

### Phase 5: Evaluate

**If hypothesis INVALIDATED:**
- State what the diagnostics revealed
- State why the hypothesis was wrong
- Remove diagnostics that are no longer relevant (keep useful ones)
- Return to Phase 2 with new information

**If hypothesis VALIDATED:**
- State the confirmed root cause clearly
- Proceed to Phase 6

### Phase 6: Fix

Propose a fix. Present:

```
**Root Cause:** [Confirmed explanation]
**Fix:** [What to change and why]
**Risk:** [What else this change could affect]
```

Implement the fix after user approval. Then verify:
1. `cargo check-all` passes
2. Runtime test confirms the bug is resolved
3. No regressions in related behavior

If fix doesn't work, the root cause hypothesis was incomplete — return to Phase 2.

### Phase 7: Clean Up

After fix is verified:

1. Remove all `// DEBUG` diagnostic lines
2. Remove any dead code introduced during investigation
3. Keep diagnostics that are genuinely useful for future debugging (remove the `// DEBUG` tag, make them permanent with proper log levels)
4. Verify `cargo check-all` still passes

### Phase 8: Remember

Save a memory if the bug revealed something non-obvious:
- A footgun in the codebase architecture
- A subtle interaction between systems
- A pattern that will likely cause similar bugs

Only save if genuinely useful for future conversations. Don't save routine fixes.

## Anti-Patterns

- **Shotgun debugging**: Changing multiple things at once hoping one works. Change one thing, test, evaluate.
- **Hypothesis-free investigation**: Adding diagnostics without a clear prediction. Always know what you expect to see.
- **Stale diagnostics**: Leaving debug instrumentation from previous hypotheses. Clean as you go.
- **Scope creep**: Fixing other issues discovered during debugging. Note them, stay focused on the reported bug.
- **Premature fixing**: Proposing a fix before confirming root cause. Instrument first.
