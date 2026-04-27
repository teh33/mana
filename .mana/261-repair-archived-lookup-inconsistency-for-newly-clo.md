---
id: '261'
title: Repair archived lookup inconsistency for newly closed root unit 260
slug: repair-archived-lookup-inconsistency-for-newly-clo
status: open
priority: 2
created_at: '2026-04-15T06:16:31.887495Z'
updated_at: '2026-04-16T11:49:50.380751Z'
notes: |-
  ---
  2026-04-16T11:49:50.380746+00:00
  2026-04-16 cleanup verification: this unit still lacks a native verify field, but direct repro remains current: archived files and archive.yaml entries exist for closed root unit 260 while native `mana show 260` / `mana tree 260` fail. Leave open as a live archived-lookup bug tracker.
labels:
- mana
- metadata
- lookup
- archive
- bug
- follow-up
kind: job
---

After closing root panic thread 260 and its children, the archived markdown files and `archive.yaml` entries exist on disk, but native `mana show 260` / `mana tree 260` now return `Unit 260 not found`. This indicates a root mana lookup inconsistency for a newly archived unit rather than missing history.

Goal:
- Repair native lookup for archived root unit 260 so `show`/`tree` can resolve it after close.
- Preserve this as a distinct mana metadata bug from the original YAML panic itself.

Current state:
- Archived files exist:
  - `/Users/asher/tower/.mana/archive/2026/04/260-investigate-new-panic-reported-as-scannerrs279817.md`
  - child archive files for `260.1`, `260.2`, `260.3`
- `/Users/asher/tower/.mana/archive.yaml` contains entries for `260`, `260.1`, `260.2`, and `260.3`.
- Native mana lookup fails for parent `260` after close.
- Similar prior lookup inconsistencies already exist in root mana history (`45.8`, `65`, `68`, `77`).

Steps:
1. Inspect archived lookup resolution logic for closed root units versus child units.
2. Reproduce the failure with unit 260 and compare it to known lookup-inconsistency cases.
3. Fix the narrow resolution/indexing bug so `mana show 260` and `mana tree 260` resolve correctly.
4. Add regression coverage if the affected code has a test seam.

Files:
- /Users/asher/tower/.mana/archive.yaml
- /Users/asher/tower/.mana/archive/2026/04/260-investigate-new-panic-reported-as-scannerrs279817.md
- /Users/asher/mana/crates/mana-core/src/ops/show.rs
- /Users/asher/mana/crates/mana-cli/src/commands/show.rs
- /Users/asher/mana/crates/mana-core/src/index.rs

In scope:
- archived lookup bug for unit 260 / same-class resolution issue

Out of scope:
- the original libyml panic fix itself (already implemented)

Do not:
- reopen 260 just to work around the lookup bug
- treat missing native lookup as missing archived evidence
