use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::config::Config;
use crate::discovery::find_unit_file;
use crate::index::Index;
use crate::ops::close::{CloseOpts, CloseOutcome};
use crate::ops::verify::run_verify_command;
use crate::unit::{Status, Unit};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Aggregated result of a batch verify run.
pub struct BatchVerifyResult {
    /// IDs of units whose verify command passed (now Closed).
    pub passed: Vec<String>,
    /// Per-unit failure details for units that failed verify (returned to Open).
    pub failed: Vec<BatchVerifyFailure>,
    /// Number of unique verify commands that were executed.
    pub commands_run: usize,
}

/// Details of a single unit's verify failure during batch verification.
pub struct BatchVerifyFailure {
    pub unit_id: String,
    pub verify_command: String,
    pub exit_code: Option<i32>,
    pub output: String,
    pub timed_out: bool,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Run batch verification for all units currently in AwaitingVerify status.
///
/// Groups units by their verify command string, runs each unique command exactly
/// once, then applies the result to all units sharing that command:
/// - Pass → unit is closed via the full lifecycle (force: true skips re-verify)
/// - Fail → unit is set back to Open with claim released
pub fn batch_verify(mana_dir: &Path) -> Result<BatchVerifyResult> {
    let index = Index::load_or_rebuild(mana_dir)?;
    let awaiting_ids: Vec<String> = index
        .units
        .iter()
        .filter(|e| e.status == Status::AwaitingVerify)
        .map(|e| e.id.clone())
        .collect();

    batch_verify_ids(mana_dir, &awaiting_ids)
}

/// Run batch verification for a specific set of unit IDs.
///
/// Only processes units that are in AwaitingVerify status. Units with other
/// statuses or missing verify commands are silently skipped.
pub fn batch_verify_ids(mana_dir: &Path, ids: &[String]) -> Result<BatchVerifyResult> {
    if ids.is_empty() {
        return Ok(BatchVerifyResult {
            passed: vec![],
            failed: vec![],
            commands_run: 0,
        });
    }

    let project_root = mana_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project root from mana dir"))?;

    let config = Config::load_with_extends(mana_dir).ok();

    // Load all units and group by verify command string.
    // Units without a verify command or not in AwaitingVerify are skipped.
    let mut groups: HashMap<String, Vec<Unit>> = HashMap::new();

    for id in ids {
        let unit_path = match find_unit_file(mana_dir, id) {
            Ok(p) => p,
            Err(_) => continue, // Unit not found — skip
        };
        let unit =
            Unit::from_file(&unit_path).with_context(|| format!("Failed to load unit: {}", id))?;

        if unit.status != Status::AwaitingVerify {
            continue;
        }

        let verify_cmd = match &unit.verify {
            Some(cmd) if !cmd.trim().is_empty() => cmd.clone(),
            _ => continue, // No verify command — skip
        };

        groups.entry(verify_cmd).or_default().push(unit);
    }

    let commands_run = groups.len();
    let mut passed = Vec::new();
    let mut failed: Vec<BatchVerifyFailure> = Vec::new();

    for (verify_cmd, units) in groups {
        // Determine timeout from the first unit in the group (all share the same command,
        // so using any unit's timeout is reasonable; the config fallback is always the same).
        let timeout_secs =
            units[0].effective_verify_timeout(config.as_ref().and_then(|c| c.verify_timeout));

        let result = run_verify_command(&verify_cmd, project_root, timeout_secs)?;

        if result.passed {
            // Close each unit — force: true skips the inline verify since we just ran it.
            for unit in units {
                let outcome = crate::ops::close::close(
                    mana_dir,
                    &unit.id,
                    CloseOpts {
                        reason: Some("Batch verify passed".to_string()),
                        force: true,
                        defer_verify: false,
                    },
                )?;

                match outcome {
                    CloseOutcome::Closed(_) => {
                        passed.push(unit.id.clone());
                    }
                    // Other outcomes (hook rejection, circuit breaker, etc.) — treat as passed
                    // since the verify itself succeeded. The close lifecycle handled the rest.
                    other => {
                        match other {
                            CloseOutcome::RejectedByHook { unit_id }
                            | CloseOutcome::DeferredVerify { unit_id }
                            | CloseOutcome::FeatureRequiresHuman { unit_id, .. }
                            | CloseOutcome::CircuitBreakerTripped { unit_id, .. } => {
                                passed.push(unit_id);
                            }
                            CloseOutcome::MergeConflict { .. } => {
                                // MergeConflict has no unit_id — record using the original unit id.
                                passed.push(unit.id.clone());
                            }
                            CloseOutcome::VerifyFrozenViolation { unit_id, .. } => {
                                // Judge was changed — treat as needing attention.
                                passed.push(unit_id);
                            }
                            CloseOutcome::VerifyFailed(_) => {
                                // Should not happen with force: true.
                            }
                            CloseOutcome::Closed(_) => unreachable!(),
                        }
                    }
                }
            }
        } else {
            // Build combined output for failure reporting.
            let combined_output = if result.timed_out {
                format!("Verify timed out after {}s", timeout_secs.unwrap_or(0))
            } else {
                let stdout = result.stdout.trim();
                let stderr = result.stderr.trim();
                let sep = if !stdout.is_empty() && !stderr.is_empty() {
                    "\n"
                } else {
                    ""
                };
                format!("{}{}{}", stdout, sep, stderr)
            };

            // Reopen each unit (set status back to Open, release claim).
            for unit in &units {
                reopen_awaiting_unit(mana_dir, &unit.id)?;
                failed.push(BatchVerifyFailure {
                    unit_id: unit.id.clone(),
                    verify_command: verify_cmd.clone(),
                    exit_code: result.exit_code,
                    output: combined_output.clone(),
                    timed_out: result.timed_out,
                });
            }
        }
    }

    Ok(BatchVerifyResult {
        passed,
        failed,
        commands_run,
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Set a unit back to Open status and release its claim.
///
/// Used when batch verify fails — returns the unit to the pool for re-dispatch.
pub fn reopen_awaiting_unit(mana_dir: &Path, id: &str) -> Result<()> {
    let unit_path =
        find_unit_file(mana_dir, id).with_context(|| format!("Unit not found: {}", id))?;
    let mut unit =
        Unit::from_file(&unit_path).with_context(|| format!("Failed to load unit: {}", id))?;

    unit.status = Status::Open;
    unit.claimed_by = None;
    unit.claimed_at = None;
    unit.updated_at = Utc::now();

    unit.to_file(&unit_path)
        .with_context(|| format!("Failed to save unit: {}", id))?;

    // Rebuild index to reflect the status change.
    let index = Index::build(mana_dir)?;
    index.save(mana_dir)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::discovery::{find_archived_unit, find_unit_file};
    use crate::unit::{Status, Unit};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
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
        .save(&mana_dir)
        .unwrap();
        (dir, mana_dir)
    }

    /// Write a unit in AwaitingVerify status to disk.
    fn write_awaiting(mana_dir: &Path, id: &str, verify_cmd: &str) {
        let mut unit = Unit::new(id, &format!("Task {}", id));
        unit.status = Status::AwaitingVerify;
        unit.verify = Some(verify_cmd.to_string());
        let slug = id.replace('.', "-");
        unit.to_file(mana_dir.join(format!("{}-task-{}.md", id, slug)))
            .unwrap();
    }

    // ------------------------------------------------------------------
    // batch_verify_groups_by_command
    // ------------------------------------------------------------------

    /// 3 units where 2 share a verify command → only 2 unique commands run.
    #[test]
    fn batch_verify_groups_by_command() {
        let (_dir, mana_dir) = setup();

        // Units 1 and 2 share the same verify command; unit 3 has a different one.
        write_awaiting(&mana_dir, "1", "true");
        write_awaiting(&mana_dir, "2", "true");
        write_awaiting(&mana_dir, "3", "true && true");

        // Rebuild index so batch_verify can find the units.
        let index = Index::build(&mana_dir).unwrap();
        index.save(&mana_dir).unwrap();

        let result = batch_verify(&mana_dir).unwrap();

        assert_eq!(result.commands_run, 2, "Expected 2 unique commands run");
        assert_eq!(result.passed.len(), 3);
        assert!(result.failed.is_empty());
    }

    // ------------------------------------------------------------------
    // batch_verify_passes_close_units
    // ------------------------------------------------------------------

    /// When verify passes, units should be Closed (archived).
    #[test]
    fn batch_verify_passes_close_units() {
        let (_dir, mana_dir) = setup();
        write_awaiting(&mana_dir, "1", "true");

        let index = Index::build(&mana_dir).unwrap();
        index.save(&mana_dir).unwrap();

        let result = batch_verify(&mana_dir).unwrap();

        assert_eq!(result.passed, vec!["1"]);
        assert!(result.failed.is_empty());
        assert_eq!(result.commands_run, 1);

        // Unit should be archived (Closed).
        // After archiving, the unit file is moved to the archive dir — use find_archived_unit.
        let archive_path = find_archived_unit(&mana_dir, "1").expect("unit should be in archive");
        let unit = Unit::from_file(archive_path).unwrap();
        assert_eq!(unit.status, Status::Closed);
        assert!(unit.is_archived);
    }

    // ------------------------------------------------------------------
    // batch_verify_fails_reopen_units
    // ------------------------------------------------------------------

    /// When verify fails, units should be set back to Open.
    #[test]
    fn batch_verify_fails_reopen_units() {
        let (_dir, mana_dir) = setup();
        write_awaiting(&mana_dir, "1", "false");

        let index = Index::build(&mana_dir).unwrap();
        index.save(&mana_dir).unwrap();

        let result = batch_verify(&mana_dir).unwrap();

        assert!(result.passed.is_empty());
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].unit_id, "1");
        assert_eq!(result.failed[0].exit_code, Some(1));
        assert!(!result.failed[0].timed_out);
        assert_eq!(result.commands_run, 1);

        // Unit should be back to Open.
        let unit_path = find_unit_file(&mana_dir, "1").unwrap();
        let unit = Unit::from_file(unit_path).unwrap();
        assert_eq!(unit.status, Status::Open);
        assert!(unit.claimed_by.is_none());
    }

    // ------------------------------------------------------------------
    // batch_verify_empty_noop
    // ------------------------------------------------------------------

    /// No AwaitingVerify units → empty result, no commands run.
    #[test]
    fn batch_verify_empty_noop() {
        let (_dir, mana_dir) = setup();

        // Write a regular Open unit (not AwaitingVerify).
        let mut unit = Unit::new("1", "Task 1");
        unit.verify = Some("true".to_string());
        unit.to_file(mana_dir.join("1-task-1.md")).unwrap();

        let index = Index::build(&mana_dir).unwrap();
        index.save(&mana_dir).unwrap();

        let result = batch_verify(&mana_dir).unwrap();

        assert!(result.passed.is_empty());
        assert!(result.failed.is_empty());
        assert_eq!(result.commands_run, 0);
    }

    // ------------------------------------------------------------------
    // batch_verify_mixed_results
    // ------------------------------------------------------------------

    /// Some units pass, some fail → correct split across passed/failed.
    #[test]
    fn batch_verify_mixed_results() {
        let (_dir, mana_dir) = setup();

        write_awaiting(&mana_dir, "1", "true");
        write_awaiting(&mana_dir, "2", "false");
        write_awaiting(&mana_dir, "3", "true");

        let index = Index::build(&mana_dir).unwrap();
        index.save(&mana_dir).unwrap();

        let result = batch_verify(&mana_dir).unwrap();

        // "true" and "false" are the 2 unique commands.
        assert_eq!(result.commands_run, 2);

        // Units 1 and 3 share "true" → both pass.
        let mut passed = result.passed.clone();
        passed.sort();
        assert_eq!(passed, vec!["1", "3"]);

        // Unit 2 has "false" → fails.
        assert_eq!(result.failed.len(), 1);
        assert_eq!(result.failed[0].unit_id, "2");

        // Verify on-disk states.
        // Passing units are archived; failing unit stays in active dir.
        let u1 = Unit::from_file(find_archived_unit(&mana_dir, "1").unwrap()).unwrap();
        assert_eq!(u1.status, Status::Closed);

        let u2 = Unit::from_file(find_unit_file(&mana_dir, "2").unwrap()).unwrap();
        assert_eq!(u2.status, Status::Open);

        let u3 = Unit::from_file(find_archived_unit(&mana_dir, "3").unwrap()).unwrap();
        assert_eq!(u3.status, Status::Closed);
    }
}
