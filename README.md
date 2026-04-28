# mana

[![CI](https://github.com/kfcafe/mana/actions/workflows/ci.yml/badge.svg)](https://github.com/kfcafe/mana/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/mana-cli)](https://crates.io/crates/mana-cli)
[![dependency status](https://deps.rs/repo/github/kfcafe/mana/status.svg)](https://deps.rs/repo/github/kfcafe/mana)

Mana is a local-first work coordination system for coding agents. It turns agent work into durable Markdown records with explicit scope, dependencies, verification gates, attempts, notes, and facts.

Instead of losing plans and failures in chat scrollback, mana keeps work legible enough for another agent, or a human, to pick up cold.

```bash
mana init
mana create "Add CSV export" --verify "cargo test csv::export"
imp run <unit-id>
```

## Why mana

Coding agents are effective, but their default working medium is fragile:

- plans live in prompts or monolithic md files
- “done” is ambiguous
- retries restart without context
- dependencies stay implicit
- useful failures disappear into logs

Mana gives agent work a durable shape:

- **units** describe the work
- **verify gates** define completion
- **dependencies** encode order
- **attempts and notes** preserve execution history
- **facts** capture verified project memory

Everything is stored in `.mana/` as plain files, so the system is inspectable, git-friendly, and usable by any agent that can read files and run shell commands.

## Core concepts

### Task

A task is one executable unit of work. It should have a concrete goal, useful context, relevant paths, and a verify command that exits `0` when the task is complete.

```bash
mana create "Fix login redirect" \
  --verify "cargo test auth::redirect" \
  --paths "crates/app/src/auth.rs,crates/app/tests/auth.rs"
```

### Epic

An epic is a non-dispatchable parent used to organize larger work. Use epics for features, migrations, audits, and refactors that need decomposition before execution.

```bash
mana create "Improve onboarding flow" --epic
mana create "Add empty-state copy" --parent 1 --verify "cargo test onboarding_empty_state"
```

### Fact

A fact is verified project knowledge. Facts are useful for durable architecture notes, environment requirements, and constraints that agents should not rediscover repeatedly.

```bash
mana fact "Tests require Docker" --verify "docker info >/dev/null 2>&1" --ttl 90
mana verify-facts
```

### Verify gate

A verify gate is a shell command attached to a task. `mana close <id>` runs the command before closing the task. If the command fails, the task stays open and the failure is recorded for the next attempt.

## Installation

```bash
cargo install mana-cli
```

Build from source:

```bash
git clone https://github.com/kfcafe/mana
cd mana
cargo build --release
cp target/release/mana ~/.local/bin/
```

## Quick start

Initialize mana in a project:

```bash
mana init
```

Create a task with a verify gate:

```bash
mana create "Fix CSV export" --verify "cargo test csv::export"
```

Inspect the queue:

```bash
mana status
mana next
mana show 1
```

Run the task with an agent runtime:

```bash
imp run 1
```

Verify and close manually:

```bash
mana verify 1
mana close 1
```

> [!TIP]
> Prefer targeted verify commands. `cargo test parser::handles_unicode` is usually a better task gate than a broad project-wide test suite.

## How it works

Mana stores records in `.mana/`:

```text
.mana/
├── config.yaml
├── index.yaml
├── 1-add-csv-export.md
├── 2-improve-errors.md
├── 2.1-add-error-type.md
└── archive/2026/04/
```

A task is a Markdown file with YAML frontmatter:

```yaml
---
id: "1"
title: Add CSV export
kind: task
status: open
verify: cargo test csv::export
paths:
  - crates/app/src/export.rs
  - crates/app/tests/export.rs
---

Add a `--format csv` option to the export command.

Acceptance:
- existing JSON export behavior is unchanged
- CSV output includes headers
- `cargo test csv::export` passes
```

The normal loop is:

1. define the work as a task
2. claim or dispatch it
3. attempt the implementation
4. run the verify gate
5. close on success, or record failure context and retry

## Working with agents

Mana is agent-agnostic. The recommended boundary is to let an agent runtime execute a single mana task:

```bash
mana run <unit-id>
```

You can also configure command templates for compatible runtimes:

```bash
mana config set-project run "imp run {id}"
mana config set-project plan "imp plan {id}"
```

`{id}` is replaced with the unit ID.

Useful commands while agents are working:

```bash
mana agents      # running and recently completed agents
mana logs 3      # logs for unit 3
mana status      # claimed, ready, blocked, and grouped work
mana context 3   # agent briefing for a unit
```

## Planning and decomposition

Use epics and child tasks when work is too broad for one safe attempt:

```bash
mana create "Refactor import pipeline" --epic
mana create "Extract parser interface" --parent 1 --verify "cargo test parser_interface" --produces ParserInterface
mana create "Move CSV parser" --parent 1 --requires ParserInterface --verify "cargo test csv_parser"
mana create "Move JSON parser" --parent 1 --requires ParserInterface --verify "cargo test json_parser"
```

Dependencies can be explicit:

```bash
mana dep add 3 2
mana tree 1
```

Or artifact-based:

```bash
mana create "Define schema" --produces Schema --verify "cargo test schema"
mana create "Build query engine" --requires Schema --verify "cargo test query"
```

Sequential work can be chained:

```bash
mana create "Step 1: scaffold" --verify "cargo check"
mana create next "Step 2: implement" --verify "cargo test feature"
mana create next "Step 3: document" --verify "grep -q 'Feature' README.md"
```

## Fail-first checks

By default, mana expects a task’s verify command to fail when the task is created. This prevents creating tasks whose completion check already passes.

Use `--pass-ok` for refactors, documentation, cleanup, or safety checks where the command may already pass:

```bash
mana create "Clean clippy warnings" --verify "cargo clippy --workspace --all-targets -- -D warnings" --pass-ok
mana create "Rewrite README" --verify "grep -q '## Quick start' README.md" --pass-ok
```

## Command overview

```bash
# lifecycle
mana create "title" --verify "cmd"
mana create "title" --epic
mana quick "title" --verify "cmd"
mana claim <id>
mana update <id>
mana verify <id>
mana close <id>
mana reopen <id>
mana delete <id>

# graph and queue
mana status
mana next
mana list
mana show <id>
mana tree [id]
mana graph
mana trace <id>

# dependencies and memory
mana dep add <id> <dep-id>
mana dep remove <id> <dep-id>
mana fact "title" --verify "cmd"
mana verify-facts
mana recall "query"

# agents and context
mana context [id]
mana run [id]
mana plan <id>
mana agents
mana logs <id>
mana review <id>
mana diff <id>

# maintenance and integration
mana tidy
mana doctor
mana doctor fix
mana config inspect
mana mcp serve
mana completions <shell>
```

See command-specific help for options and examples:

```bash
mana create --help
mana run --help
mana config --help
```

## Pipe-friendly usage

```bash
mana create "fix parser" --verify "cargo test parser" --pass-ok --json | jq -r '.id'
mana list --json | jq '.[] | select(.priority == 0)'
mana list --ids | mana close --stdin --force
cat spec.md | mana create "Implement spec" --description - --verify "cargo test spec"
mana list --format '{id}\t{status}\t{title}'
```

## Configuration

Project configuration lives in `.mana/config.yaml`. Global configuration lives under the user mana config directory.

```bash
mana config set-project run "imp run {id}"
mana config set-project plan "imp plan {id}"
mana config set-project max_concurrent 4
mana config set-project batch_verify true
mana config inspect
```

Common settings:

| Key | Description |
| --- | --- |
| `run` | Agent command template for task execution. `{id}` is replaced with the unit ID. |
| `plan` | Agent command template for decomposing larger work. |
| `run_model` | Default model for run-compatible agent flows. |
| `plan_model` | Default model for planning flows. |
| `review_model` | Default model for review flows. |
| `max_concurrent` | Maximum parallel agents. |
| `max_loops` | Maximum run-loop cycles before stopping. |
| `verify_timeout` | Default verify timeout in seconds. |
| `rules_file` | Rules file included in `mana context`. |
| `file_locking` | Avoid scheduling concurrent tasks with overlapping paths. |
| `batch_verify` | Run shared verify commands once after agents complete. |
| `auto_close_parent` | Close parent units when all children are closed. |
| `auto_commit` | Commit changes when a unit closes. |
| `on_close` / `on_fail` | Hook templates for close and failure events. |

> [!WARNING]
> Do not store secrets in mana config or unit files. Use environment variables or your agent/runtime’s secret mechanism.

## MCP integration

Mana includes an MCP server for IDE and agent integrations:

```bash
mana mcp serve
```

The MCP surface exposes project status, unit context, tree views, and common unit operations to compatible clients.

## Development

```bash
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The workspace contains:

- `crates/mana-core` — durable model, operations, config, index, graph, verification
- `crates/mana-cli` — CLI commands, output, MCP server, runtime adapters

## Documentation

- `mana --help` — command groups and examples
- `mana <command> --help` — detailed command help
- `CONTRIBUTING.md` — contribution workflow
- `CHANGELOG.md` — release history
