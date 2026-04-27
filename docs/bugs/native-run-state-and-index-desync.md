# Bug: native run state and unit index/file desync during mana orchestration

Date: 2026-04-20
Observed by: imp while orchestrating `mailfor`
Repo: `~/mana`

## Summary

During real use of `mana` from within a project using a local `.mana` directory, native orchestration behaved inconsistently in ways that suggest a coordination-layer bug rather than only user error:

1. `mana run` returned a valid run id and reported a unit as started.
2. Subsequent `mana run_state` / `mana logs` calls for that run id sometimes reported the run id as unknown or missing.
3. Some units existed as markdown files but were missing from `.mana/index.yaml`, which made dependency resolution and addressing inconsistent.
4. `mana show` / `mana close` sometimes reported `Unit not found` for units that clearly existed on disk.
5. The scheduler sometimes surfaced parent feature units as `ready` in a way that was not useful for actual execution.

This made orchestration unreliable enough that I had to fall back to direct file inspection, manual verification, and targeted unit runs.

## Concrete session symptoms

### 1) Run id lost after successful start

Observed pattern:
- `mana(action="run", id="2.1.1")` returned a run id (for example `run-1` or `run-2`) and reported the unit as started/running.
- Shortly after, `mana(action="run_state", run_id="run-1")` returned either an empty object or `Unknown native mana run_id: run-1`.
- `mana(action="logs", run_id="run-1")` similarly reported no native run state.

Expected:
- A started run id should remain queryable until completion and some reasonable retention window.

Actual:
- Run state disappeared immediately or unpredictably.

## 2) Unit file exists but index entry missing

In the `mailfor` project, a unit (`2.1.1`) was referenced by dependencies and had a markdown unit file expectation, but was absent from `.mana/index.yaml`.

Effects:
- downstream units treated `2.1.1` as dependency input
- `mana next` suggested `2.1.1` as the best next step
- `mana show` / `mana close` on `2.1.1` sometimes failed with `Unit not found`

I had to manually repair:
- `.mana/index.yaml`
- `.mana/2.1.1-...md`

Expected:
- unit creation should atomically update both file and index
- scheduler/addressing should never observe a partial state

Actual:
- file/index desync was possible in real use

## 3) `show` / `close` inconsistency against existing units

Observed pattern:
- `mana show 2.1.1` failed with `Unit not found`
- the corresponding `.mana/*.md` file existed on disk
- after repairing index state, some operations still intermittently failed to address the unit correctly

Expected:
- addressing should be based on a single consistent source of truth or robust reconciliation between index and files

Actual:
- disk state and addressing behavior diverged

## 4) Parent features surfaced as ready alongside leaf work

Observed in `mana status` / `mana next`:
- parent feature units like `2.1`, `2.2`, `2.3` showed up as ready together with concrete child tasks
- this made generic `mana run` less useful, because the truly actionable units were leaf tasks with verify gates

Expected:
- scheduler recommendations should strongly prefer concrete runnable leaf units over umbrella features
- or feature units with empty verify should be excluded from ready recommendations by default

Actual:
- parent features cluttered the ready set

## 5) Unit archival / file movement may be interacting badly with live orchestration

I also observed signs that a unit markdown file appeared to have been deleted/archived while related state was still in flight:
- `.mana/2.1.2-...md` disappeared from its original path
- a copy appeared under `.mana/archive/...`
- local git status showed staged deletion + archived file

This may be valid behavior, but in context it contributed to confusion because the live unit state was still needed for orchestration and verification.

## Repro sketch

A minimal repro candidate:

1. Create a project with a local `.mana` directory and index.
2. Create a feature plus several dependent child units.
3. Start a targeted run on a leaf unit with `mana run id=<leaf>`.
4. Immediately poll:
   - `mana run_state run_id=<returned>`
   - `mana logs run_id=<returned>`
   - `mana show id=<leaf>`
5. Compare:
   - run state consistency
   - index entry presence
   - markdown file presence
   - whether the unit remains addressable

A second repro candidate:

1. Create many units quickly, especially with parent/child/dependency relationships.
2. Inspect `.mana/index.yaml` and `.mana/*.md` after each creation.
3. Confirm whether creation is fully atomic or if interrupted/failed writes can leave index/file divergence.

## Suspected root-cause areas

Potential areas worth auditing:

1. **Atomicity of unit creation/update**
   - file write succeeds but index update fails
   - index update succeeds but file write fails
   - partial write observed by later commands

2. **Native run state retention / lookup**
   - run ids not persisted long enough
   - run state stored per-session/in-memory only
   - race between run completion and run-state query

3. **Multiple sources of truth**
   - addressing may rely on index while other commands rely on files
   - reconciliation may be incomplete or absent

4. **Archival side effects**
   - close/archive path may move unit files while references are still needed
   - addressing code may not handle archived-but-recent units consistently

5. **Ready-set computation**
   - feature/umbrella nodes with no verify being surfaced as ready work
   - scheduler not preferring leaves strongly enough

## Expected behavior

- Unit creation should be atomic and durable: file + index always consistent.
- `mana run` should return a run id that remains queryable until completion and shortly after.
- `show`, `close`, `logs`, and `run_state` should all agree on unit identity.
- If desync occurs, mana should detect and repair or at least report it explicitly.
- `next` and `status` should default to concrete executable leaf units, not parent feature containers.

## Actual impact

This bug degraded orchestration enough that I had to:
- inspect `.mana/index.yaml` directly
- inspect `.mana/*.md` directly
- manually restore a missing unit entry
- manually verify work with shell commands instead of trusting run state
- dispatch specific runs one-by-one rather than relying on the scheduler

## Suggested fixes

1. Add an invariant check command or automatic reconciliation step:
   - every indexed unit must have a file
   - every unit file must have an indexed unit
   - parent/dependency references must resolve

2. Make create/update/close operations transactional from the perspective of readers.

3. Persist native run state durably enough for follow-up inspection.

4. Improve `next` / `status` heuristics to prioritize leaf units by default.

5. Add explicit diagnostics when file/index mismatch is detected instead of generic `Unit not found`.

## Notes

This report comes from an actual orchestration session, not synthetic testing. The symptoms may involve both a bug in mana and edge cases triggered by interrupted or partial operations, but the current UX makes those conditions very hard to distinguish.
