# mana_rebuild_strategy

## Recommendation

Do **not** rewrite `mana` from scratch.

Do a **selective rebuild around a new architectural spine**:

- keep the core domain model: units, dependencies, attempts, verify history, facts, review
- keep the `mana = durable substrate` role
- keep the local-first file backend as a first-class mode
- replace the internal seams that currently make local-only and shell-driven assumptions

The right strategy is:

> **semantic continuity, infrastructural replacement**

That means preserving the meaning of the system while replacing the storage boundary, runner boundary, evidence model, and `mana ↔ imp` protocol.

---

## Executive summary

`mana` already has the right conceptual center:

- work graph as the primary object
- units as durable work contracts
- verify gates as machine-checkable completion
- attempts and notes as retry memory
- review as a first-class concern
- a clear `mana` / `imp` split at the architecture-doc level

What it lacks is not the core idea. It lacks a few explicit systems boundaries that will matter more and more if you want:

- a dual backend (`file` + `hosted`)
- stronger multi-agent coordination
- first-class worktree / sandbox leases
- artifact-based verification and review
- a protocol-first `mana ↔ imp` boundary
- hosted multi-user operation later

So the recommendation is:

- **keep the concepts**
- **keep most user-facing workflow semantics**
- **introduce backend-neutral contracts and stores**
- **move orchestration from shell-template coupling toward typed runner protocols**
- **upgrade evidence and review from thin history to first-class artifacts**

---

## What to preserve

These parts are already strong enough that a full rewrite would likely lose more than it gains.

### 1. Units as the durable work contract

Keep:

- title
- description
- acceptance
- verify
- parent / child relationships
- dependencies
- produces / requires
- attempts
- notes
- decisions
- facts

This is already a good vocabulary.

### 2. The work graph mental model

Keep the principle that the primary object is the work graph, not the transcript and not only the file.

### 3. Verify-gated completion

This is one of `mana`'s best ideas and should remain central.

### 4. Local-first usability

The current file-native workflow is still strategically valuable.

### 5. The `mana = durable truth` / `imp = live execution` split

Keep this architectural split, but make the contract explicit in code rather than only in docs.

---

## What to change aggressively

### 1. Introduce a first-class contracts layer

Create a shared protocol crate for the `mana ↔ imp ↔ runner` boundary.

Suggested name:

- `mana-contracts`

Suggested types:

- `TaskAssignment`
- `TaskNormalization`
- `ExecutionPlan`
- `ExecutionLease`
- `WorktreeLease`
- `SandboxSpec`
- `VerifierResult`
- `EvidenceBundle`
- `ReviewDecision`
- `ApprovalRecord`
- `PromotionDecision`
- `ArtifactRef`

This should become the canonical integration surface.

### 2. Introduce storage backends behind traits

Create backend-neutral interfaces such as:

- `UnitStore`
- `FactStore`
- `RunStore`
- `ArtifactStore`
- `ReviewStore`
- `LeaseStore`
- `IndexStore` or derived-view builder

Implement at least:

- `file-store` — local-first mode
- `sql-store` — hosted mode

### 3. Upgrade evidence and review into first-class artifact systems

Current attempt/run history is useful but too thin.

Add first-class artifacts for:

- verify runs
- diff bindings
- scope checks
- skeptic review passes
- approval decisions
- promotion outcomes
- worktree/sandbox provenance

### 4. Replace shell-template orchestration as the long-term core

Keep CLI templates as an adapter if useful, but move the architecture toward typed runner requests/responses.

### 5. Make worktrees and sandboxes durable coordination objects

Worktree state should not just be helper logic. It should be durable enough that a dead worker can be reasoned about and cleaned up.

### 6. Reduce heuristic extraction from prose as a primary mechanism

Heuristic file extraction is still useful as a fallback, but should not be the main source of truth for execution-critical context.

---

## Storage model recommendation

## Local-first backend

If `mana` remains local-first, keep a file-native backend, but do **not** keep all state in Markdown.

Recommended shape:

```text
.mana/
  units/
    1-parent.md
    1.1-child.md
  state/
    1-parent.json
    1.1-child.json
  artifacts/
    1/
    1.1/
  index/
    units.json
    ready.json
    review.json
  leases/
  locks/
```

Meaning:

- `units/*.md` = canonical human work contract
- `state/*.json` = machine-authoritative mutable state
- `artifacts/` = append-only evidence and logs
- `index/` = rebuildable derived views
- `leases/` = active worktree / runner / sandbox claims

## Hosted backend

If `mana` becomes hosted and multi-user, make the **database** canonical.

