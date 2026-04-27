---
id: '79'
title: Root CI verify gate excludes some audit strings, but the workflow does include a RustSec dependency-audit job
slug: root-ci-verify-gate-excludes-some-audit-strings-bu
status: open
priority: 3
created_at: '2026-04-09T23:15:53.882645Z'
updated_at: '2026-04-16T09:41:44.129810Z'
notes: |-
  ---
  2026-04-16T09:41:44.129804+00:00
  2026-04-16 cleanup note: treat this fact as the canonical root-CI audit coverage record. Overlapping fact `80` preserved essentially the same narrow RustSec conclusion as a recovered record; this unit keeps the broader current-state nuance that root CI already has a RustSec dependency-audit job while unit `62` remained overstated/stale.
labels:
- fact
verify: 'cd /Users/asher/tower && test -f .github/workflows/ci.yml && rg -q ''dependency-audit'' .github/workflows/ci.yml && rg -q ''rustsec/audit-check@v2.0.0'' .github/workflows/ci.yml && rg -q ''name: MSRV \(1.85\)'' mana/.github/workflows/ci.yml'
kind: epic
unit_type: fact
last_verified: '2026-04-09T23:16:10.195220Z'
stale_after: '2026-05-09T23:16:10.195220Z'
paths:
- '.github/workflows/ci.yml'
- mana/.github/workflows/ci.yml
- '.mana/62-root-and-mana-local-ci-workflows-exist-current-cov.md'
---

## Task
Capture the current repo reality for root CI after verifying unit 62's gate.

## Verified reality
- `.github/workflows/ci.yml` exists and still contains the expected build/test/lint/format jobs.
- The root workflow also contains a dedicated `dependency-audit` job using `rustsec/audit-check@v2.0.0`.
- Unit 62's verify gate passes because it only excludes the strings `cargo-audit|osv|gitleaks|trivy|dependency-review`; it does **not** establish that there is no audit/security coverage at all.
- `mana/.github/workflows/ci.yml` still does not appear to mirror the root RustSec audit slice.

## Why this matters
The task description for unit 62 is now stale at the root level. Future workers should not repeat the broader claim without checking the actual workflow contents.

## Files
- `.github/workflows/ci.yml`
- `mana/.github/workflows/ci.yml`
- `.mana/62-*`

## Acceptance
A future worker inspecting this fact should understand that the root workflow already has narrow Rust dependency audit coverage, while the unit 62 verify gate is narrower than the unit title implies.
