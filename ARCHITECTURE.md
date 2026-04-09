# Architecture

> Last updated: 2026-04-09
> Manual edits welcome — recon preserves them and flags drift.

## Overview

**mana** is a coordination substrate for AI coding agents. Each task ("unit") has a verify gate — a shell command that must exit 0 to close. This enforces fail-first TDD: the verify command must fail before implementation, then pass after. No databases, no daemons — just `.mana/` markdown files you can `cat`, `grep`, and `git diff`.

One-sentence: **Markdown task files with dependency graphs and verification gates, orchestrated for parallel AI agents.**

## Tech Stack

- **Language:** Rust (edition 2021)
- **CLI framework:** clap 4 (derive macros)
- **Serialization:** serde + serde_json + serde_yml (serde_yaml 0.9 was deprecated; this repo migrated to serde_yml)
- **Error handling:** anyhow (Result + bail! + .context() throughout)
- **Terminal UI:** termimad (markdown rendering), dialoguer (interactive prompts)
- **Hashing:** sha2 (for content checksums)
- **Time:** chrono with serde feature
- **Storage:** Plain files in `.mana/` directory — YAML index + markdown unit files

## System Context

```
┌─────────────┐     ┌──────────────┐     ┌─────────────┐
│  Developer   │────▶│   mana CLI     │────▶│  .mana/    │
│  or Agent    │     │              │     │  (files)    │
└─────────────┘     └──────┬───────┘     └─────────────┘
                           │
                    ┌──────┴───────┐
                    │  mana run      │──── spawns ────▶ Agent processes (pi, claude, etc.)
                    └──────────────┘
                           │
                    ┌──────┴───────┐
                    │  MCP server  │◀─── stdio ────── IDE (Cursor, Claude Desktop, etc.)
                    └──────────────┘
```

**External systems this touches:**
- **git** — worktree operations for agent sandboxing, change history
- **Shell** — verify commands, agent process spawning
- **pi / claude CLI** — agent processes spawned by `mana run`
- **IDE MCP clients** — Cursor, Claude Desktop, Windsurf via JSON-RPC 2.0 over stdio

## Building Blocks

```
src/
├── main.rs              — CLI entry point, command dispatch
├── cli.rs               — clap definitions (35+ subcommands)
├── lib.rs               — Module declarations (20 modules)
├── unit.rs              — Core Unit type, Status, RunRecord, verification history
├── index.rs             — Index (YAML cache of all unit metadata)
├── config.rs            — Config (.mana/config.yaml parsing, inheritance via extends)
├── discovery.rs         — Find .mana/ dir, locate unit files by ID
├── graph.rs             — Dependency graph, cycle detection, topological sort
├── commands/
│   ├── mod.rs           — Command module declarations
│   ├── create.rs        — Unit creation with slug generation
│   ├── close.rs         — Verification + close logic (largest command, 3330L)
│   ├── run/
│   │   ├── mod.rs       — Agent orchestration entry point (cmd_run, spawn modes)
│   │   ├── plan.rs      — Dispatch planning (SizedUnit, waves, priority)
│   │   ├── ready_queue.rs — Ready-queue executor (direct mode, dep-aware dispatch)
│   │   └── wave.rs      — Wave-based executor (template mode, legacy)
│   ├── plan.rs          — Unit decomposition planning
│   ├── show.rs          — Unit display with markdown rendering
│   ├── edit.rs          — Interactive unit editing ($EDITOR)
│   ├── update.rs        — Field-level unit updates
│   ├── init.rs          — Project initialization with agent presets
│   ├── list.rs          — Filtered listing with status/label/assignee
│   ├── claim.rs         — Unit claiming (locks for agents)
│   ├── quick.rs         — Create + claim in one step
│   ├── context.rs       — Assemble file context from unit descriptions
│   ├── agents.rs        — Monitor running agents
│   ├── status.rs        — Project overview (claimed/ready/blocked)
│   ├── ready.rs         — Show unblocked units
│   ├── verify.rs        — Run verify without closing
│   ├── dep.rs           — Dependency management (add/remove/list)
│   ├── tidy.rs          — Clean up stale data
│   ├── tree.rs          — Hierarchical unit display
│   ├── graph.rs         — DOT/Mermaid dependency visualization
│   ├── logs.rs          — Agent log viewer
│   ├── doctor.rs        — Health checks
│   ├── sync.rs          — Index rebuild from files
│   ├── adopt.rs         — Reparent units
│   ├── trust.rs         — Trust management for verify commands
│   ├── recall.rs        — Memory recall
│   ├── memory_context.rs — Memory context assembly
│   ├── fact.rs          — Fact storage
│   ├── stats.rs         — Project statistics
│   ├── config_cmd.rs    — Config CLI (get/set)
│   ├── reopen.rs        — Reopen closed units
│   ├── delete.rs        — Unit deletion with cleanup
│   ├── unarchive.rs     — Restore archived units
│   ├── stdin.rs         — Pipe input handling
│   └── interactive.rs   — Interactive unit creation (dialoguer)
├── mcp/
│   ├── mod.rs           — MCP module
│   ├── server.rs        — JSON-RPC 2.0 stdio server loop
│   ├── protocol.rs      — Request/response types
│   ├── tools.rs         — Tool definitions (create, close, list, etc.)
│   └── resources.rs     — Resource definitions (unit content)
├── api/
│   └── mod.rs           — Library API (programmatic access, re-exports core types)
├── spawner.rs           — Agent process lifecycle (spawn, track, log, cleanup)
├── stream.rs            — JSON streaming events for mana run --json-stream
├── pi_output.rs         — Parse pi agent output (events, tokens, costs)
├── ctx_assembler.rs     — Extract file paths from descriptions, assemble context
├── relevance.rs         — File relevance scoring for context assembly
├── hooks.rs             — Post-close and on-fail hook execution
├── agent_presets.rs     — Detect and configure agents (pi, claude, aider, etc.)
├── worktree.rs          — Git worktree isolation for parallel agents
├── timeout.rs           — Agent timeout monitoring
├── tokens.rs            — Token counting for context budgets
├── project.rs           — Project type detection (Rust, Node, Python, etc.)
└── util.rs              — Shared utilities (ID validation, natural sort, slugs)

tests/
├── cli_tests.rs         — Integration tests (5 test functions)
├── test_ctx_assembler.rs — Context assembler unit tests (22 tests)
├── adopt_test.rs        — Adopt command tests (10 tests)
├── api_test.rs          — Library API tests
└── mcp_test.rs          — MCP protocol tests

docs/
├── SKILL.md             — Agent skill definition for units
├── BEST_PRACTICES.md    — Guide for creating effective units
├── fail-then-pass-design.md — Design doc for fail-first verification
└── design/
    └── CONFLICT_RESOLUTION.md — Design doc for merge conflicts
```

