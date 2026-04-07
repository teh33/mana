---
name: mana
description: >
  Coordination substrate for AI coding agents. Verified gates, dependency scheduling, multi-agent
  dispatch. Create jobs to delegate work — `mana run` dispatches ready jobs automatically.
  Default action: `mana create "task" --verify "cmd"` (don't claim — let orchestration handle it).
---

# Mana — Quick Reference

Mana is a task tracker for AI agents where jobs, epics, and facts share the same durable substrate. Jobs have a **verify gate** — a shell command that must exit 0 to close. `mana run` dispatches ready jobs to agents, while epics organize larger efforts and facts capture verified project knowledge.

Mana speaks publicly in terms of epics, jobs, and facts.

**For syntax and examples:** `mana --help` or `mana <command> --help`

## When to Create

- Bug found while working → `mana create "bug: ..." --verify "test"`
- Multi-step feature job → `mana create "feat: ..." --verify "test"`
- Tests needed → `mana create "test: ..." --verify "test"`
- Refactor/docs/chore job → `mana create "refactor: ..." --verify "cmd" -p`
- Bigger parent effort → create an **epic** to decompose into child jobs
- Durable knowledge → create a **fact** with `mana fact`

Use `--paths` to specify which files a unit touches (used by `mana context`):
```bash
mana create "fix auth" --verify "cargo test auth" --paths "src/auth.rs,src/routes.rs"
```

Don't claim jobs manually by default — `mana run` dispatches ready work. Use `-p` when verify already passes.

**Don't create** jobs for questions, lookups, or trivial one-line fixes.

## Agent Context

`mana context <id>` is the single source of truth for agents. It outputs:
1. Unit spec (title, verify, description, acceptance)
2. Previous attempt notes (what was tried, what failed)
3. Project rules (RULES.md)
4. Dependency context (sibling units that produce required artifacts)
5. Referenced file contents (from `paths` field + description text)

## Writing Good Descriptions

Unit descriptions are agent prompts. Quality determines agent success.

**Include:**
1. **Concrete steps** — numbered, actionable ("Add test for X in Y" not "test things")
2. **File paths with intent** — `src/auth.rs (modify — add validation)`
3. **Embedded context** — paste actual types/signatures the agent needs
4. **Acceptance criteria** — what "done" looks like beyond the verify command
5. **Anti-patterns** — what NOT to do (learned from previous failures)

**Example:**

```bash
mana create "Add expired token test" \
  --verify "cargo test auth::tests::test_expired" \
  --description "## Task
Add a test that verifies expired JWT tokens return 401.

## Steps
1. Open src/auth/tests/jwt_test.rs
2. Add test_expired_token_returns_401 using create_test_token() from fixtures
3. Set expiry to 1 hour ago, assert 401 response

## Context
\`\`\`rust
// from src/auth/token.rs
pub struct AuthToken {
    pub user_id: UserId,
    pub expires_at: DateTime<Utc>,
}
\`\`\`

## Files
- src/auth/tests/jwt_test.rs (modify)
- src/auth/tests/fixtures.rs (read — has create_test_token)
- src/auth/token.rs (read only — do NOT modify)

## Don't
- Don't modify AuthToken or add dependencies
- Don't change existing tests"
```

## Batch Verify

When `batch_verify: true` is in the project config, the runner handles verification centrally:
- Agents skip verify and exit after `mana close`
- The runner groups completed units by verify command and runs each once
- Use scoped checks (`cargo check -p <crate>`) for fast feedback during work
- Don't run the full verify command yourself in batch mode

## On Failure

**Never retry with identical instructions.** Add what went wrong via `mana update <id> --note "..."`.

If an agent fails twice, the unit is too big or underspecified — `mana plan <id>` to break it down.
