# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Mana is pre-1.0, so minor releases may include behavior changes.

Entries before 0.3.1 are best-effort summaries from repository history and release metadata. The changelog is maintained intentionally from 0.3.1 onward.

## [Unreleased]

### Added

- Added `mana reparent` and the corresponding `mana-core` API for moving a unit under a new parent with cycle/descendant protection.
- Added native parsing and validation for a root `facts.mana` project fact sheet.
- Added support for compact fact lines in the form `- <fact text> @<status> [mana-ref] [{anchor}]`.
- Added fact-sheet lifecycle statuses: `draft`, `spec`, `in_progress`, `verified`, `stale`, and `rejected`.
- Added optional `{anchor}` identifiers for stable fact references, with duplicate-anchor diagnostics.
- Added fact-sheet checks for malformed lines, unknown statuses, missing backing units, and `@verified` facts backed by open work.
- Added `mana groom` as a dry-run project-management cleanup proposal command.
- Added richer `mana brief` output, including scope, current-truth summaries, and management signals.

### Changed

- Began evolving Mana facts toward a single-file fact sheet model while keeping existing fact-unit verification compatible.
- Installed `facts.mana` checking behind `mana-core` APIs so future CLI, SQLite, and Imp context surfaces can share one parser/check path.
- Updated the local Mana work graph with the planned fact-sheet, SQLite indexing, and Imp context integration slices.
- Refined close auto-commit targeting to include the intended unit/archive paths and avoid broad incidental commits.
- Updated close auto-commit handling to preserve already-staged user changes.

### Fixed

- Hardened index loading so a corrupt cache can be rebuilt instead of breaking normal reads.
- Fixed default Mana unit kind handling.
- Fixed migrated Mana verify commands.
- Fixed MCP close verify formatting.
- Fixed generated archive/index state tracking so derived Mana state is not treated as canonical source.

### Removed

- Stopped tracking generated Mana archive index state.

## [0.3.1] - 2026-04-27

### Added

- Published `mana-pool` as the support crate for resource-aware run orchestration.
- Added SQLite-backed mana indexing and integrated it into context assembly.
- Added SQLite diagnostics to `mana doctor`.
- Added `mana search`.

### Changed

- Rewrote the README for crates.io and first-time users.
- Updated mana help positioning.
- Refreshed dependency lockfile.
- Aligned published crate versions so `mana-cli` depends on `mana-core 0.3.1`.
- Moved direct-mode deferred grouped verify ownership through `mana-pool::execute_deferred_verify`.
- Normalized mana graph/index data after migration into the repository.

### Fixed

- Fixed clippy failures on current stable Rust.
- Fixed default mana unit kind handling.
- Fixed migrated mana verify commands.
- Hardened mana parsing against libyml scanner panics in corrupt YAML paths.
- Fixed MCP close verify formatting.

### Removed

- Removed `mana-review` from the active workspace and archived the code locally outside the tracked tree.
- Removed the human review UI module from the active CLI build.

## [0.3.0] - 2026-03-23

### Added

- Initial crates.io publication of `mana-cli`.
- Added `mana-pool` crate as a resource-aware dispatch engine for agent runs.
- Added `mana-review` crate and wired review queue, HTML review, approve/reject/request-changes flows into the CLI.
- Added native mana tool surfaces for imp, including create, close, update, orchestration actions, and run state.
- Added worktree isolation for parallel agents.
- Added feature-aware task creation with `--feature` and parent auto-close gating.
- Added smart wave dispatching and file-conflict avoidance in the ready queue.
- Added critical-path weighting and prioritization.
- Added `mana diff` to show changes produced by an agent.
- Added verify-command static analysis to `mana create`.
- Added decision fields as an execution gate.
- Added auto-JSON behavior when stdout is piped.
- Enriched list JSON with verify and creation metadata.
- Extracted close lifecycle logic into `mana-core` operations.
- Added retry context to the spawner trait.
- Added global and project-scoped config commands with effective-value output.
- Added progress events from running agents.
- Added release CI workflows for mana.
- Added a hardened verification pipeline with verify freeze, diff evidence, and risk scoring.

### Changed

- Renamed the project/package lineage from `bn` toward `mana-cli` for publication.
- Cleaned up CLI shape by merging several commands into flags.
- Renamed `mana show` toward `mana read`, while retaining compatibility aliases.
- Improved `mana plan` by removing dead code and sharpening the decomposition prompt.
- Improved model config discoverability.
- Improved README and package metadata for publication.
- Suppressed child stdio during embedded JSON runs.
- Updated runtime handoff semantics so mana run can integrate with imp-run style workers.

