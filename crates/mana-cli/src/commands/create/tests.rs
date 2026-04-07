use super::*;
use std::fs;

use crate::config::Config;
use crate::index::Index;
use crate::unit::{OnFailAction, Status, Unit, UnitKind};
use tempfile::TempDir;

fn setup_mana_dir_with_config() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let mana_dir = dir.path().join(".mana");
    fs::create_dir(&mana_dir).unwrap();

    let config = Config {
        project: "test".to_string(),
        next_id: 1,
        auto_close_parent: true,
        run: None,
        plan: None,
        max_loops: 10,
        max_concurrent: 4,
        poll_interval: 30,
        extends: vec![],
        rules_file: None,
        file_locking: false,
        worktree: false,
        on_close: None,
        on_fail: None,
        verify_timeout: None,
        review: None,
        user: None,
        user_email: None,
        auto_commit: false,
        commit_template: None,
        research: None,
        run_model: None,
        plan_model: None,
        review_model: None,
        research_model: None,
        batch_verify: false,
        memory_reserve_mb: 0,
        notify: None,
    };
    config.save(&mana_dir).unwrap();

    (dir, mana_dir)
}

#[test]
fn create_minimal_unit() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "First task".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("cargo test unit::check".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    cmd_create(&mana_dir, args).unwrap();

    // Check the unit file exists with new naming convention
    let unit_path = mana_dir.join("1-first-task.md");
    assert!(unit_path.exists());

    // Verify content
    let unit = Unit::from_file(&unit_path).unwrap();
    assert_eq!(unit.id, "1");
    assert_eq!(unit.title, "First task");
    assert_eq!(unit.slug, Some("first-task".to_string()));
}

#[test]
fn create_allows_unit_without_verify_or_acceptance() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "Goal unit".to_string(),
        description: Some("A parent/goal unit with no verify".to_string()),
        acceptance: None,
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(
        result.is_ok(),
        "Should allow unit without verify or acceptance"
    );

    let unit_path = mana_dir.join("1-goal-unit.md");
    assert!(unit_path.exists());
    let unit = Unit::from_file(&unit_path).unwrap();
    assert_eq!(unit.title, "Goal unit");
    assert!(unit.verify.is_none());
    assert!(unit.acceptance.is_none());
}

