---
id: '52'
title: Review gating policy is risk-triggered with mandatory escalation from RiskLevel and CloseEvidence
slug: review-gating-policy-is-risk-triggered-with-mandat
status: open
priority: 3
created_at: '2026-04-09T07:24:10.656673Z'
updated_at: '2026-04-09T17:32:33.163301Z'
notes: |-
  ---
  2026-04-09T17:31:55.395137+00:00
  2026-04-09T00:00Z execution check: inspected `docs/rebuild/review-gating-policy.md` and confirmed it already encodes the requested first review gate. Verified explicit `RiskLevel` defaults (`Low` -> skipped, `Normal` -> optional, `High`/`Critical` -> mandatory), suspicious `CloseEvidence` escalation via `only_mana_changes` and `no_path_overlap`, mandatory-risk `RiskFlagKind` coverage, and durable skipped-review provenance requirements. Ran the unit verify command successfully with no doc changes needed.

  ---
  2026-04-09T17:32:16.563359+00:00
  2026-04-09T00:00Z durable execution decomposition externalized at root scope for this cross-project rebuild fact:
  1. Inspect `docs/rebuild/review-gating-policy.md` directly rather than inferring from prior summaries.
  2. Check the requested gate mapping against the current doc: `RiskLevel::Low -> skipped`, `Normal -> optional`, `High/Critical -> mandatory`.
  3. Confirm mandatory escalation signals are spelled out from `CloseEvidence` (`only_mana_changes`, `no_path_overlap`) and existing high-risk `RiskFlagKind` cases.
  4. Confirm skipped-review path still requires durable review-policy provenance plus candidate / verify / diff-scope / risk evidence.
  5. Run the unit verify command as the completion gate.
  6. If verify passes and the doc already matches, make no file edits; record that this unit resolved by inspection + verify rather than implementation.

  ---
  2026-04-09T17:32:26.524968+00:00
  2026-04-09 execution delta: treated this as a root-scope rebuild fact check, not an implementation change. Decomposition used: (1) inspect `docs/rebuild/review-gating-policy.md` in place, (2) confirm the doc matches the referenced repo vocabulary from `mana-review::RiskLevel` / `RiskFlagKind` and `mana-core::CloseEvidence`, (3) verify the required gate mapping and suspicious-close signals are present, (4) run the unit verify command unchanged. Result: doc already satisfied the requested policy shape; no file edits were needed. Evidence confirmed in the doc includes `Low -> skipped`, `Normal -> optional`, `High/Critical -> mandatory`, escalation from `CloseEvidence.only_mana_changes` and `CloseEvidence.no_path_overlap`, high-risk `RiskFlagKind` handling including `SecuritySensitive`, `VerifyModified`, `TestModified`, `FilesDeleted`, `LargeDiff`, `ManyAttempts`, and conditional `ScopeCreep`, plus required durable skipped-review provenance with candidate/verify/diff-scope/risk evidence.

  ---
  2026-04-09T17:32:33.163294+00:00
  2026-04-09: Externalized completion delta before continuing.

  Observed in docs/rebuild/review-gating-policy.md:
  - RiskLevel::Low defaults to skipped review.
  - RiskLevel::Normal defaults to optional review.
  - RiskLevel::High and RiskLevel::Critical require mandatory skeptical review.
  - Suspicious CloseEvidence escalators are explicitly called out: only_mana_changes and no_path_overlap.
  - Existing high-risk RiskFlagKind cases are covered, including SecuritySensitive, VerifyModified, TestModified, FilesDeleted, LargeDiff, ManyAttempts, and ScopeCreep with context-sensitive escalation.
  - Even when review is skipped, durable provenance plus candidate / verify / diff-scope / risk evidence is still required.

  Execution note:
  - No file edits were needed; the document already satisfied the requested policy.
  - Ran the unit verify command from /Users/asher/tower and it passed as-is.
labels:
- fact
verify: test -f .mana/52-review-gating-policy-is-risk-triggered-with-mandat.md && rg -q '^id:' .mana/52-review-gating-policy-is-risk-triggered-with-mandat.md
kind: epic
unit_type: fact
last_verified: '2026-04-09T23:16:10.195220Z'
stale_after: '2026-05-09T23:16:10.195220Z'
paths:
- docs/rebuild/review-gating-policy.md
- mana/crates/mana-review/src/types.rs
- mana/crates/mana-core/src/ops/close.rs
decisions:
- This unit is cross-project rebuild policy work, so the decomposition and no-edit completion basis should live in root mana. The gate for completion is direct inspection of `docs/rebuild/review-gating-policy.md` plus the unit verify command, not implementation for its own sake. When the doc already contains the required `RiskLevel`, `CloseEvidence`, and skipped-review provenance rules, the correct outcome is to persist the evidence-backed no-change result in mana rather than inventing extra edits.
---

`docs/rebuild/review-gating-policy.md` defines the first rebuild review gate: `RiskLevel::Low` defaults to skipped review, `RiskLevel::Normal` defaults to optional review, and `RiskLevel::High` / `RiskLevel::Critical` require skeptical review. Mandatory review also triggers for suspicious `CloseEvidence` (`only_mana_changes`, `no_path_overlap`) and existing high-risk `RiskFlagKind` cases such as `SecuritySensitive`, `VerifyModified`, `TestModified`, `FilesDeleted`, `ScopeCreep`, `LargeDiff`, and `ManyAttempts`. Even when review is skipped, durable review-policy provenance plus candidate/verify/diff-scope/risk evidence is required.
