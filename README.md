# mana

[![CI](https://github.com/kfcafe/mana/actions/workflows/ci.yml/badge.svg)](https://github.com/kfcafe/mana/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/mana-cli)](https://crates.io/crates/mana-cli)
[![License](https://img.shields.io/badge/license-AGPL--3.0-blue)](LICENSE-AGPL)
[![dependency status](https://deps.rs/repo/github/kfcafe/mana/status.svg)](https://deps.rs/repo/github/kfcafe/mana)

Mana is the medium for coding agent work: it gives that work structure, verification, dependencies, and memory.

It turns work that would normally disappear into prompts and scrollback into durable units that can be verified, retried, decomposed, and built on over time.

Plain Markdown files in `.mana/`. Any agent that can read files and run shell commands is fluent in mana.

```bash
mana init --agent claude
mana create "Add CSV export" --verify "cargo test csv::export"
mana run
```

## Contents

- [Why mana exists](#why-mana-exists)
- [The core model](#the-core-model)
- [Install](#install)
- [Quick start](#quick-start)
- [How mana works](#how-mana-works)
- [Working with agents](#working-with-agents)
- [Fail-first development](#fail-first-development)
- [Decomposition and dependencies](#decomposition-and-dependencies)
- [Facts and memory](#facts-and-memory)
- [Command reference](#command-reference)
- [Configuration](#configuration)
- [Documentation](#documentation)

## Why mana exists

Coding agents are powerful, but their work is usually ephemeral.

- The plan lives in a prompt
- "Done" is often vague
- Retries start cold
- Dependencies stay implicit
- Useful lessons vanish into logs

Mana gives that work a durable shape.

A unit says **what to do**. A verify gate says **how to know it's done**. Dependencies say **what it relies on**. Attempts, notes, and facts record **what was learned**.

Agents come and go. Mana keeps the work legible.

## The core model

Mana tracks three main kinds of work records:

### Job
A **job** is one executable piece of work.

It has:
- an ID
- a title
- a status
- a verify command
- optional description, acceptance criteria, paths, labels, and notes

Jobs are what `mana run` dispatches to agents.

### Epic
An **epic** is a parent/grouping record for larger work.

Epics help you:
- decompose larger efforts into child jobs
- track progress across a subtree
- keep structure without dispatching an intermediate parent as if it were executable work

An epic may still have rich description and acceptance context, but it is not the default target for agent execution. Break epics into child jobs.

### Fact
A **fact** is verified project knowledge.

Facts are not implementation work. They capture durable knowledge with a verify command so agents can reuse what was learned.

### Verify gate
Every executable job has a **verify gate**: a shell command that must exit `0` before the job can close.

This makes "done" machine-checkable.

### Graph
Units form a graph through:
- **parent / child** relationships for decomposition
- **dependencies** for ordering
- **produces / requires** for artifact-based coordination

### Memory
Mana keeps context with the work itself:
- failed attempts
- notes
- verified facts
- related files
- dependency context

### Medium
Everything lives in `.mana/` as plain files.

That means the system is:
- local-first
- git-friendly
- inspectable
- agent-agnostic
- resilient when any one tool or process dies

## Install

```bash
cargo install mana-cli
```

<details>
<summary>Build from source</summary>

```bash
git clone https://github.com/kfcafe/mana
cd mana
cargo build --release
cp target/release/mana ~/.local/bin/
```

</details>

## Quick start

Initialize a project:

```bash
mana init --agent claude
```

Create a job:

```bash
mana create "Fix CSV export" --verify "cargo test csv"
```

Create another job:

```bash
mana create "Add pagination" --verify "cargo test page"
```

Dispatch ready work to your agent:

```bash
mana run
```

Watch what happens:

```bash
mana agents
mana logs 3
mana status
```

Manual workflow:

```bash
mana quick "Fix CSV export" --verify "cargo test csv"
mana verify 1
mana close 1
```

## How mana works

Work lives in Markdown records inside `.mana/`:

```text
.mana/
├── 1-add-csv-export.md
├── 2-add-tests.md
├── 2.1-unit-tests.md
└── archive/2026/01/
```

A job looks like this:

```yaml
---
id: "1"
title: Add CSV export
status: in_progress
verify: cargo test csv::export
attempts: 0
---

Add a `--format csv` flag to the export command.

**Files:** src/export.rs, tests/export_test.rs
```

When you run `mana close 1` on a job:

1. Mana runs the verify command
2. Exit `0` → the unit closes and archives
3. Non-zero exit → the unit stays open and the failure is recorded for the next attempt

That simple loop is the foundation:

**define → attempt → verify → learn → retry or close**

## Working with agents

Configure an agent once, then dispatch ready jobs to it:

```bash
mana init --agent claude
```

Or set the run template directly:

```bash
mana config set run "claude -p 'read unit {id}, implement it, then run mana close {id}'"
```

`{id}` is replaced with the unit ID.

### Dispatching

```bash
mana run                    # Dispatch all ready units
mana run 3                  # Dispatch a specific unit
mana run -j 8               # Up to 8 parallel agents
mana run --loop-mode        # Keep dispatching until work is done
mana run --review           # Adversarial review after each close
mana run --dry-run          # Preview dispatch plan
```

### Batch verify

When parallel agents share the same verify command (e.g. `cargo build`), each agent running it independently causes lock contention and redundant work. Enable batch verify to run each unique command once:

```bash
mana config set batch_verify true
```

With batch verify enabled:
1. Agents skip verify and exit after calling `mana close`
2. The runner collects all completed units
3. Groups them by verify command and runs each command once
4. Passing units close normally; failing units reopen for retry

### Monitoring

```bash
mana agents                 # Show running/completed agents
mana logs 3                 # View agent output for unit 3
mana status                 # See what's ready, blocked, or in flight
```

### Agent context

`mana context <id>` produces a complete briefing for an agent about a job or epic:

1. unit spec
2. previous attempts
3. project rules
4. dependency context
5. file structure
6. relevant file contents

```bash
mana context 5
mana context 5 --structure-only
mana context 5 --json
mana context                 # No ID: project-wide memory context
```

This is how agents stay grounded in the work instead of re-deriving context from scratch.

## Fail-first development

Mana defaults to **fail-first**.

Before a unit is created, the verify command runs and must fail.

- If it already passes, the unit is rejected
- If it fails, the unit is accepted
- Later, `mana close` runs the same verify command and it must pass

```bash
# Rejected: verify already passes
mana quick "..." --verify "python -c 'assert True'"

# Accepted: real failing check
mana quick "..." --verify "pytest test_unicode.py"
```

Use `--pass-ok` (`-p`) when fail-first is not appropriate, like refactors or safety checks:

```bash
mana quick "extract helper" --verify "cargo test" -p
mana quick "remove secrets" --verify "! grep 'api_key' src/" -p
```

## Decomposition and dependencies

### Parent / child units

Break large work into smaller units:

```bash
mana create "Search feature" --verify "make test-search"
mana create "Index builder" --parent 1 --verify "cargo test index::build"
mana create "Query parser" --parent 1 --verify "cargo test query::parse"

mana tree 1
```

Parents can auto-close when all children close.

### Dependencies

Coordinate units explicitly:

```bash
mana create "Define schema types" --parent 1 \
  --produces "Schema,FieldType" \
  --verify "cargo test schema::types"

mana create "Build query engine" --parent 1 \
  --requires "Schema" \
  --verify "cargo test query::engine"
```

Mana blocks work until its dependencies are satisfied.

### Sequential chaining

```bash
mana create "Step 1: scaffold" --verify "cargo build"
mana create next "Step 2: implement" --verify "cargo test"
mana create next "Step 3: docs" --verify "grep -q 'API' README.md"
```

Each `next` unit automatically depends on the previous one.

### Planning

```bash
mana plan 3
mana plan --auto
mana plan --dry-run
```

Use planning when a unit is too large or underspecified for a single attempt.

## Facts and memory

Mana can also store verified project facts.

```bash
mana fact "DB is PostgreSQL" --verify "grep -q 'postgres' docker-compose.yml" -p
mana fact "Tests require Docker" --verify "docker info >/dev/null 2>&1" --ttl 90
mana verify-facts
mana recall "database"
```

Facts appear in context and can go stale if their verification expires or fails.

This gives agents a durable memory that lives outside chat history.

## Command reference

### Unit lifecycle

```bash
mana create "title" --verify "cmd"      # Create a unit
mana create "title" -p                  # Skip fail-first
mana create next "title" --verify "cmd"
mana quick "title" --verify "cmd"       # Create + claim
mana claim <id>
mana verify <id>
mana close <id>
mana close --defer-verify <id>
mana close --failed <id>
mana update <id>
mana edit <id>
mana delete <id>
mana reopen <id>
mana unarchive <id>
```

### Orchestration

```bash
mana run [id] [-j N]
mana run --loop-mode
mana run --review
mana plan <id>
mana review <id>
mana agents
mana logs <id>
```

### Inspecting the graph

```bash
mana status
mana show <id>
mana list
mana tree [id]
mana graph
mana trace <id>
mana context [id]
mana recall "query"
```

### Dependencies and memory

```bash
mana dep add <id> <dep-id>
mana dep remove <id> <dep-id>
mana fact "title" --verify "cmd"
mana verify-facts
```

### Maintenance and integration

```bash
mana tidy
mana doctor              # graph/index health + stale config checks
mana doctor fix          # apply safe, deterministic fixes
mana sync
mana locks [--clear]
mana config get <key>
mana config get-project <key>
mana config get-global <key>
mana config set <key> <value>
mana config set-project <key> <value>
mana config set-global <key> <value>
mana mcp serve
mana completions <shell>
```

### Pipe-friendly usage

```bash
mana create "fix parser" --verify "cargo test" -p --json | jq -r '.id'
mana list --json | jq '.[] | select(.priority == 0)'
mana list --ids | mana close --stdin --force
cat spec.md | mana create "task" --description - --verify "cmd"
mana list --format '{id}\t{status}\t{title}'
```

## Configuration

Project configuration lives in `.mana/config.yaml`.

```bash
mana config set-project run "claude -p 'read unit {id}, implement it, then run mana close {id}'"
mana config set-project plan "claude -p 'read unit {id} and split it into subtasks'"
mana config set-global run "imp --model {model} run {id}"
mana config set-global run_model gpt-5.4
mana config set-global plan_model gpt-5.4
mana config set-project max_concurrent 4
mana config get run_model
mana config get-project run
mana config get-global run
mana config inspect
mana config inspect run_model
mana config doctor
```

Model settings let you pick different defaults for different kinds of agent work:
- `mana config get` returns the effective merged value
- `mana config get-project` and `mana config get-global` show raw scoped values
- `mana config inspect` shows effective values and whether they come from project config, global config, or built-in defaults
- `mana config doctor` focuses just on config drift and inheritance issues
- `mana doctor` now includes those config checks alongside graph/index health
- `mana doctor fix` applies safe, deterministic fixes like index rebuilds

- `run_model` powers `mana run`
- `plan_model` powers `mana plan`
- `review_model` powers AI review flows
- `research_model` powers project-level research/planning

| Key | Default | Description |
|-----|---------|-------------|
| `run` | — | Command template for agent dispatch. `{id}` = unit ID. |
| `plan` | — | Command template to split large units. |
| `run_model` | — | Default model for `mana run`. |
| `plan_model` | — | Default model for `mana plan`. |
| `review_model` | — | Default model for AI review flows. |
| `research_model` | — | Default model for project-level research/planning. |
| `max_concurrent` | `4` | Max parallel agents. |
| `max_loops` | `10` | Max agent loops before stopping (`0` = unlimited). |
| `poll_interval` | `30` | Seconds between loop mode cycles. |
| `auto_close_parent` | `true` | Close parent when all children close. |
| `verify_timeout` | — | Default verify timeout in seconds. |
| `rules_file` | — | Path to rules file injected into `mana context`. |
| `file_locking` | `false` | Lock unit `paths` files during concurrent work. |
| `extends` | `[]` | Parent config files to inherit from. |
| `batch_verify` | `false` | Batch shared verify commands: run each once after agents complete. |
| `auto_commit` | `false` | Commit all changes on close. Skipped in worktree mode. |
| `commit_template` | `feat(mana-{id}): {title}` | Template for auto-commit messages. Vars: `{id}`, `{title}`, `{parent_id}`, `{labels}`. |
| `on_close` | — | Hook after close. Vars: `{id}`, `{title}`, `{status}`, `{branch}`. |
| `on_fail` | — | Hook after verify failure. Vars: `{id}`, `{title}`, `{attempt}`, `{output}`, `{branch}`. |
| `review.run` | — | Review agent command. Falls back to `run`. |
| `review.max_reopens` | `2` | Max review reopen cycles. |

### Config inheritance

```yaml
# .mana/config.yaml
extends:
  - ~/.mana/global-config.yaml
project: my-app
run: "claude -p 'read unit {id}, implement it, then run mana close {id}'"
```

Child values override parent. Multiple parents are applied in order; later values win.

Use `extends` for shared non-secret defaults such as agent command templates or concurrency settings. Do not use it as a secret-distribution mechanism.

## Documentation

- [Agent Skill](docs/SKILL.md) — Quick reference for AI agents
- [Best Practices](docs/BEST_PRACTICES.md) — Writing effective units for agents
- `mana --help` — Full command reference

## Contributing

Contributions welcome. Fork the repo, create a branch, and open a pull request.

## License

[AGPL-3.0](LICENSE-AGPL) (CLI) / [Apache-2.0](LICENSE-APACHE) (core library)