#[test]
fn create_increments_id() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    // Create first unit
    let args1 = CreateArgs {
        title: "First".to_string(),
        description: None,
        acceptance: Some("Done".to_string()),
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    cmd_create(&mana_dir, args1).unwrap();

    // Create second unit
    let args2 = CreateArgs {
        title: "Second".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("false".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    cmd_create(&mana_dir, args2).unwrap();

    // Verify both exist with correct IDs and new filenames
    let unit1 = Unit::from_file(mana_dir.join("1-first.md")).unwrap();
    let unit2 = Unit::from_file(mana_dir.join("2-second.md")).unwrap();
    assert_eq!(unit1.id, "1");
    assert_eq!(unit2.id, "2");
}

#[test]
fn create_with_parent_assigns_child_id() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    // Create parent unit
    let parent_args = CreateArgs {
        title: "Parent".to_string(),
        description: None,
        acceptance: Some("Children complete".to_string()),
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    cmd_create(&mana_dir, parent_args).unwrap();

    // Create child unit
    let child_args = CreateArgs {
        title: "Child 1".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("cargo test unit::check".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: Some("1".to_string()),
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    cmd_create(&mana_dir, child_args).unwrap();

    // Verify child ID is 1.1 with new filename
    let unit = Unit::from_file(mana_dir.join("1.1-child-1.md")).unwrap();
    assert_eq!(unit.id, "1.1");
    assert_eq!(unit.parent, Some("1".to_string()));
}

#[test]
fn create_multiple_children() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    // Create parent
    let parent_args = CreateArgs {
        title: "Parent".to_string(),
        description: None,
        acceptance: Some("All children complete".to_string()),
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    cmd_create(&mana_dir, parent_args).unwrap();

    // Create multiple children
    for i in 1..=3 {
        let child_args = CreateArgs {
            title: format!("Child {}", i),
            description: None,
            acceptance: None,
            notes: None,
            design: None,
            verify: Some("cargo test unit::check".to_string()),
            priority: None,
            labels: None,
            assignee: None,
            deps: None,
            parent: Some("1".to_string()),
            produces: None,
            requires: None,
            paths: None,
            on_fail: None,
            pass_ok: true,
            feature: false,
            epic: false,
            claim: false,
            by: None,
            verify_timeout: None,
            decisions: Vec::new(),
            force: true,
        };
        cmd_create(&mana_dir, child_args).unwrap();
    }

    // Verify all children exist with new naming
    for i in 1..=3 {
        let expected_id = format!("1.{}", i);
        let expected_slug = format!("child-{}", i);
        let path = mana_dir.join(format!("{}-{}.md", expected_id, expected_slug));
        assert!(path.exists(), "Child {} should exist at {:?}", i, path);

        let unit = Unit::from_file(&path).unwrap();
        assert_eq!(unit.id, expected_id);
    }
}

#[test]
fn create_with_all_fields() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "Complex unit".to_string(),
        description: Some("A description".to_string()),
        acceptance: Some("All tests pass".to_string()),
        notes: Some("Some notes".to_string()),
        design: Some("Design decision".to_string()),
        verify: None,
        priority: Some(1),
        labels: Some("bug,critical".to_string()),
        assignee: Some("alice".to_string()),
        deps: Some("2,3".to_string()),
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    cmd_create(&mana_dir, args).unwrap();

    let unit = Unit::from_file(mana_dir.join("1-complex-unit.md")).unwrap();
    assert_eq!(unit.title, "Complex unit");
    assert_eq!(unit.description, Some("A description".to_string()));
    assert_eq!(unit.acceptance, Some("All tests pass".to_string()));
    assert_eq!(unit.notes, Some("Some notes".to_string()));
    assert_eq!(unit.design, Some("Design decision".to_string()));
    assert_eq!(unit.priority, 1);
    assert_eq!(unit.labels, vec!["bug", "critical"]);
    assert_eq!(unit.assignee, Some("alice".to_string()));
    assert_eq!(unit.dependencies, vec!["2", "3"]);
}

#[test]
fn create_epic_sets_kind_epic() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "Epic parent".to_string(),
        description: Some("Top-level grouping record".to_string()),
        acceptance: None,
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let id = cmd_create(&mana_dir, args).unwrap();
    let unit = Unit::from_file(mana_dir.join(format!("{}-epic-parent.md", id))).unwrap();
    assert_eq!(unit.kind, UnitKind::Epic);
    assert!(unit.verify.is_none());
}

#[test]
fn create_updates_index() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "Indexed unit".to_string(),
        description: None,
        acceptance: Some("Indexed correctly".to_string()),
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    cmd_create(&mana_dir, args).unwrap();

    // Load and check index
    let index = Index::load(&mana_dir).unwrap();
    assert_eq!(index.units.len(), 1);
    assert_eq!(index.units[0].id, "1");
    assert_eq!(index.units[0].title, "Indexed unit");
    assert_eq!(index.units[0].kind, UnitKind::Epic);
}

#[test]
fn assign_child_id_starts_at_1() {
    let dir = TempDir::new().unwrap();
    let mana_dir = dir.path().join(".mana");
    fs::create_dir(&mana_dir).unwrap();

    let id = assign_child_id(&mana_dir, "parent").unwrap();
    assert_eq!(id, "parent.1");
}

#[test]
fn assign_child_id_finds_existing_children() {
    let dir = TempDir::new().unwrap();
    let mana_dir = dir.path().join(".mana");
    fs::create_dir(&mana_dir).unwrap();

    // Create some child files with new naming convention
    let unit1 = Unit::new("parent.1", "Child 1");
    let unit2 = Unit::new("parent.2", "Child 2");
    let unit5 = Unit::new("parent.5", "Child 5");

    unit1.to_file(mana_dir.join("parent.1-child-1.md")).unwrap();
    unit2.to_file(mana_dir.join("parent.2-child-2.md")).unwrap();
    unit5.to_file(mana_dir.join("parent.5-child-5.md")).unwrap();

    let id = assign_child_id(&mana_dir, "parent").unwrap();
    assert_eq!(id, "parent.6");
}

#[test]
fn create_rejects_priority_too_high() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "Invalid priority unit".to_string(),
        description: None,
        acceptance: Some("Done".to_string()),
        notes: None,
        design: None,
        verify: None,
        priority: Some(5),
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(result.is_err(), "Should reject priority > 4");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("priority"),
        "Error should mention priority"
    );
}

