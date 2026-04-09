//! Integration tests for the mana-core public API.
//!
//! Tests the full unit lifecycle and all major API entry points using
//! real filesystem I/O in tempdir — no mocking, no side effects.

use std::fs;
use std::path::Path;

use mana_core::api::*;
use mana_core::config::Config;
use mana_core::ops::claim::ClaimParams;
use mana_core::ops::close::{CloseOpts, CloseOutcome};
use mana_core::ops::create::CreateParams;
use mana_core::ops::fact::FactParams;
use mana_core::ops::list::ListParams;
use mana_core::ops::update::UpdateParams;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Create a minimal .mana/ directory with a default config and return the path.
fn setup_mana_dir() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().unwrap();
    let mana_dir = dir.path().join(".mana");
    fs::create_dir(&mana_dir).unwrap();
    Config {
        project: "test-project".to_string(),
        next_id: 1,
        auto_close_parent: true,
        ..Default::default()
    }
    .save(&mana_dir)
    .unwrap();
    (dir, mana_dir)
}

fn create_params(title: &str) -> CreateParams {
    CreateParams {
        title: title.to_string(),
        ..Default::default()
    }
}

fn create_params_with_verify(title: &str, verify: &str) -> CreateParams {
    CreateParams {
        title: title.to_string(),
        verify: Some(verify.to_string()),
        force: true, // skip verify-lint for test commands
        ..Default::default()
    }
}

fn force_claim(mana_dir: &Path, id: &str) -> mana_core::ops::claim::ClaimResult {
    claim_unit(
        mana_dir,
        id,
        ClaimParams {
            by: Some("test-agent".to_string()),
            force: true,
        },
    )
    .unwrap()
}

// ---------------------------------------------------------------------------
// Unit lifecycle: create → claim → close
// ---------------------------------------------------------------------------

#[test]
fn lifecycle_create_claim_force_close() {
    let (_dir, mana_dir) = setup_mana_dir();

    // Create
    let r = create_unit(&mana_dir, create_params_with_verify("Fix the bug", "true")).unwrap();
    assert_eq!(r.unit.id, "1");
    assert_eq!(r.unit.title, "Fix the bug");
    assert_eq!(r.unit.status, Status::Open);
    assert!(r.path.exists());

    // Retrieve it
    let unit = get_unit(&mana_dir, "1").unwrap();
    assert_eq!(unit.title, "Fix the bug");

    // Claim it
    let claim_r = force_claim(&mana_dir, "1");
    assert_eq!(claim_r.unit.status, Status::InProgress);
    assert_eq!(claim_r.claimer, "test-agent");

    // Close with --force (skip verify)
    let outcome = close_unit(
        &mana_dir,
        "1",
        CloseOpts {
            reason: Some("Done".to_string()),
            force: true,
            defer_verify: false,
        },
    )
    .unwrap();

    match outcome {
        CloseOutcome::Closed(result) => {
            assert_eq!(result.unit.status, Status::Closed);
            assert!(result.unit.closed_at.is_some());
            assert!(result.archive_path.exists());
        }
        other => panic!("Expected Closed, got {:?}", other),
    }

    // Unit no longer findable in active index
    let err = get_unit(&mana_dir, "1");
    assert!(err.is_err());

    // But findable in archive
    let archived = get_archived_unit(&mana_dir, "1").unwrap();
    assert_eq!(archived.id, "1");
    assert_eq!(archived.status, Status::Closed);
}

#[test]
fn lifecycle_verify_passes_and_closes() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_unit(&mana_dir, create_params_with_verify("Passing test", "true")).unwrap();
    force_claim(&mana_dir, "1");

    let outcome = close_unit(
        &mana_dir,
        "1",
        CloseOpts {
            reason: None,
            force: false, // let it run verify
            defer_verify: false,
        },
    )
    .unwrap();

    assert!(
        matches!(outcome, CloseOutcome::Closed(_)),
        "Expected Closed with passing verify"
    );
}

