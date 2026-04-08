# mana rebuild plan

Status: active planning draft
Audience: `mana` maintainers
Scope: `~/tower/mana`

This file converts `mana_rebuild_strategy.md` into an execution plan with a strict order of operations.

The main principle remains:

> preserve the semantics, replace the spine

That means keeping the durable work model while rebuilding the seams around contracts, stores, artifacts, leases, and runner integration.

---

## Mission

`mana` should become the authoritative durable substrate for:
- work contracts
- graph structure
- attempts and retry memory
- facts and verified project knowledge
- evidence bundles and artifacts
- review and approval records
- worktree / runner / sandbox leases
- readiness and orchestration state another worker can inherit cold

The rebuild should reduce the amount of durable truth that currently leaks into:
- shell templates
- CLI-only orchestration behavior
- transient runtime structs
- prose-only notes with weak machine structure

---

## Decisions already made

### 1. `mana` stays local-first

The rebuild starts from the local file-native mode.

Hosted / SQL support is a later compatibility target, not the opening move.

### 2. Canonical unit semantics stay stable

Do not churn the meaning of:
- epic
- job
- fact
- verify
- attempt
- dependency
- review
- close

The migration should preserve user-facing semantics where possible.

### 3. Canonical cross-project protocol does not live only inside `mana`

`mana` should depend on the shared Tower contracts crate, not define the whole boundary unilaterally.

### 4. Markdown remains human-facing, but not the only mutable state carrier

The likely target is:
- human contract in Markdown
- machine-authoritative mutable state in sidecar JSON
- append-only artifacts in dedicated directories

### 5. Worktrees and runner coordination become durable lease objects

They should stop being only helper behavior.

---

## Required end-state inside `mana`

`mana` should own durable objects for:
- unit contracts
- unit machine state
- attempt and run history
- verifier outputs
- evidence bundle refs
- artifact refs
- review decisions
- approval/promotion decisions
- execution leases
- worktree leases
- sandbox leases when durable

If another worker must inherit it cold, it should be representable in `mana`.

---

## Order of operations

## Phase M0 — freeze vocabulary and current invariants

Goal:
- stop semantic churn while internals move

Deliverables:
- preserve current durable concepts in docs and code comments
- identify current invariants that tests must keep protecting
- enumerate current shell-template assumptions that are transitional

Likely files:
- `mana/README.md`
- `mana/ARCHITECTURE.md`
- `mana/mana_rebuild_strategy.md`
- `mana/crates/mana-core/src/unit/*`
- `mana/crates/mana-core/src/ops/*`

## Phase M1 — adopt shared contracts from Tower

Goal:
- make cross-boundary workflow types explicit before store or runner changes

Deliverables:
- `tower-contracts` dependency wired into `mana`
- mapping from existing `mana` types to shared protocol types
- first canonical definitions for assignment/result/evidence/lease references

Important rule:
- do not simultaneously redesign stores and protocol types in the same slice

## Phase M2 — split domain from storage/adapters

Goal:
- stop file parsing and CLI orchestration from being the architectural center

Target seams:
- domain entities and invariants
- store traits
- file-backed implementations
- API/service layer
- CLI adapters

Likely end-state shape:
- domain
- store traits
- file store
- scheduler/services
- API/adapters

Deliverables:
- storage traits for units, facts, runs, artifacts, reviews, leases
- file-backed implementations that preserve current behavior
- compatibility path from current `.mana/` layout

## Phase M3 — introduce machine state sidecars and artifact registration

Goal:
- stop overloading Markdown with high-churn machine state

Deliverables:
- sidecar JSON for mutable machine state
- artifact registration and stable artifact refs
- rebuildable derived indexes
- append-only storage model for evidence bundles and logs

Target local shape:

```text
.mana/
  units/
  state/
  artifacts/
  index/
  leases/
```

Notes:
- exact layout can evolve, but these categories should become explicit

## Phase M4 — make leases first-class durable objects

Goal:
- represent execution surfaces durably enough for crash recovery and coordination