#[test]
fn create_accepts_valid_priorities() {
    for priority in 0..=4 {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let args = CreateArgs {
            title: format!("Unit with priority {}", priority),
            description: None,
            acceptance: Some("Done".to_string()),
            notes: None,
            design: None,
            verify: None,
            priority: Some(priority),
            labels: None,
            assignee: None,
            deps: None,
            parent: None,
            produces: None,
            requires: None,
            paths: None,
            on_fail: None,
            pass_ok: true,
            feature: false,
            epic: false,
            claim: false,
            by: None,
            verify_timeout: None,
            decisions: Vec::new(),
            force: false,
        };

        let result = cmd_create(&mana_dir, args);
        assert!(result.is_ok(), "Priority {} should be valid", priority);
    }
}

// =========================================================================
// Hook Integration Tests
// =========================================================================

#[test]
fn pre_create_hook_accepts_unit_creation() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, mana_dir) = setup_mana_dir_with_config();
    let project_dir = dir.path();
    let hooks_dir = mana_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).unwrap();

    // Enable trust and create a pre-create hook that succeeds
    crate::hooks::create_trust(project_dir).unwrap();

    let hook_path = hooks_dir.join("pre-create");
    fs::write(&hook_path, "#!/bin/bash\nexit 0").unwrap();

    #[cfg(unix)]
    fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();

    let args = CreateArgs {
        title: "Unit with accepting hook".to_string(),
        description: None,
        acceptance: Some("Done".to_string()),
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    // Unit should be created
    let result = cmd_create(&mana_dir, args);
    assert!(
        result.is_ok(),
        "Creation should succeed with accepting pre-create hook"
    );

    // Verify unit was created
    let unit_path = mana_dir.join("1-unit-with-accepting-hook.md");
    assert!(unit_path.exists(), "Unit file should exist");
}

#[test]
fn pre_create_hook_rejects_unit_creation() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, mana_dir) = setup_mana_dir_with_config();
    let project_dir = dir.path();
    let hooks_dir = mana_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).unwrap();

    // Enable trust and create a pre-create hook that fails
    crate::hooks::create_trust(project_dir).unwrap();

    let hook_path = hooks_dir.join("pre-create");
    fs::write(&hook_path, "#!/bin/bash\nexit 1").unwrap();

    #[cfg(unix)]
    fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();

    let args = CreateArgs {
        title: "Unit with rejecting hook".to_string(),
        description: None,
        acceptance: Some("Done".to_string()),
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    // Unit creation should fail
    let result = cmd_create(&mana_dir, args);
    assert!(
        result.is_err(),
        "Creation should fail with rejecting pre-create hook"
    );

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Pre-create hook rejected"),
        "Error should indicate hook rejection"
    );

    // Verify unit was NOT created
    let unit_path = mana_dir.join("1-unit-with-rejecting-hook.md");
    assert!(
        !unit_path.exists(),
        "Unit file should NOT exist when pre-create hook rejects"
    );
}

#[test]
fn post_create_hook_runs_after_creation() {
    use std::os::unix::fs::PermissionsExt;

    let (dir, mana_dir) = setup_mana_dir_with_config();
    let project_dir = dir.path();
    let hooks_dir = mana_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).unwrap();

    // Enable trust and create a post-create hook that writes to a file
    crate::hooks::create_trust(project_dir).unwrap();

    let hook_path = hooks_dir.join("post-create");
    let marker_file = project_dir.join("hook-executed.txt");
    let marker_file_str = marker_file.to_string_lossy().to_string();

    // Create hook that writes to marker file
    let hook_script = format!(
        "#!/bin/bash\necho 'post-create executed' >> '{}'\nexit 0",
        marker_file_str
    );
    fs::write(&hook_path, hook_script).unwrap();

    #[cfg(unix)]
    fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();

    let args = CreateArgs {
        title: "Unit with post-create hook".to_string(),
        description: None,
        acceptance: Some("Done".to_string()),
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    // Create unit
    let result = cmd_create(&mana_dir, args);
    assert!(result.is_ok(), "Creation should succeed");

    // Verify unit was created
    let unit_path = mana_dir.join("1-unit-with-post-create-hook.md");
    assert!(unit_path.exists(), "Unit file should exist");

    // Verify post-create hook ran (marker file exists)
    assert!(
        marker_file.exists(),
        "Post-create hook should have run and created marker file"
    );
}