#[test]
fn lifecycle_verify_fails_and_stays_open() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_unit(
        &mana_dir,
        create_params_with_verify("Failing test", "false"),
    )
    .unwrap();
    force_claim(&mana_dir, "1");

    let outcome = close_unit(
        &mana_dir,
        "1",
        CloseOpts {
            reason: None,
            force: false,
            defer_verify: false,
        },
    )
    .unwrap();

    assert!(
        matches!(outcome, CloseOutcome::VerifyFailed(_)),
        "Expected VerifyFailed, got {:?}",
        outcome
    );

    // Unit is still in active index (open or in-progress)
    let unit = get_unit(&mana_dir, "1").unwrap();
    assert_ne!(unit.status, Status::Closed);
}

#[test]
fn lifecycle_release_returns_unit_to_open() {
    let (_dir, mana_dir) = setup_mana_dir();
    create_unit(&mana_dir, create_params("Claimable")).unwrap();

    force_claim(&mana_dir, "1");
    let unit = get_unit(&mana_dir, "1").unwrap();
    assert_eq!(unit.status, Status::InProgress);

    let rel = release_unit(&mana_dir, "1").unwrap();
    assert_eq!(rel.unit.status, Status::Open);
    assert!(rel.unit.claimed_by.is_none());

    // Attempt log reflects the abandoned attempt
    assert_eq!(rel.unit.attempt_log.len(), 1);
    assert_eq!(
        rel.unit.attempt_log[0].outcome,
        mana_core::unit::AttemptOutcome::Abandoned
    );
}

#[test]
fn lifecycle_fail_unit_reopens_it() {
    let (_dir, mana_dir) = setup_mana_dir();
    create_unit(&mana_dir, create_params("Will fail")).unwrap();
    force_claim(&mana_dir, "1");

    let unit = fail_unit(&mana_dir, "1", Some("Out of time".to_string())).unwrap();
    assert_eq!(unit.status, Status::Open);
    assert!(unit.claimed_by.is_none());
    // Notes should record the failure reason
    assert!(
        unit.notes.as_deref().unwrap_or("").contains("Out of time")
            || unit.close_reason.as_deref().unwrap_or("").contains("Out")
    );
}

// ---------------------------------------------------------------------------
// Index and listing
// ---------------------------------------------------------------------------

#[test]
fn index_rebuilds_from_unit_files() {
    let (_dir, mana_dir) = setup_mana_dir();
    create_unit(&mana_dir, create_params("Alpha")).unwrap();
    create_unit(&mana_dir, create_params("Beta")).unwrap();
    create_unit(&mana_dir, create_params("Gamma")).unwrap();

    let index = load_index(&mana_dir).unwrap();
    assert_eq!(index.units.len(), 3);

    let titles: Vec<&str> = index.units.iter().map(|e| e.title.as_str()).collect();
    assert!(titles.contains(&"Alpha"));
    assert!(titles.contains(&"Beta"));
    assert!(titles.contains(&"Gamma"));
}

