---
id: '265'
title: Audit and clean root mana library for stale, duplicate, defunct, and lookup-broken units
slug: audit-and-clean-root-mana-library-for-stale-duplic
status: closed
priority: 1
created_at: '2026-04-16T06:49:05.051048Z'
updated_at: '2026-04-27T21:46:12.168251Z'
notes: |-
  ---
  2026-04-16T06:54:30.682944+00:00
  Initial audit findings from root graph inspection:
  - Root .mana currently has 209 live markdown unit files and 273 archived markdown files.
  - 24 IDs are reused across multiple records (mostly archived collisions from old numbering discipline). Highest-collision examples:
    - 28.6 appears 8 times in archive with different titles.
    - 45.4.4 appears 6 times total (1 open + 5 archived with different titles).
    - 45.5.4 appears 5 archived times.
    - 45.8 appears 4 times total (1 open recovery unit + 3 archived unrelated historical meanings).
    - .2, .2.1, .2.2, .2.3 have repeated archived records from early empty-title/placeholder usage.
  - Open duplicate/supersession candidates visible from current graph state:
    - 67 and 69 are duplicate facts with identical titles/verify.
    - 75 remains open while closed fact 58 states the same claim; likely keep one canonical fact.
    - 79 and 80 are near-duplicate CI audit facts; 64 is a related closed predecessor.
    - 248.14 open implementation unit is likely the follow-on of closed design/spec unit with same id lineage collision.
  - Open recovery/metadata-debt units likely worth batching together:
    - 45.8, 60, 65, 68, 77, 78, 261.
  - Historical archive contains many reused IDs with different meanings, so cleanup should prefer canonicalization + notes over destructive deletion.
  - User preference for this turn: produce a full audit report before any graph edits.

  ---
  2026-04-16T06:55:31.427165+00:00
  Proposed cleanup grouping after deeper inspection:

  Batch A — likely safe after brief confirmation
  - 67 vs 69: exact duplicate open facts (same title, verify, paths). Keep 67 or 69 as canonical; close/archive the other with a superseded note.
  - 79 vs 80: near-duplicate root CI audit facts. Keep one canonical fact and demote the other to provenance/superseded status.
  - 50.15 vs 50.16: same title/description; 50.16 has the child decomposition and note, so 50.15 looks superseded by 50.16.
  - 60: appears completed from its own durable notes and closed replacement fact 47/61 lineage, but needs fresh verify before closing because it is still in_progress.

  Batch B — metadata bug cluster; probably keep but consolidate later
  - 45.8 recovery unit for missing root lookup after successful implementation.
  - 65 fact about 51.3 list/show inconsistency.
  - 68 fact about 50.10.3 list/lookup inconsistency.
  - 77 follow-up bug and 78 recovered decomposition for vanished fact 56.
  - 261 follow-up for archived lookup inconsistency on 260.
  These are all same-class lookup/archive/index drift rather than product work; could eventually be grouped under one root metadata-bug epic once current evidence is preserved.

  Batch C — historical archive debt, not urgent root clutter
  - Reused archived IDs: 28.6 (8 variants), 45.4.4 (6 variants), 45.5.4 (5 variants), 45.8 (4 meanings), plus early dot-prefixed placeholder IDs (.1/.2/.2.1/.2.2/.2.3).
  - This is mostly archive/history hygiene. It is confusing for lookup and provenance, but lower priority than cleaning current open root units.

  Batch D — probably live, keep open for now
  - 45.8 open recovery task itself appears legitimate.
  - 248.14 is not a duplicate of the archived design predecessor; it is a follow-on implementation slice with a different scope, despite ID lineage confusion.
  - 75 vs closed 58 is duplicate lineage around the same fact, but 75 may now be the intended open canonical current-state fact while 58 remains historical provenance.

  Recommendation order:
  1. Safe duplicate cleanup (67/69, 79/80, 50.15/50.16).
  2. Re-verify and likely close 60 if its gate still passes.
  3. Decide whether to keep individual metadata-bug follow-ups (45.8, 65, 68, 77, 78, 261) or consolidate them under a single graph-hygiene thread.
  4. Leave archive-wide ID-collision cleanup for a later dedicated pass unless it is needed to unblock lookup fixes.

  ---
  2026-04-16T07:20:40.795707+00:00
  User clarified that the cleanup scope is broader than the first obvious duplicate batch. Treat this as a full library audit, not a quick duplicate pass. The next useful output should categorize the entire live root graph into: active/keep, likely completed-but-open, duplicate/superseded, metadata-bug/recovery, contradictory direction, and archive-history debt — with counts and concrete candidate IDs per bucket.

  ---
  2026-04-16T09:42:13.177331+00:00
  Cleanup pass 1 executed:
  - Closed exact duplicate `50.15` as superseded by canonical `50.16`.
  - Closed exact duplicate fact `67` as superseded by canonical `69`.
  - Closed overlapping recovered fact `80` as superseded by canonical `79`.
  - Repaired verify drift on `51.6` so it now points at the actual created file `51.6-...` rather than nonexistent `51.7-...`.
  - Repaired verify drift on `45.8` so it now points at the active root file for the metadata-bug unit rather than a nonexistent historical implementation-path filename.

  Remaining next cleanup targets:
  - contradictory direction cluster around `248.8`, `248.15`, and `50.12`
  - likely completed-but-open planning units such as `60`, `44.1.2.1`, `44.1.5.1.1`, and possibly parent `44.1.5`
  - broader metadata/lookup cluster consolidation (`45.8`, `65`, `68`, `77`, `78`, `261`, `27.22`, `44.1.5.4`)

  ---
  2026-04-16T09:47:08.042358+00:00
  Cleanup pass 2 executed:
  - Closed stale completed unit `60` after native verify passed; its notes already documented successful metadata-only reconciliation and current truth living in fact `61`.
  - Closed stale completed planning unit `44.1.2.1` after native verify passed; its artifact already exists in `docs/design/imp-semantic-write-policy-model.md`.
  - Resolved the contradictory default-entrypoint cluster by treating `248.8` as completed historical experiment, closing `248.15` as the current restored-reality implementation after verify passed, and keeping `50.12` open as the future-facing staged migration epic.
  - Added provenance notes so `248.8` is understood as superseded experiment rather than a still-live target.
  - Added semantic-write lineage note to `44.1.5.4`: duplicate legacy unit is now closed; remaining work is lookup consistency around `44.1.5.1` plus verify-string/closure decision for `44.1.5.1.1`.
  - Re-checked `44.1.5.1.1`: artifact exists, but native verify still fails due to case-sensitive verify-string drift (`Capability matrix` vs `## Capability matrix resolution`). Left open for now pending explicit verify repair or lineage fold-in.

  Current highest-value remaining cleanup targets:
  - metadata/lookup cluster: `45.8`, `65`, `68`, `77`, `78`, `261`, `27.22`, `44.1.5.4`
  - verify-string drift / lineage cleanup for `44.1.5.1.1`
  - broader fact-library rationalization beyond the first duplicate batch

  ---
  2026-04-16T11:49:03.545476+00:00
  Cleanup pass 3 in progress:
  - Re-verified metadata-bug cluster and confirmed these still preserve real unresolved lookup/archive anomalies rather than dead debris: `27.22`, `44.1.5.4`, `261`.
  - Re-verified metadata-history facts and confirmed these are still true current evidence records: `65`, `68`, `77`, `78`, `45.8`.
  - Repaired verify-string drift on `44.1.5.1.1` by making its capability-matrix check case-insensitive to match the existing authored heading.
  - Added parent-lineage note on `44.1.5` clarifying that archived `44.1.5.1` and active `44.1.5.1.1` are one typed-policy-model lineage, with the doc artifact as canonical output.

  Remaining cleanup opportunities now look less like safe closures and more like structural rationalization:
  - decide whether metadata bug trackers (`27.22`, `44.1.5.4`, `261`) should stay split by repro or collapse under one canonical root lookup/archive bug epic
  - decide whether recovered record `78` should remain open as durable execution provenance or be folded into notes on closed fact `56` once archived lookup works
  - broader fact-library policy cleanup for open factual records (`57`, `66`, `68`-`79`) that are true but noisy at root scope

  ---
  2026-04-16T11:50:26.863832+00:00
  Cleanup pass 4 executed:
  - Repaired verify drift on `44.1.5.1.1`, then closed it because the typed policy-model artifact exists and verify now passes.
  - Confirmed `27.22`, `44.1.5.4`, and `261` are still live metadata bugs rather than stale debris; left them open with updated notes.
  - Closed open duplicate fact `75` as superseded by canonical closed fact `58`.

  Current safe-closure wins now look mostly exhausted. Remaining open clutter is dominated by:
  - still-live metadata bug trackers (`27.22`, `44.1.5.4`, `45.8`, `261`, plus supportive facts `65`, `68`, `77`, `78`)
  - intentionally open fact library (`57`, `66`, `70`, `71`, `72`, `73`, `74`, `76`, `79`) that is true but noisy
  - large active planning trees whose cleanliness is more about graph policy than mistaken status

  ---
  2026-04-16T11:51:31.213610+00:00
  Cleanup pass 5 executed:
  - Closed `50.10.3.1` after native verify passed; it was a stale completed setup smoke-verification slice.
  - Re-checked `28.5.1`: its notes strongly suggest the prompt-doctrine work landed, but native verify still fails due to unrelated imp-core compile drift (`TaskContext` constraints field). Left it open as a likely completed-but-blocked cleanup candidate rather than forcing closure.
  - Re-ran duplicate-title and verify-path scans on the live graph; no additional exact duplicate live titles or broken `.mana/<file>.md` verify paths remain in the obvious safe bucket.

  Current remaining cleanup is now mostly judgment-heavy rather than mechanical:
  - open metadata-bug/recovery thread (`27.22`, `44.1.5.4`, `45.8`, `261`, with facts `65`, `68`, `77`, `78`)
  - likely-completed-but-verify-blocked unit `28.5.1`
  - root fact-library policy/noise (`57`, `66`, `70`, `71`, `72`, `73`, `74`, `76`, `79`)

  ---
  2026-04-24T05:06:11.222927+00:00
  Resuming graph hygiene pass on 2026-04-24 after user explicitly requested it. First finding: prior hygiene unit 265 already exists in project/root tower mana and contains passes 1-5. I claimed it and will continue from its remaining cleanup targets rather than creating a duplicate planning unit. Scope: prioritize safe metadata/status cleanup and report judgment-heavy items; avoid deleting history or closing active work without evidence.

  ---
  2026-04-24T05:31:14.669656+00:00
  Cleanup pass 6 executed on 2026-04-24.
  - Closed stale completed design unit `248.16.5` after rechecking that the SVG wireframes and companion memo still exist in `docs/design/wireframes/` and `docs/design/imp-tui-core-redesign-answers.md`.
  - Reclassified `248.16.7` from in_progress to open because it is a user-review follow-up, not active claimed execution.
  - Reclassified smoke trial units `254` and `256` from in_progress to open because the prior attempts timed out and no successful end-to-end artifact exists; `256` currently had only an empty output file.
  - Repaired malformed/empty active unit `.3` by setting a real title derived from its slug (`Set up Harbor adapter and terminal-bench 2.0 runner`) and reclassifying it from in_progress to open since the prior attempt was abandoned and end-to-end validation did not happen.

  Remaining likely hygiene targets after this pass:
  - other long-stale claimed/in_progress units that are really speculative planning or blocked follow-ups rather than active execution
  - intentionally noisy root fact library (`57`, `66`, `68`, `70`, `71`, `72`, `73`, `74`, `76`, `79`) if you want a stronger keep/archive policy
  - archive/history debt from repeated ID reuse (especially older `45.4.4`, `45.8`, `45.5.4`, and dot-prefixed placeholder history), which is judgment-heavy and riskier than the status cleanup above.

  ---
  2026-04-24T05:36:31.547917+00:00
  Graph hygiene pass 6 started 2026-04-24. Safe actions taken so far: repaired malformed live unit `.3` title/status from empty in_progress to open named Harbor/TB2 runner after its artifact verify passed but notes showed E2E validation incomplete; released its stale claim. Closed completed/stale fact `53` after verify passed. Closed stale contradictory fact `62` as superseded by canonical current CI fact `79`. Released stale claim on metadata fact `65`. Repaired verify drift on canonical fact `69` so it now checks imp-cli lacks duplicate helpers and imp-core mana_worker/docs contain canonical helpers; verify now passes.

  ---
  2026-04-24T05:37:40.992852+00:00
  Graph hygiene pass 6 continued: released stale `imp` claims on long-abandoned in_progress units older than ~7 days, leaving status/scope intact for future prioritization. Released claims: 27.3, 27.4, 27.9, 27.14, 27.15, 27.17, 28.4, 28.5.1, 28.5.6, 45.7.4, 50.2, 50.6.2, 50.8, 50.9, 50.9.1, 50.10.1, 50.10.1.1, 50.10.1.2, 50.10.2, 50.11, 50.11.1, 50.14, 50.16.1, 51.4, 51.5, 81, 249, 254, 256, 264.3.1, 264.3.2, 264.3.3, 264.3.4, 248.16.5, 248.16.7, 248.18.1, 248.18.2. This makes active claim signals trustworthy again without asserting those units are done.

  ---
  2026-04-24T05:37:56.257161+00:00
  Cleanup pass 7 executed on 2026-04-24, focused on stale claimed/in_progress units with safe verify evidence.

  Closed after verify passed:
  - `81` Design imp-native delegation tool around imp run and mana orchestration
  - `50.2` Define canonical imp command grammar and default-entrypoint migration
  - `50.6.2` Write concrete command taxonomy for CLI-first imp shell, subcommands, and viewer handoff
  - `50.8` Design the simplified TUI role after a CLI-first shift
  - `50.9.1` Expand imp chat with the next minimal operational command set
  - `50.10.1.1` Fix mana-core close.rs compile mismatch blocking imp settings smoke verification
  - `50.11.1` Wire imp chat shell handoff commands into imp view submodes

  Reclassified to open instead of closing:
  - `51.4` verify failed because the stale strings were absent; the verify gate likely encodes inverse success and needs repair/close review.
  - `50.10.1.2` verify failed with exit 127; likely missing command/tool in verify gate, needs repair.

  No source files changed. The pass reduced stale in_progress noise and preserved questionable items for explicit review.

  ---
  2026-04-24T06:01:14.043818+00:00
  Prevention recommendations from hygiene discussion 2026-04-24:
  1. Add lifecycle semantics that distinguish executable jobs from durable memos/decisions/facts. Only executable jobs should require verify gates; planning captures should not fake verifies.
  2. Add lease/claim expiry and stale-claim surfacing. Claims should have heartbeat/updated_at TTL; expired claims should become `stale_claim` or show separately from active in_progress.
  3. Add verify-gate health checks. Track verify command exit status and classify failures as objective-failed vs verify-broken (e.g. exit 127, missing files/tools, inverted grep patterns). Make verify repair a first-class hygiene lane.
  4. Add ID integrity enforcement: unique IDs, no dot-prefixed placeholders, no duplicate historical IDs in active lookup, append-only migration records for renames/reparents.
  5. Add state-transition guards: closing requires passing verify or explicit override reason; in_progress requires active lease; blocked requires blocker text; open should not have active assignee unless claimed.
  6. Add graph views for Now/Next/Later/Archive plus project/area filters so root mana can remain comprehensive without every old planning artifact polluting execution views.
  7. Add hygiene automation: periodic `mana doctor` that reports stale claims, duplicate/malformed IDs, empty titles, fake verifies, verify exit 127, closed units with failing verifies, in_progress units not updated recently, and orphaned/dependency-broken nodes.
  8. Add unit templates by kind: epic, executable job, investigation, decision, fact, memo. Each template should require only the fields that make sense for that kind.
  9. Add artifact/evidence references as structured fields instead of burying them in notes, so completion evidence can be rechecked without reading long histories.
  10. Add archive policy and tooling: old completed/planning units remain searchable but are hidden from active status/tree by default.

  ---
  2026-04-24T06:01:28.090753+00:00
  Execution mode for follow-on prevention design, per mana skill and user instruction (2026-04-24): treat this as planning/design first, not implementation. Use existing cleanup tracker `265` for durable notes unless a concrete, bounded implementation job emerges. Prefer notes/decisions over placeholder jobs. Create child jobs only when they have: one outcome, concrete files/functions, clear scope boundaries, and a real verify gate. Prefer root/project tower scope because the fixes affect cross-project mana behavior and graph trust. Avoid duplicate mana/imp boundary planning units and avoid fake verifies.