#[test]
fn post_create_hook_failure_does_not_break_creation() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, mana_dir) = setup_mana_dir_with_config();
    let project_dir = dir.path();
    let hooks_dir = mana_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).unwrap();

    // Enable trust and create a post-create hook that fails
    crate::hooks::create_trust(project_dir).unwrap();

    let hook_path = hooks_dir.join("post-create");
    fs::write(&hook_path, "#!/bin/bash\nexit 1").unwrap();

    #[cfg(unix)]
    fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();

    let args = CreateArgs {
        title: "Unit with failing post-create hook".to_string(),
        description: None,
        acceptance: Some("Done".to_string()),
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    // Unit creation should STILL succeed (post-create failures are non-blocking)
    let result = cmd_create(&mana_dir, args);
    assert!(
        result.is_ok(),
        "Creation should succeed even if post-create hook fails"
    );

    // Verify unit WAS created
    let unit_path = mana_dir.join("1-unit-with-failing-post-create-hook.md");
    assert!(
        unit_path.exists(),
        "Unit file should exist even when post-create hook fails"
    );
}

#[test]
fn untrusted_hooks_are_silently_skipped() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, mana_dir) = setup_mana_dir_with_config();
    let hooks_dir = mana_dir.join("hooks");
    fs::create_dir_all(&hooks_dir).unwrap();

    // DO NOT enable trust - hooks should be skipped

    let hook_path = hooks_dir.join("pre-create");
    fs::write(&hook_path, "#!/bin/bash\nexit 1").unwrap();

    #[cfg(unix)]
    fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();

    let args = CreateArgs {
        title: "Unit with untrusted hook".to_string(),
        description: None,
        acceptance: Some("Done".to_string()),
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    // Unit creation should succeed (untrusted hooks are skipped)
    let result = cmd_create(&mana_dir, args);
    assert!(
        result.is_ok(),
        "Creation should succeed when hooks are untrusted"
    );

    // Verify unit WAS created
    let unit_path = mana_dir.join("1-unit-with-untrusted-hook.md");
    assert!(
        unit_path.exists(),
        "Unit file should exist when hooks are untrusted"
    );
}

#[test]
fn default_rejects_passing_verify() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "Cheating test".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("grep -q 'project: test' .mana/config.yaml".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: false, // default: fail-first enforced
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("verify command already passes"));
}

#[test]
fn default_accepts_failing_verify() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "Real test".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("false".to_string()), // always fails
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: false, // default: fail-first enforced
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(result.is_ok());

    // Unit should be created
    let unit_path = mana_dir.join("1-real-test.md");
    assert!(unit_path.exists());

    // Should have fail_first set in the unit
    let unit = Unit::from_file(&unit_path).unwrap();
    assert!(unit.fail_first);
}

#[test]
fn pass_ok_skips_fail_first_check() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "Passing verify ok".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("grep -q 'project: test' .mana/config.yaml".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(result.is_ok());

    // Unit should be created
    let unit_path = mana_dir.join("1-passing-verify-ok.md");
    assert!(unit_path.exists());

    // Should NOT have fail_first set
    let unit = Unit::from_file(&unit_path).unwrap();
    assert!(!unit.fail_first);
}

#[test]
fn no_verify_skips_fail_first_check() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "No verify".to_string(),
        description: None,
        acceptance: Some("Done".to_string()),
        notes: None,
        design: None,
        verify: None, // no verify command — fail-first not applicable
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: false,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(result.is_ok());

    // Should NOT have fail_first set (no verify)
    let unit_path = mana_dir.join("1-no-verify.md");
    let unit = Unit::from_file(&unit_path).unwrap();
    assert!(!unit.fail_first);
}

mod lint {
    use super::*;

    #[test]
    fn create_rejects_verify_lint_errors_without_force() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let args = CreateArgs {
            title: "Linted error".to_string(),
            description: None,
            acceptance: None,
            notes: None,
            design: None,
            verify: Some("true".to_string()),
            priority: None,
            labels: None,
            assignee: None,
            deps: None,
            parent: None,
            produces: None,
            requires: None,
            paths: None,
            on_fail: None,
            pass_ok: true,
            feature: false,
            epic: false,
            claim: false,
            by: None,
            verify_timeout: None,
            decisions: Vec::new(),
            force: false,
        };