### Internal Dependency Flow

```
main.rs ──▶ cli.rs (parse) ──▶ commands/*.rs (execute)
                                     │
                                     ▼
                              ┌─────────────┐
                              │  unit.rs     │ ◀── Core types
                              │  index.rs    │ ◀── Metadata cache
                              │  config.rs   │ ◀── Project settings
                              │  discovery.rs│ ◀── File location
                              └─────────────┘
                                     │
                              ┌──────┴──────┐
                              │  graph.rs   │ ◀── Dependency resolution
                              │  spawner.rs │ ◀── Agent process management
                              │  hooks.rs   │ ◀── Event-driven actions
                              │  worktree.rs│ ◀── Git isolation
                              └─────────────┘
```

**Load-bearing modules** (high fan-in — most commands import these):
- `unit.rs` — Unit, Status
- `index.rs` — Index, IndexEntry
- `discovery.rs` — find_mana_dir, find_unit_file
- `config.rs` — Config
- `util.rs` — validate_unit_id, natural_cmp, title_to_slug

## Data Model

**No database.** All state lives in `.mana/` directory as plain files.

| File | Format | Purpose |
|------|--------|---------|
| `.mana/config.yaml` | YAML | Project settings, agent templates, inheritance |
| `.mana/index.yaml` | YAML | Fast lookup cache of all unit metadata |
| `.mana/{id}-{slug}.md` | Markdown with YAML frontmatter | Individual unit definitions |
| `.mana/archive/` | Same as above | Closed/archived units |

**Unit file structure** (markdown format):
- YAML frontmatter: id, title, status, priority, parent, dependencies, verify, produces, requires, labels, assignee, claimed_by, attempts, on_fail, on_close, created_at, updated_at
- Markdown body: description, acceptance criteria, context

**Index is a cache** — can be rebuilt from unit files via `mana sync`.

**Key relationships:**
- Mana form a tree (parent/child via `parent` field)
- Mana form a DAG (dependencies via `dependencies` field)
- Mana can declare `produces`/`requires` for artifact-based dependency inference

## Development

### Prerequisites
- Rust toolchain (edition 2021)
- git (for worktree features)

### Commands
| Action | Command |
|--------|---------|
| Build | `cargo build` |
| Build release | `cargo build --release` |
| Test | `cargo test` |
| Install from source | `cargo install --path .` |
| Install from git | `cargo install --git https://github.com/kfcafe/mana` |

### CI workflows
GitHub Actions workflows are configured.

