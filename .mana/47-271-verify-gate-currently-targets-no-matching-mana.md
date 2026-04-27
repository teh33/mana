---
id: '47'
title: 'Former 27.1 no-match verify claim became stale after budget tests landed'
slug: 271-verify-gate-currently-targets-no-matching-mana
status: closed
priority: 3
created_at: '2026-04-09T15:10:00Z'
updated_at: '2026-04-09T15:12:00Z'
labels:
- fact
closed_at: '2026-04-09T12:02:41.625966Z'
close_reason: Reconciled stale metadata after archived unit `27.1` landed the budget-named tests. Current repo truth is preserved by fact `61` and the positive gate `cargo test -p mana-pool budget -- 2>&1 | grep 'test result' | grep -v '0 passed'`.
kind: epic
unit_type: fact
verify: cd /Users/asher/tower && cargo test -p mana-pool budget -- 2>&1 | grep 'test result' | grep -v '0 passed'
paths:
- '.mana/archive/2026/04/27.1-budget-and-circuit-breaker-for-dispatch.md'
- '.mana/61-mana-pool-budget-filter-currently-matches-2-dispat.md'
- mana/crates/mana-pool/src/dispatch.rs
notes: |-
  ---
  2026-04-09T12:02:33.026877+00:00
  This former negative fact became stale after the `budget` tests landed. Current runtime evidence shows `cargo test -p mana-pool budget` runs 2 matching tests in `mana/crates/mana-pool/src/dispatch.rs`: `budget_enforces_max_concurrent_limit` and `budget_circuit_breaker_stops_new_spawns_after_failure`.

  Archived unit `27.1` records the repair and closure with the positive narrow verify gate. Fact `61` is the replacement current-state fact. This closed fact remains only as provenance for the metadata drift.
