use std::path::Path;

use anyhow::Result;
use mana_core::ops::claim as ops_claim;

/// Claim a unit for work.
///
/// Sets status to InProgress, records who claimed it and when.
/// The unit must be in Open status to be claimed.
///
/// If the unit has a verify command and `force` is false, fail-first units run
/// verify before claim. If it already passes, the claim is rejected (nothing to do).
/// Units with verify commands also record the current git HEAD SHA as
/// `checkpoint` so diff/review flows can compare against the claim baseline.
pub fn cmd_claim(mana_dir: &Path, id: &str, by: Option<String>, force: bool) -> Result<()> {
    let result = ops_claim::claim(mana_dir, id, ops_claim::ClaimParams { by, force })?;

    if result.is_goal {
        eprintln!(
            "Warning: Claiming an epic, not a task yet. Consider decomposing with: mana create \"child task\" --parent {} --verify \"test\"",
            id
        );
    }

    println!(
        "Claimed unit {}: {} (by {})",
        id, result.unit.title, result.claimer
    );
    Ok(())
}

/// Release a claim on a unit.
///
/// Clears claimed_by/claimed_at and sets status back to Open.
pub fn cmd_release(mana_dir: &Path, id: &str) -> Result<()> {
    let result = ops_claim::release(mana_dir, id)?;
    println!("Released claim on unit {}: {}", id, result.unit.title);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Index;
    use crate::unit::{AttemptOutcome, AttemptRecord, Status, Unit};
    use chrono::Utc;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_mana_dir() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        (dir, mana_dir)
    }

    #[test]
    fn test_claim_open_unit() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let unit = Unit::new("1", "Task");
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        cmd_claim(&mana_dir, "1", Some("alice".to_string()), true).unwrap();

        let updated = Unit::from_file(mana_dir.join("1.yaml")).unwrap();
        assert_eq!(updated.status, Status::InProgress);
        assert_eq!(updated.claimed_by, Some("alice".to_string()));
        assert!(updated.claimed_at.is_some());
    }

    #[test]
    fn test_claim_without_by() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let unit = Unit::new("1", "Task");
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        cmd_claim(&mana_dir, "1", None, true).unwrap();

        let updated = Unit::from_file(mana_dir.join("1.yaml")).unwrap();
        assert_eq!(updated.status, Status::InProgress);
        // When no --by is given, identity is auto-resolved from config/git.
        // claimed_by may be Some(...) or None depending on environment.
        assert!(updated.claimed_at.is_some());
    }

    #[test]
    fn test_claim_non_open_unit_fails() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let mut unit = Unit::new("1", "Task");
        unit.status = Status::InProgress;
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        let result = cmd_claim(&mana_dir, "1", Some("bob".to_string()), true);
        assert!(result.is_err());
    }

    #[test]
    fn test_claim_closed_unit_fails() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let mut unit = Unit::new("1", "Task");
        unit.status = Status::Closed;
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        let result = cmd_claim(&mana_dir, "1", Some("bob".to_string()), true);
        assert!(result.is_err());
    }

    #[test]
    fn test_claim_nonexistent_unit_fails() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let result = cmd_claim(&mana_dir, "99", Some("alice".to_string()), true);
        assert!(result.is_err());
    }

    #[test]
    fn test_release_claimed_unit() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let mut unit = Unit::new("1", "Task");
        unit.status = Status::InProgress;
        unit.claimed_by = Some("alice".to_string());
        unit.claimed_at = Some(Utc::now());
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        cmd_release(&mana_dir, "1").unwrap();

        let updated = Unit::from_file(mana_dir.join("1.yaml")).unwrap();
        assert_eq!(updated.status, Status::Open);
        assert_eq!(updated.claimed_by, None);
        assert_eq!(updated.claimed_at, None);
    }

    #[test]
    fn test_release_nonexistent_unit_fails() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let result = cmd_release(&mana_dir, "99");
        assert!(result.is_err());
    }

    #[test]
    fn test_claim_rebuilds_index() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let unit = Unit::new("1", "Task");
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        cmd_claim(&mana_dir, "1", Some("alice".to_string()), true).unwrap();

        let index = Index::load(&mana_dir).unwrap();
        assert_eq!(index.units.len(), 1);
        let entry = &index.units[0];
        assert_eq!(entry.status, Status::InProgress);
    }

    #[test]
    fn test_release_rebuilds_index() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let mut unit = Unit::new("1", "Task");
        unit.status = Status::InProgress;
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        cmd_release(&mana_dir, "1").unwrap();

        let index = Index::load(&mana_dir).unwrap();
        assert_eq!(index.units.len(), 1);
        let entry = &index.units[0];
        assert_eq!(entry.status, Status::Open);
    }

    #[test]
    fn test_claim_unit_without_verify_succeeds_with_warning() {
        let (_dir, mana_dir) = setup_test_mana_dir();

        // Create unit without verify (this looks like an epic, not a task yet)
        let unit = Unit::new("1", "Add authentication");
        // unit.verify is None by default
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        // Claim should succeed (warning is printed but doesn't block)
        let result = cmd_claim(&mana_dir, "1", Some("alice".to_string()), true);
        assert!(result.is_ok());

        let updated = Unit::from_file(mana_dir.join("1.yaml")).unwrap();
        assert_eq!(updated.status, Status::InProgress);
        assert_eq!(updated.claimed_by, Some("alice".to_string()));
    }

    #[test]
    fn test_claim_unit_with_verify_succeeds() {
        let (_dir, mana_dir) = setup_test_mana_dir();

        // Create unit with verify (this is a task)
        let mut unit = Unit::new("1", "Add login endpoint");
        unit.verify = Some("cargo test login".to_string());
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        // Claim should succeed without warning
        let result = cmd_claim(&mana_dir, "1", Some("alice".to_string()), true);
        assert!(result.is_ok());

        let updated = Unit::from_file(mana_dir.join("1.yaml")).unwrap();
        assert_eq!(updated.status, Status::InProgress);
    }

    #[test]
    fn test_claim_unit_with_empty_verify_warns() {
        let (_dir, mana_dir) = setup_test_mana_dir();

        // Create unit with empty verify string (should be treated as no verify)
        let mut unit = Unit::new("1", "Vague task");
        unit.verify = Some("   ".to_string()); // whitespace only
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        // Claim should succeed (warning is printed but doesn't block)
        let result = cmd_claim(&mana_dir, "1", Some("alice".to_string()), true);
        assert!(result.is_ok());

        let updated = Unit::from_file(mana_dir.join("1.yaml")).unwrap();
        assert_eq!(updated.status, Status::InProgress);
    }

    // =================================================================
    // verify_on_claim tests
    // =================================================================

    #[test]
    fn verify_on_claim_passing_verify_rejected() {
        let (_dir, mana_dir) = setup_test_mana_dir();

        // Unit with verify that passes immediately ("true" exits 0)
        let mut unit = Unit::new("1", "Already done");
        unit.verify = Some("true".to_string());
        unit.fail_first = true; // created without -p, enforces fail-first
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        // Claim without force — should be rejected because verify passes
        let result = cmd_claim(&mana_dir, "1", Some("alice".to_string()), false);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("verify already passes"));
        assert!(err_msg.contains("--force"));

        // Unit should still be open (claim was rejected)
        let unchanged = Unit::from_file(mana_dir.join("1.yaml")).unwrap();
        assert_eq!(unchanged.status, Status::Open);
    }

    #[test]
    fn verify_on_claim_failing_verify_succeeds() {
        let (_dir, mana_dir) = setup_test_mana_dir();

        // Unit with verify that fails ("false" exits 1)
        let mut unit = Unit::new("1", "Real work needed");
        unit.verify = Some("false".to_string());
        unit.fail_first = true; // created without -p, enforces fail-first
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        // Claim without force — should succeed because verify fails
        let result = cmd_claim(&mana_dir, "1", Some("alice".to_string()), false);
        assert!(result.is_ok());

        let updated = Unit::from_file(mana_dir.join("1.yaml")).unwrap();
        assert_eq!(updated.status, Status::InProgress);
        assert_eq!(updated.claimed_by, Some("alice".to_string()));
        assert!(
            updated.fail_first,
            "fail_first should be set when verify fails at claim time"
        );
    }

    #[test]
    fn verify_on_claim_force_overrides() {
        let (_dir, mana_dir) = setup_test_mana_dir();

        // Unit with verify that passes immediately
        let mut unit = Unit::new("1", "Force claim");
        unit.verify = Some("true".to_string());
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        // Claim with force — should succeed even though verify passes
        let result = cmd_claim(&mana_dir, "1", Some("alice".to_string()), true);
        assert!(result.is_ok());

        let updated = Unit::from_file(mana_dir.join("1.yaml")).unwrap();
        assert_eq!(updated.status, Status::InProgress);
        assert_eq!(updated.claimed_by, Some("alice".to_string()));
    }

    #[test]
    fn verify_on_claim_checkpoint_sha_stored() {
        let (_dir, mana_dir) = setup_test_mana_dir();

        // Unit with verify that fails
        let mut unit = Unit::new("1", "Checkpoint test");
        unit.verify = Some("false".to_string());
        unit.fail_first = true; // created without -p, enforces fail-first
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        // Initialize a git repo in the temp dir so we get a real SHA
        let project_root = mana_dir.parent().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(project_root)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(project_root)
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "init", "--allow-empty"])
            .current_dir(project_root)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();

        let result = cmd_claim(&mana_dir, "1", Some("alice".to_string()), false);
        assert!(result.is_ok());

        let updated = Unit::from_file(mana_dir.join("1.yaml")).unwrap();
        assert!(
            updated.checkpoint.is_some(),
            "checkpoint SHA should be stored"
        );
        let sha = updated.checkpoint.unwrap();
        assert_eq!(sha.len(), 40, "SHA should be 40 hex chars, got: {}", sha);
        assert!(
            sha.chars().all(|c| c.is_ascii_hexdigit()),
            "SHA should be hex"
        );
    }

    #[test]
    fn verify_on_claim_no_verify_skips_check() {
        let (_dir, mana_dir) = setup_test_mana_dir();

        // Unit without verify — should not run verify check
        let unit = Unit::new("1", "No verify");
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        let result = cmd_claim(&mana_dir, "1", Some("alice".to_string()), false);
        assert!(result.is_ok());

        let updated = Unit::from_file(mana_dir.join("1.yaml")).unwrap();
        assert_eq!(updated.status, Status::InProgress);
        assert!(
            !updated.fail_first,
            "fail_first should not be set without verify"
        );
        assert!(updated.checkpoint.is_none(), "no checkpoint without verify");
    }

    // =====================================================================
    // Attempt Tracking Tests
    // =====================================================================

    #[test]
    fn claim_starts_attempt() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let unit = Unit::new("1", "Task");
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        cmd_claim(&mana_dir, "1", Some("agent-1".to_string()), true).unwrap();

        let updated = Unit::from_file(mana_dir.join("1.yaml")).unwrap();
        assert_eq!(updated.attempt_log.len(), 1);
        assert_eq!(updated.attempt_log[0].num, 1);
        assert_eq!(updated.attempt_log[0].agent, Some("agent-1".to_string()));
        assert!(updated.attempt_log[0].started_at.is_some());
        assert!(updated.attempt_log[0].finished_at.is_none());
    }

    #[test]
    fn release_marks_attempt_abandoned() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let mut unit = Unit::new("1", "Task");
        unit.status = Status::InProgress;
        unit.claimed_by = Some("agent-1".to_string());
        unit.attempt_log.push(AttemptRecord {
            num: 1,
            outcome: AttemptOutcome::Abandoned,
            notes: None,
            agent: Some("agent-1".to_string()),
            started_at: Some(Utc::now()),
            finished_at: None,
            autonomy_observation: None,
        });
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        cmd_release(&mana_dir, "1").unwrap();

        let updated = Unit::from_file(mana_dir.join("1.yaml")).unwrap();
        assert_eq!(updated.attempt_log.len(), 1);
        assert_eq!(updated.attempt_log[0].outcome, AttemptOutcome::Abandoned);
        assert!(updated.attempt_log[0].finished_at.is_some());
    }

    #[test]
    fn multiple_claims_accumulate_attempts() {
        let (_dir, mana_dir) = setup_test_mana_dir();
        let unit = Unit::new("1", "Task");
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        // First claim
        cmd_claim(&mana_dir, "1", Some("agent-1".to_string()), true).unwrap();
        // Release
        cmd_release(&mana_dir, "1").unwrap();
        // Second claim
        cmd_claim(&mana_dir, "1", Some("agent-2".to_string()), true).unwrap();

        let updated = Unit::from_file(mana_dir.join("1.yaml")).unwrap();
        assert_eq!(updated.attempt_log.len(), 2);
        assert_eq!(updated.attempt_log[0].num, 1);
        assert_eq!(updated.attempt_log[0].outcome, AttemptOutcome::Abandoned);
        assert!(updated.attempt_log[0].finished_at.is_some());
        assert_eq!(updated.attempt_log[1].num, 2);
        assert_eq!(updated.attempt_log[1].agent, Some("agent-2".to_string()));
        assert!(updated.attempt_log[1].finished_at.is_none());
    }
}
