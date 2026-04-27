---
id: '274'
title: 'Fix mana index kind mismatch: writer emits task but CLI parser rejects it'
slug: fix-mana-index-kind-mismatch-writer-emits-task-but-cli-parser-rejects-it
status: open
priority: 1
created_at: '2026-04-27T21:20:00Z'
updated_at: '2026-04-27T23:29:58.226922Z'
acceptance: Unit 274 is inspected and either claimed for implementation with a corrected verify gate or explicitly deferred with notes explaining why.
notes: |2-

  ## Attempt 1 — 2026-04-27T21:46:12Z
  Exit code: 2

  ```
  crates/mana-core/src/failure.rs:214:        return Some("- Agent went idle — it may be stuck in a loop or waiting for input. Try a more focused prompt or break the task into smaller steps.");
  crates/mana-core/src/failure.rs:217:        return Some("- Agent ran out of time. Consider increasing the timeout or simplifying the task scope.");
  crates/mana-core/src/ops/move_units.rs:207:        let mut unit = Unit::new("1.1", "Child task");
  crates/mana-core/src/ops/move_units.rs:208:        unit.slug = Some("child-task".to_string());
  crates/mana-core/src/ops/move_units.rs:213:        unit.to_file(src_units.join("1.1-child-task.md")).unwrap();
  crates/mana-core/src/ops/move_units.rs:218:        let moved = Unit::from_file(dst_units.join(format!("{}-child-task.md", new_id))).unwrap();
  crates/mana-core/src/ops/context.rs:669:        let mut child = Unit::new("1.1", "Child task");
  crates/mana-core/src/ops/tidy.rs:252:        let mut unit = Unit::new("1", "Done task");
  crates/mana-core/src/ops/tidy.rs:270:        let unit = Unit::new("1", "Open task");
  crates/mana-core/src/ops/tidy.rs:283:        let mut unit = Unit::new("1", "Done task");
  crates/mana-core/src/ops/batch_verify.rs:280:        unit.to_file(mana_dir.join(format!("{}-task-{}.md", id, slug)))
  crates/mana-core/src/ops/batch_verify.rs:377:        unit.to_file(mana_dir.join("1-task-1.md")).unwrap();
  crates/mana-core/src/ops/unarchive.rs:175:        let main_path = mana_dir.join("1-task.md");
  crates/mana-core/src/ops/status.rs:129:        let mut task = Unit::new("2", "Task");
  crates/mana-core/src/ops/status.rs:130:        task.kind = UnitType::Task;
  crates/mana-core/src/ops/status.rs:131:        task.verify = Some("cargo test task".to_string());
  crates/mana-core/src/ops/status.rs:132:        write_unit(&mana_dir, &task);
  crates/mana-core/src/ops/status.rs:153:        let mut ready_unit = Unit::new("1", "Ready task");
  crates/mana-core/src/ops/status.rs:158:        let goal_unit = Unit::new("2", "Goal task");
  crates/mana-core/src/ops/status.rs:162:        let mut claimed_unit = Unit::new("3", "Claimed task");
  crates/mana-core/src/ops/status.rs:190:        let mut blocked_unit = Unit::new("2", "Blocked task");
  crates/mana-core/src/ops/status.rs:218:        let mut unit = Unit::new("1", "Closed task");
  crates/mana-core/src/ops/status.rs:234:        let mut unit = Unit::new("1", "Awaiting verify task");
  crates/mana-core/src/ops/status.rs:261:        let mut unit = Unit::new("2", "Dependent task");
  crates/mana-core/src/prompt.rs:1://! Structured task-packet and compatibility prompt builder.
  crates/mana-core/src/prompt.rs:3://! Constructs a multi-section task packet that gives runtimes/agents the context
  crates/mana-core/src/prompt.rs:6://! durable task/input preparation, with final runtime/system prompt assembly
  crates/mana-core/src/prompt.rs:11://! Long-term, treat the material here as durable task-packet source content
  crates/mana-core/src/prompt.rs:45:/// Result of building durable task-packet material for compatibility flows.
  crates/mana-core/src/prompt.rs:47:    /// Compatibility system-prompt content assembled from durable task/input sections.
  crates/mana-core/src/prompt.rs:105:/// Build the full structured task-packet material for a unit.
  crates/mana-core/src/prompt.rs:109:/// sections that capture durable task/input material another runtime can use.
  crates/mana-core/src/prompt.rs:897:        sibling.notes = Some("Just regular notes about the task".to_string());
  crates/mana-core/src/prompt.rs:1146:        assert!(result.file_ref.contains("1-simple-task.md"));
  crates/mana-core/src/ops/plan.rs:149:        let unit = Unit::new("1", "Big task");
  crates/mana-core/src/ops/show.rs:77:        create::create(&bd, minimal_params("My task")).unwrap();
  crates/mana-core/src/ops/show.rs:79:        assert_eq!(r.unit.title, "My task");
  crates/mana-core/src/ops/show.rs:89:        let mut unit = Unit::new("1", "Archived task");
  crates/mana-core/src/ops/show.rs:96:        assert_eq!(r.unit.title, "Archived task");
  crates/mana-core/src/ops/run.rs:569:        let mut unit = Unit::new("2", "Dispatchable task with unresolved decisions");
  crates/mana-core/src/ops/run.rs:576:        unit.to_file(mana_dir.join("2-dispatchable-task-with-unresolved-decisions.md"))
  crates/mana-core/src/ops/run.rs:645:        let mut task = Unit::new("2", "Dispatchable task");
  crates/mana-core/src/ops/run.rs:646:        task.kind = UnitType::Task;
  crates/mana-core/src/ops/run.rs:647:        task.verify = Some("cargo test dispatchable_task".to_string());
  crates/mana-core/src/ops/run.rs:648:        task.to_file(mana_dir.join("2-dispatchable-task.md"))
  crates/mana-core/src/index.rs:611:        let unit1 = Unit::new("1", "First task");
  crates/mana-core/src/index.rs:612:        let unit2 = Unit::new("2", "Second task");
  crates/mana-core/src/index.rs:613:        let unit10 = Unit::new("10", "Tenth task");
  crates/mana-core/src/index.rs:786:        let unit = Unit::new("1", "Modified first task");
  crates/mana-core/src/index.rs:948:        let mut unit = crate::unit::Unit::new("1", "Archived task");

  ... (214 lines omitted) ...

  crates/mana-cli/src/commands/adopt.rs:269:        parent.to_file(mana_dir.join("1-parent-task.md")).unwrap();
  crates/mana-cli/src/commands/adopt.rs:272:        let mut child = Unit::new("2", "Child task");
  crates/mana-cli/src/commands/adopt.rs:273:        child.slug = Some("child-task".to_string());
  crates/mana-cli/src/commands/adopt.rs:275:        child.to_file(mana_dir.join("2-child-task.md")).unwrap();
  crates/mana-cli/src/commands/adopt.rs:284:        assert!(!mana_dir.join("2-child-task.md").exists());
  crates/mana-cli/src/commands/adopt.rs:287:        assert!(mana_dir.join("1.1-child-task.md").exists());
  crates/mana-cli/src/commands/adopt.rs:290:        let adopted = Unit::from_file(mana_dir.join("1.1-child-task.md")).unwrap();
  crates/mana-cli/src/commands/adopt.rs:293:        assert_eq!(adopted.title, "Child task");
  crates/mana-cli/src/commands/run/ready_queue.rs:914:            file_ref: "@.mana/1-task.md".to_string(),
  crates/mana-cli/src/commands/create/tests.rs:54:        title: "First task".to_string(),
  crates/mana-cli/src/commands/create/tests.rs:83:    let unit_path = mana_dir.join("1-first-task.md");
  crates/mana-cli/src/commands/create/tests.rs:89:    assert_eq!(unit.title, "First task");
  crates/mana-cli/src/commands/create/tests.rs:90:    assert_eq!(unit.slug, Some("first-task".to_string()));
  crates/mana-cli/src/commands/create/tests.rs:1172:        title: "Claimed task".to_string(),
  crates/mana-cli/src/commands/create/tests.rs:1199:    let unit_path = mana_dir.join("1-claimed-task.md");
  crates/mana-cli/src/commands/create/tests.rs:1204:    assert_eq!(unit.title, "Claimed task");
  crates/mana-cli/src/commands/create/tests.rs:1255:        title: "Unclaimed task".to_string(),
  crates/mana-cli/src/commands/create/tests.rs:1283:    let unit_path = mana_dir.join("1-unclaimed-task.md");
  crates/mana-cli/src/commands/status.rs:294:        let mut task = Unit::new("2", "Ready task");
  crates/mana-cli/src/commands/status.rs:295:        task.kind = UnitType::Task;
  crates/mana-cli/src/commands/status.rs:296:        task.verify = Some("true".to_string());
  crates/mana-cli/src/commands/status.rs:297:        task.to_file(mana_dir.join("2.yaml")).unwrap();
  crates/mana-cli/src/commands/doctor.rs:524:        unit2.to_file(mana_dir.join("2-task-two-in-md.md")).unwrap();
  crates/mana-cli/src/commands/doctor.rs:546:        unit1.to_file(mana_dir.join("1-task-one.md")).unwrap();
  crates/mana-cli/src/commands/doctor.rs:547:        unit2.to_file(mana_dir.join("2-task-two.md")).unwrap();
  crates/mana-cli/src/commands/doctor.rs:610:        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();
  crates/mana-cli/src/commands/doctor.rs:616:        fs::remove_file(mana_dir.join("1-task-one.md")).unwrap();
  crates/mana-cli/src/commands/doctor.rs:630:        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();
  crates/mana-cli/src/commands/doctor.rs:649:        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();
  crates/mana-cli/src/commands/doctor.rs:675:        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();
  crates/mana-cli/src/commands/claim.rs:20:            "Warning: Claiming an epic, not a task yet. Consider decomposing with: mana create \"child task\" --parent {} --verify \"test\"",
  crates/mana-cli/src/commands/claim.rs:172:        // Create unit without verify (this looks like an epic, not a task yet)
  crates/mana-cli/src/commands/claim.rs:190:        // Create unit with verify (this is a task)
  crates/mana-cli/src/commands/claim.rs:208:        let mut unit = Unit::new("1", "Vague task");
  crates/mana-cli/src/commands/tree.rs:144:        let unit1 = Unit::new("1", "Root task");
  crates/mana-cli/src/commands/tree.rs:195:        let b1 = Unit::new("1", "Open task");
  crates/mana-cli/src/commands/context.rs:127:/// Format child task summaries into a compact context section.
  crates/mana-cli/src/commands/context.rs:170:    // --agent-prompt: output the full structured prompt-shaped task packet
  crates/mana-cli/src/commands/create/mod.rs:45:    /// Mark the new unit as an epic instead of a task.
  crates/mana-cli/src/commands/next.rs:103:            println!("No ready units. Create one with: mana create \"task\" --verify \"cmd\"");
  crates/mana-cli/src/commands/trace.rs:420:        let parent_unit = Unit::new("10", "parent task");
  crates/mana-cli/src/commands/trace.rs:424:        let mut dep_unit = Unit::new("11", "dep task");
  crates/mana-cli/src/commands/trace.rs:429:        let mut main_unit = Unit::new("12", "main task");
  crates/mana-cli/src/commands/run/plan.rs:480:        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();
  crates/mana-cli/src/commands/run/plan.rs:486:        unit2.to_file(mana_dir.join("2-task-two.md")).unwrap();
  crates/mana-cli/src/commands/run/plan.rs:504:        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();
  crates/mana-cli/src/commands/run/plan.rs:510:        unit2.to_file(mana_dir.join("2-task-two.md")).unwrap();
  crates/mana-cli/src/commands/run/plan.rs:529:        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();
  rg: src: No such file or directory (os error 2)
  rg: tests: No such file or directory (os error 2)
  ```


  ---
  2026-04-27T23:29:58.226918+00:00
  Recommended next feature/bug work after the SQLite stack is verified/published: inspect and likely execute this priority-1 compatibility fix. Context from cleanup: mana metadata now emits `kind: task` broadly, and prior status showed this unit specifically tracks parser/index compatibility around `task` vs older `job` naming. Do not start until pre-push verification/publication decision is settled unless user explicitly interrupts that flow.