#[test]
fn list_units_returns_open_by_default() {
    let (_dir, mana_dir) = setup_mana_dir();
    create_unit(&mana_dir, create_params("Open one")).unwrap();
    create_unit(&mana_dir, create_params_with_verify("Will close", "true")).unwrap();

    force_claim(&mana_dir, "2");
    close_unit(
        &mana_dir,
        "2",
        CloseOpts {
            reason: None,
            force: true,
            defer_verify: false,
        },
    )
    .unwrap();

    let units = list_units(&mana_dir, &ListParams::default()).unwrap();
    // Default list should not include closed units
    for entry in &units {
        assert_ne!(entry.status, Status::Closed, "Closed unit in default list");
    }
    assert!(
        units.iter().any(|e| e.title == "Open one"),
        "Open unit missing from list"
    );
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

#[test]
fn update_unit_fields() {
    let (_dir, mana_dir) = setup_mana_dir();
    create_unit(&mana_dir, create_params("Original title")).unwrap();

    let r = update_unit(
        &mana_dir,
        "1",
        UpdateParams {
            title: Some("Updated title".to_string()),
            notes: Some("Added a note".to_string()),
            priority: Some(1),
            add_label: Some("backend".to_string()),
            ..Default::default()
        },
    )
    .unwrap();

    assert_eq!(r.unit.title, "Updated title");
    assert_eq!(r.unit.priority, 1);
    assert!(r.unit.labels.contains(&"backend".to_string()));
    assert!(r
        .unit
        .notes
        .as_deref()
        .unwrap_or("")
        .contains("Added a note"));

    // Verify persisted
    let unit = get_unit(&mana_dir, "1").unwrap();
    assert_eq!(unit.title, "Updated title");
    assert_eq!(unit.priority, 1);
}

#[test]
fn update_notes_appends_rather_than_replaces() {
    let (_dir, mana_dir) = setup_mana_dir();
    create_unit(&mana_dir, create_params("Notes test")).unwrap();

    update_unit(
        &mana_dir,
        "1",
        UpdateParams {
            notes: Some("First note".to_string()),
            ..Default::default()
        },
    )
    .unwrap();

    update_unit(
        &mana_dir,
        "1",
        UpdateParams {
            notes: Some("Second note".to_string()),
            ..Default::default()
        },
    )
    .unwrap();

    let unit = get_unit(&mana_dir, "1").unwrap();
    let notes = unit.notes.as_deref().unwrap_or("");
    assert!(notes.contains("First note"), "First note lost: {}", notes);
    assert!(
        notes.contains("Second note"),
        "Second note missing: {}",
        notes
    );
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

#[test]
fn delete_unit_removes_it() {
    let (_dir, mana_dir) = setup_mana_dir();
    create_unit(&mana_dir, create_params("To be deleted")).unwrap();

    let r = delete_unit(&mana_dir, "1").unwrap();
    assert_eq!(r.title, "To be deleted");

    // No longer findable
    assert!(get_unit(&mana_dir, "1").is_err());
}

// ---------------------------------------------------------------------------
// Dependency graph and ordering
// ---------------------------------------------------------------------------

#[test]
fn dependency_resolution_respects_order() {
    let (_dir, mana_dir) = setup_mana_dir();

    // 1 → 2 (2 depends on 1)
    create_unit(&mana_dir, create_params("Foundation")).unwrap(); // id=1
    create_unit(&mana_dir, create_params("Depends on Foundation")).unwrap(); // id=2

    add_dep(&mana_dir, "2", "1").unwrap();

    let index = load_index(&mana_dir).unwrap();
    let order = topological_sort(&index).unwrap();

    let pos_1 = order.iter().position(|id| id == "1").unwrap();
    let pos_2 = order.iter().position(|id| id == "2").unwrap();
    assert!(
        pos_1 < pos_2,
        "Unit 1 must come before unit 2 in sort order"
    );
}

#[test]
fn ready_units_requires_deps_closed() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_unit(&mana_dir, create_params_with_verify("Dep", "true")).unwrap(); // 1
    create_unit(
        &mana_dir,
        create_params_with_verify("Blocked on dep", "true"),
    )
    .unwrap(); // 2

    add_dep(&mana_dir, "2", "1").unwrap();

    let index = load_index(&mana_dir).unwrap();
    let ready = ready_units(&index);

    // Only unit 1 should be ready; unit 2 is blocked by unclosed dep
    let ready_ids: Vec<&str> = ready.iter().map(|e| e.id.as_str()).collect();
    assert!(ready_ids.contains(&"1"), "Unit 1 should be ready");
    assert!(
        !ready_ids.contains(&"2"),
        "Unit 2 should be blocked by dep on 1"
    );
}

#[test]
fn ready_units_unblocked_after_dep_closed() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_unit(&mana_dir, create_params_with_verify("First", "true")).unwrap(); // 1
    create_unit(&mana_dir, create_params_with_verify("Second", "true")).unwrap(); // 2
    add_dep(&mana_dir, "2", "1").unwrap();

    // Close unit 1
    force_claim(&mana_dir, "1");
    close_unit(
        &mana_dir,
        "1",
        CloseOpts {
            reason: None,
            force: true,
            defer_verify: false,
        },
    )
    .unwrap();

    // After archiving unit 1, the ready_units() API (index-only) won't see it as closed.
    // Use compute_ready_queue() which checks the archive for satisfied deps.
    let queue = compute_ready_queue(&mana_dir, None, false).unwrap();
    let ready_ids: Vec<&str> = queue.units.iter().map(|u| u.id.as_str()).collect();
    assert!(
        ready_ids.contains(&"2"),
        "Unit 2 should be ready after dep closed (via compute_ready_queue)"
    );
}