From the files inspected in this repo:
- Tower root: `.github/workflows/ci.yml` runs `cargo check --workspace`, targeted `cargo test -p ...` for the active mana/imp crates, `cargo clippy --workspace -- -D warnings`, and `cargo fmt --check`.
- mana-local: `mana/.github/workflows/ci.yml` runs `cargo check`, `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`, and an MSRV `cargo check` on Rust 1.85.

The inspected workflows do **not** yet include dedicated dependency or security audit jobs (for example `cargo-audit`, OSV scanning, or secrets scanning).

## Conventions & Patterns

- **Error handling:** `anyhow::Result` everywhere, `.context()` for error chains, `bail!` for early returns
- **CLI structure:** One file per command in `src/commands/`, each exports a `cmd_*` function
- **Serialization:** serde derive on all types, `#[serde(skip_serializing_if)]` for optional fields
- **File naming:** Unit files use `{id}-{slug}.md` format (legacy: `{id}.yaml`)
- **ID validation:** All unit IDs validated via `util::validate_unit_id()` to prevent path traversal
- **Sorting:** Natural sort (`util::natural_cmp`) for unit IDs (1, 2, 10 not 1, 10, 2)
- **Testing:** Heavy inline `#[cfg(test)]` modules — 891+ tests, mostly unit tests inside source files
- **No async:** Entire codebase is synchronous (no tokio/async-std)
- **Module exports:** `lib.rs` re-exports all modules as `pub mod`, commands behind `commands::mod.rs`

## Health & Risks

### Hotspots (churn × size)

| Score | Churn | Size | File | Notes |
|-------|-------|------|------|-------|
| 60,214 | 22× | 3,330L | `commands/close.rs` | Largest command — verification + fail-first + hooks |
| 56,753 | 29× | 1,961L | `commands/create.rs` | Most-changed command |
| 32,460 | 20× | 1,631L | `unit.rs` | Core type — changes ripple everywhere |
| — | — | 2,327L | `commands/run/` | Agent orchestration (split into 4 files) |
| 27,520 | 40× | 709L | `main.rs` | High churn from command dispatch growth |
| 26,336 | 32× | 1,052L | `cli.rs` | Grows with every new subcommand |

### Temporal Coupling (files that change together)

| Co-commits | Pair | Why |
|-----------|------|-----|
| 15 | `cli.rs` ↔ `main.rs` | Every new command touches both |
| 9 | `commands/mod.rs` ↔ `main.rs` | Command registration |
| 7 | `cli.rs` ↔ `commands/mod.rs` | Command definition chain |

This is expected — adding a command requires cli.rs (args) + mod.rs (module) + main.rs (dispatch).

### Test Coverage

- **891+ tests** across source files and 5 test files
- Heavy inline testing (`#[cfg(test)]` modules in most source files)
- `close.rs` has the most tests (89) — appropriate given its complexity
- `ctx_assembler.rs` (49 inline + 22 in test file) and `unit.rs` (53) well tested
- `util.rs` has 54 tests — good coverage of shared utilities
- MCP tool handlers covered by `tests/mcp_test.rs` (38 tests)
- **Zero-test files:** `status.rs`, `verify.rs`, `interactive.rs`, `locks.rs` (command-level)

### Notable Gaps
- **CI exists, but coverage is still limited** — the inspected root and mana-local GitHub Actions workflows cover build/test/lint/format (and mana-local MSRV), but they do not yet include dedicated dependency or security audit jobs.
- **serde_yml is pre-1.0** — currently pinned to 0.0.12. Monitor for security advisories and consider migrating to a stable, maintained YAML crate when available.
- **No dependency/security audit step in the inspected CI workflows** — no `cargo-audit`, OSV, or secrets-scanning job was present in the workflows inspected for this update.
- **Public API coverage is broad but still uneven for embedding** — `mana-core::api` already exposes discovery, core queries, graph helpers, unit mutations, claim/release, dependency edits, ready-queue computation, context assembly, verify helpers, and fact operations. The remaining gaps are narrower: some operations still live only in lower-level `ops::*` modules rather than stable top-level `api::*` wrappers (for example `init`, `sync`, `adopt`, `unarchive`, `dep_list`, `batch_verify`, `config_get`/`config_set`, and move operations), and full agent-spawn orchestration is not exposed here as a single stable library entry point.
- **MCP tools duplicate CLI logic** — close, claim, auto-close-parent are reimplemented in `mcp/tools.rs` rather than sharing with `commands/close.rs`, causing behavioral divergence

### In-Progress Work (from .mana/)
- Unit 78: Worktree isolation for parallel agents
- Unit 79: Gitignore index.yaml — treat as local cache
- Unit 80: Identity model (mana config set user)
- Unit 84: Codebase improvements (parent, needs decomposition)
- Mana 89-92: Vibecheck improvement batches (duplicated across 4 runs)
- Mana 94-96: Split large files (close.rs, unit.rs, create.rs) into module directories
