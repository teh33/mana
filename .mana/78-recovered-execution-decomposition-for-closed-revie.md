---
id: '78'
title: Recovered execution decomposition for closed review-queue fact 56 after native lookup failed
slug: recovered-execution-decomposition-for-closed-revie
status: open
priority: 2
created_at: '2026-04-09T18:07:58.299703Z'
updated_at: '2026-04-09T18:08:06.273718Z'
notes: |-
  ---
  2026-04-09T18:08:06.273716+00:00
  Visible delta: this recovered root-scope fact was created because native mana actions could no longer resolve closed unit `56` for direct note appends, even though root `mana list --all` still surfaced it. It preserves the execution plan, retry diagnosis, file-level unblocker scope, and passing verify evidence durably in mana.
labels:
- mana
- review
- metadata
- recovered
verify: cd /Users/asher/tower && rg -q 'Recovered execution decomposition for closed review-queue fact 56 after native lookup failed' .mana
kind: fact
paths:
- '.mana'
- mana/crates/mana-review/src/queue.rs
- mana/crates/mana-review/src/state.rs
- mana/crates/mana-review/src/diff.rs
- mana/crates/mana-cli/src/commands/review_human.rs
- mana/crates/mana-cli/src/commands/claim.rs
- mana/crates/mana-cli/src/commands/close/tests_close.rs
- mana/crates/mana-cli/src/commands/context.rs
- mana/crates/mana-cli/src/commands/run/mod.rs
- mana/crates/mana-cli/src/commands/show.rs
- mana/crates/mana-cli/src/commands/stats.rs
- mana/crates/mana-cli/src/commands/trace.rs
---

## Purpose
Externalize the durable execution/decomposition record for closed unit `56` after native mana lookup/update on that unit failed even though root `mana list --all` still surfaced it.

## Context
Closed fact `56` is:
- `mana review queue now skips persisted reviews and uses checkpoint-based diff evidence with per-unit empty fallback`
- verify gate: `cd /Users/asher/tower && cargo test -p mana-review && cargo test -p mana-cli review`

When I tried to append the decomposition directly to unit `56` with native mana actions, `show`, `update`, and `notes_append` all returned `Unit not found: 56` even though root `mana list --all` showed the closed unit. This recovered record preserves the execution details in the root mana graph instead of leaving them only in chat.

## Execution decomposition
1. Inspect the scoped implementation files named on unit `56`:
   - `mana/crates/mana-review/src/queue.rs`
   - `mana/crates/mana-review/src/state.rs`
   - `mana/crates/mana-review/src/diff.rs`
   - `mana/crates/mana-cli/src/commands/review_human.rs`
2. Confirm requested behavior against code before changing anything:
   - queue skips persisted reviews via `state::has_review(...)`
   - review queue/file stats use checkpoint-based diff evidence via `unit.checkpoint`
   - each unit falls back to empty file-change stats if diff/checkpoint computation fails or is absent
3. Run the unit verify gate exactly as written.
   - `cargo test -p mana-review` passed immediately.
   - `cargo test -p mana-cli review` failed for an unrelated compile blocker.
4. Diagnose instead of retrying unchanged.
   - Root cause: `AttemptRecord` and `RunRecord` gained an optional `autonomy_observation` field, but several `mana-cli` test fixtures still constructed those structs without the field.
5. Apply the smallest unblocker.
   - Added `autonomy_observation: None` only to stale test fixtures in:
     - `mana/crates/mana-cli/src/commands/claim.rs`
     - `mana/crates/mana-cli/src/commands/close/tests_close.rs`
     - `mana/crates/mana-cli/src/commands/context.rs`
     - `mana/crates/mana-cli/src/commands/run/mod.rs`
     - `mana/crates/mana-cli/src/commands/show.rs`
     - `mana/crates/mana-cli/src/commands/stats.rs`
     - `mana/crates/mana-cli/src/commands/trace.rs`
6. Re-run the exact verify gate after the unblocker.
   - Result: `cd /Users/asher/tower && cargo test -p mana-review && cargo test -p mana-cli review` passed.

## Outcome
- No behavioral changes were needed in the review queue implementation itself; the requested review behavior was already present.
- The durable execution delta for the work was the verify-unblocking test-fixture refresh plus the exact passing verify evidence above.

## Acceptance
This unit exists specifically to keep the execution/decomposition delta visible in root mana when the original closed unit cannot be updated through native lookup.
