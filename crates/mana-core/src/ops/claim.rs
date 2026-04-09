use std::path::{Path, PathBuf};
use std::process::Command as ShellCommand;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::config::resolve_identity;
use crate::discovery::find_unit_file;
use crate::index::Index;
use crate::unit::{AttemptOutcome, AttemptRecord, Status, Unit};

/// Parameters for claiming a unit.
pub struct ClaimParams {
    /// Who is claiming the unit. If None, resolved from config/git/env.
    pub by: Option<String>,
    /// Skip verify-on-claim check.
    pub force: bool,
}

/// Result of successfully claiming a unit.
#[derive(Debug)]
pub struct ClaimResult {
    pub unit: Unit,
    pub path: PathBuf,
    /// The resolved claimer identity.
    pub claimer: String,
    /// Whether the unit had no verify command (a GOAL, not a SPEC).
    pub is_goal: bool,
}

/// Result of releasing a claim on a unit.
pub struct ReleaseResult {
    pub unit: Unit,
    pub path: PathBuf,
}

/// Try to get the current git HEAD SHA. Returns None if not in a git repo.
fn git_head_sha(working_dir: &Path) -> Option<String> {
    ShellCommand::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(working_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Run a verify command and return whether it passed (exit 0).
fn run_verify_check(verify_cmd: &str, project_root: &Path) -> Result<bool> {
    let output = ShellCommand::new("sh")
        .args(["-c", verify_cmd])
        .current_dir(project_root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .with_context(|| format!("Failed to execute verify command: {}", verify_cmd))?;

    Ok(output.success())
}

/// Claim a unit for work.
///
/// Sets status to InProgress, records who claimed it and when.
/// The unit must be in Open status to be claimed.
///
/// If the unit has a verify command and `force` is false, the verify command
/// is run first for fail-first units. If it already passes, the claim is
/// rejected (nothing to do). The current git HEAD SHA is stored as
/// `checkpoint` for any claimed unit with a verify command so later review/
/// diff flows can compare against the attempt baseline.
pub fn claim(mana_dir: &Path, id: &str, params: ClaimParams) -> Result<ClaimResult> {
    let unit_path = find_unit_file(mana_dir, id).map_err(|_| anyhow!("Unit not found: {}", id))?;
    let mut unit =
        Unit::from_file(&unit_path).with_context(|| format!("Failed to load unit: {}", id))?;

    if unit.status != Status::Open {
        return Err(anyhow!(
            "Unit {} is {} -- only open units can be claimed",
            id,
            unit.status
        ));
    }

    let has_verify = unit.verify.as_ref().is_some_and(|v| !v.trim().is_empty());
    let is_goal = !unit.is_dispatchable_job();
    let project_root = mana_dir
        .parent()
        .ok_or_else(|| anyhow!("Cannot determine project root from units dir"))?;

    // Verify-on-claim: run verify before granting claim (TDD enforcement)
    // Skip when fail_first is false (unit created with -p / pass-ok)
    if has_verify && !params.force && unit.fail_first {
        let verify_cmd = unit.verify.as_ref().unwrap();

        let passed = run_verify_check(verify_cmd, project_root)?;

        if passed {
            return Err(anyhow!(
                "Cannot claim unit {}: verify already passes\n\n\
                 The verify command succeeded before any work was done.\n\
                 This means either the test is bogus or the work is already complete.\n\n\
                 Use --force to override.",
                id
            ));
        }

        // Verify failed — good, this proves the test is meaningful
        unit.fail_first = true;
    }

    if has_verify {
        unit.checkpoint = git_head_sha(project_root);

        // Freeze the judge: hash the verify command so we can detect changes at close time.
        if let Some(ref verify_cmd) = unit.verify {
            let mut hasher = Sha256::new();
            hasher.update(verify_cmd.as_bytes());
            unit.verify_hash = Some(format!("{:x}", hasher.finalize()));
        }
    }

    // Resolve identity: explicit --by > resolved identity > "anonymous"
    let resolved_by = params.by.or_else(|| resolve_identity(mana_dir));
    let claimer = resolved_by
        .clone()
        .unwrap_or_else(|| "anonymous".to_string());

    let now = Utc::now();
    unit.status = Status::InProgress;
    unit.claimed_by = resolved_by.clone();
    unit.claimed_at = Some(now);
    unit.updated_at = now;

    // Start a new attempt in the attempt log (for memory system tracking)
    let attempt_num = unit.attempt_log.len() as u32 + 1;
    unit.attempt_log.push(AttemptRecord {
        num: attempt_num,
        outcome: AttemptOutcome::Abandoned, // default until close/release updates it
        notes: None,
        agent: resolved_by,
        started_at: Some(now),
        finished_at: None,
        autonomy_observation: None,
    });

    unit.to_file(&unit_path)
        .with_context(|| format!("Failed to save unit: {}", id))?;

    // Rebuild index
    let index = Index::build(mana_dir).with_context(|| "Failed to rebuild index")?;
    index
        .save(mana_dir)
        .with_context(|| "Failed to save index")?;

    Ok(ClaimResult {
        unit,
        path: unit_path,
        claimer,
        is_goal,
    })
}

/// Release a claim on a unit.
///
/// Clears claimed_by/claimed_at and sets status back to Open.
/// Marks the current attempt as abandoned.
pub fn release(mana_dir: &Path, id: &str) -> Result<ReleaseResult> {
    let unit_path = find_unit_file(mana_dir, id).map_err(|_| anyhow!("Unit not found: {}", id))?;
    let mut unit =
        Unit::from_file(&unit_path).with_context(|| format!("Failed to load unit: {}", id))?;

    let now = Utc::now();

    // Finalize the current attempt as abandoned (if one is in progress)
    if let Some(attempt) = unit.attempt_log.last_mut() {
        if attempt.finished_at.is_none() {
            attempt.outcome = AttemptOutcome::Abandoned;
            attempt.finished_at = Some(now);
        }
    }

    unit.claimed_by = None;
    unit.claimed_at = None;
    unit.status = Status::Open;
    unit.updated_at = now;

    unit.to_file(&unit_path)
        .with_context(|| format!("Failed to save unit: {}", id))?;

    // Rebuild index
    let index = Index::build(mana_dir).with_context(|| "Failed to rebuild index")?;
    index
        .save(mana_dir)
        .with_context(|| "Failed to save index")?;

    Ok(ReleaseResult {
        unit,
        path: unit_path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ops::create::{self, tests::minimal_params};
    use std::fs;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let bd = dir.path().join(".mana");
        fs::create_dir(&bd).unwrap();
        Config {
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
        }
        .save(&bd)
        .unwrap();
        (dir, bd)
    }

    fn force_params(by: Option<&str>) -> ClaimParams {
        ClaimParams {
            by: by.map(String::from),
            force: true,
        }
    }

    fn strict_params(by: Option<&str>) -> ClaimParams {
        ClaimParams {
            by: by.map(String::from),
            force: false,
        }
    }

    #[test]
    fn claim_open_unit() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();

        let result = claim(&bd, "1", force_params(Some("alice"))).unwrap();
        assert_eq!(result.unit.status, Status::InProgress);
        assert_eq!(result.unit.claimed_by, Some("alice".to_string()));
        assert!(result.unit.claimed_at.is_some());
        assert_eq!(result.claimer, "alice");
    }

    #[test]
    fn claim_without_by() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();

        let result = claim(&bd, "1", force_params(None)).unwrap();
        assert_eq!(result.unit.status, Status::InProgress);
        assert!(result.unit.claimed_at.is_some());
    }

    #[test]
    fn claim_non_open_unit_fails() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();
        let bp = find_unit_file(&bd, "1").unwrap();
        let mut unit = Unit::from_file(&bp).unwrap();
        unit.status = Status::InProgress;
        unit.to_file(&bp).unwrap();

        assert!(claim(&bd, "1", force_params(Some("bob"))).is_err());
    }

    #[test]
    fn claim_closed_unit_fails() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();
        let bp = find_unit_file(&bd, "1").unwrap();
        let mut unit = Unit::from_file(&bp).unwrap();
        unit.status = Status::Closed;
        unit.to_file(&bp).unwrap();

        assert!(claim(&bd, "1", force_params(Some("bob"))).is_err());
    }

    #[test]
    fn claim_nonexistent_unit_fails() {
        let (_dir, bd) = setup();
        assert!(claim(&bd, "99", force_params(Some("alice"))).is_err());
    }

    #[test]
    fn release_claimed_unit() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();
        // First claim it
        claim(&bd, "1", force_params(Some("alice"))).unwrap();

        let result = release(&bd, "1").unwrap();
        assert_eq!(result.unit.status, Status::Open);
        assert_eq!(result.unit.claimed_by, None);
        assert_eq!(result.unit.claimed_at, None);
    }

    #[test]
    fn release_nonexistent_unit_fails() {
        let (_dir, bd) = setup();
        assert!(release(&bd, "99").is_err());
    }

    #[test]
    fn claim_rebuilds_index() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();

        claim(&bd, "1", force_params(Some("alice"))).unwrap();

        let index = Index::load(&bd).unwrap();
        assert_eq!(index.units.len(), 1);
        assert_eq!(index.units[0].status, Status::InProgress);
    }

    #[test]
    fn release_rebuilds_index() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();
        claim(&bd, "1", force_params(Some("alice"))).unwrap();

        release(&bd, "1").unwrap();

        let index = Index::load(&bd).unwrap();
        assert_eq!(index.units.len(), 1);
        assert_eq!(index.units[0].status, Status::Open);
    }

    #[test]
    fn claim_epic_is_goal_even_with_verify() {
        let (_dir, bd) = setup();
        let mut params = minimal_params("Epic");
        params.verify = Some("cargo test claim_epic_is_goal_even_with_verify".to_string());
        create::create(&bd, params).unwrap();

        let bp = find_unit_file(&bd, "1").unwrap();
        let mut unit = Unit::from_file(&bp).unwrap();
        unit.kind = crate::unit::UnitKind::Epic;
        unit.to_file(&bp).unwrap();

        let result = claim(&bd, "1", force_params(Some("alice"))).unwrap();
        assert!(result.is_goal);
    }

    #[test]
    fn claim_unit_without_verify_is_goal() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();

        let result = claim(&bd, "1", force_params(Some("alice"))).unwrap();
        assert!(result.is_goal);
    }

    #[test]
    fn claim_unit_with_verify_is_not_goal() {
        let (_dir, bd) = setup();
        let mut params = minimal_params("Task");
        params.verify = Some("cargo test auth::login".to_string());
        create::create(&bd, params).unwrap();

        let result = claim(&bd, "1", force_params(Some("alice"))).unwrap();
        assert!(!result.is_goal);
    }

    #[test]
    fn claim_unit_with_empty_verify_is_goal() {
        let (_dir, bd) = setup();
        let mut params = minimal_params("Task");
        params.verify = Some("   ".to_string());
        params.force = true;
        create::create(&bd, params).unwrap();

        let result = claim(&bd, "1", force_params(Some("alice"))).unwrap();
        assert!(result.is_goal);
    }

    #[test]
    fn verify_on_claim_passing_verify_rejected() {
        let (_dir, bd) = setup();
        let mut params = minimal_params("Already done");
        params.verify = Some("grep -q 'project: test' .mana/config.yaml".to_string());
        params.fail_first = true;
        create::create(&bd, params).unwrap();

        let result = claim(&bd, "1", strict_params(Some("alice")));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("verify already passes"));

        // Unit should still be open
        let bp = find_unit_file(&bd, "1").unwrap();
        let unit = Unit::from_file(&bp).unwrap();
        assert_eq!(unit.status, Status::Open);
    }

    #[test]
    fn verify_on_claim_failing_verify_succeeds() {
        let (_dir, bd) = setup();
        let mut params = minimal_params("Real work");
        params.verify = Some("false".to_string());
        params.fail_first = true;
        create::create(&bd, params).unwrap();

        let result = claim(&bd, "1", strict_params(Some("alice"))).unwrap();
        assert_eq!(result.unit.status, Status::InProgress);
        assert!(result.unit.fail_first);
    }

    #[test]
    fn verify_on_claim_force_overrides() {
        let (_dir, bd) = setup();
        let mut params = minimal_params("Force claim");
        params.verify = Some("grep -q 'project: test' .mana/config.yaml".to_string());
        create::create(&bd, params).unwrap();

        let result = claim(&bd, "1", force_params(Some("alice"))).unwrap();
        assert_eq!(result.unit.status, Status::InProgress);
    }

    #[test]
    fn verify_on_claim_checkpoint_sha_stored() {
        let (_dir, bd) = setup();
        let mut params = minimal_params("Checkpoint");
        params.verify = Some("false".to_string());
        params.fail_first = true;
        create::create(&bd, params).unwrap();

        // Initialize a git repo so we get a real SHA
        let project_root = bd.parent().unwrap();
        ShellCommand::new("git")
            .args(["init"])
            .current_dir(project_root)
            .output()
            .unwrap();
        ShellCommand::new("git")
            .args(["commit", "-m", "init", "--allow-empty"])
            .current_dir(project_root)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();

        let result = claim(&bd, "1", strict_params(Some("alice"))).unwrap();
        assert!(result.unit.checkpoint.is_some());
        let sha = result.unit.checkpoint.unwrap();
        assert_eq!(sha.len(), 40);
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn pass_ok_claim_also_stores_checkpoint_sha() {
        let (_dir, bd) = setup();
        let mut params = minimal_params("Checkpoint without fail-first");
        params.verify = Some("grep -q 'project: test' .mana/config.yaml".to_string());
        params.fail_first = false;
        create::create(&bd, params).unwrap();

        let project_root = bd.parent().unwrap();
        ShellCommand::new("git")
            .args(["init"])
            .current_dir(project_root)
            .output()
            .unwrap();
        ShellCommand::new("git")
            .args(["commit", "-m", "init", "--allow-empty"])
            .current_dir(project_root)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();

        let result = claim(&bd, "1", strict_params(Some("alice"))).unwrap();
        assert!(result.unit.checkpoint.is_some());
        assert_eq!(result.unit.status, Status::InProgress);
    }

    #[test]
    fn claim_starts_attempt() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();

        let result = claim(&bd, "1", force_params(Some("agent-1"))).unwrap();
        assert_eq!(result.unit.attempt_log.len(), 1);
        assert_eq!(result.unit.attempt_log[0].num, 1);
        assert_eq!(
            result.unit.attempt_log[0].agent,
            Some("agent-1".to_string())
        );
        assert!(result.unit.attempt_log[0].started_at.is_some());
        assert!(result.unit.attempt_log[0].finished_at.is_none());
    }

    #[test]
    fn release_marks_attempt_abandoned() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();
        claim(&bd, "1", force_params(Some("agent-1"))).unwrap();

        let result = release(&bd, "1").unwrap();
        assert_eq!(result.unit.attempt_log.len(), 1);
        assert_eq!(
            result.unit.attempt_log[0].outcome,
            AttemptOutcome::Abandoned
        );
        assert!(result.unit.attempt_log[0].finished_at.is_some());
    }

    #[test]
    fn multiple_claims_accumulate_attempts() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();

        claim(&bd, "1", force_params(Some("agent-1"))).unwrap();
        release(&bd, "1").unwrap();
        let result = claim(&bd, "1", force_params(Some("agent-2"))).unwrap();

        assert_eq!(result.unit.attempt_log.len(), 2);
        assert_eq!(result.unit.attempt_log[0].num, 1);
        assert_eq!(
            result.unit.attempt_log[0].outcome,
            AttemptOutcome::Abandoned
        );
        assert!(result.unit.attempt_log[0].finished_at.is_some());
        assert_eq!(result.unit.attempt_log[1].num, 2);
        assert_eq!(
            result.unit.attempt_log[1].agent,
            Some("agent-2".to_string())
        );
        assert!(result.unit.attempt_log[1].finished_at.is_none());
    }

    // -----------------------------------------------------------------------
    // Timeout / stuck-in-progress recovery tests
    // -----------------------------------------------------------------------

    /// When an agent times out, mana run calls release() to reset the unit back
    /// to Open so the next dispatch can claim it again without manual intervention.
    #[test]
    fn release_resets_timed_out_in_progress_unit_to_open() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();

        // Simulate agent claiming and then timing out (unit stuck in_progress)
        claim(&bd, "1", force_params(Some("agent-1"))).unwrap();
        // Verify unit is now in_progress (as it would be after a timeout)
        let bp = find_unit_file(&bd, "1").unwrap();
        let in_progress = Unit::from_file(&bp).unwrap();
        assert_eq!(in_progress.status, Status::InProgress);

        // mana run calls release() after a failed/timed-out agent
        let result = release(&bd, "1").unwrap();

        // Unit must be Open so the next mana run can claim it
        assert_eq!(result.unit.status, Status::Open);
        assert_eq!(result.unit.claimed_by, None);
        assert_eq!(result.unit.claimed_at, None);

        // Subsequent claim must succeed (the core fix: no manual intervention needed)
        let second = claim(&bd, "1", force_params(Some("agent-2"))).unwrap();
        assert_eq!(second.unit.status, Status::InProgress);
        assert_eq!(second.unit.claimed_by, Some("agent-2".to_string()));
    }

    /// Attempting to claim a unit that is still in_progress (e.g. if release was
    /// never called) must fail with a descriptive error.
    #[test]
    fn claim_stuck_in_progress_without_release_fails() {
        let (_dir, bd) = setup();
        create::create(&bd, minimal_params("Task")).unwrap();

        // First agent claims the unit
        claim(&bd, "1", force_params(Some("agent-1"))).unwrap();

        // Second agent tries to claim without release — must fail
        let result = claim(&bd, "1", force_params(Some("agent-2")));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("in_progress") || err.contains("InProgress") || err.contains("only open"),
            "Error should explain the unit is not open: {}",
            err
        );
    }
}