Deliverables:
- execution lease model
- worktree lease model
- runner lease model
- sandbox lease model where applicable
- cleanup and stale-lease semantics

Why this phase matters:
- the runner protocol and multi-agent safety depend on these objects being explicit

## Phase M5 — move orchestration onto a typed runner protocol

Goal:
- make `mana run` target typed runner requests/responses instead of shell templates as the center

Deliverables:
- runner request/result types consumed from shared contracts
- first local runner adapter
- binding of run outputs to durable run and artifact records
- CLI template retained only as adapter/compat path if needed

Important rule:
- shell-template spawning may survive temporarily, but only as an adapter layer

## Phase M6 — upgrade evidence, review, and promotion into explicit durable stages

Goal:
- make completion flow through evidence rather than narration

Deliverables:
- minimum evidence bundle representation
- verifier result recording
- skeptic/review queue builder and decision records
- approval/promotion objects
- clearer closure semantics based on durable records

## Phase M7 — prepare optional hosted / SQL backend without changing semantics

Goal:
- make backend-neutral design real, but only after the local-first model is strong

Deliverables:
- SQL store interfaces and first implementation plan or scaffold
- parity checklist against file backend semantics
- explicit compatibility boundaries

This phase is later on purpose.

---

## Dependency rules for mana work

### Must happen first
1. shared contracts adoption
2. domain/store separation
3. machine state + artifact model
4. lease model
5. typed runner integration
6. explicit review/promotion stages
7. hosted backend preparation

### Can proceed in parallel later
After M2, limited parallel work is reasonable across:
- artifact registration
- lease implementation
- review queue shaping

But do not fork competing designs for:
- artifact refs
- lease identity
- verifier result schema
- run/result storage

---

## Suggested mana epics

### Epic A — adopt shared contracts in mana

Jobs should cover:
- inspect existing worker/run/result structs
- design `mana` mappings to shared contracts
- land compatibility adapters
- verify existing commands still compile and pass focused tests

### Epic B — split domain/store/adapter seams

Jobs should cover:
- isolate domain entities and invariants
- define store traits
- move file-native logic behind traits
- keep CLI behavior stable

### Epic C — machine state and artifact model

Jobs should cover:
- define state sidecars
- define artifact refs and artifact registration
- define append-only evidence layout
- add focused migration tests

### Epic D — durable lease model

Jobs should cover:
- execution lease schema
- worktree lease schema
- stale lease recovery semantics
- integration with current run/close flows

### Epic E — typed runner protocol integration

Jobs should cover:
- local runner adapter
- request/result bindings
- replacement of shell-centered orchestration path
- run artifact capture

### Epic F — explicit review and promotion stages

Jobs should cover:
- verifier result recording
- skeptic/review records
- approval/promotion records
- close semantics that consult durable evidence

### Epic G — backend-neutral preparation

Jobs should cover:
- SQL-store compatibility design
- semantic parity checklist
- migration boundaries and versioning

---

## Verify philosophy for mana rebuild work

Use narrow verification.

Prefer:
- focused `cargo test -p mana-core <target>`
- focused `cargo test -p mana-cli <target>`
- existence checks paired with targeted tests
- migration tests that prove old data still loads or adapts correctly

Avoid broad verify gates like:
- `cargo test`
- vague grep-only success checks unless paired with behavior tests

---

## What not to do

Do not:
- rewrite `mana` from scratch
- make hosted backend assumptions drive the local-first model early
- redesign all user-visible semantics at once
- keep introducing durable concepts only as CLI glue or ad hoc structs
- treat worktree and runner state as helper details forever

---

## Success condition

The `mana` rebuild is successful when:
- durable workflow truth is represented by explicit domain + store + artifact + lease objects
- Markdown remains readable and useful, but no longer carries all mutable authority
- runner integration is typed and attributable
- evidence, review, and approval are durable workflow stages
- another worker can inherit task state, prior evidence, and execution-surface state cold from `mana`