#[test]
fn cycle_detection_prevents_circular_deps() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_unit(&mana_dir, create_params("A")).unwrap(); // 1
    create_unit(&mana_dir, create_params("B")).unwrap(); // 2
    create_unit(&mana_dir, create_params("C")).unwrap(); // 3

    add_dep(&mana_dir, "2", "1").unwrap(); // 2 depends on 1
    add_dep(&mana_dir, "3", "2").unwrap(); // 3 depends on 2

    // Adding 1 → 3 would create a cycle: 1→2→3→1
    let result = add_dep(&mana_dir, "1", "3");
    assert!(result.is_err(), "Adding cycle dep should fail");
    let err = result.unwrap_err().to_string();
    assert!(
        err.to_lowercase().contains("cycle"),
        "Error should mention cycle: {}",
        err
    );
}

#[test]
fn detect_cycle_returns_true_for_cycle() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_unit(&mana_dir, create_params("A")).unwrap(); // 1
    create_unit(&mana_dir, create_params("B")).unwrap(); // 2
    add_dep(&mana_dir, "2", "1").unwrap(); // 2 depends on 1

    let index = load_index(&mana_dir).unwrap();

    // Adding 1→2 would be a cycle
    assert!(detect_cycle(&index, "1", "2").unwrap());
    // Adding 2→1 is fine (already exists, not a NEW cycle)
    assert!(!detect_cycle(&index, "2", "1").unwrap());
}

#[test]
fn remove_dep_works() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_unit(&mana_dir, create_params("A")).unwrap();
    create_unit(&mana_dir, create_params("B")).unwrap();
    add_dep(&mana_dir, "2", "1").unwrap();

    let unit = get_unit(&mana_dir, "2").unwrap();
    assert!(unit.dependencies.contains(&"1".to_string()));

    remove_dep(&mana_dir, "2", "1").unwrap();

    let unit = get_unit(&mana_dir, "2").unwrap();
    assert!(!unit.dependencies.contains(&"1".to_string()));
}

#[test]
fn dependency_graph_has_correct_structure() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_unit(&mana_dir, create_params("Root")).unwrap(); // 1
    create_unit(&mana_dir, create_params("Child A")).unwrap(); // 2
    create_unit(&mana_dir, create_params("Child B")).unwrap(); // 3

    add_dep(&mana_dir, "2", "1").unwrap();
    add_dep(&mana_dir, "3", "1").unwrap();

    let index = load_index(&mana_dir).unwrap();
    let graph = dependency_graph(&index);

    assert_eq!(graph.nodes.len(), 3);
    assert!(graph.nodes.contains_key("1"));
    assert!(graph.nodes.contains_key("2"));
    assert!(graph.nodes.contains_key("3"));

    // Unit 2 and 3 both depend on 1
    assert!(graph.edges["2"].contains(&"1".to_string()));
    assert!(graph.edges["3"].contains(&"1".to_string()));
    // Unit 1 has no dependencies
    assert!(graph.edges["1"].is_empty());
}

// ---------------------------------------------------------------------------
// Tree building
// ---------------------------------------------------------------------------

#[test]
fn tree_reflects_parent_child_hierarchy() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_unit(&mana_dir, create_params("Parent")).unwrap(); // 1
    create_unit(
        &mana_dir,
        CreateParams {
            title: "Child A".to_string(),
            parent: Some("1".to_string()),
            force: true,
            ..Default::default()
        },
    )
    .unwrap(); // 1.1
    create_unit(
        &mana_dir,
        CreateParams {
            title: "Child B".to_string(),
            parent: Some("1".to_string()),
            force: true,
            ..Default::default()
        },
    )
    .unwrap(); // 1.2

    let tree = get_tree(&mana_dir, "1").unwrap();
    assert_eq!(tree.id, "1");
    assert_eq!(tree.title, "Parent");
    assert_eq!(tree.children.len(), 2);

    let child_titles: Vec<&str> = tree.children.iter().map(|c| c.title.as_str()).collect();
    assert!(child_titles.contains(&"Child A"));
    assert!(child_titles.contains(&"Child B"));
}

