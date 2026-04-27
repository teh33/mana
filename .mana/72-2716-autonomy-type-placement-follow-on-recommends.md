---
id: '72'
title: '27.16 autonomy type-placement follow-on recommends mana-core ownership with Unit.autonomy and DispatchUnit projection'
slug: '2716-autonomy-type-placement-follow-on-recommends'
status: open
priority: 3
created_at: '2026-04-09T13:51:57.861792Z'
updated_at: '2026-04-09T13:54:40.205967Z'
notes: |-
  ---
  2026-04-09T13:54:40.205963+00:00
  Follow-on decomposition from 27.16: the concrete planning recommendation is to keep the canonical autonomy-gating vocabulary in `mana-core`, stabilize the existing `Unit.autonomy_disposition` / `RunRecord.autonomy_observation` schema in place, and project the typed disposition onto `DispatchUnit` rather than creating a parallel `tower-contracts` autonomy surface or letting `mana-pool` re-derive policy from raw unit internals.
labels:
- fact
verify: test -f .mana/72-2716-autonomy-type-placement-follow-on-recommends.md && rg -q '^id:' .mana/72-2716-autonomy-type-placement-follow-on-recommends.md
kind: epic
unit_type: fact
last_verified: '2026-04-09T23:16:10.195220Z'
stale_after: '2026-05-09T23:16:10.195220Z'
paths:
- docs/rebuild/autonomy-gating-type-placement.md
- mana/crates/mana-core/src/unit/types.rs
- mana/crates/mana-pool/src/types.rs
- mana/crates/mana-cli/src/commands/run/mod.rs
- imp/crates/imp-core/src/agent.rs
---

Derived from docs/rebuild/autonomy-gating-type-placement.md. The follow-on plan recommends keeping the canonical autonomy-gating vocabulary in mana-core, storing the latest scheduler-facing AutonomyDisposition on Unit, treating RunRecord.autonomy_observation as the primary attempt-scoped evidence location, and projecting the typed disposition directly onto DispatchUnit so mana-pool consumes it without re-deriving policy. Raw confidence remains imp-local and is explicitly excluded from the durable scheduler contract.
