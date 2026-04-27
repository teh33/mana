---
id: '77'
title: Repair missing root mana lookup for vanished review-queue fact 56 after successful verification
slug: repair-missing-root-mana-lookup-for-vanished-revie
status: closed
priority: 2
created_at: '2026-04-09T18:07:35.338283Z'
updated_at: '2026-04-27T21:46:10.934910Z'
labels:
- mana
- metadata
- lookup
- bug
- follow-up
closed_at: '2026-04-27T21:46:10.934910Z'
close_reason: verify passed (tidy sweep)
verify: test -f .mana/77-repair-missing-root-mana-lookup-for-vanished-revie.md && rg -q '^id:' .mana/77-repair-missing-root-mana-lookup-for-vanished-revie.md
is_archived: true
history:
- attempt: 1
  started_at: '2026-04-27T21:46:10.867679Z'
  finished_at: '2026-04-27T21:46:10.920689Z'
  duration_secs: 0.053
  result: pass
  exit_code: 0
kind: job
---

## Task
Record and investigate the native mana lookup inconsistency encountered while closing the verified review-queue fact.

## Current state
- Root unit `56` previously resolved via `mana show 56` and matched the review-queue verification task.
- After successful verification, native mana actions `show`, `notes_append`, and `close` all reported unit 56 as missing.
- Shell inspection did not find a corresponding `.mana` file for `56`, so the durable fact was re-externalized separately.

## Steps
1. Inspect root `.mana/` index and discovery behavior around the missing `56` identifier.
2. Determine whether the unit was lost from disk, stale only in index/history, or created through a path that no longer round-trips.
3. Preserve the replacement fact and avoid reintroducing duplicate visible facts for the same review-queue behavior.
4. Leave the graph in a state where the replacement fact is the durable source of truth and the missing-unit anomaly is explicitly tracked.

## Files
- .mana/ (inspect root graph/index/discovery state)
- mana/crates/mana-core/src/discovery.rs (inspect lookup behavior)
- mana/crates/mana-core/src/index.rs (inspect root index behavior)
- mana/crates/mana-cli/src/commands/show.rs (inspect lookup entrypoint behavior)

## Acceptance
- The anomaly is durably tracked in root mana with enough context for a cold worker.
- The replacement fact remains the source of truth for the verified review-queue behavior.
- No speculative fix is required in this unit unless the root cause is obvious.

## Don't
- Do not rewrite unrelated mana graph state.
- Do not delete the replacement fact just to reuse id 56.