Use:

- Postgres for workflow state
- object storage for logs, traces, verifier outputs, diffs, reports
- Markdown as a rendering/export surface, not as canonical truth

That is why the dual-backend design should be introduced now.

---

## Proposed target architecture

### Layer 1: domain model

Stable concepts:

- units
- facts
- attempts
- runs
- dependencies
- reviews
- approvals
- artifacts
- leases

### Layer 2: contracts

Shared typed boundary between `mana`, `imp`, and runner components.

### Layer 3: stores

Backend-neutral interfaces and implementations.

### Layer 4: workflow services

Examples:

- ready queue planner
n- review queue builder
- verify recorder
- artifact registrar
- lease manager
- promotion gate

### Layer 5: adapters

Examples:

- CLI
- TUI / UI
- MCP
- local runner
- hosted API

The key shift is this:

> file parsing and shell orchestration should become adapters, not the architectural center.

---

## Important architecture decisions

These were the most important decisions to make explicitly. The following defaults are now the recommended path for the rebuild.

### Decision 1: what is the true canonical unit state?

Options:

- A. Markdown only
- B. Markdown + sidecar JSON in file backend; DB rows in hosted backend
- C. DB only, even locally

Recommendation:

- **B**

Reason:

- keeps local-first transparency
- supports hosted mode cleanly
- avoids overloading Markdown with high-churn machine state

### Decision 2: where should the `mana ↔ imp` boundary live, and where should the runner belong?

Options:

- A. continue with worker-facing types inside `imp`
- B. move to shared contracts crate and treat the runner as its own execution boundary

Recommendation:

- **B**

Reason:

- better protocol clarity
- easier backend evolution
- cleaner testing and documentation
- preserves the architecture split: `mana` owns durable truth, `imp` owns worker runtime, and the runner owns execution isolation

Practical interpretation:

- architecturally, the runner should be a **third thing**
- repository-wise, it can live under `imp` at first if that reduces churn
- but it should not be modeled as just another incidental helper inside the worker runtime

### Decision 3: should verification remain inline with editing or become a separate run?

Options:

- A. same run does edit + verify
- B. editor run produces candidate, fresh verifier run checks it

Recommendation:

- **B** for meaningful work
- allow A only for tiny/local loops as an optimization

Reason:

- stronger evidence
- less hidden state
- better trust boundary

### Decision 4: should review be optional advice or a true workflow stage?

Options:

- A. best-effort side feature
- B. explicit stateful workflow stage

Recommendation:

- **B**, but **optional by policy/risk class**

Reason:

- aligns with `agent_design.md`
- gives you a real skeptic gate when needed
- avoids forcing heavyweight review on every tiny or low-risk workflow
- maps better to a future hosted product with configurable policy tiers

### Decision 5: how should worktrees be represented?

Options:

- A. helper functions only
- B. durable leased objects with status and cleanup metadata

Recommendation:

- **B**, but make the representation **mostly derived where possible**

Reason:

- better concurrency handling
- better crash recovery
- needed for hosted / distributed runners later
- avoids over-persisting values that can be recomputed from the underlying git and lease state

Practical interpretation:

Persist only what must survive process death and coordination boundaries, such as:

- lease owner
- branch/worktree identity
- base commit
- sandbox profile
- lifecycle status
- cleanup metadata

Derive the rest when possible.

### Decision 6: how much shell-template spawning should survive?

Options:

- A. keep shell-template spawning as the core
- B. move to typed runner protocol, keep shell template as legacy/local adapter

Recommendation:

- **B**

Reason:

- stronger observability and control
- easier hosted future
- less stringly orchestration

### Decision 7: how much backward compatibility should be preserved?

Options:

- A. strict full compatibility forever
- B. compatibility during migration, then codify v2 format
- C. allow cleaner breakage where it materially improves the architecture

Recommendation:

- **C**

Reason:

- avoids freezing the rebuild around legacy implementation accidents
- allows cleaner backend, artifact, and runner boundaries
- reduces the risk of shipping a permanently compromised v2 just to preserve old file shapes

Practical interpretation:

- provide migration tools where easy and high-value
- preserve semantic continuity where possible
- do **not** preserve old layout or API behavior if it blocks the new architecture

---

## Rewrite strategy

## Do not do a big-bang rewrite

That would risk:

- losing domain behavior already encoded in the project
- shipping a cleaner architecture with weaker semantics
- pausing feature progress too long

## Do a strangler migration

Recommended order:

### Phase 0: freeze semantic vocabulary

Do not churn meanings of:

- job
- epic
- fact
- verify
- attempt
- review
- close
- dependency