labels:
- bug
- index
- compatibility
- close
- cli
verify: cd /Users/asher/mana && rg -n "unknown variant|kind.*task|enum.*Kind|UnitKind|\\btask\\b" crates src tests && cargo test kind task index close --quiet
attempts: 1
history:
- attempt: 1
  started_at: '2026-04-27T21:46:12.485239Z'
  finished_at: '2026-04-27T21:46:12.536687Z'
  duration_secs: 0.051
  result: fail
  exit_code: 2
  output_snippet: |-
    crates/mana-core/src/failure.rs:214:        return Some("- Agent went idle — it may be stuck in a loop or waiting for input. Try a more focused prompt or break the task into smaller steps.");
    crates/mana-core/src/failure.rs:217:        return Some("- Agent ran out of time. Consider increasing the timeout or simplifying the task scope.");
    crates/mana-core/src/ops/move_units.rs:207:        let mut unit = Unit::new("1.1", "Child task");
    crates/mana-core/src/ops/move_units.rs:208:        unit.slug = Some("child-task".to_string());
    crates/mana-core/src/ops/move_units.rs:213:        unit.to_file(src_units.join("1.1-child-task.md")).unwrap();
    crates/mana-core/src/ops/move_units.rs:218:        let moved = Unit::from_file(dst_units.join(format!("{}-child-task.md", new_id))).unwrap();
    crates/mana-core/src/ops/context.rs:669:        let mut child = Unit::new("1.1", "Child task");
    crates/mana-core/src/ops/tidy.rs:252:        let mut unit = Unit::new("1", "Done task");
    crates/mana-core/src/ops/tidy.rs:270:        let unit = Unit::new("1", "Open task");
    crates/mana-core/src/ops/tidy.rs:283:        let mut unit = Unit::new("1", "Done task");
    crates/mana-core/src/ops/batch_verify.rs:280:        unit.to_file(mana_dir.join(format!("{}-task-{}.md", id, slug)))
    crates/mana-core/src/ops/batch_verify.rs:377:        unit.to_file(mana_dir.join("1-task-1.md")).unwrap();
    crates/mana-core/src/ops/unarchive.rs:175:        let main_path = mana_dir.join("1-task.md");
    crates/mana-core/src/ops/status.rs:129:        let mut task = Unit::new("2", "Task");
    crates/mana-core/src/ops/status.rs:130:        task.kind = UnitType::Task;
    crates/mana-core/src/ops/status.rs:131:        task.verify = Some("cargo test task".to_string());
    crates/mana-core/src/ops/status.rs:132:        write_unit(&mana_dir, &task);
    crates/mana-core/src/ops/status.rs:153:        let mut ready_unit = Unit::new("1", "Ready task");
    crates/mana-core/src/ops/status.rs:158:        let goal_unit = Unit::new("2", "Goal task");
    crates/mana-core/src/ops/status.rs:162:        let mut claimed_unit = Unit::new("3", "Claimed task");

    ... (274 lines omitted) ...

    crates/mana-cli/src/commands/claim.rs:20:            "Warning: Claiming an epic, not a task yet. Consider decomposing with: mana create \"child task\" --parent {} --verify \"test\"",
    crates/mana-cli/src/commands/claim.rs:172:        // Create unit without verify (this looks like an epic, not a task yet)
    crates/mana-cli/src/commands/claim.rs:190:        // Create unit with verify (this is a task)
    crates/mana-cli/src/commands/claim.rs:208:        let mut unit = Unit::new("1", "Vague task");
    crates/mana-cli/src/commands/tree.rs:144:        let unit1 = Unit::new("1", "Root task");
    crates/mana-cli/src/commands/tree.rs:195:        let b1 = Unit::new("1", "Open task");
    crates/mana-cli/src/commands/context.rs:127:/// Format child task summaries into a compact context section.
    crates/mana-cli/src/commands/context.rs:170:    // --agent-prompt: output the full structured prompt-shaped task packet
    crates/mana-cli/src/commands/create/mod.rs:45:    /// Mark the new unit as an epic instead of a task.
    crates/mana-cli/src/commands/next.rs:103:            println!("No ready units. Create one with: mana create \"task\" --verify \"cmd\"");
    crates/mana-cli/src/commands/trace.rs:420:        let parent_unit = Unit::new("10", "parent task");
    crates/mana-cli/src/commands/trace.rs:424:        let mut dep_unit = Unit::new("11", "dep task");
    crates/mana-cli/src/commands/trace.rs:429:        let mut main_unit = Unit::new("12", "main task");
    crates/mana-cli/src/commands/run/plan.rs:480:        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();
    crates/mana-cli/src/commands/run/plan.rs:486:        unit2.to_file(mana_dir.join("2-task-two.md")).unwrap();
    crates/mana-cli/src/commands/run/plan.rs:504:        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();
    crates/mana-cli/src/commands/run/plan.rs:510:        unit2.to_file(mana_dir.join("2-task-two.md")).unwrap();
    crates/mana-cli/src/commands/run/plan.rs:529:        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();
    rg: src: No such file or directory (os error 2)
    rg: tests: No such file or directory (os error 2)