// ---------------------------------------------------------------------------
// Config: load, save, extends inheritance
// ---------------------------------------------------------------------------

#[test]
fn config_load_and_save_roundtrip() {
    let (_dir, mana_dir) = setup_mana_dir();

    let mut config = Config::load(&mana_dir).unwrap();
    assert_eq!(config.project, "test-project");
    assert_eq!(config.next_id, 1);

    config.max_loops = 42;
    config.run = Some("pi run {id}".to_string());
    config.save(&mana_dir).unwrap();

    let reloaded = Config::load(&mana_dir).unwrap();
    assert_eq!(reloaded.max_loops, 42);
    assert_eq!(reloaded.run, Some("pi run {id}".to_string()));
}

#[test]
fn config_next_id_increments_with_units() {
    let (_dir, mana_dir) = setup_mana_dir();

    let config = Config::load(&mana_dir).unwrap();
    assert_eq!(config.next_id, 1);

    create_unit(&mana_dir, create_params("One")).unwrap();
    let config = Config::load(&mana_dir).unwrap();
    assert_eq!(config.next_id, 2);

    create_unit(&mana_dir, create_params("Two")).unwrap();
    let config = Config::load(&mana_dir).unwrap();
    assert_eq!(config.next_id, 3);
}

#[test]
fn config_extends_merges_parent_settings() {
    let root = tempfile::TempDir::new().unwrap();

    // Write a base config at the root
    let base_config_path = root.path().join("base-config.yaml");
    fs::write(
        &base_config_path,
        "project: base\nnext_id: 1\nmax_loops: 99\nrun: \"base-runner {id}\"\n",
    )
    .unwrap();

    // Create a child project with extends
    let child_dir = root.path().join("child");
    fs::create_dir_all(&child_dir).unwrap();
    let mana_dir = child_dir.join(".mana");
    fs::create_dir_all(&mana_dir).unwrap();

    Config {
        project: "child".to_string(),
        next_id: 1,
        extends: vec![base_config_path.to_string_lossy().to_string()],
        ..Default::default()
    }
    .save(&mana_dir)
    .unwrap();

    // Load with extends resolution
    let resolved = Config::load_with_extends(&mana_dir).unwrap();
    // max_loops from parent (99) should be inherited when child doesn't override
    assert_eq!(resolved.max_loops, 99);
}

// ---------------------------------------------------------------------------
// Facts lifecycle
// ---------------------------------------------------------------------------

#[test]
fn fact_create_sets_correct_type_and_label() {
    let (_dir, mana_dir) = setup_mana_dir();

    let r = create_fact(
        &mana_dir,
        FactParams {
            title: "Project has mana dir".to_string(),
            verify: "test -d .mana".to_string(),
            description: Some("The project has a .mana directory".to_string()),
            paths: None,
            ttl_days: Some(30),
            pass_ok: true,
        },
    )
    .unwrap();

    assert_eq!(r.unit.unit_type, "fact");
    assert!(r.unit.labels.contains(&"fact".to_string()));
    assert!(r.unit.stale_after.is_some());
    assert!(r.unit.verify.is_some());
}

#[test]
fn fact_requires_verify_command() {
    let (_dir, mana_dir) = setup_mana_dir();

    let result = create_fact(
        &mana_dir,
        FactParams {
            title: "Unverifiable claim".to_string(),
            verify: "".to_string(),
            description: None,
            paths: None,
            ttl_days: None,
            pass_ok: false,
        },
    );

    assert!(result.is_err(), "Empty verify should be rejected");
}