### Fixed

- Fixed CI failures from absolute local path dependencies and compile errors.
- Fixed flaky plan tests by forcing index rebuilds.
- Fixed project detection compatibility with `project-detect 0.1`.
- Fixed claim release on spawned-agent failure.
- Fixed archived dependencies so archived units can satisfy dependency checks.
- Fixed frontmatter handling bugs.
- Fixed close test isolation.
- Fixed release CI cross-compilation for macOS x86_64 from arm64 runners.
- Fixed read-tool hang and file suggestion performance issues.
- Fixed UTF-8 panic behavior in streaming tool output.

### Removed

- Removed promptfoo installation while retaining evaluation scaffolding for later.

## [0.2.0] - 2026-03-18

### Added

- File locking to prevent concurrent agent writes.
- Atomic file writes for crash safety.
- `CONTRIBUTING.md`.
- Agent orchestration through `mana run` with ready-queue scheduling.
- Loop mode for continuously dispatching until no work remains.
- Auto-planning for decomposing large units before dispatch.
- Adversarial review support for agent-produced work.
- Agent monitoring through `mana agents` and `mana logs`.
- Verified project memory through `mana fact`, TTLs, and stale fact detection.
- Memory context output through `mana context` without arguments.
- MCP server support through `mana mcp serve`.
- Library API re-exports for Rust consumers.
- Interactive `mana create` wizard.
- Sequential chaining through `mana create next`.
- `mana trace` for lineage, dependencies, artifacts, and attempt history.
- `mana recall` keyword search across open and archived units.
- Pipe-friendly output: `--json`, `--ids`, and `--format`.
- Stdin support for descriptions, notes, and batch operations.
- Batch close through `mana close --stdin`.
- Failure escalation policies through `--on-fail`.
- Config inheritance through `extends`.
- Shell completions.
- Agent presets for initialization.
- File context extraction and structure-only context.
- `mana unarchive`.
- Lock management through `mana locks`.
- `mana quick` for create-and-claim workflows.
- Status overview for claimed, ready, and blocked units.
- `$EDITOR`-based editing with schema validation and backup/rollback.
- Hook system with trust management.
- Smart selectors such as `@latest`, `@blocked`, `@parent`, and `@me`.
- Verify-as-spec behavior for goal-like units without verify commands.
- Auto-suggested verify commands from project type detection.
- Fail-first enforcement on task creation, with `--pass-ok` opt-out.
- Agent liveness reporting in status output.
- Acceptance criteria fields.
- Core CLI commands: `init`, `create`, `show`, `list`, and `close`.
- Verification gates for closing work.
- Hierarchical dot-notation tasks and tree rendering.
- Smart dependencies with `produces`/`requires` fields.
- Dependency graph output in ASCII, Mermaid, and DOT formats.
- Task lifecycle commands: `claim`, `close`, `reopen`, and `delete`.
- Failure tracking with attempts and appended failure output.
- Ready and blocked dependency queries.
- Dependency management commands.
- Cached index and `mana doctor` health checks.
- Project stats and `mana tidy` maintenance.
- Markdown/YAML-backed unit storage, slug filenames, archive layout, and git-native local state.

### Changed

- Improved robustness for parallel agent workflows.
- Renamed the package from `bn` to `mana-cli` for crates.io publication.
- Improved help text and README coverage.
- Improved `mana show` rendering.
- Rewrote README with a table of contents and consolidated documentation.
- Tightened selected dependency floors above known CVEs.

### Fixed

- Fixed `mana context` crash on corrupt archive YAML.
- Fixed missing config fields in test struct literals.
- Fixed shell escaping in verify commands.
- Fixed file extension preservation during archiving.
- Fixed `.md` format support in dependency and verify commands.
- Fixed verify-on-claim behavior with `--pass-ok` and `fail_first=false`.

### Removed

- Removed `mana ready`; use `mana status` instead.
- Removed `mana blocked`; use `mana status` instead.
- Removed `mana dep tree`; use `mana graph` instead.
- Removed `mana dep cycles`; use `mana doctor` instead.

## Earlier internal history

Before the public `0.2.0`/`0.3.0` line, mana evolved through local/internal iterations that established the core model: local Markdown units, verification gates, hierarchical work, dependency tracking, archive behavior, project memory, agent context assembly, and git-friendly state.

[Unreleased]: https://github.com/kfcafe/mana/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/kfcafe/mana/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/kfcafe/mana/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/kfcafe/mana/releases/tag/v0.2.0