        let result = cmd_create(&mana_dir, args);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("lint error"));
    }

    #[test]
    fn create_allows_verify_lint_errors_with_force() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let args = CreateArgs {
            title: "Forced linted error".to_string(),
            description: None,
            acceptance: None,
            notes: None,
            design: None,
            verify: Some("true".to_string()),
            priority: None,
            labels: None,
            assignee: None,
            deps: None,
            parent: None,
            produces: None,
            requires: None,
            paths: None,
            on_fail: None,
            pass_ok: true,
            feature: false,
            epic: false,
            claim: false,
            by: None,
            verify_timeout: None,
            decisions: Vec::new(),
            force: true,
        };

        let result = cmd_create(&mana_dir, args);
        assert!(result.is_ok());
    }

    #[test]
    fn create_allows_verify_lint_warnings() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let args = CreateArgs {
            title: "Linted warning".to_string(),
            description: None,
            acceptance: None,
            notes: None,
            design: None,
            verify: Some("cargo test unit::check".to_string()),
            priority: None,
            labels: None,
            assignee: None,
            deps: None,
            parent: None,
            produces: None,
            requires: None,
            paths: None,
            on_fail: None,
            pass_ok: true,
            feature: false,
            epic: false,
            claim: false,
            by: None,
            verify_timeout: None,
            decisions: Vec::new(),
            force: false,
        };

        let result = cmd_create(&mana_dir, args);
        assert!(result.is_ok());
    }
}

// =========================================================================
// --claim Flag Tests
// =========================================================================

#[test]
fn create_with_claim_sets_in_progress() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "Claimed task".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("cargo test unit::check".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,
        claim: true,
        by: Some("agent-1".to_string()),
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    cmd_create(&mana_dir, args).unwrap();

    let unit_path = mana_dir.join("1-claimed-task.md");
    assert!(unit_path.exists());

    let unit = Unit::from_file(&unit_path).unwrap();
    assert_eq!(unit.id, "1");
    assert_eq!(unit.title, "Claimed task");
    assert_eq!(unit.status, Status::InProgress);
    assert_eq!(unit.claimed_by, Some("agent-1".to_string()));
    assert!(unit.claimed_at.is_some());
}

#[test]
fn create_with_claim_without_by() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "Anon claimed".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("false".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,
        claim: true,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    cmd_create(&mana_dir, args).unwrap();

    let unit_path = mana_dir.join("1-anon-claimed.md");
    let unit = Unit::from_file(&unit_path).unwrap();
    assert_eq!(unit.status, Status::InProgress);
    // When no --by is given, identity is auto-resolved from config/git.
    // claimed_by may be Some(...) or None depending on environment.
    assert!(unit.claimed_at.is_some());
}

#[test]
fn create_without_claim_stays_open() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "Unclaimed task".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("cargo test unit::check".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    cmd_create(&mana_dir, args).unwrap();

    let unit_path = mana_dir.join("1-unclaimed-task.md");
    let unit = Unit::from_file(&unit_path).unwrap();
    assert_eq!(unit.status, Status::Open);
    assert_eq!(unit.claimed_by, None);
    assert_eq!(unit.claimed_at, None);
}

#[test]
fn create_with_claim_and_parent() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    // Create parent first
    let parent_args = CreateArgs {
        title: "Parent".to_string(),
        description: None,
        acceptance: Some("Children done".to_string()),
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    cmd_create(&mana_dir, parent_args).unwrap();

    // Create child with --claim
    let child_args = CreateArgs {
        title: "Child claimed".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("cargo test unit::check".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: Some("1".to_string()),
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,
        claim: true,
        by: Some("agent-2".to_string()),
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    cmd_create(&mana_dir, child_args).unwrap();

    let unit_path = mana_dir.join("1.1-child-claimed.md");
    let unit = Unit::from_file(&unit_path).unwrap();
    assert_eq!(unit.id, "1.1");
    assert_eq!(unit.parent, Some("1".to_string()));
    assert_eq!(unit.status, Status::InProgress);
    assert_eq!(unit.claimed_by, Some("agent-2".to_string()));
}

// =========================================================================
// --claim Validation: require --acceptance or --verify
// =========================================================================

#[test]
fn create_claim_rejects_missing_validation_criteria() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "No criteria claimed".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,
        claim: true,
        by: Some("agent-1".to_string()),
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(
        result.is_err(),
        "Should reject --claim without --acceptance or --verify"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("validation criteria"),
        "Error should mention validation criteria, got: {}",
        err_msg
    );
}

#[test]
fn create_claim_accepts_with_acceptance() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "Claimed with acceptance".to_string(),
        description: None,
        acceptance: Some("Done when tests pass".to_string()),
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,
        claim: true,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(result.is_ok(), "Should accept --claim with --acceptance");
}