#[test]
fn fact_ttl_sets_stale_after() {
    let (_dir, mana_dir) = setup_mana_dir();

    let r = create_fact(
        &mana_dir,
        FactParams {
            title: "Short-lived fact".to_string(),
            verify: "test -d .mana".to_string(),
            description: None,
            paths: None,
            ttl_days: Some(7),
            pass_ok: true,
        },
    )
    .unwrap();

    let stale_after = r.unit.stale_after.unwrap();
    let now = chrono::Utc::now();
    let delta = stale_after - now;

    // stale_after should be ~7 days in the future (within 1 day tolerance)
    assert!(
        delta.num_days() >= 6 && delta.num_days() <= 8,
        "Expected ~7 day TTL, got {} days",
        delta.num_days()
    );
}

#[test]
fn verify_facts_runs_and_reports_results() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_fact(
        &mana_dir,
        FactParams {
            title: "Passing fact".to_string(),
            verify: "test -d .mana".to_string(),
            description: None,
            paths: None,
            ttl_days: Some(30),
            pass_ok: true,
        },
    )
    .unwrap();

    create_fact(
        &mana_dir,
        FactParams {
            title: "Failing fact".to_string(),
            verify: "false".to_string(),
            description: None,
            paths: None,
            ttl_days: Some(30),
            pass_ok: true,
        },
    )
    .unwrap();

    let result = verify_facts(&mana_dir).unwrap();
    assert_eq!(result.total_facts, 2);
    assert_eq!(result.failing_count, 1);
}

// ---------------------------------------------------------------------------
// Index staleness and rebuild
// ---------------------------------------------------------------------------

#[test]
fn index_is_rebuilt_when_stale() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_unit(&mana_dir, create_params("A")).unwrap();
    create_unit(&mana_dir, create_params("B")).unwrap();

    // Delete the index file to simulate staleness
    let index_path = mana_dir.join("index.yaml");
    if index_path.exists() {
        fs::remove_file(&index_path).unwrap();
    }

    // load_index should rebuild
    let index = load_index(&mana_dir).unwrap();
    assert_eq!(index.units.len(), 2);

    // Index file should be recreated
    assert!(index_path.exists());
}

// ---------------------------------------------------------------------------
// Status summary and stats
// ---------------------------------------------------------------------------

#[test]
fn get_status_returns_correct_counts() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_unit(&mana_dir, create_params("Open unit")).unwrap();
    create_unit(&mana_dir, create_params_with_verify("Will close", "true")).unwrap();

    force_claim(&mana_dir, "2");
    close_unit(
        &mana_dir,
        "2",
        CloseOpts {
            reason: None,
            force: true,
            defer_verify: false,
        },
    )
    .unwrap();

    let summary = get_status(&mana_dir).unwrap();
    // There should be at least one open unit and one closed
    assert!(
        summary.goals.len() + summary.ready.len() + summary.claimed.len() >= 1,
        "At least one open unit expected"
    );
}

#[test]
fn get_stats_tracks_completion_percentage() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_unit(&mana_dir, create_params("A")).unwrap();
    create_unit(&mana_dir, create_params_with_verify("B", "true")).unwrap();

    let stats_before = get_stats(&mana_dir).unwrap();
    assert_eq!(stats_before.open, 2);
    assert_eq!(stats_before.closed, 0);
    assert_eq!(stats_before.total, 2);

    force_claim(&mana_dir, "2");
    close_unit(
        &mana_dir,
        "2",
        CloseOpts {
            reason: None,
            force: true,
            defer_verify: false,
        },
    )
    .unwrap();

    let stats_after = get_stats(&mana_dir).unwrap();
    // After archive, the active index only has unit A (open). Archived units are
    // not counted in active index stats — stats reads from active index only.
    assert_eq!(stats_after.open, 1);
    assert_eq!(stats_after.total, 1);
    // Closed count in active index is 0 (archived units are removed from active index)
    assert_eq!(stats_after.closed, 0);
}

// ---------------------------------------------------------------------------
// Ready queue and orchestration
// ---------------------------------------------------------------------------