### Phase 1: add contracts crate

Goal:

- make the integration surface explicit before changing backends or runners

### Phase 2: add store interfaces and file backend adapter

Goal:

- preserve current functionality while re-anchoring operations behind traits

### Phase 3: add artifact model and lease model

Goal:

- make evidence and runtime coordination first-class durable state

### Phase 4: move orchestration to runner protocol

Goal:

- make `mana run` talk to a typed local runner instead of relying on command-template architecture as the center

### Phase 5: add SQL/hosted backend

Goal:

- support multi-user / hosted without changing domain semantics

### Phase 6: move review and promotion into explicit stages

Goal:

- stronger candidate → evidence → gate workflow

---

## Suggested monorepo shape inside `mana/`

```text
mana/
  crates/
    mana-contracts
    mana-domain
    mana-store
    mana-file-store
    mana-sql-store
    mana-api
    mana-scheduler
    mana-review
    mana-runner-proto
    mana-runner-local
    mana-cli
```

### Meaning

- `mana-contracts` — shared typed protocol
- `mana-domain` — core entities and invariants
- `mana-store` — storage traits
- `mana-file-store` — local-first implementation
- `mana-sql-store` — hosted implementation
- `mana-api` — stable service-level operations
- `mana-scheduler` — ready queue / run plan / review queue logic
- `mana-review` — skeptic/review/risk model
- `mana-runner-proto` — typed local/remote runner requests
- `mana-runner-local` — local process/worktree/sandbox adapter
- `mana-cli` — user-facing command layer

You do not have to move to this exact shape immediately, but this is the direction I would target.

---

## Suggested epics and decomposition

The following epics are the right top-level organizing structure for the rebuild.

### Epic A: protocol-first core

Goal:
- define the stable `mana ↔ imp ↔ runner` contracts

Suggested child jobs:
- inventory current cross-boundary types
- design `mana-contracts` crate
- define assignment / result / artifact / review / lease types
- migrate one narrow path to the new contracts first

### Epic B: storage abstraction and dual backend

Goal:
- make file backend and hosted backend both possible without changing domain semantics

Suggested child jobs:
- define store traits
- implement file backend adapter on top of current layout
- split Markdown contract from mutable state storage
- stub SQL backend and schema
- write parity tests across backends

### Epic C: evidence and artifact model

Goal:
- upgrade verification, review, and provenance into first-class durable artifacts

Suggested child jobs:
- define artifact taxonomy
- define verifier result records
- define skeptic review records
- define approval records
- define artifact references from units and runs
- add artifact bundle rendering for humans

### Epic D: runner protocol and local runner

Goal:
- replace shell-template orchestration as the long-term core

Suggested child jobs:
- define spawn / finish protocol
- define execution lease and worktree lease models
- implement local runner adapter
- connect `mana run` through the adapter
- keep shell templates only as a compatibility layer

### Epic E: review and promotion as explicit workflow stages

Goal:
- make review and promotion first-class gates, not side effects

Suggested child jobs:
- define review states and transitions
- define promotion states and transitions
- integrate `mana-review` with artifact model
- add approval hooks / records
- add CLI/UI views for review queues and evidence packets

### Epic F: migration and compatibility

Goal:
- transition current users without breaking everything at once

Suggested child jobs:
- define v1 ↔ v2 compatibility guarantees
- write migration tools for unit/state layout
- preserve existing CLI flows where practical
- document migration path for local projects
- add golden tests for old-format behavior where necessary

---

## Recommended first wave of implementation

If you want the fastest path to meaningful progress, start here:

1. **contracts crate**
2. **store trait layer**
3. **artifact model**
4. **local runner protocol**
5. **review stage integration**

That sequence gives you the biggest architectural improvement without forcing a full rewrite first.

---

## What success looks like

The rebuild is successful when:

- the core user model of `mana` still feels recognizably like `mana`
- `mana` can run on a file backend and a hosted backend with the same semantics
- `imp` consumes a stable typed contract instead of ad hoc worker-facing structs
- verification and review produce durable evidence bundles
- worktrees and runners are durable leased objects, not just helper logic
- shell-template orchestration is no longer the architectural center
- hosted multi-user mode becomes an additive backend choice, not a separate product rewrite

---

## Bottom line

`mana` is already pointing in the right direction.

The best move is not to start over.
The best move is to:

- preserve the domain model
- preserve the durable-substrate thesis
- replace the infrastructure seams that currently constrain the future

That is the path that best aligns `mana` with `agent_design.md` and with the likely future of both local-first and hosted agent workflows.