labels:
- mana
- cleanup
- metadata
- triage
- root
closed_at: '2026-04-27T21:46:12.168251Z'
close_reason: 'Auto-closed: all children completed'
verify: cd /Users/asher/mana && mana show 265 >/dev/null
checkpoint: '5854ad71b63145627539c9b6a07c4f5e781a9e4e'
verify_hash: e40932f0c50cc8aff1f29ab2b01c9d9aaf7ad6ca579eb403f512f9c70eb16cf2
is_archived: true
kind: job
attempt_log:
- num: 1
  outcome: abandoned
  agent: imp
  started_at: '2026-04-24T05:06:05.518795Z'
---

Goal: reduce root /tower/.mana clutter and repair obvious metadata drift so the root mana graph reflects current work rather than stale duplicates.

Current state:
- Root mana currently has 209 live markdown unit files plus 273 archived markdown files.
- The graph contains multiple obvious duplicate or conflicting records, including repeated IDs with different titles across open/closed history (for example 45.4.4, 45.8, 248.14), duplicate facts with near-identical meaning (for example 67/69, 79/80, 58/75), and explicit follow-up units about broken lookup/archive recovery (for example 65, 68, 77, 78, 261).
- Some units appear to have expanded scope or been superseded, but remain open.

Steps:
1. Audit the root mana graph for cleanup classes: completed-but-open, duplicate/superseded, malformed/empty-title, lookup-broken, and defunct-by-scope-change.
2. Produce a proposed cleanup batch grouped by action: close as completed, archive/supersede, merge by keeping canonical unit and noting replacement, or keep/open because still live.
3. For each cleanup candidate, record the exact evidence from current mana state (IDs, titles, collisions, dependent children, lookup anomalies).
4. After review/confirmation on consequential merges, execute the first safe cleanup batch and record what changed.

Files / paths:
- /Users/asher/tower/.mana (audit and update)
- /Users/asher/tower/.mana/archive (audit archived collisions and superseded records)

In scope:
- Root mana metadata cleanup and graph hygiene
- Duplicate/superseded fact and job consolidation plan
- Identification of malformed or recovery-needed units

Out of scope:
- Changing actual product/code scope unless needed to reflect already-finished work
- Project-local mana trees outside the root graph

Do not:
- Silently delete history
- Collapse distinct historical decisions just because titles look similar
- Close active units without checking current dependencies/children/claims first