#[test]
fn compute_ready_queue_returns_units_with_no_unmet_deps() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_unit(&mana_dir, create_params_with_verify("Free unit", "true")).unwrap(); // 1
    create_unit(&mana_dir, create_params_with_verify("Blocked unit", "true")).unwrap(); // 2
    add_dep(&mana_dir, "2", "1").unwrap();

    let queue = compute_ready_queue(&mana_dir, None, false).unwrap();
    let ready_ids: Vec<&str> = queue.units.iter().map(|u| u.id.as_str()).collect();

    // Only unit 1 is ready; unit 2's dep is unsatisfied so it won't appear in the
    // ready queue at all — compute_ready_queue only includes units whose deps are met.
    assert!(ready_ids.contains(&"1"), "Unit 1 should be ready");
    assert!(
        !ready_ids.contains(&"2"),
        "Unit 2 should not be in ready queue"
    );
    // In simulation mode unit 2 should appear
    let sim_queue = compute_ready_queue(&mana_dir, None, true).unwrap();
    let sim_ids: Vec<&str> = sim_queue.units.iter().map(|u| u.id.as_str()).collect();
    assert!(
        sim_ids.contains(&"2"),
        "Unit 2 should appear in simulation mode"
    );
}

#[test]
fn compute_ready_queue_simulation_shows_all() {
    let (_dir, mana_dir) = setup_mana_dir();

    create_unit(&mana_dir, create_params_with_verify("A", "true")).unwrap(); // 1
    create_unit(&mana_dir, create_params_with_verify("B", "true")).unwrap(); // 2
    add_dep(&mana_dir, "2", "1").unwrap();

    let queue = compute_ready_queue(&mana_dir, None, true).unwrap();
    let ready_ids: Vec<&str> = queue.units.iter().map(|u| u.id.as_str()).collect();

    // In simulation mode, all units with verify commands should appear
    assert!(ready_ids.contains(&"1"));
    assert!(ready_ids.contains(&"2"));
}

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

#[test]
fn get_unit_returns_error_for_missing_id() {
    let (_dir, mana_dir) = setup_mana_dir();

    let result = get_unit(&mana_dir, "99");
    assert!(result.is_err());
}

#[test]
fn claim_nonexistent_unit_errors() {
    let (_dir, mana_dir) = setup_mana_dir();

    let result = claim_unit(
        &mana_dir,
        "999",
        ClaimParams {
            by: None,
            force: true,
        },
    );
    assert!(result.is_err());
}

#[test]
fn close_nonexistent_unit_errors() {
    let (_dir, mana_dir) = setup_mana_dir();

    let result = close_unit(
        &mana_dir,
        "999",
        CloseOpts {
            reason: None,
            force: true,
            defer_verify: false,
        },
    );
    assert!(result.is_err());
}

#[test]
fn claim_already_in_progress_unit_errors() {
    let (_dir, mana_dir) = setup_mana_dir();
    create_unit(&mana_dir, create_params("Claimed")).unwrap();
    force_claim(&mana_dir, "1");

    let result = claim_unit(
        &mana_dir,
        "1",
        ClaimParams {
            by: Some("other-agent".to_string()),
            force: true,
        },
    );
    assert!(result.is_err(), "Double-claiming should fail");
}

// ---------------------------------------------------------------------------
// Record attempt
// ---------------------------------------------------------------------------

#[test]
fn record_attempt_appends_to_log() {
    let (_dir, mana_dir) = setup_mana_dir();
    create_unit(&mana_dir, create_params("Log test")).unwrap();

    let now = chrono::Utc::now();
    let attempt = mana_core::unit::AttemptRecord {
        num: 1,
        outcome: mana_core::unit::AttemptOutcome::Success,
        notes: Some("First attempt passed".to_string()),
        agent: Some("test-agent".to_string()),
        started_at: Some(now),
        finished_at: Some(now),
        autonomy_observation: None,
    };

    let updated = record_attempt(&mana_dir, "1", attempt).unwrap();
    assert_eq!(updated.attempt_log.len(), 1);
    assert_eq!(
        updated.attempt_log[0].outcome,
        mana_core::unit::AttemptOutcome::Success
    );
    assert_eq!(
        updated.attempt_log[0].notes,
        Some("First attempt passed".to_string())
    );
}