kind: task
---

Goal: Fix the mana CLI/index compatibility bug where mana-generated indexes contain `kind: task`, but the installed CLI parser rejects `task` and only accepts `epic`, `job`, and `fact`.

Observed in `/Users/asher/aush` on 2026-04-27 while trying to close verified AUSH units:
- `mana close .1.1.2` initially hung because the unit had a broad/stale verify, then close/index paths exposed metadata incompatibility.
- `mana doctor` failed with:
  - `Error: Failed to parse index.yaml`
  - `units.[0].kind: unknown variant task, expected one of epic, job, fact at line 18 column 9`
- Manual normalization of project `.mana/index.yaml` and unit files from `kind: task` to `kind: job` allowed `mana doctor` / `mana close --check` to return.
- Running `mana next` later regenerated `/Users/asher/aush/.mana/index.yaml` back to `kind: task`; subsequent `mana status` failed parsing again.
- This means the writer/rebuilder and reader/parser disagree. Manual metadata repair is temporary and unsafe.
- Native `mana create` in `/Users/asher/mana` also failed because this checkout has no `.mana/config.yaml`, so this bug was recorded by writing this unit file directly using the existing markdown unit format.

Reproduction:
1. In a project whose mana graph has jobs/tasks, run `mana next -n 3` or another command that rebuilds the index.
2. Inspect `.mana/index.yaml`; it may contain `kind: task`.
3. Run `mana status` or `mana doctor`.
4. Observe parser failure: `unknown variant task, expected one of epic, job, fact`.

Likely fix options:
- Preferred compatibility fix: update the reader/parser enum/deserializer to accept `task` as an alias for `job`, while preserving canonical output semantics deliberately.
- Also audit writers/index rebuild code so they either consistently emit `job` for legacy compatibility or intentionally emit `task` only after all parsers accept it.
- Add regression coverage around index rebuild followed by `status`, `next`, and `close --check` parsing the regenerated index.

Files likely relevant:
- mana core model/type definitions for unit kind (`UnitKind`, equivalent enum/deserializer)
- index rebuild/writer code that serializes units into `.mana/index.yaml`
- CLI commands: `doctor`, `next`, `status`, `close --check`
- tests around index parse/rebuild/close

In scope:
- Make `task`/`job` kind compatibility robust across reader and writer.
- Add regression tests that fail on the current AUSH scenario.
- Document canonical kind behavior if user-visible.

Out of scope:
- AUSH builtin parity or release work.
- Redesigning mana storage.
- Manual cleanup of any individual project’s `.mana` graph except as a test fixture.

Do not:
- Require users to hand-edit `index.yaml`.
- Fix only `doctor` while leaving `next/status/close` broken.
- Retry close loops without resolving reader/writer kind compatibility.
