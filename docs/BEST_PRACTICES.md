# Mana Best Practices for Agents

A walkthrough guide for agents (and developers) on creating, executing, and managing jobs, epics, and facts effectively.

---

## Table of Contents

1. [When to Create a Unit](#when-to-create-a-unit)
2. [Unit Anatomy](#unit-anatomy)
3. [Creating Effective Mana](#creating-effective-units)
4. [Writing Descriptions That Agents Can Execute](#writing-descriptions-that-agents-can-execute)
5. [Acceptance Criteria & Verification](#acceptance-criteria--verification)
6. [Splitting Work Into Subtasks](#splitting-work-into-subtasks)
7. [The Agent Workflow](#the-agent-workflow)
8. [Dependency Management](#dependency-management)
9. [Common Mistakes & How to Avoid Them](#common-mistakes--how-to-avoid-them)
10. [Practical Walkthroughs](#practical-walkthroughs)

---

## When to Create a Job, Epic, or Fact

Create a **job** when the work needs **tracking, verification, or delegation**.

### Create a job when:

- **Multi-step work** — The task spans 3+ steps or multiple files
- **Verification matters** — You need a concrete command to prove it's done
- **Agents will execute it** — The work will be dispatched to an agent via `mana run`
- **Dependencies exist** — Some work blocks other work
- **Context is complex** — Rich background, multiple file references, strategic decisions
- **Retry is likely** — The work might fail and need another attempt with accumulated context
- **Decomposition helps** — Breaking into smaller sub-units clarifies the path forward

### Create an epic when:

- **The work is too large for one job** — You need a parent record before decomposition
- **You need progress tracking across children** — Multiple jobs roll up into one larger effort
- **You want structure before execution** — The parent should organize child jobs, not be dispatched directly

### Create a fact when:

- **You learned something durable** — A command can re-verify it later
- **Future agents will need the knowledge** — API shape, environment constraint, migration state, etc.
- **The value is memory, not implementation work** — Capture what is true, not what should be built

### Don't create a job for:

- **Trivial fixes** — One-line typo corrections, adding a comment
- **Questions or lookups** — "What does this function do?" isn't a task
- **Work you're doing right now** — If you'll finish it in the next 5 minutes, just do it

---

## Job Anatomy

Every job has fields that serve specific purposes. Understand what goes where.

```yaml
# IDENTITY
id: '3.2.1'                                    # Auto-assigned, read-only
title: Implement token refresh logic           # One-liner summary (required)
status: open                                   # open | in_progress | awaiting_verify | closed
priority: 2                                    # 0-4 (P0 critical, P4 trivial)
created_at: 2026-01-26T15:00:00Z              # Auto-set, read-only
updated_at: 2026-01-26T15:00:00Z              # Auto-updated, read-only

# RELATIONSHIPS
parent: '3.2'                                  # Parent unit ID (if child)
dependencies:                                 # List of unit IDs this waits for
  - '2.1'
  - '2.2'
labels:                                       # Categorical tags
  - auth
  - backend

# CONTENT (Agent Prompt)
description: |                                # Rich context for the agent
  Implement token refresh logic in the auth service.

  ## Current State
  - Tokens expire after 1 hour
  - No refresh endpoint exists

  ## Task
  1. Add refresh_token grant type to src/auth/grants.rs
  2. Implement refresh endpoint in src/routes/auth.rs
  3. Add client-side retry in src/client/auth.ts

  ## Files to Modify
  - src/auth/grants.rs (add RefreshToken variant)
  - src/routes/auth.rs (POST /token endpoint)
  - src/client/auth.ts (handle 401 with refresh)

acceptance: |                                 # Concrete, testable criteria
  - Token refresh endpoint returns 200 with new access_token
  - Expired access token triggers automatic refresh
  - Refresh token expiry is enforced (7 days)
  - Existing access token still works (no immediate refresh)
  - Test: npm test -- --grep "refresh"

# VERIFICATION & EXECUTION
verify: npm test -- --grep "refresh" && npm run build
max_attempts: 3                               # Retry limit before escalation
attempts: 0                                   # Current attempt count

# EXECUTION STATE
claimed_by: "agent-7"                         # Who claimed this unit
claimed_at: 2026-01-27T10:30:00Z             # When claimed
closed_at: null                               # When closed (if closed)
close_reason: null                            # Why closed

# NOTES (Append-Only History)
notes: |
  2026-01-27 10:30 — claimed by agent-7
  2026-01-27 10:45 — attempted 1: test failures in refresh logic
  2026-01-27 11:15 — released by agent-7, see description for context
assignee: alice@example.com                   # Human owner (optional)
```

### Field Purposes at a Glance

| Field | Who sets | Why | Mutable |
|-------|----------|-----|---------|
| `id` | System | Unique identifier | No |
| `title` | Creator | Brief summary | Yes |
| `status` | System (via commands) | Workflow state (`open`, `in_progress`, `awaiting_verify`, `closed`) | No (use `claim`, `close`) |
| `priority` | Creator | Scheduling priority | Yes |
| `description` | Creator | Agent prompt (full context) | Yes |
| `acceptance` | Creator | What "done" means | Yes |
| `verify` | Creator | Gate to closing (shell command) | Yes |
| `dependencies` | Creator | Scheduling constraints | Yes |
| `claimed_by` | Agent | Who's working on it | Auto |
| `notes` | Anyone | Execution log (timestamps auto) | Append-only |

---

## Creating Effective Jobs

### Size Your Jobs Right

A single job should be **completable by one agent in one attempt** without needing to ask clarifying questions.

#### Too Big (Break into children)

```yaml
title: Build authentication system
description: |
  Implement user registration, login, token refresh, MFA, password reset...
```

This is 2-3 weeks of work. **Split it.**

#### Just Right

```yaml
title: Implement token refresh endpoint
description: |
  Add a POST /token endpoint that accepts refresh_token grant type.

  Context:
  - Access tokens expire after 1 hour
  - Refresh tokens are 30-day JWTs signed with HMAC-SHA256
  - Return { access_token, expires_in, token_type }

  Files:
  - src/routes/auth.rs

  Acceptance:
  - Endpoint validates signature and expiry
  - Returns 401 if refresh token expired
  - Returns new access_token with 1-hour expiry

  Test: npm test -- --grep "refresh"
```

This is **1-2 hours** of focused work. **Good size.**

#### Too Small (Combine or track differently)

```yaml
title: Add a comment to the refresh function
description: Explain what the signature validation does
```

Too small for a unit — just do it directly.

### Estimating Job Size

Ask yourself:
- **How many files will the agent modify?** → 2-5 ideally
- **How many functions to write/modify?** → 1-5 ideally
- **How many reads to understand context?** → 2-10 files
- **How many tests to write?** → 3-10 test cases
- **Token budget** → estimate <64k tokens total

If any of these balloons, split the unit.

### Priority Guidelines

```
P0 — Blocking multiple units, critical path
P1 — High value, unblocks work
P2 — Standard priority (default)
P3 — Nice to have, lower urgency
P4 — Wishlist, can defer indefinitely
```

---

## Writing Descriptions That Agents Can Execute

The description is **the agent prompt**. It lives in the job file, so agents can read it without CLI dependencies.

### Structure for Agent Success

Write descriptions that answer these questions **in order**:

1. **What's the current state?** — Context the agent needs
2. **What do you want the agent to build?** — Concrete deliverable
3. **Which files touch?** — File paths upfront
4. **How to verify?** — What "done" looks like

### Example: Good Description

```yaml
description: |
  ## Context
  The authentication service needs to refresh expired tokens.
  Currently, clients get a 401 error and must re-login.
  We want silent refresh: the client detects 401, refreshes
  the token, and retries the original request.

  ## Task
  Implement token refresh in the auth service:
  1. Add POST /api/auth/refresh endpoint
  2. Accept refresh_token in request body
  3. Validate signature and expiry
  4. Return new access_token (1-hour expiry)

  ## Important Files
  - src/routes/auth.rs — where to add the endpoint
  - src/auth/mod.rs — verify_token and issue_token functions
  - tests/auth_test.rs — where refresh tests go

  ## Edge Cases
  - Refresh token expired → return 401
  - Invalid signature → return 401
  - Valid refresh token → return 200 with new access_token

  ## How to Test
  Run: npm test -- --grep "token.*refresh"
```

### Example: Poor Description

```yaml
description: |
  Add token refresh.
```

This is **too vague**. Agent will ask clarifying questions, wasting time and tokens.

### Description Dos & Don'ts

#### Do ✅

- **Start with context** — Why are we doing this?
- **Name the files** — `src/routes/auth.rs`, not just "the routes"
- **Show the shape of the code** — "Add a `RefreshToken` variant to the `GrantType` enum"
- **List edge cases** — What should fail? What should succeed?
- **Link to examples** — "See `IssueToken` in src/auth/mod.rs for the pattern"
- **Specify the test command** — `npm test -- --grep "refresh"` not "write tests"

#### Don't ❌

- **Be vague** — "Make it work" is not enough
- **Force exploration** — Don't make the agent dig through code to understand structure
- **Mix concerns** — One unit = one feature. Don't bundle "refresh tokens" with "MFA" in one unit
- **Assume prior context** — Every description is read cold. Include what you need
- **Delegate design decisions** — Tell the agent the approach, not "figure out the best way"

---

## Acceptance Criteria & Verification

Acceptance criteria define when the work is done. Verification is the test that proves it.

### Acceptance Criteria: Human-Readable Definition

Write **concrete, testable statements**, not vague goals.

#### Too Vague ❌

```yaml
acceptance: |
  - Token refresh works correctly
  - Security is maintained
  - Performance is good
```

What does "works correctly" mean? How do we test "good performance"?

#### Concrete ✅

```yaml
acceptance: |
  - POST /api/auth/refresh accepts refresh_token in body
  - Returns 200 with { access_token, expires_in, token_type }
  - Returns 401 if refresh_token is expired or invalid
  - New access_token is valid for exactly 3600 seconds
  - Invalid signature returns 401 (no partial tokens)
  - All existing token validation tests pass
```

Each criterion should be **testable by the verify command**.

### Verify: The Machine-Checkable Gate

The `verify` field is a shell command that proves the unit is done. `mana close` runs it.

**Rules:**
- Must exit with code **0 if successful**, non-zero if failed
- Runs from the project root (wherever `.mana/` is)
- Can be a single command or shell script with `&&` chaining
- Examples:
  - `npm test -- --grep "refresh"`
  - `cargo test auth::refresh`
  - `python -m pytest tests/test_auth.py -k refresh`
  - `./scripts/verify-feature.sh`

#### Good Verify Commands

```yaml
# Single test suite
verify: npm test -- --grep "token.*refresh"

# Multiple gates
verify: pytest tests/test_auth.py && mypy src/auth/

# Custom script
verify: ./scripts/verify-auth-refresh.sh

# Go test with specific package
verify: go test ./internal/auth/... -run TestRefresh
```

#### Poor Verify Commands

```yaml
# Too broad (will catch unrelated failures)
verify: npm test

# Not deterministic (manual inspection)
verify: "echo 'Check if refresh works'"

# Requires interaction
verify: "read -p 'Does refresh work?' && echo ok"

# Always passes (useless)
verify: "echo 'Done!'"
```

### Linking Acceptance to Verify

The acceptance criteria **define what to test**. The verify command **proves it**.

```yaml
acceptance: |
  - POST /auth/refresh accepts { refresh_token }
  - Returns { access_token, expires_in }
  - Returns 401 if token expired
  - Returns 401 if signature invalid

verify: npm test -- --grep "refresh"
# ↑ This test suite must cover all acceptance criteria
```

**Before closing:** the agent should have written tests for every acceptance criterion. The verify command runs those tests.

---

## Splitting Work Into Child Jobs

Strategic parents provide context. Leaf units are agent-executable units.

### Parent Unit (Strategic Context)

Parents are **not meant to be closed**. They exist to provide context and bundle related work.

```yaml
id: '3'
title: Implement User Authentication
status: open
priority: 1
description: |
  ## Overview
  Build a complete user auth system with registration, login, and token refresh.

  ## Architecture Decision
  - Use JWT tokens (stateless)
  - Refresh tokens stored in httpOnly cookies
  - Access tokens in memory (client-side)
  - HMAC-SHA256 for signing

  ## Phased Approach
  1. Registration & login endpoints (3.1)
  2. Token refresh logic (3.2)
  3. Client-side auth manager (3.3)
  4. MFA optional features (3.4+)

  ## Files
  - Backend: src/routes/auth.rs, src/auth/mod.rs
  - Frontend: src/client/auth.ts
  - Tests: tests/auth_test.rs

  ## Common Gotchas
  - Token rotation: refresh increments version number
  - Cookie security: httpOnly, secure, sameSite=strict
  - Client retry: 401 triggers refresh, then retry original request
```

### Leaf Mana (Executable Units)

Leaf units are children that **an agent can claim and close**.

```yaml
id: '3.1'
title: Implement user registration endpoint
parent: '3'
status: open
priority: 1
dependencies: []
description: |
  ## Context
  See parent unit 3 for architecture overview.

  ## Task
  Implement POST /api/auth/register endpoint.

  1. Accept { email, password, name }
  2. Validate email format and password strength
  3. Hash password with bcrypt (cost 12)
  4. Create user record in database
  5. Return { id, email, name, created_at }

  ## Files
  - src/routes/auth.rs — add register route
  - src/auth/mod.rs — hash_password, create_user functions
  - tests/auth_test.rs — registration tests

  ## Edge Cases
  - Email already registered → 409 Conflict
  - Password too weak → 400 Bad Request (list requirements)
  - Database error → 500 Internal Server Error

  Test: npm test -- --grep "register"

acceptance: |
  - POST /api/auth/register accepts { email, password, name }
  - Rejects duplicate emails with 409
  - Rejects weak passwords with 400 (must include requirements)
  - Creates user with hashed password (verifiable with bcrypt)
  - Returns user object with id, email, name, created_at

verify: npm test -- --grep "register" && npm run build
```

### Decomposition Rules

1. **Parent should not be closed** — It's context and organization
2. **Leaves should be closeable** — One agent, one attempt, verifiable
3. **Children inherit parent's context** — Don't repeat architecture docs
4. **Dependencies should cross hierarchy** — "3.3 depends on 3.1" means can't start until 3.1 is done

---

## Agent Context

`mana context <id>` is the single source of truth for an agent working on a unit. It outputs a complete briefing:

1. **Unit spec** — ID, title, verify command, priority, status, description, acceptance criteria
2. **Previous attempts** — what was tried and why it failed (from attempt log and notes)
3. **Project rules** — conventions from `.mana/RULES.md`
4. **Dependency context** — descriptions of sibling units that produce artifacts this unit requires
5. **File structure** — function signatures and imports for referenced files
6. **File contents** — full source of all referenced files

```bash
mana context 3              # Complete agent context for unit 3
mana context 3 --structure-only  # Signatures only (smaller output)
mana context 3 --json       # Machine-readable
```

File paths come from two sources:
- **Explicit `paths` field** — set via `--paths` on `mana create` (takes priority)
- **Regex-extracted** — file paths mentioned naturally in the description text

```bash
# Explicit paths ensure the right files are always included
mana create "fix auth" --verify "cargo test auth" \
  --paths "src/auth.rs,src/middleware.rs"
```

### Handoff Notes

Log progress and failures so the next agent (or human) knows what happened:

```bash
mana update <id> --note "Implemented X in file Y, tests in Z"
mana update <id> --note "Failed: JWT lib incompatible. Avoid: jsonwebtoken 8.x"
```

---

## The Agent Workflow

This is how agents use units in practice.

### Step 0: Prepare Context

Before claiming, agents gather the complete context:

```bash
mana context <unit-id>      # Complete briefing: spec, attempts, rules, deps, files
```

This outputs everything the agent needs — no exploring required.

### Step 1: Agent Claims Work

Agent finds ready units:

```bash
mana status
```

Output shows the Ready section:
```
## Ready (3)
  1.1 [ ] Implement user registration endpoint
  1.2 [ ] Implement login endpoint
  2.1 [ ] Token refresh logic
```

Agent claims a unit (atomic — only one agent can win):

```bash
mana claim 1.1
```

Status transitions: **open → in_progress**. The unit is now claimed by this agent.

> **Batch verify mode:** When `batch_verify: true` is configured, agents skip running the verify command themselves. Instead, `mana close` marks the unit as `awaiting_verify` and the runner batches shared verify commands after all agents complete. This eliminates redundant builds (e.g. multiple agents all running `cargo build`) and cargo lock contention. Agents should use scoped checks like `cargo check -p <crate>` for fast feedback during work.

**What the agent sees via `mana context 1.1`:**
- Full description with context
- Acceptance criteria
- Verification command
- Notes from previous attempts (if retried)
- Referenced file contents

### Step 2: Agent Works

Agent modifies code, writes tests, iterates locally:

```bash
# ... implement the feature ...

# Test the unit's verify command (without closing)
mana verify 1.1
```

This runs the verify command **without closing the unit**. If tests fail, agent debugs and retries.

**Mid-work notes (recommended):**

```bash
mana update 1.1 --note "Added password validation, testing edge cases"
```

Notes are timestamped automatically. Useful for logging progress if the unit needs to be released.

### Step 3: Agent Closes (or Releases)

#### Success Path

Agent believes work is done:

```bash
mana close 1.1
```

What happens:
1. Verify command runs: `npm test -- --grep "register"`
2. If exit code 0 → Unit closes. Status: **in_progress → closed**. `closed_at` set.
3. Dependents (e.g., 1.2) become ready

#### Failure Path

Verify fails (exit code non-zero):

1. Status stays: **in_progress → open**
2. `attempts` incremented
3. Claim is released
4. Unit is available for retry

**Retry example:**

Agent 1 claimed 1.1, worked, failed verify, released.
Agent 2 runs `mana status`, sees 1.1 in the Ready section.
Agent 2 claims 1.1, runs `mana context 1.1` and sees the full briefing including Agent 1's attempt notes.
Agent 2 knows what was tried and avoids the same mistake.

#### Middle Path: Release Without Closing

Agent realizes the unit needs more context or design work:

```bash
mana claim 1.1 --release
```

Status: **in_progress → open**, claim released.
Agent adds notes:

```bash
mana update 1.1 --note "Need to clarify password validation rules with team"
```

Human reads the notes, updates the description, re-prioritizes.

#### Handoff & Commit

When work is verified and closed:

```bash
# Write handoff notes for downstream workers
mana update 1.1 --note "Implemented registration in src/routes/auth.rs, tests in tests/auth.test.ts, validates email uniqueness and password strength"

# Commit with unit ID prefix
git add -A
git commit -m "[units-1.1] Implement user registration endpoint"
```

### Step 4: Dependents Become Ready

Once 1.1 is closed, any unit that depended on it becomes ready:

```bash
mana dep add 1.2 1.1    # "1.2 depends on 1.1"
# ...later...
mana close 1.1          # closes successfully
mana status             # now shows 1.2 in Ready section
```

---

## Smart Selectors (@ Syntax)

Instead of memorizing or typing unit IDs, use smart selectors:

### Available Selectors

| Selector | Purpose | Example |
|----------|---------|---------|
| `@latest` | Most recently created unit | `mana show @latest` |
| `@blocked` | All blocked units (waiting on dependencies) | `mana list @blocked` |
| `@ready` | All ready units (no blockers) | `mana list @ready --tree` |
| `@parent` | Parent of the current unit | `mana close @parent` |
| `@me` | Current unit you're working on | `mana update @me --assignee alice` |

### Examples

```bash
# Show the newest unit
mana show @latest

# List blocked units
mana list @blocked

# Display ready units in tree format
mana list @ready --tree

# Update your current unit's assignee
mana update @me --assignee $(whoami)

# Close a unit's parent (useful in scripts)
mana close @parent
```

This eliminates the need to remember IDs and makes scripts more readable.

---

## Dependency Management

Dependencies block work. Use them to enforce ordering.

### Add Dependencies

```bash
# "Unit 2 depends on unit 1 (waits for 1 to close)"
mana dep add 2 1

# "Unit 3 depends on both 1 and 2"
mana dep add 3 1
mana dep add 3 2
```

### Understand Blocking

```bash
mana status   # Shows ready units (no blockers) and blocked units in one view
```

### Dependency Visualization

```bash
mana graph
```

Output:
```
[ ] 1  Task one
│  ├─ 3 (depends on 2)
│  └─ 4 (depends on 2)
└─ 5 (depends on 1)
```

### Common Patterns

#### Sequential Work (Phases)

```
Phase 1: Design
  └─ 1.1 (no dependencies)

Phase 2: Core Implementation
  ├─ 2.1 (depends on 1.1)
  └─ 2.2 (depends on 1.1)

Phase 3: Integration
  ├─ 3.1 (depends on 2.1 and 2.2)
  └─ 3.2 (depends on 2.1 and 2.2)
```

#### Parallel Work (Independent)

```
1.1 (no dependencies)
1.2 (no dependencies)
1.3 (no dependencies)
```

All three can be claimed simultaneously by different agents.

#### Diamond Pattern

```
    2.1
   /   \
  /     \
1.1     3.1
  \     /
   \   /
    2.2
```

Both 2.1 and 2.2 depend on 1.1. 3.1 depends on both 2.1 and 2.2. Can't start 3.1 until both are done.

### Cycle Detection

Avoid cycles (A depends on B, B depends on A). Use `mana doctor` to detect them:

```bash
mana doctor
```

If you see cycle warnings, fix them or the system gets stuck.

---

## Common Mistakes & How to Avoid Them

### Mistake 1: Unit Too Big

**Problem:** Unit requires 20+ functions, 15+ files, days of work.

**Symptom:** Agent gets overwhelmed, verify takes forever, context balloons.

**Fix:** Split into children.

```yaml
# Before (too big)
title: Implement authentication system

# After (better)
- 1. Design token schema
- 1.1 Implement registration
- 1.2 Implement login
- 1.3 Implement token refresh
```

### Mistake 2: Vague Description

**Problem:** "Add auth validation" without context on where, what validation, what files.

**Symptom:** Agent asks clarifying questions, wastes tokens exploring.

**Fix:** Write rich descriptions with file paths, edge cases, and examples.

```yaml
# Before
description: Validate authentication tokens

# After
description: |
  Add token validation to middleware in src/middleware/auth.ts.

  Validate: JWT signature, expiry, issuer claim.

  On invalid token: return 401 with { error: "Unauthorized" }
  On expired token: return 401 with { error: "Token expired" }
  On valid token: attach user_id to request.user

  Files: src/middleware/auth.ts, tests/middleware_test.ts
  See: src/auth/verify_token function (reuse this)
```

### Mistake 3: Unclear Acceptance Criteria

**Problem:** Criteria don't match what the agent actually tests.

**Symptom:** Agent finishes, runs verify, it fails. "But I thought it was done."

**Fix:** Make criteria testable and specific.

```yaml
# Before
acceptance: |
  - Token validation works
  - Security is maintained

# After
acceptance: |
  - Valid JWT passes validation, attaches user_id to request
  - Expired JWT returns 401
  - Invalid signature returns 401
  - Missing Authorization header returns 401
  - Malformed JWT returns 400
  All tested by: npm test -- --grep "auth.*validation"
```

### Mistake 4: Verify Command Doesn't Match Acceptance

**Problem:** You write acceptance criteria but the verify command doesn't test them.

**Symptom:** Unit closes but acceptance criteria aren't actually met.

**Fix:** Ensure every acceptance criterion has a test, and verify runs those tests.

```yaml
acceptance: |
  - Registration rejects duplicate emails with 409
  - Registration hashes passwords (never stores plaintext)
  - Registration returns { id, email, name, created_at }

verify: npm test -- --grep "register"
# ↑ This test file MUST have tests for:
#   - duplicate email rejection
#   - password hashing
#   - response shape
```

### Mistake 5: Circular Dependencies

**Problem:** A depends on B, B depends on A.

**Symptom:** `mana status` shows no ready units. No progress possible.

**Fix:** Use `mana doctor` to detect cycles and break them.

```bash
# Detect cycles
mana doctor

# Output: "[!] Dependency cycle detected: 1 -> 2"

# Break it
mana dep remove 2 1  # or restructure dependencies
```

### Mistake 6: Dependencies as Parent-Child Substitute

**Problem:** Using dependencies instead of hierarchy.

```yaml
# Anti-pattern
id: 1
title: Feature A

id: 2
title: Feature A - Part 2
dependencies: [1]  # Should be parent-child instead
```

**Fix:** Use hierarchy (parent.id) to split work into subtasks, dependencies for blocking.

```yaml
# Better
id: 1
title: Feature A (parent)

id: 1.1
title: Feature A - Part 1
parent: 1

id: 1.2
title: Feature A - Part 2
parent: 1
dependencies: [1.1]  # Only if 1.2 truly requires 1.1 to be done first
```

### Mistake 7: Forgetting Context When Updating

**Problem:** Agent releases a unit with notes but no description update. Next agent is confused.

**Symptom:** Second attempt: "Wait, what's the issue? The description doesn't explain what failed."

**Fix:** When releasing, update the description with findings.

```bash
mana claim 1.1
# ... agent works, fails verify, releases ...
mana update 1.1 --note "Signature validation was failing; see line 45 of auth.rs"
# Better: edit the description to clarify the issue
```

### Mistake 8: Verify Command Too Slow or Flaky

**Problem:** Verify command takes 10+ minutes or randomly fails.

**Symptom:** Mana fail to close even when work is done.

**Fix:** Use targeted test suites, avoid full test runs.

```yaml
# Too slow
verify: npm test   # runs all 500 tests

# Better
verify: npm test -- --grep "register"  # runs 5 relevant tests
```

---

## Practical Walkthroughs

Real examples of creating and executing units.

### Walkthrough 1: User Registration (Single Leaf Unit)

#### Scenario
You want an agent to implement a user registration endpoint. Simple feature, no blockers, self-contained.

#### Step 1: Create the Unit

```bash
mana create "Implement user registration endpoint"
```

Outputs: `New unit: 1`

#### Step 2: View It

```bash
mana show 1
```

```yaml
id: '1'
title: Implement user registration endpoint
status: open
priority: 2
created_at: 2026-01-27T10:00:00Z
description: null
acceptance: null
verify: null
```

Empty. Let's fill it in.

#### Step 3: Edit the Unit

Fill in the details with `mana edit 1` or `mana update`:

```yaml
id: '1'
title: Implement user registration endpoint
status: open
priority: 1  # User auth is high-priority
description: |
  ## Context
  The API needs a user registration endpoint.
  New users provide email, password, and name.
  Passwords are hashed with bcrypt before storage.

  ## Task
  Implement POST /api/auth/register:
  1. Accept { email, password, name } in body
  2. Validate email is unique (409 if duplicate)
  3. Validate password is strong (8+ chars, uppercase, number)
  4. Hash password with bcrypt cost 12
  5. Create user record
  6. Return { id, email, name, created_at }

  ## Files
  - src/routes/auth.rs — add /register route
  - src/auth/mod.rs — hash_password, create_user helpers
  - tests/auth_test.rs — registration tests

  ## Test It
  Run: npm test -- --grep "register"

  ## Reference
  Look at login endpoint (src/routes/auth.rs) for patterns.

acceptance: |
  - POST /api/auth/register accepts { email, password, name }
  - Rejects duplicate email with 409
  - Rejects weak password (< 8 chars) with 400
  - Hashes password before storage (never stored plaintext)
  - Returns user: { id, email, name, created_at }
  - Returns 500 on database error

verify: npm test -- --grep "register" && npm run build
```

#### Step 4: Agent Claims the Unit

```bash
mana claim 1
```

Agent runs `mana context 1` and starts work.

#### Step 5: Agent Checks Progress

Mid-work, agent runs:

```bash
mana verify 1
```

If tests pass, agent continues. If they fail, agent fixes.

#### Step 6: Agent Closes

When done:

```bash
mana close 1
```

Verify runs. If it passes, unit closes. Dependents become ready.

---

### Walkthrough 2: Complex Feature with Decomposition (Parent + Children)

#### Scenario
You want to build "Complete Authentication System."
This is too big for one unit, so break it into phases.

#### Step 1: Create Parent Unit

```bash
mana create "Complete Authentication System"
```

Output: `New unit: 2`

Fill in the parent unit details:

```yaml
id: '2'
title: Complete Authentication System
status: open
priority: 0  # Critical path
description: |
  ## Overview
  Build a complete auth system: registration, login, token refresh, MFA prep.

  ## Architecture
  - JWT tokens (HS256)
  - Refresh tokens in httpOnly cookies
  - Access tokens in memory (frontend)
  - Password: bcrypt (cost 12)

  ## Implementation Plan
  1. Phase 1: Registration & Login (2.1)
  2. Phase 2: Token Refresh (2.2)
  3. Phase 3: Client-side Auth Manager (2.3)
  4. Phase 4: Email Verification (2.4, optional)

  ## Key Files
  - Backend: src/routes/auth.rs, src/auth/mod.rs
  - Frontend: src/client/auth.ts
  - Tests: tests/auth_test.rs, src/client/__tests__/auth.test.ts

  ## Security Considerations
  - Never log passwords or tokens
  - Tokens in secure, httpOnly cookies
  - CSRF protection on token endpoints
  - Rate limit login attempts

# Leave description empty for parent; it's context only
# Don't set verify; parent units aren't closed
```

#### Step 2: Create Phase 1 (Registration & Login)

```bash
mana create "Implement registration endpoint" --parent 2
```

Output: `New unit: 2.1`

```bash
mana create "Implement login endpoint" --parent 2
```

Output: `New unit: 2.2`

```bash
# Make 2.2 depend on 2.1 (login needs user table from registration)
mana dep add 2.2 2.1
```

Fill in `.mana/2.1` and `.mana/2.2` with full descriptions (like Walkthrough 1).

#### Step 3: Create Phase 2 (Token Refresh)

```bash
mana create "Implement token refresh endpoint" --parent 2
```

Output: `New unit: 2.3`

```bash
mana dep add 2.3 2.1  # Needs registration (user table)
mana dep add 2.3 2.2  # Needs login (to understand token flow)
```

#### Step 4: View the Hierarchy

```bash
mana tree 2
```

Output:
```
[  ] 2. Complete Authentication System
  [ ] 2.1 Implement registration endpoint
  [ ] 2.2 Implement login endpoint
    └─ depends on 2.1
  [ ] 2.3 Implement token refresh endpoint
    ├─ depends on 2.1
    └─ depends on 2.2
```

#### Step 5: Check Readiness

```bash
mana status
```

Output:
```
## Ready (1)
  2.1 [ ] Implement registration endpoint

## Blocked (2)
  2.2 [!] Implement login endpoint
  2.3 [!] Implement token refresh endpoint
```

Only 2.1 is ready (no blockers). 2.2 and 2.3 are blocked.

#### Step 6: Agents Swarm

Agent 1 claims 2.1, implements registration.
After 2.1 closes, 2.2 becomes ready.
Agent 2 claims 2.2, implements login.
After 2.2 closes, 2.3 becomes ready.
Agent 3 claims 2.3, implements token refresh.

All work in dependency order, no parallelism bottleneck.

---

### Walkthrough 3: Handling Retry (Agent Fails, Second Agent Tries)

#### Scenario
Agent 1 claims unit, works, fails verify. Agent 2 retries.

#### Step 1: Agent 1 Claims

```bash
mana claim 2.1
```

Status: open → in_progress

#### Step 2: Agent 1 Works

Agent modifies code, writes tests. Mid-work:

```bash
mana verify 2.1
# Test fails: registration test times out
```

Agent debugs, realizes there's an issue but decides to release for human review.

```bash
mana claim 2.1 --release
mana update 2.1 --note "Database timeout on create_user; check connection pool"
```

Status: in_progress → open

#### Step 3: Human Reviews

Human reads notes, sees the issue. Updates description to clarify:

```yaml
description: |
  ... existing description ...

  ## Known Issues (from previous attempt)
  - Database connection pool may be maxed; verify pool size in .env
  - See src/config/database.rs for pool config
```

#### Step 4: Agent 2 Retries

```bash
mana claim 2.1
```

Agent runs `mana context 2.1`, sees notes, updated description, and referenced files.
Agent checks connection pool, finds and fixes the issue.

```bash
mana close 2.1
```

Verify passes. Unit closes.

---

## The Development Loop

All work in the project follows a standard workflow:

1. **Understand** — `mana context <id>` to get the complete briefing before touching code
2. **Plan** — Single task: just do it. Multi-step: break into units with `mana create` or `mana plan`
3. **Implement** — Single unit: implement directly. Epic: `mana run` to dispatch agents in parallel
4. **Verify** — `mana verify <id>` to test without closing
5. **Close** — `mana close <id>` when verify passes

### Project Research Mode (`mana plan` with no ID)

When you need fresh work rather than a split of an existing unit, run `mana plan` with no ID.

What happens:
- Mana detects the project stack from marker files such as `Cargo.toml`, `package.json`, `pyproject.toml`, or `go.mod`
- It runs best-effort static checks for the detected stack and collects the output
- It creates a parent research unit such as `Project research — 2026-03-21`
- It spawns the configured research agent, which should create child units for concrete findings

Example config:

```yaml
research: "pi -p 'Analyze this project for bugs, missing tests, refactors, perf issues, and security gaps. For each finding run: mana create \"category: description\" --parent {parent_id} --verify \"test command\"'"
```

Template notes:
- Use `{parent_id}` in `research` templates so findings attach to the research parent
- If `research` is not configured, Mana falls back to the normal `plan` template
- `mana plan --dry-run` previews the static analysis output and the built-in research prompt without creating units

---

## The Unitstalk Vision: Future Toolchain

Mana is evolving from a task tracker into a comprehensive orchestration platform. Here's the strategic vision:

### Planned Companion Tools

**`mana context`** — Context Assembler (Killer App for Agents)
- Single source of truth: outputs unit spec, attempts, rules, dependency context, and file contents
- Merges explicit `--paths` with regex-extracted paths from description
- Solves the "cold start" problem for agents
- Usage: `mana context 3.2 | llm "Implement this"`
- Status: **Implemented**

**`bpick`** — Fuzzy Selector
- Interactive unit selection using `fzf`
- Never type an ID manually
- Usage: `mana close $(bpick)`
- Status: Planned

**`bmake`** — Dependency-Aware Execution
- Execute commands only when DAG permits (CI/CD gatekeeper)
- Usage: `bmake units-50 "./deploy.sh"` (only runs if unit is closed)
- Status: Planned

**`btime`** — Punch Clock
- Calculate cycle time from `created_at` to `closed_at`
- Track active time via git log analysis
- Status: Planned

**`bgrep`** — Semantic Grep
- Search units with field filtering
- Usage: `bgrep "database" --field description --status open`
- Status: Planned

**`bviz`** — TUI Dashboard
- Left pane: Tree view of units
- Right pane: Markdown renderer
- Bottom pane: Dependency graph
- Status: Planned

### Infrastructure Improvements

**Git Hook Integration**
- Auto-prepend unit ID to commits
- Branch: `feat/units-3.2-list-command` → Commit: `[units-3.2] Added sorting`
- Status: Planned

**Markdown Format Migration**
- Current: Pure YAML files (flexible, direct file access)
- Future: Markdown with YAML frontmatter (better for agents, LLMs, humans)
- Status: Foundation in place, migration planned

**Unit Server Protocol**
- JSON-RPC interface for IDE/Agent integration
- Status: Planned

### Current Implementation Status

Already available:
- `mana run` / `mana plan` — Built-in agent orchestration, unit decomposition, and project-level research
- `mana agents` / `mana logs` — Agent monitoring and log viewing
- `mana init --agent` — Guided agent setup wizard with presets
- `mana claim` / `mana verify` — Atomic task claiming and verification without closing
- `mana edit` — Edit units in $EDITOR with schema validation
- Smart selectors (@latest, @blocked, @parent, @me)
- Hook system — Pre-close hooks for CI gatekeeper patterns
- Archive system — Auto-archiving closed units to dated directories
- Multi-format support — YAML and Markdown
- Agent context — `mana context <id>` outputs complete agent briefing (spec, attempts, rules, deps, files)

---

## Summary

### Key Takeaways

1. **Mana for work that needs tracking, verification, or delegation.** Trivial fixes don't need units.
2. **Size units to ~1-5 files, 1-5 functions.** Bigger = split into children.
3. **Write descriptions for cold reads.** Assume the agent has no prior context.
4. **Acceptance criteria must be testable.** Verify command proves it.
5. **Use hierarchy to split work** (parent/child), **dependencies for blocking** (A waits for B).
6. **Parents provide context.** Leaves are executable.
7. **`mana context <id>` is the single source of truth** — agents read it before starting work.
8. **Agents claim atomically.** Only one agent per unit.
9. **Verify gates closing.** No force-close. If verify fails, retry with a fresh agent.
10. **Notes are the execution log.** Timestamp automatically, visible to next agent.
11. **Dependencies enable waves.** Agents work in parallel, constrained by scheduling.
12. **Follow the development loop** — Understand → Plan → Implement → Verify → Close.

### Quick Checklist: Is My Unit Ready for Agents?

#### Before Creation
- [ ] **Standalone or part of epic?** If epic: create parent, assign as child
- [ ] **Size right?** 1-5 files, 1-5 functions modified, <64k tokens
- [ ] **Blockers identified?** Know what must be done first

#### During Creation
- [ ] **Title** — One-liner, clear and descriptive
- [ ] **Priority** — Set appropriately (P0-P4)
- [ ] **Description** — Rich context, file paths, edge cases, no exploration needed
- [ ] **Acceptance** — Concrete, testable criteria (not vague goals)
- [ ] **Verify** — Shell command that proves acceptance is met
- [ ] **Dependencies** — Only include true blocking relationships
- [ ] **Labels** — Tagged for organization (optional but helpful)

#### Before Agent Execution
- [ ] **Unit is in ready state** — `mana status` shows it in the Ready section
- [ ] **Acceptance criteria are complete** — No ambiguity
- [ ] **Verify command is tested** — You've run it locally

If all checked, the unit is ready for an agent to claim.

---

## Further Reading

**Project Documentation:**
- [units README.md](../README.md) — System overview and commands
- [Agent Skill](./SKILL.md) — Quick reference for AI agents
- [Agent Workflow](../README.md#agent-workflow) — How agents execute units

**Command Reference:**
- `mana --help` — Full CLI help
- `mana <command> --help` — Help for specific command

**Key Commands:**
- `mana context <id>` — Complete agent briefing (single source of truth)
- `mana run [id]` — Dispatch ready units to agents
- `mana run --loop` — Continuous dispatch mode
- `mana plan [id]` — With an ID, decompose a large unit; without an ID, run project research and create grouped findings
- `mana agents` / `mana logs <id>` — Monitor agents and view output
- `mana init --agent <preset>` — Guided agent setup
- `mana claim <id>` — Atomically claim unit for work
- `mana claim <id> --release` — Release claim
- `mana verify <id>` — Test verify without closing
- `mana edit <id>` — Edit in $EDITOR with validation

---

*Last updated: 2026-03-21*