#[test]
fn create_claim_accepts_with_verify() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "Claimed with verify".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("cargo test unit::check".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,
        claim: true,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(result.is_ok(), "Should accept --claim with --verify");
}

#[test]
fn create_claim_with_parent_exempt_from_validation() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    // Create parent first
    let parent_args = CreateArgs {
        title: "Parent".to_string(),
        description: None,
        acceptance: Some("Children done".to_string()),
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    cmd_create(&mana_dir, parent_args).unwrap();

    // Create child with --claim but no acceptance/verify
    // Should succeed because child units with --parent are exempt
    let child_args = CreateArgs {
        title: "Child no criteria".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: Some("1".to_string()),
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,
        claim: true,
        by: Some("agent-1".to_string()),
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let result = cmd_create(&mana_dir, child_args);
    assert!(
        result.is_ok(),
        "Should allow --claim --parent without --acceptance or --verify"
    );
}

#[test]
fn create_without_claim_exempt_from_validation() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    // Parent/goal units without --claim don't need acceptance or verify
    let args = CreateArgs {
        title: "Goal unit no criteria".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(
        result.is_ok(),
        "Should allow create without --claim and without criteria"
    );
}

// =========================================================================
// parse_on_fail Tests
// =========================================================================

#[test]
fn parse_on_fail_retry_bare() {
    let action = parse_on_fail("retry").unwrap();
    assert_eq!(
        action,
        OnFailAction::Retry {
            max: None,
            delay_secs: None
        }
    );
}

#[test]
fn parse_on_fail_retry_with_max() {
    let action = parse_on_fail("retry:5").unwrap();
    assert_eq!(
        action,
        OnFailAction::Retry {
            max: Some(5),
            delay_secs: None
        }
    );
}

#[test]
fn parse_on_fail_escalate_bare() {
    let action = parse_on_fail("escalate").unwrap();
    assert_eq!(
        action,
        OnFailAction::Escalate {
            priority: None,
            message: None
        }
    );
}

#[test]
fn parse_on_fail_escalate_with_priority_uppercase() {
    let action = parse_on_fail("escalate:P0").unwrap();
    assert_eq!(
        action,
        OnFailAction::Escalate {
            priority: Some(0),
            message: None
        }
    );
}

#[test]
fn parse_on_fail_escalate_with_priority_lowercase() {
    let action = parse_on_fail("escalate:p1").unwrap();
    assert_eq!(
        action,
        OnFailAction::Escalate {
            priority: Some(1),
            message: None
        }
    );
}

#[test]
fn parse_on_fail_escalate_with_priority_number() {
    let action = parse_on_fail("escalate:3").unwrap();
    assert_eq!(
        action,
        OnFailAction::Escalate {
            priority: Some(3),
            message: None
        }
    );
}

#[test]
fn parse_on_fail_rejects_invalid_action() {
    let result = parse_on_fail("unknown");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Unknown on-fail"));
}

#[test]
fn parse_on_fail_rejects_invalid_retry_max() {
    let result = parse_on_fail("retry:abc");
    assert!(result.is_err());
}

#[test]
fn parse_on_fail_rejects_priority_out_of_range() {
    let result = parse_on_fail("escalate:P5");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("priority"));
}

// =========================================================================
// cmd_create_next Tests
// =========================================================================

