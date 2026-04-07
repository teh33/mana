# Embedding-Oriented Mana Run API

Status: draft  
Parent feature: root `.mana/.4.2`  
Related units: `.4.2.1`, `.4.2.3`, `mana/.mana/237`, `mana/.mana/240.4`

---

## 1. Problem

`mana` already exposes useful programmatic orchestration entry points:
- `run_with_stream_capture`
- `run_with_stream_capture_and_sink`
- `RunView`
- `StreamEvent`
- `compute_ready_queue`

But the current run contract is still shaped primarily around the CLI:
- run inputs are represented by `RunArgs`, which mirror command-line flags
- target selection is still essentially `Option<id>`
- the embedding path is an adaptation of CLI execution rather than a first-class API shape
- consumers must infer some orchestration meaning from CLI-oriented concepts rather than from explicit embedding-oriented types

This is workable, but it is not yet the cleanest substrate for native consumers like `imp` and later `wizard`.

---

## 2. Goals

1. Keep one canonical run engine in `mana`.
2. Provide a clearer embedding-oriented launch contract.
3. Preserve the existing CLI experience and compatibility where practical.
4. Make target selection semantics explicit and reusable.
5. Return structured run data suitable for native consumers without scraping CLI text.

---

## 3. Non-goals

1. Do **not** move consumer UI/session concerns into `mana`.
2. Do **not** force the CLI to adopt an unnatural API-only surface.
3. Do **not** add `imp`-specific persistence or follow-up behavior to `mana`.
4. Do **not** create parallel run semantics for CLI vs native consumers.

---

## 4. Current state

## 4.1 Strengths already present

Current foundations are good:
- `plan_dispatch` already centralizes dispatch planning
- `StreamEvent` already provides a structured event model
- `RunView` already provides a final summary + per-unit view + captured events
- `run_with_stream_capture_and_sink` already supports in-process event consumption

That means `.4.2.2` is not about inventing orchestration from scratch. It is about making the API shape more explicit for embedders.

## 4.2 Current API smell

The main smell is not capability but shape.

Today, a native consumer calls a CLI-shaped function using CLI-shaped args:

- `RunArgs { id: Option<String>, jobs, dry_run, loop_mode, ... }`

This leaves several things too implicit:
- what exactly is being targeted
- which fields are canonical orchestration semantics vs CLI conveniences
- which results/events should embedders treat as stable contract

---

## 5. Proposed design

## 5.1 Add an embedding-oriented target type

Introduce an explicit target-selection type for programmatic run calls.

```rust
pub enum RunTarget {
    /// Run all currently ready executable work.
    AllReady,

    /// Run one explicit unit or subtree using canonical target semantics.
    Unit(String),

    /// Run an explicit set of targets, if canonical semantics support this.
    Explicit(Vec<String>),
}
```

Rationale:
- replaces overloading `Option<id>` with explicit meaning
- gives native consumers a type they can reason about directly
- provides a canonical place for multi-target support if accepted by `.4.2.1`

Notes:
- if explicit multi-target support is not accepted canonically, `Explicit` can be deferred
- `Unit(String)` must continue to rely on canonical subtree semantics from `.4.2.1`

---

## 5.2 Add an embedding-oriented run params type

Introduce a programmatic params type distinct from CLI `RunArgs`.

```rust
pub struct NativeRunParams {
    pub target: RunTarget,
    pub jobs: u32,
    pub dry_run: bool,
    pub loop_mode: bool,
    pub keep_going: bool,
    pub timeout_minutes: u32,
    pub idle_timeout_minutes: u32,
    pub review: bool,
}
```

Rationale:
- separates embedding semantics from clap/CLI mirroring
- makes the contract readable in native consumers
- clarifies what the canonical run engine actually accepts

Important rule:
- this type should only include canonical orchestration semantics
- consumer-specific presentation/session concerns stay out

---

## 5.3 Keep CLI `RunArgs`, but translate it

The CLI can remain backwards-compatible.

Suggested structure:
- CLI parses into current `RunArgs`
- a translation layer converts `RunArgs` into `NativeRunParams`
- the canonical runner consumes `NativeRunParams`

This keeps:
- CLI compatibility
- a clean embedding-oriented core
- one canonical execution path

---

## 5.4 Add an embedding-oriented entry point

Introduce a stable embedding-facing function that takes `NativeRunParams`.

Possible shape:

```rust
pub fn run_native(
    mana_dir: &Path,
    params: NativeRunParams,
    sink: Option<stream::StreamSink>,
) -> Result<RunView>
```

or, if sink/no-sink variants are preferred:

