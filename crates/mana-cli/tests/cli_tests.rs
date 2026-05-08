//! Integration tests for the `bn create` command validation.

use std::fs;

use mana::commands::create::{cmd_create, CreateArgs};
use mana::config::Config;
use tempfile::TempDir;

/// Setup a test environment with a .mana directory and config.
fn setup_test_env() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let mana_dir = dir.path().join(".mana");
    fs::create_dir(&mana_dir).unwrap();

    let config = Config {
        project: "test-cli".to_string(),
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
fn create_claim_without_criteria_shows_error() {
    let (_dir, mana_dir) = setup_test_env();

    let args = CreateArgs {
        title: "Bad claimed unit".to_string(),
        handle: None,
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
        decisions: vec![],
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(result.is_err(), "PASS: --claim without criteria rejected");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("validation criteria"),
        "PASS: error mentions validation criteria"
    );
    assert!(
        err_msg.contains("--acceptance or --verify"),
        "PASS: error mentions --acceptance or --verify"
    );
}

#[test]
fn create_claim_with_acceptance_succeeds() {
    let (_dir, mana_dir) = setup_test_env();

    let args = CreateArgs {
        title: "Claimed with acceptance".to_string(),
        handle: None,
        description: None,
        acceptance: Some("Feature works".to_string()),
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
        decisions: vec![],
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(result.is_ok(), "PASS: --claim with --acceptance accepted");
}

#[test]
fn create_claim_with_verify_succeeds() {
    let (_dir, mana_dir) = setup_test_env();

    let args = CreateArgs {
        title: "Claimed with verify".to_string(),
        handle: None,
        description: None,
        acceptance: None,
        notes: None,
        design: None,
        verify: Some("cargo test create_claim_with_verify_succeeds".to_string()),
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
        decisions: vec![],
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(result.is_ok(), "PASS: --claim with --verify accepted");
}

#[test]
fn create_without_claim_no_criteria_succeeds() {
    let (_dir, mana_dir) = setup_test_env();

    let args = CreateArgs {
        title: "Goal unit".to_string(),
        handle: None,
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
        decisions: vec![],
        force: false,
    };

    let result = cmd_create(&mana_dir, args);
    assert!(
        result.is_ok(),
        "PASS: create without --claim needs no criteria"
    );
}

#[test]
fn create_claim_with_parent_no_criteria_succeeds() {
    let (_dir, mana_dir) = setup_test_env();

    // Create parent first
    let parent_args = CreateArgs {
        title: "Parent".to_string(),
        handle: None,
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
        decisions: vec![],
        force: false,
    };
    cmd_create(&mana_dir, parent_args).unwrap();

    // Create child with --claim but no criteria — exempt because --parent
    let child_args = CreateArgs {
        title: "Child claimed".to_string(),
        handle: None,
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
        by: Some("agent-2".to_string()),
        verify_timeout: None,
        decisions: vec![],
        force: false,
    };

    let result = cmd_create(&mana_dir, child_args);
    assert!(
        result.is_ok(),
        "PASS: --claim --parent exempt from criteria check"
    );
}