#[test]
fn create_next_depends_on_latest() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    // Create the first unit
    let args1 = CreateArgs {
        title: "First step".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("false".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    let id1 = cmd_create(&mana_dir, args1).unwrap();

    // Create second unit via create_next
    let args2 = CreateArgs {
        title: "Second step".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("false".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    let id2 = cmd_create_next(&mana_dir, args2).unwrap();

    // Verify the second unit depends on the first
    let unit2_path = mana_dir.join(format!("{}-second-step.md", id2));
    let unit2 = Unit::from_file(&unit2_path).unwrap();
    assert!(
        unit2.dependencies.contains(&id1),
        "Second unit should depend on first unit ({}), got deps: {:?}",
        id1,
        unit2.dependencies
    );
}

#[test]
fn create_next_chain_three_units() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    // Create first unit normally
    let args1 = CreateArgs {
        title: "Step one".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("false".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    let id1 = cmd_create(&mana_dir, args1).unwrap();

    // Chain second unit
    let args2 = CreateArgs {
        title: "Step two".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("false".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    let id2 = cmd_create_next(&mana_dir, args2).unwrap();

    // Chain third unit
    let args3 = CreateArgs {
        title: "Step three".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("false".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    let id3 = cmd_create_next(&mana_dir, args3).unwrap();

    // Verify chain: 1 <- 2 <- 3
    let unit2_path = mana_dir.join(format!("{}-step-two.md", id2));
    let unit2 = Unit::from_file(&unit2_path).unwrap();
    assert!(
        unit2.dependencies.contains(&id1),
        "Unit 2 should depend on unit 1"
    );

    let unit3_path = mana_dir.join(format!("{}-step-three.md", id3));
    let unit3 = Unit::from_file(&unit3_path).unwrap();
    assert!(
        unit3.dependencies.contains(&id2),
        "Unit 3 should depend on unit 2, got deps: {:?}",
        unit3.dependencies
    );
}

#[test]
fn create_next_merges_explicit_deps() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    // Create two units normally
    let args1 = CreateArgs {
        title: "First".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("false".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    cmd_create(&mana_dir, args1).unwrap();

    let args2 = CreateArgs {
        title: "Second".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("false".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    cmd_create(&mana_dir, args2).unwrap();

    // Create next with explicit deps — should merge @latest (2) + explicit (1)
    let args3 = CreateArgs {
        title: "Third".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("false".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: Some("1".to_string()),
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    let id3 = cmd_create_next(&mana_dir, args3).unwrap();

    let unit3_path = mana_dir.join(format!("{}-third.md", id3));
    let unit3 = Unit::from_file(&unit3_path).unwrap();
    assert!(
        unit3.dependencies.contains(&"1".to_string()),
        "Should have explicit dep on 1"
    );
    assert!(
        unit3.dependencies.contains(&"2".to_string()),
        "Should have auto dep on @latest (2)"
    );
}

#[test]
fn create_next_fails_with_no_units() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    // Try create next with no existing units — should fail
    let args = CreateArgs {
        title: "Orphan".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("false".to_string()),
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };
    let result = cmd_create_next(&mana_dir, args);
    assert!(result.is_err(), "Should fail with no existing units");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("No previous unit"),
        "Error should mention no previous unit, got: {}",
        err_msg
    );
}

// =========================================================================
// --feature Flag Tests
// =========================================================================

#[test]
fn create_feature_sets_feature_flag() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    let args = CreateArgs {
        title: "User onboarding flow".to_string(),
        description: Some("Product feature for onboarding".to_string()),
        acceptance: None,
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: true,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let id = cmd_create(&mana_dir, args).unwrap();
    let unit_path = mana_dir.join(format!("{}-user-onboarding-flow.md", id));
    assert!(unit_path.exists());

    let unit = Unit::from_file(&unit_path).unwrap();
    assert!(unit.feature, "Unit should have feature flag set");
    assert_eq!(unit.title, "User onboarding flow");
}

#[test]
fn create_feature_works_without_verify() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    // --feature should work without --verify (features have no verify gate)
    let args = CreateArgs {
        title: "Dashboard redesign".to_string(),
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: true,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(
        result.is_ok(),
        "Feature should be creatable without --verify"
    );

    let unit_path = mana_dir.join("1-dashboard-redesign.md");
    let unit = Unit::from_file(&unit_path).unwrap();
    assert!(unit.feature);
    assert!(unit.verify.is_none());
}

#[test]
fn create_without_feature_preserves_existing_behavior() {
    let (_dir, mana_dir) = setup_mana_dir_with_config();

    // Without --feature, existing behavior unchanged: non-claimed units
    // can still be created without verify (goal/parent units)
    let args = CreateArgs {
        title: "Regular unit".to_string(),
        description: None,
        acceptance: Some("Done".to_string()),
        notes: None,
        design: None,
        verify: None,
        priority: None,
        labels: None,
        assignee: None,
        deps: None,
        parent: None,
        produces: None,
        requires: None,
        paths: None,
        on_fail: None,
        pass_ok: true,
        feature: false,
        epic: false,

        claim: false,
        by: None,
        verify_timeout: None,
        decisions: Vec::new(),
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(result.is_ok(), "Non-feature unit should work as before");

    let unit_path = mana_dir.join("1-regular-unit.md");
    let unit = Unit::from_file(&unit_path).unwrap();
    assert!(!unit.feature, "Non-feature unit should have feature=false");
}