```rust
pub fn run_native(mana_dir: &Path, params: NativeRunParams) -> Result<RunView>
pub fn run_native_with_sink(
    mana_dir: &Path,
    params: NativeRunParams,
    sink: stream::StreamSink,
) -> Result<RunView>
```

Rationale:
- gives native consumers a function that is explicitly for them
- avoids making embedders depend on CLI naming forever
- still reuses the same internal engine and event model

---

## 5.5 Preserve `RunView` as the final result shape, but clarify its contract

`RunView` is already close to what embedders need:
- `summary`
- `units`
- `events`

For `.4.2.2`, the work is less about replacing `RunView` and more about clarifying which parts are stable embedding contract.

Stable contract candidates:
- `summary` counts
- per-unit final status/outcome fields
- ordered event list as captured history

Possible additions if needed:
- explicit target/scope echo in the final result
- explicit effective runtime config block if `.4.2.3` adds it canonically

---

## 5.6 Event contract remains canonical

`StreamEvent` should remain the canonical live-update channel.

`.4.2.2` should not create a second event model. Instead, it should:
- keep using `StreamEvent`
- clarify that embedders may depend on these event categories as part of the run contract
- let `.4.2.3` extend events with effective runtime config if needed

---

## 6. Where types should live

## 6.1 Immediate stable home: `mana::api`

The embedding-oriented run contract should be exposed immediately through the
public `mana::api` surface of the `mana` crate.

Why:
- the orchestration engine already lives in the `mana` crate today
- current programmatic run helpers already live alongside that engine
- `imp` already depends on `mana` for native run behavior
- this avoids blocking useful contract cleanup on a larger crate-boundary refactor

So the recommended immediate home is:
- public embedding-oriented run types and functions exposed from `mana::api`
- implementation staying in current `mana-cli` run modules as needed internally

## 6.2 Long-term possible move

Longer-term, orchestration logic may migrate into a more substrate-oriented
library location if that becomes valuable. If that happens, the embedding
contract should migrate behind the same public semantics, not change meaning.

Practical rule:
- expose the stable contract from `mana::api` now
- do **not** block this work on relocating orchestration code into `mana-core`
- if a later refactor moves the engine, preserve the same embedding-facing
  types and semantics

---

## 7. Migration plan

### Phase A
- introduce `RunTarget`
- introduce `NativeRunParams`
- add translation from CLI `RunArgs`
- keep existing CLI behavior unchanged

### Phase B
- add `run_native` / `run_native_with_sink`
- reimplement existing `run_with_stream_capture*` helpers on top of the new path or rename carefully

### Phase C
- update native consumers like `imp` to use the embedding-oriented types/function

### Phase D
- if desirable, lift the stable API surface into `mana-core::api`

---

## 8. Decisions

### 8.1 Explicit multi-target support should be canonical

Recommendation: **yes**.

`RunTarget` should include:

```rust
Explicit(Vec<String>)
```

Semantics:
- each explicit target is interpreted using canonical target semantics
  (`Unit(id)` meaning leaf-or-subtree according to `.4.2.1`)
- the dispatch set is the union of those canonical target scopes
- duplicate units are deduplicated
- dependency and ready-queue behavior remain canonical for the merged set

Why this should be canonical:
- Pi already proves the workflow value
- `imp` and later `wizard` should not each reinvent subset selection semantics
- target-set dispatch is orchestration meaning, not presentation

### 8.2 The stable embedding surface should live in `mana::api` now

Recommendation: **yes**.

Expose the embedding-facing contract from `mana::api` immediately, even if the
implementation still delegates into existing run modules.

Why:
- this gives native consumers a stable public home now
- it avoids cementing `commands::run::*` as the de facto consumer contract
- it keeps a future internal relocation possible without forcing consumer churn

### 8.3 Effective runtime configuration should become part of final run data

Recommendation: **yes**, via `.4.2.3`.

`RunView` should gain a structured runtime/config block once `.4.2.3` lands,
so native consumers can trust what worker/model/runtime settings were actually
used.

That same information should be available for dry-run/plan visibility where
practical.

---

## 9. Recommendation

For `.4.2.2`, the minimum good implementation is:

1. add `RunTarget` including canonical explicit-target-set support
2. add `NativeRunParams`
3. expose a native embedding-oriented run function from `mana::api`
4. keep `RunView` and `StreamEvent` as the result/event contract
5. translate CLI `RunArgs` into the new params type
6. let `.4.2.3` extend `RunView` / dry-run data with effective runtime config

That gives `imp` a clean canonical launch surface without requiring a big refactor.

---

## 10. One-sentence summary

`mana` should keep one canonical run engine, but native consumers should call it through explicit embedding-oriented types rather than through a permanently CLI-shaped `RunArgs` contract.
