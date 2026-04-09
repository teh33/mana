use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;

use sha2::{Digest, Sha256};

use crate::config::{Config, DEFAULT_COMMIT_TEMPLATE};
use crate::discovery::{archive_path_for_unit, find_archived_unit, find_unit_file};
use crate::graph;
use crate::hooks::{
    current_git_branch, execute_config_hook, execute_hook, is_trusted, HookEvent, HookVars,
};
use crate::index::{ArchiveIndex, Index, IndexEntry, LockedIndex};
use crate::ops::verify::run_verify_command;
use crate::unit::{
    AttemptOutcome, OnCloseAction, OnFailAction, RunRecord, RunResult, Status, Unit,
    VerifyPosture,
};
use crate::util::title_to_slug;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// What action was taken by `process_on_fail`.
#[derive(Debug, serde::Serialize)]
pub enum OnFailActionTaken {
    /// Claim released for retry (attempt N / max M).
    Retry {
        attempt: u32,
        max: u32,
        delay_secs: Option<u64>,
    },
    /// Max retries exhausted — claim kept.
    RetryExhausted { max: u32 },
    /// Priority escalated and/or message appended.
    Escalated,
    /// No on_fail configured.
    None,
}

/// Result of a circuit breaker check.
#[derive(Debug)]
pub struct CircuitBreakerStatus {
    pub tripped: bool,
    pub subtree_total: u32,
    pub max_loops: u32,
}

/// Metadata about a verify failure, used by `record_failure`.
#[derive(Debug)]
pub struct VerifyFailure {
    pub exit_code: Option<i32>,
    pub output: String,
    pub timed_out: bool,
    pub duration_secs: f64,
    pub started_at: chrono::DateTime<Utc>,
    pub finished_at: chrono::DateTime<Utc>,
    pub agent: Option<String>,
}

/// Options for the full `close` lifecycle.
pub struct CloseOpts {
    pub reason: Option<String>,
    pub force: bool,
    /// Skip verify and mark as AwaitingVerify instead of Closed.
    ///
    /// Set when `--defer-verify` is passed or `MANA_BATCH_VERIFY=1` is in the environment.
    /// The runner is responsible for running verify later and finalizing the unit.
    pub defer_verify: bool,
}

/// Structured warnings emitted during close lifecycle steps.
#[derive(Debug, serde::Serialize)]
pub enum CloseWarning {
    /// The pre-close hook errored, but close was allowed to continue.
    PreCloseHookError { message: String },
    /// The post-close hook returned a non-zero exit status.
    PostCloseHookRejected,
    /// The post-close hook errored.
    PostCloseHookError { message: String },
    /// Worktree cleanup failed after a successful close.
    WorktreeCleanupFailed { message: String },
    /// The verify command was changed since claim (--force overrode the block).
    VerifyChanged,
}

/// Evidence collected at close time from the diff since claim checkpoint.
#[derive(Debug, Default, serde::Serialize)]
pub struct CloseEvidence {
    /// Files changed since checkpoint.
    pub changed_files: Vec<String>,
    /// Total lines added across all files.
    pub additions: u32,
    /// Total lines deleted across all files.
    pub deletions: u32,
    /// Whether only .mana/ files changed (suspicious for code tasks).
    pub only_mana_changes: bool,
    /// Whether no changed file overlaps with unit.paths (suspicious).
    pub no_path_overlap: bool,
}

/// Result of an auto-commit attempt after close.
#[derive(Debug, serde::Serialize)]
pub struct AutoCommitResult {
    pub message: String,
    pub committed: bool,
    pub warning: Option<String>,
}

/// Outcome of attempting to close a single unit.
#[derive(Debug, serde::Serialize)]
pub enum CloseOutcome {
    /// The unit was closed and archived.
    Closed(CloseResult),
    /// The verify command failed.
    VerifyFailed(VerifyFailureResult),
    /// The pre-close hook rejected the close.
    RejectedByHook { unit_id: String },
    /// Feature unit requires interactive TTY confirmation.
    FeatureRequiresHuman {
        unit_id: String,
        title: String,
        warnings: Vec<CloseWarning>,
    },
    /// Circuit breaker tripped — too many attempts across the subtree.
    CircuitBreakerTripped {
        unit_id: String,
        total_attempts: u32,
        max: u32,
        warnings: Vec<CloseWarning>,
    },
    /// Worktree merge had conflicts — unit stays open.
    MergeConflict {
        files: Vec<String>,
        warnings: Vec<CloseWarning>,
    },
    /// Verify was deferred — unit is now AwaitingVerify.
    ///
    /// Emitted when `CloseOpts::defer_verify` is true. The runner is expected to
    /// run verify later and transition the unit to Closed or back to Open.
    DeferredVerify { unit_id: String },
    /// The verify command was changed after claim — judge integrity violated.
    VerifyFrozenViolation {
        unit_id: String,
        warnings: Vec<CloseWarning>,
    },
}

/// Details of a successful close.
#[derive(Debug, serde::Serialize)]
pub struct CloseResult {
    pub unit: Unit,
    pub archive_path: PathBuf,
    pub auto_closed_parents: Vec<String>,
    pub on_close_results: Vec<OnCloseActionResult>,
    pub warnings: Vec<CloseWarning>,
    pub auto_commit_result: Option<AutoCommitResult>,
    /// Diff evidence from claim checkpoint, if available.
    pub evidence: Option<CloseEvidence>,
}

/// Result of one on_close action execution.
#[derive(Debug, serde::Serialize)]
pub enum OnCloseActionResult {
    /// A `run` command was executed.
    RanCommand {
        command: String,
        success: bool,
        exit_code: Option<i32>,
        error: Option<String>,
    },
    /// A `notify` message was emitted.
    Notified { message: String },
    /// A `run` command was skipped (not trusted).
    Skipped { command: String },
}

/// Details of a verify failure during close.
#[derive(Debug, serde::Serialize)]
pub struct VerifyFailureResult {
    pub unit: Unit,
    pub attempt_number: u32,
    pub exit_code: Option<i32>,
    pub output: String,
    pub timed_out: bool,
    pub on_fail_action_taken: Option<OnFailActionTaken>,
    pub verify_command: String,
    pub timeout_secs: Option<u64>,
    pub warnings: Vec<CloseWarning>,
}

struct HookDecision {
    accepted: bool,
    warning: Option<CloseWarning>,
}

struct PostCloseActionsReport {
    warnings: Vec<CloseWarning>,
    on_close_results: Vec<OnCloseActionResult>,
}

enum WorktreeMergeStatus {
    Merged,
    Conflict { files: Vec<String> },
}

/// Maximum stdout size to capture as outputs (64 KB).
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// Compute close-time evidence: diff from checkpoint, changed files, stats.
fn compute_close_evidence(
    project_root: &Path,
    checkpoint: Option<&str>,
    unit_paths: &[String],
) -> Option<CloseEvidence> {
    let checkpoint = checkpoint?;

    let name_output = std::process::Command::new("git")
        .args(["diff", "--name-only", checkpoint, "HEAD"])
        .current_dir(project_root)
        .output()
        .ok()?;

    if !name_output.status.success() {
        return None;
    }

    let changed_files: Vec<String> = String::from_utf8_lossy(&name_output.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();

    let numstat_output = std::process::Command::new("git")
        .args(["diff", "--numstat", checkpoint, "HEAD"])
        .current_dir(project_root)
        .output()
        .ok();

    let (mut additions, mut deletions) = (0u32, 0u32);
    if let Some(ref out) = numstat_output {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                additions += parts[0].parse::<u32>().unwrap_or(0);
                deletions += parts[1].parse::<u32>().unwrap_or(0);
            }
        }
    }

    let only_mana_changes = !changed_files.is_empty()
        && changed_files
            .iter()
            .all(|f| f.starts_with(".mana/") || f.starts_with(".mana\\"));

    let no_path_overlap = if unit_paths.is_empty() {
        false
    } else {
        !changed_files.iter().any(|changed| {
            unit_paths
                .iter()
                .any(|expected| changed == expected || changed.starts_with(expected))
        })
    };

    Some(CloseEvidence {
        changed_files,
        additions,
        deletions,
        only_mana_changes,
        no_path_overlap,
    })
}

fn has_non_mana_changes_since_checkpoint(project_root: &Path, checkpoint: &str) -> Result<bool> {
    let diff_output = std::process::Command::new("git")
        .args(["diff", "--name-only", checkpoint, "--"])
        .current_dir(project_root)
        .output()
        .context("Failed to compare working tree against checkpoint")?;

    if !diff_output.status.success() {
        return Ok(true);
    }

    let tracked_changed = String::from_utf8_lossy(&diff_output.stdout)
        .lines()
        .map(str::trim)
        .any(|path| !path.is_empty() && !path.starts_with(".mana/"));
    if tracked_changed {
        return Ok(true);
    }

    let untracked_output = std::process::Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(project_root)
        .output()
        .context("Failed to list untracked files")?;

    if !untracked_output.status.success() {
        return Ok(true);
    }

    Ok(String::from_utf8_lossy(&untracked_output.stdout)
        .lines()
        .map(str::trim)
        .any(|path| !path.is_empty() && !path.starts_with(".mana/")))
}

// ---------------------------------------------------------------------------
// Core close lifecycle
// ---------------------------------------------------------------------------

/// Close a single unit — the full lifecycle.
///
/// Steps: pre-close hook → verify → worktree merge → feature gate → mark closed
/// → archive → post-close cascade → auto-close parents → rebuild index.
///
/// Does NOT handle TTY confirmation for feature units — if the unit is a feature,
/// returns `CloseOutcome::FeatureRequiresHuman` and the caller decides.
pub fn close(mana_dir: &Path, id: &str, opts: CloseOpts) -> Result<CloseOutcome> {
    let project_root = mana_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project root from units dir"))?;

    let config = Config::load_with_extends(mana_dir).ok();

    let unit_path =
        find_unit_file(mana_dir, id).with_context(|| format!("Unit not found: {}", id))?;
    let mut unit =
        Unit::from_file(&unit_path).with_context(|| format!("Failed to load unit: {}", id))?;

    // 1. Pre-close hook
    let pre_close = run_pre_close_hook(&unit, project_root, opts.reason.as_deref());
    if !pre_close.accepted {
        return Ok(CloseOutcome::RejectedByHook {
            unit_id: id.to_string(),
        });
    }

    let mut warnings = Vec::new();
    if let Some(warning) = pre_close.warning {
        warnings.push(warning);
    }

    // 1b. Defer verify — mark as AwaitingVerify and return immediately.
    //
    // The runner will collect all AwaitingVerify units after agents complete,
    // run each unique verify command once, and finalize units accordingly.
    if opts.defer_verify {
        unit.status = Status::AwaitingVerify;
        unit.updated_at = Utc::now();
        if let Some(disposition) = unit.autonomy_disposition.as_mut() {
            disposition.verify = VerifyPosture::Deferred;
        }
        refresh_autonomy_disposition(&mut unit);
        unit.to_file(&unit_path)
            .with_context(|| format!("Failed to save unit: {}", id))?;
        rebuild_index(mana_dir)?;
        return Ok(CloseOutcome::DeferredVerify {
            unit_id: id.to_string(),
        });
    }

    // 1c. Verify freeze check — was the judge changed since claim?
    if let Some(ref stored_hash) = unit.verify_hash {
        if let Some(ref verify_cmd) = unit.verify {
            let mut hasher = Sha256::new();
            hasher.update(verify_cmd.as_bytes());
            let current_hash = format!("{:x}", hasher.finalize());
            if current_hash != *stored_hash {
                if !opts.force {
                    if let Some(disposition) = unit.autonomy_disposition.as_mut() {
                        disposition.verify = VerifyPosture::FrozenViolation;
                    }
                    refresh_autonomy_disposition(&mut unit);
                    unit.updated_at = Utc::now();
                    unit.to_file(&unit_path)
                        .with_context(|| format!("Failed to save unit: {}", id))?;
                    rebuild_index(mana_dir)?;
                    return Ok(CloseOutcome::VerifyFrozenViolation {
                        unit_id: id.to_string(),
                        warnings,
                    });
                }
                warnings.push(CloseWarning::VerifyChanged);
            }
        }
    }

    // 2. Verify (if applicable and not force)
    if let Some(verify_cmd) = unit.verify.clone() {
        if !verify_cmd.trim().is_empty() && !opts.force {
            let timeout_secs =
                unit.effective_verify_timeout(config.as_ref().and_then(|c| c.verify_timeout));

            let started_at = Utc::now();
            let verify_result = run_verify_command(&verify_cmd, project_root, timeout_secs)?;
            let finished_at = Utc::now();
            let duration_secs = (finished_at - started_at).num_milliseconds() as f64 / 1000.0;
            let agent = std::env::var("MANA_AGENT").ok();

            if !verify_result.passed {
                // Build combined output — on timeout, synthesize a message
                let combined_output = if verify_result.timed_out {
                    format!("Verify timed out after {}s", timeout_secs.unwrap_or(0))
                } else {
                    let stdout = verify_result.stdout.trim();
                    let stderr = verify_result.stderr.trim();
                    let sep = if !stdout.is_empty() && !stderr.is_empty() {
                        "\n"
                    } else {
                        ""
                    };
                    format!("{}{}{}", stdout, sep, stderr)
                };

                // Record the failure
                let failure = VerifyFailure {
                    exit_code: verify_result.exit_code,
                    output: combined_output,
                    timed_out: verify_result.timed_out,
                    duration_secs,
                    started_at,
                    finished_at,
                    agent,
                };
                record_failure_on_unit(&mut unit, &failure);

                // Circuit breaker
                let root_id = find_root_parent(mana_dir, &unit)?;
                let config_max = config.as_ref().map(|c| c.max_loops).unwrap_or(10);
                let max_loops_limit = resolve_max_loops(mana_dir, &unit, &root_id, config_max);

                if max_loops_limit > 0 {
                    // Save unit first so subtree count is accurate
                    unit.to_file(&unit_path)
                        .with_context(|| format!("Failed to save unit: {}", id))?;

                    let cb = check_circuit_breaker(mana_dir, &mut unit, &root_id, max_loops_limit)?;
                    if cb.tripped {
                        unit.to_file(&unit_path)
                            .with_context(|| format!("Failed to save unit: {}", id))?;

                        // Rebuild index
                        rebuild_index(mana_dir)?;

                        return Ok(CloseOutcome::CircuitBreakerTripped {
                            unit_id: id.to_string(),
                            total_attempts: cb.subtree_total,
                            max: cb.max_loops,
                            warnings,
                        });
                    }
                }

                // Process on_fail action
                let action_taken = process_on_fail(&mut unit);

                unit.to_file(&unit_path)
                    .with_context(|| format!("Failed to save unit: {}", id))?;

                // Fire on_fail config hook
                run_on_fail_hook(&unit, project_root, config.as_ref(), &failure.output);

                // Rebuild index
                rebuild_index(mana_dir)?;

                return Ok(CloseOutcome::VerifyFailed(VerifyFailureResult {
                    attempt_number: unit.attempts,
                    exit_code: failure.exit_code,
                    output: failure.output,
                    timed_out: failure.timed_out,
                    on_fail_action_taken: Some(action_taken),
                    verify_command: verify_cmd,
                    timeout_secs,
                    warnings,
                    unit,
                }));
            }

            if !unit.fail_first {
                if let Some(checkpoint) = unit.checkpoint.as_deref() {
                    if !has_non_mana_changes_since_checkpoint(project_root, checkpoint)? {
                        anyhow::bail!(
                            "Cannot close unit {}: verify already passed when work began and no non-.mana changes were detected since claim.\n\nUse --force to override, or add acceptance criteria / a failing verify gate for this kind of work.",
                            id
                        );
                    }
                }
            }

            // Record success in history
            unit.history.push(RunRecord {
                attempt: unit.attempts + 1,
                started_at,
                finished_at: Some(finished_at),
                duration_secs: Some(duration_secs),
                agent,
                result: RunResult::Pass,
                exit_code: verify_result.exit_code,
                tokens: None,
                cost: None,
                output_snippet: None,
                autonomy_observation: None,
            });

            // Capture stdout as unit outputs
            capture_verify_outputs(&mut unit, &verify_result.stdout);
            refresh_autonomy_disposition(&mut unit);
        }
    }

    // 3. Worktree merge (after verify passes, before archiving)
    let worktree_info = detect_valid_worktree(project_root);
    if let Some(ref wt_info) = worktree_info {
        match handle_worktree_merge(wt_info, &unit)? {
            WorktreeMergeStatus::Merged => {}
            WorktreeMergeStatus::Conflict { files } => {
                return Ok(CloseOutcome::MergeConflict { files, warnings });
            }
        }
    }

    // 4. Feature gate — delegate to caller
    if unit.feature {
        use std::io::IsTerminal;
        if !opts.force || !std::io::stdin().is_terminal() {
            return Ok(CloseOutcome::FeatureRequiresHuman {
                unit_id: unit.id.clone(),
                title: unit.title.clone(),
                warnings,
            });
        }
    }

    // 4b. Compute close evidence from diff
    let evidence = compute_close_evidence(project_root, unit.checkpoint.as_deref(), &unit.paths);

    if let Some(record) = unit.history.last_mut() {
        if record.result == RunResult::Pass {
            record.output_snippet =
                build_pass_output_snippet(unit.verify.as_deref(), evidence.as_ref());
        }
    }

    // 5. Mark the unit closed
    let now = Utc::now();
    unit.status = Status::Closed;
    unit.closed_at = Some(now);
    unit.close_reason = opts.reason.clone();
    unit.updated_at = now;

    // Finalize the current attempt as success
    if let Some(attempt) = unit.attempt_log.last_mut() {
        if attempt.finished_at.is_none() {
            attempt.outcome = AttemptOutcome::Success;
            attempt.finished_at = Some(now);
            attempt.notes = opts.reason.clone();
        }
    }

    // Update last_verified for facts
    if unit.unit_type == "fact" {
        unit.last_verified = Some(now);
    }

    refresh_autonomy_disposition(&mut unit);

    unit.to_file(&unit_path)
        .with_context(|| format!("Failed to save unit: {}", id))?;

    // 6. Archive
    let archive_path = archive_unit(mana_dir, &mut unit, &unit_path)?;

    // 6b. Rebuild index immediately after archive so stale entries don't linger.
    // Without this, readers between archive (file rename) and the later rebuild
    // would see a stale index referencing a now-moved file.
    rebuild_index(mana_dir)?;

    // 7. Post-close cascade
    let post_close =
        run_post_close_actions(&unit, project_root, opts.reason.as_deref(), config.as_ref());
    warnings.extend(post_close.warnings);

    // Clean up worktree after successful close
    if let Some(ref wt_info) = worktree_info {
        if let Some(warning) = cleanup_worktree(wt_info) {
            warnings.push(warning);
        }
    }

    // 8. Auto-close parents
    let auto_closed_parents = if mana_dir.exists() {
        if let Some(parent_id) = &unit.parent {
            let auto_close_enabled = config.as_ref().map(|c| c.auto_close_parent).unwrap_or(true);
            if auto_close_enabled {
                auto_close_parents(mana_dir, parent_id)?
            } else {
                vec![]
            }
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    // Rebuild index before auto-commit so archived units, parent cascades, and
    // index updates are included in the close commit.
    rebuild_index(mana_dir)?;

    // Auto-commit if configured (skip in worktree mode — it already commits)
    let auto_commit_result = if worktree_info.is_none() {
        let auto_commit_enabled = config.as_ref().map(|c| c.auto_commit).unwrap_or(false);
        if auto_commit_enabled {
            let template = config.as_ref().and_then(|c| c.commit_template.clone());
            Some(auto_commit_on_close(
                project_root,
                id,
                &unit.title,
                unit.parent.as_deref(),
                &unit.labels,
                template.as_deref(),
            ))
        } else {
            None
        }
    } else {
        None
    };

    Ok(CloseOutcome::Closed(CloseResult {
        unit,
        archive_path,
        auto_closed_parents,
        on_close_results: post_close.on_close_results,
        warnings,
        auto_commit_result,
        evidence,
    }))
}

/// Mark a unit as explicitly failed. Stays open with claim released.
///
/// Records the failure in attempt_log for episodic memory and appends
/// a structured failure summary to notes.
pub fn close_failed(mana_dir: &Path, id: &str, reason: Option<String>) -> Result<Unit> {
    let now = Utc::now();

    let unit_path =
        find_unit_file(mana_dir, id).with_context(|| format!("Unit not found: {}", id))?;
    let mut unit =
        Unit::from_file(&unit_path).with_context(|| format!("Failed to load unit: {}", id))?;

    // Finalize the current attempt as failed
    if let Some(attempt) = unit.attempt_log.last_mut() {
        if attempt.finished_at.is_none() {
            attempt.outcome = AttemptOutcome::Failed;
            attempt.finished_at = Some(now);
            attempt.notes = reason.clone();
        }
    }

    // Release the claim (unit stays open for retry)
    unit.claimed_by = None;
    unit.claimed_at = None;
    unit.status = Status::Open;
    unit.updated_at = now;

    // Generate structured failure summary and append to notes
    {
        let attempt_num = unit.attempt_log.len() as u32;
        let duration_secs = unit
            .attempt_log
            .last()
            .and_then(|a| a.started_at)
            .map(|started| (now - started).num_seconds().max(0) as u64)
            .unwrap_or(0);

        let ctx = crate::failure::FailureContext {
            unit_id: id.to_string(),
            unit_title: unit.title.clone(),
            attempt: attempt_num.max(1),
            duration_secs,
            tool_count: 0,
            turns: 0,
            input_tokens: 0,
            output_tokens: 0,
            cost: 0.0,
            error: reason,
            tool_log: vec![],
            verify_command: unit.verify.clone(),
        };
        let summary = crate::failure::build_failure_summary(&ctx);

        match &mut unit.notes {
            Some(notes) => {
                notes.push('\n');
                notes.push_str(&summary);
            }
            None => unit.notes = Some(summary),
        }
    }

    unit.to_file(&unit_path)
        .with_context(|| format!("Failed to save unit: {}", id))?;

    // Rebuild index
    rebuild_index(mana_dir)?;

    Ok(unit)
}

// ---------------------------------------------------------------------------
// Public composable functions
// ---------------------------------------------------------------------------

/// Check if all children of a parent unit are closed.
///
/// Checks both active and archived units. Returns true if the parent has no
/// children, or if all children have status=closed.
pub fn all_children_closed(mana_dir: &Path, parent_id: &str) -> Result<bool> {
    let index = Index::build(mana_dir)?;
    let archived = Index::collect_archived(mana_dir).unwrap_or_default();

    let mut all_units = index.units;
    all_units.extend(archived);

    let children: Vec<_> = all_units
        .iter()
        .filter(|b| b.parent.as_deref() == Some(parent_id))
        .collect();

    if children.is_empty() {
        return Ok(true);
    }

    for child in children {
        if child.status != Status::Closed {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Auto-close parent chain when all children are done.
///
/// Recursively walks up the parent chain, closing and archiving each parent
/// whose children are all closed. Feature parents are skipped. Returns the
/// list of parent IDs that were auto-closed.
pub fn auto_close_parents(mana_dir: &Path, parent_id: &str) -> Result<Vec<String>> {
    let mut closed = Vec::new();
    auto_close_parent_recursive(mana_dir, parent_id, &mut closed)?;
    Ok(closed)
}

fn auto_close_parent_recursive(
    mana_dir: &Path,
    parent_id: &str,
    closed: &mut Vec<String>,
) -> Result<()> {
    if !all_children_closed(mana_dir, parent_id)? {
        return Ok(());
    }

    let unit_path = match find_unit_file(mana_dir, parent_id) {
        Ok(path) => path,
        Err(_) => return Ok(()), // Already archived
    };

    let mut unit = Unit::from_file(&unit_path)
        .with_context(|| format!("Failed to load parent unit: {}", parent_id))?;

    if unit.status == Status::Closed {
        return Ok(());
    }

    // Feature units are never auto-closed
    if unit.feature {
        return Ok(());
    }

    let now = Utc::now();
    unit.status = Status::Closed;
    unit.closed_at = Some(now);
    unit.close_reason = Some("Auto-closed: all children completed".to_string());
    unit.updated_at = now;

    unit.to_file(&unit_path)
        .with_context(|| format!("Failed to save parent unit: {}", parent_id))?;

    archive_unit(mana_dir, &mut unit, &unit_path)?;
    closed.push(parent_id.to_string());

    // Recurse to grandparent
    if let Some(grandparent_id) = &unit.parent {
        auto_close_parent_recursive(mana_dir, grandparent_id, closed)?;
    }

    Ok(())
}

/// Archive a closed unit to the dated archive directory.
///
/// Moves the unit file, marks `is_archived = true`, and updates the archive index.
/// Returns the archive path.
pub fn archive_unit(mana_dir: &Path, unit: &mut Unit, unit_path: &Path) -> Result<PathBuf> {
    let id = &unit.id;
    let slug = unit
        .slug
        .clone()
        .unwrap_or_else(|| title_to_slug(&unit.title));
    let ext = unit_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("md");
    let today = chrono::Local::now().naive_local().date();
    let archive_path = archive_path_for_unit(mana_dir, id, &slug, ext, today);

    if let Some(parent) = archive_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create archive directories for unit {}", id))?;
    }

    std::fs::rename(unit_path, &archive_path)
        .with_context(|| format!("Failed to move unit {} to archive", id))?;

    unit.is_archived = true;
    unit.to_file(&archive_path)
        .with_context(|| format!("Failed to save archived unit: {}", id))?;

    // Append to archive index
    {
        let mut archive_index =
            ArchiveIndex::load(mana_dir).unwrap_or(ArchiveIndex { units: Vec::new() });
        archive_index.append(IndexEntry::from(&*unit));
        let _ = archive_index.save(mana_dir);
    }

    Ok(archive_path)
}

/// Record a failed verify attempt on a unit.
///
/// Increments attempts, appends failure details to notes, and pushes
/// a structured history entry. Does not save to disk — caller decides when to write.
pub fn record_failure(unit: &mut Unit, failure: &VerifyFailure) {
    record_failure_on_unit(unit, failure);
}

fn build_pass_output_snippet(
    verify_command: Option<&str>,
    evidence: Option<&CloseEvidence>,
) -> Option<String> {
    let mut parts = Vec::new();

    if let Some(verify) = verify_command.map(str::trim).filter(|v| !v.is_empty()) {
        parts.push(format!("verify passed: {}", verify));
    } else {
        parts.push("verify passed".to_string());
    }

    let file_count = evidence.map(|e| e.changed_files.len()).unwrap_or(0);
    if file_count > 0 {
        let evidence = evidence.expect("file_count > 0 implies evidence exists");
        let mut scope = format!(
            "changed {} file{} (+{}/-{})",
            file_count,
            if file_count == 1 { "" } else { "s" },
            evidence.additions,
            evidence.deletions
        );
        if evidence.only_mana_changes {
            scope.push_str(", only .mana changes");
        }
        if evidence.no_path_overlap {
            scope.push_str(", no declared path overlap");
        }
        parts.push(scope);
    } else if evidence.map(|e| e.only_mana_changes || e.no_path_overlap).unwrap_or(false) {
        let evidence = evidence.expect("scope flags imply evidence exists");
        let mut scope_flags = Vec::new();
        if evidence.only_mana_changes {
            scope_flags.push("only .mana changes");
        }
        if evidence.no_path_overlap {
            scope_flags.push("no declared path overlap");
        }
        if !scope_flags.is_empty() {
            parts.push(scope_flags.join(", "));
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

/// Process on_fail actions (retry release, escalate).
///
/// Mutates unit in-place (releases claim for retry, escalates priority).
/// Returns what action was taken.
pub fn process_on_fail(unit: &mut Unit) -> OnFailActionTaken {
    let on_fail = match &unit.on_fail {
        Some(action) => action.clone(),
        None => {
            refresh_attempt_pressure(unit);
            return OnFailActionTaken::None;
        }
    };

    let action_taken = match on_fail {
        OnFailAction::Retry { max, delay_secs } => {
            let max_retries = max.unwrap_or(unit.max_attempts);
            if unit.attempts < max_retries {
                unit.claimed_by = None;
                unit.claimed_at = None;
                OnFailActionTaken::Retry {
                    attempt: unit.attempts,
                    max: max_retries,
                    delay_secs,
                }
            } else {
                OnFailActionTaken::RetryExhausted { max: max_retries }
            }
        }
        OnFailAction::Escalate { priority, message } => {
            if let Some(p) = priority {
                unit.priority = p;
            }
            if let Some(msg) = &message {
                let note = format!(
                    "\n## Escalated — {}\n{}",
                    Utc::now().format("%Y-%m-%dT%H:%M:%SZ"),
                    msg
                );
                match &mut unit.notes {
                    Some(notes) => notes.push_str(&note),
                    None => unit.notes = Some(note),
                }
            }
            if !unit.labels.contains(&"escalated".to_string()) {
                unit.labels.push("escalated".to_string());
            }
            OnFailActionTaken::Escalated
        }
    };

    refresh_attempt_pressure(unit);
    action_taken
}

/// Check circuit breaker for a unit.
///
/// If subtree attempts exceed `max_loops`, trips the breaker: adds
/// "circuit-breaker" label and sets priority to P0. Unit is mutated
/// but NOT saved — caller decides when to write.
pub fn check_circuit_breaker(
    mana_dir: &Path,
    unit: &mut Unit,
    root_id: &str,
    max_loops: u32,
) -> Result<CircuitBreakerStatus> {
    if max_loops == 0 {
        refresh_attempt_pressure(unit);
        return Ok(CircuitBreakerStatus {
            tripped: false,
            subtree_total: 0,
            max_loops: 0,
        });
    }

    let subtree_total = graph::count_subtree_attempts(mana_dir, root_id)?;
    if subtree_total >= max_loops {
        if !unit.labels.contains(&"circuit-breaker".to_string()) {
            unit.labels.push("circuit-breaker".to_string());
        }
        unit.priority = 0;
        refresh_attempt_pressure(unit);
        Ok(CircuitBreakerStatus {
            tripped: true,
            subtree_total,
            max_loops,
        })
    } else {
        refresh_attempt_pressure(unit);
        Ok(CircuitBreakerStatus {
            tripped: false,
            subtree_total,
            max_loops,
        })
    }
}

/// Walk up the parent chain to find the root ancestor of a unit.
///
/// Returns the ID of the topmost parent (the unit with no parent).
/// If the unit itself has no parent, returns its own ID.
pub fn find_root_parent(mana_dir: &Path, unit: &Unit) -> Result<String> {
    let mut current_id = match &unit.parent {
        None => return Ok(unit.id.clone()),
        Some(pid) => pid.clone(),
    };

    loop {
        let path = find_unit_file(mana_dir, &current_id)
            .or_else(|_| find_archived_unit(mana_dir, &current_id));

        match path {
            Ok(p) => {
                let b = Unit::from_file(&p)
                    .with_context(|| format!("Failed to load parent unit: {}", current_id))?;
                match b.parent {
                    Some(parent_id) => current_id = parent_id,
                    None => return Ok(current_id),
                }
            }
            Err(_) => return Ok(current_id),
        }
    }
}

/// Resolve the effective max_loops for a unit, considering root parent overrides.
pub fn resolve_max_loops(mana_dir: &Path, unit: &Unit, root_id: &str, config_max: u32) -> u32 {
    if root_id == unit.id {
        unit.effective_max_loops(config_max)
    } else {
        let root_path =
            find_unit_file(mana_dir, root_id).or_else(|_| find_archived_unit(mana_dir, root_id));
        match root_path {
            Ok(p) => Unit::from_file(&p)
                .map(|b| b.effective_max_loops(config_max))
                .unwrap_or(config_max),
            Err(_) => config_max,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Record a verify failure on a unit (internal).
fn record_failure_on_unit(unit: &mut Unit, failure: &VerifyFailure) {
    unit.attempts += 1;
    unit.updated_at = Utc::now();

    // Append failure to notes
    let failure_note = format_failure_note(unit.attempts, failure.exit_code, &failure.output);
    match &mut unit.notes {
        Some(notes) => notes.push_str(&failure_note),
        None => unit.notes = Some(failure_note),
    }

    // Record structured history entry
    let output_snippet = if failure.output.is_empty() {
        None
    } else {
        Some(truncate_output(&failure.output, 20))
    };
    unit.history.push(RunRecord {
        attempt: unit.attempts,
        started_at: failure.started_at,
        finished_at: Some(failure.finished_at),
        duration_secs: Some(failure.duration_secs),
        agent: failure.agent.clone(),
        result: if failure.timed_out {
            RunResult::Timeout
        } else {
            RunResult::Fail
        },
        exit_code: failure.exit_code,
        tokens: None,
        cost: None,
        output_snippet,
        autonomy_observation: None,
    });
    refresh_autonomy_disposition(unit);
}

fn refresh_autonomy_disposition(unit: &mut Unit) {
    unit.refresh_autonomy_disposition();
}

fn refresh_attempt_pressure(unit: &mut Unit) {
    refresh_autonomy_disposition(unit);
}

/// Capture verify stdout as unit outputs.
fn capture_verify_outputs(unit: &mut Unit, stdout: &str) {
    let stdout = stdout.trim();
    if stdout.is_empty() {
        return;
    }

    if stdout.len() > MAX_OUTPUT_BYTES {
        let end = truncate_to_char_boundary(stdout, MAX_OUTPUT_BYTES);
        let truncated = &stdout[..end];
        unit.outputs = Some(serde_json::json!({
            "text": truncated,
            "truncated": true,
            "original_bytes": stdout.len()
        }));
    } else {
        match serde_json::from_str::<serde_json::Value>(stdout) {
            Ok(json) => {
                unit.outputs = Some(json);
            }
            Err(_) => {
                unit.outputs = Some(serde_json::json!({
                    "text": stdout
                }));
            }
        }
    }
}

/// Find the largest byte index <= `max_bytes` that falls on a UTF-8 char boundary.
pub fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> usize {
    if max_bytes >= s.len() {
        return s.len();
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// Truncate output to first N + last N lines.
pub fn truncate_output(output: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = output.lines().collect();

    if lines.len() <= max_lines * 2 {
        return output.to_string();
    }

    let first = &lines[..max_lines];
    let last = &lines[lines.len() - max_lines..];

    format!(
        "{}\n\n... ({} lines omitted) ...\n\n{}",
        first.join("\n"),
        lines.len() - max_lines * 2,
        last.join("\n")
    )
}

/// Format a verify failure as a Markdown block to append to notes.
pub fn format_failure_note(attempt: u32, exit_code: Option<i32>, output: &str) -> String {
    let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let truncated = truncate_output(output, 50);
    let exit_str = exit_code
        .map(|c| format!("Exit code: {}\n", c))
        .unwrap_or_default();

    format!(
        "\n## Attempt {} — {}\n{}\n```\n{}\n```\n",
        attempt, timestamp, exit_str, truncated
    )
}

// ---------------------------------------------------------------------------
// Hook helpers
// ---------------------------------------------------------------------------

/// Run pre-close hook. Hook errors are returned as warnings but do not block close.
fn run_pre_close_hook(unit: &Unit, project_root: &Path, reason: Option<&str>) -> HookDecision {
    let result = execute_hook(
        HookEvent::PreClose,
        unit,
        project_root,
        reason.map(|s| s.to_string()),
    );

    match result {
        Ok(hook_passed) => HookDecision {
            accepted: hook_passed,
            warning: None,
        },
        Err(e) => HookDecision {
            accepted: true,
            warning: Some(CloseWarning::PreCloseHookError {
                message: e.to_string(),
            }),
        },
    }
}

/// Run post-close hook + on_close actions + config hooks.
fn run_post_close_actions(
    unit: &Unit,
    project_root: &Path,
    reason: Option<&str>,
    config: Option<&Config>,
) -> PostCloseActionsReport {
    let mut warnings = Vec::new();

    // Fire post-close hook
    match execute_hook(
        HookEvent::PostClose,
        unit,
        project_root,
        reason.map(|s| s.to_string()),
    ) {
        Ok(false) => warnings.push(CloseWarning::PostCloseHookRejected),
        Err(e) => warnings.push(CloseWarning::PostCloseHookError {
            message: e.to_string(),
        }),
        Ok(true) => {}
    }

    // Process on_close actions
    let mut on_close_results = Vec::new();
    for action in &unit.on_close {
        match action {
            OnCloseAction::Run { command } => {
                if !is_trusted(project_root) {
                    on_close_results.push(OnCloseActionResult::Skipped {
                        command: command.clone(),
                    });
                    continue;
                }

                let status = std::process::Command::new("sh")
                    .args(["-c", command.as_str()])
                    .current_dir(project_root)
                    .status();
                let result = match status {
                    Ok(status) => OnCloseActionResult::RanCommand {
                        command: command.clone(),
                        success: status.success(),
                        exit_code: status.code(),
                        error: None,
                    },
                    Err(e) => OnCloseActionResult::RanCommand {
                        command: command.clone(),
                        success: false,
                        exit_code: None,
                        error: Some(e.to_string()),
                    },
                };
                on_close_results.push(result);
            }
            OnCloseAction::Notify { message } => {
                on_close_results.push(OnCloseActionResult::Notified {
                    message: message.clone(),
                });
            }
        }
    }

    // Fire on_close config hook
    if let Some(config) = config {
        if let Some(ref on_close_template) = config.on_close {
            let vars = HookVars {
                id: Some(unit.id.clone()),
                title: Some(unit.title.clone()),
                status: Some("closed".into()),
                branch: current_git_branch(),
                ..Default::default()
            };
            execute_config_hook("on_close", on_close_template, &vars, project_root);
        }
    }

    PostCloseActionsReport {
        warnings,
        on_close_results,
    }
}

/// Fire the on_fail config hook.
fn run_on_fail_hook(unit: &Unit, project_root: &Path, config: Option<&Config>, output: &str) {
    if let Some(config) = config {
        if let Some(ref on_fail_template) = config.on_fail {
            let vars = HookVars {
                id: Some(unit.id.clone()),
                title: Some(unit.title.clone()),
                status: Some(format!("{}", unit.status)),
                attempt: Some(unit.attempts),
                output: Some(output.to_string()),
                branch: current_git_branch(),
                ..Default::default()
            };
            execute_config_hook("on_fail", on_fail_template, &vars, project_root);
        }
    }
}

// ---------------------------------------------------------------------------
// Worktree helpers
// ---------------------------------------------------------------------------

/// Detect and validate worktree context.
fn detect_valid_worktree(project_root: &Path) -> Option<crate::worktree::WorktreeInfo> {
    let info = crate::worktree::detect_worktree(project_root).unwrap_or(None)?;

    let canonical_root =
        std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    if canonical_root.starts_with(&info.worktree_path) {
        Some(info)
    } else {
        None
    }
}

/// Commit worktree changes and merge to main.
fn handle_worktree_merge(
    wt_info: &crate::worktree::WorktreeInfo,
    unit: &Unit,
) -> Result<WorktreeMergeStatus> {
    let message = expand_commit_template(
        DEFAULT_COMMIT_TEMPLATE,
        &unit.id,
        &unit.title,
        unit.parent.as_deref(),
        &unit.labels,
    );
    crate::worktree::commit_worktree_changes(&wt_info.worktree_path, &message)?;

    match crate::worktree::merge_to_main(wt_info, &unit.id)? {
        crate::worktree::MergeResult::Success | crate::worktree::MergeResult::NothingToCommit => {
            Ok(WorktreeMergeStatus::Merged)
        }
        crate::worktree::MergeResult::Conflict { files } => {
            Ok(WorktreeMergeStatus::Conflict { files })
        }
    }
}

/// Clean up worktree after successful close.
fn cleanup_worktree(wt_info: &crate::worktree::WorktreeInfo) -> Option<CloseWarning> {
    crate::worktree::cleanup_worktree(wt_info)
        .err()
        .map(|e| CloseWarning::WorktreeCleanupFailed {
            message: e.to_string(),
        })
}

/// Expand a commit template with placeholder values.
///
/// Supported placeholders: `{id}`, `{title}`, `{parent_id}`, `{labels}`.
fn expand_commit_template(
    template: &str,
    id: &str,
    title: &str,
    parent_id: Option<&str>,
    labels: &[String],
) -> String {
    template
        .replace("{id}", id)
        .replace("{title}", title)
        .replace("{parent_id}", parent_id.unwrap_or(""))
        .replace("{labels}", &labels.join(","))
}

/// Auto-commit changes on close (non-worktree mode).
fn auto_commit_on_close(
    project_root: &Path,
    id: &str,
    title: &str,
    parent_id: Option<&str>,
    labels: &[String],
    template: Option<&str>,
) -> AutoCommitResult {
    let message = expand_commit_template(
        template.unwrap_or(DEFAULT_COMMIT_TEMPLATE),
        id,
        title,
        parent_id,
        labels,
    );

    let add_status = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(project_root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status();

    match add_status {
        Ok(status) if !status.success() => {
            return AutoCommitResult {
                message,
                committed: false,
                warning: Some(format!(
                    "git add -A failed (exit {})",
                    status.code().unwrap_or(-1)
                )),
            };
        }
        Err(e) => {
            return AutoCommitResult {
                message,
                committed: false,
                warning: Some(format!("git add -A failed: {}", e)),
            };
        }
        _ => {}
    }

    let commit_result = std::process::Command::new("git")
        .args(["commit", "-m", &message])
        .current_dir(project_root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output();

    match commit_result {
        Ok(output) if output.status.success() => AutoCommitResult {
            message,
            committed: true,
            warning: None,
        },
        Ok(output) if output.status.code() == Some(1) => AutoCommitResult {
            message,
            committed: false,
            warning: None,
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            AutoCommitResult {
                message,
                committed: false,
                warning: Some(format!(
                    "git commit failed (exit {}): {}",
                    output.status.code().unwrap_or(-1),
                    stderr.trim()
                )),
            }
        }
        Err(e) => AutoCommitResult {
            message,
            committed: false,
            warning: Some(format!("git commit failed: {}", e)),
        },
    }
}

/// Rebuild the index.
fn rebuild_index(mana_dir: &Path) -> Result<()> {
    if mana_dir.exists() {
        let mut locked =
            LockedIndex::acquire(mana_dir).with_context(|| "Failed to acquire locked index")?;
        locked.index = Index::build(mana_dir).with_context(|| "Failed to rebuild index")?;
        locked
            .save_and_release()
            .with_context(|| "Failed to save index")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::{AutonomyBlockerCode, VerifyPosture};
    use crate::config::{Config, DEFAULT_COMMIT_TEMPLATE};
    use std::fs;
    use tempfile::TempDir;

    fn with_temp_home<T>(f: impl FnOnce() -> T) -> T {
        use std::sync::{Mutex, OnceLock};

        static HOME_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let guard = HOME_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();

        let home = tempfile::tempdir().unwrap();
        let old_home = std::env::var_os("HOME");
        std::env::set_var("HOME", home.path());
        let result = f();
        if let Some(old_home) = old_home {
            std::env::set_var("HOME", old_home);
        } else {
            std::env::remove_var("HOME");
        }
        drop(guard);
        result
    }

    fn setup_mana_dir() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        (dir, mana_dir)
    }

    fn setup_mana_dir_with_config() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        Config {
            project: "test".to_string(),
            next_id: 100,
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

    fn setup_git_mana_dir_with_config(config: Config) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let project_root = dir.path();
        let mana_dir = project_root.join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        config.save(&mana_dir).unwrap();

        run_git(project_root, &["init"]);
        run_git(project_root, &["config", "user.email", "test@test.com"]);
        run_git(project_root, &["config", "user.name", "Test"]);

        fs::write(project_root.join("initial.txt"), "initial").unwrap();
        run_git(project_root, &["add", "-A"]);
        run_git(project_root, &["commit", "-m", "Initial commit"]);

        (dir, mana_dir)
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|e| unreachable!("git {:?} failed to execute: {}", args, e));
        assert!(
            output.status.success(),
            "git {:?} in {} failed (exit {:?}):\nstdout: {}\nstderr: {}",
            args,
            dir.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    fn git_stdout(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|e| unreachable!("git {:?} failed to execute: {}", args, e));
        assert!(
            output.status.success(),
            "git {:?} in {} failed (exit {:?}):\nstdout: {}\nstderr: {}",
            args,
            dir.display(),
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn write_unit(mana_dir: &Path, unit: &Unit) {
        let slug = title_to_slug(&unit.title);
        unit.to_file(mana_dir.join(format!("{}-{}.md", unit.id, slug)))
            .unwrap();
    }

    // =====================================================================
    // close() tests
    // =====================================================================

    #[test]
    fn close_single_unit() {
        let (_dir, mana_dir) = setup_mana_dir();
        let unit = Unit::new("1", "Task");
        write_unit(&mana_dir, &unit);

        let result = close(
            &mana_dir,
            "1",
            CloseOpts {
                reason: None,
                force: false,
                defer_verify: false,
            },
        )
        .unwrap();

        match result {
            CloseOutcome::Closed(r) => {
                assert_eq!(r.unit.status, Status::Closed);
                assert!(r.unit.closed_at.is_some());
                assert!(r.unit.is_archived);
                assert!(r.archive_path.exists());
            }
            _ => panic!("Expected Closed outcome"),
        }
    }

    #[test]
    fn close_with_reason() {
        let (_dir, mana_dir) = setup_mana_dir();
        let unit = Unit::new("1", "Task");
        write_unit(&mana_dir, &unit);

        let result = close(
            &mana_dir,
            "1",
            CloseOpts {
                reason: Some("Fixed".to_string()),
                force: false,
                defer_verify: false,
            },
        )
        .unwrap();

        match result {
            CloseOutcome::Closed(r) => {
                assert_eq!(r.unit.close_reason, Some("Fixed".to_string()));
            }
            _ => panic!("Expected Closed outcome"),
        }
    }

    #[test]
    fn close_with_passing_verify() {
        let (_dir, mana_dir) = setup_mana_dir();
        let mut unit = Unit::new("1", "Task");
        unit.verify = Some("true".to_string());
        write_unit(&mana_dir, &unit);

        let result = close(
            &mana_dir,
            "1",
            CloseOpts {
                reason: None,
                force: false,
                defer_verify: false,
            },
        )
        .unwrap();

        match result {
            CloseOutcome::Closed(r) => {
                assert_eq!(r.unit.status, Status::Closed);
                assert!(r.unit.is_archived);
                assert_eq!(r.unit.history.len(), 1);
                let record = &r.unit.history[0];
                assert_eq!(record.result, RunResult::Pass);
                let snippet = record.output_snippet.as_deref().unwrap_or("");
                assert!(snippet.contains("verify passed"));
            }
            _ => panic!("Expected Closed outcome"),
        }
    }

    #[test]
    fn close_rejects_pass_ok_unit_with_no_non_mana_changes() {
        let config = Config {
            project: "test".to_string(),
            next_id: 100,
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
        let (dir, mana_dir) = setup_git_mana_dir_with_config(config);
        let checkpoint = git_stdout(dir.path(), &["rev-parse", "HEAD"])
            .trim()
            .to_string();

        let mut unit = Unit::new("1", "Pass-ok no-op");
        unit.status = Status::InProgress;
        unit.verify = Some("true".to_string());
        unit.checkpoint = Some(checkpoint);
        write_unit(&mana_dir, &unit);

        let result = close(
            &mana_dir,
            "1",
            CloseOpts {
                reason: None,
                force: false,
                defer_verify: false,
            },
        );

        let err = result.unwrap_err().to_string();
        assert!(err.contains("no non-.mana changes were detected since claim"));
    }

    #[test]
    fn close_allows_pass_ok_unit_when_non_mana_changes_exist() {
        let config = Config {
            project: "test".to_string(),
            next_id: 100,
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
        let (dir, mana_dir) = setup_git_mana_dir_with_config(config);
        let checkpoint = git_stdout(dir.path(), &["rev-parse", "HEAD"])
            .trim()
            .to_string();
        fs::write(dir.path().join("feature.txt"), "changed").unwrap();

        let mut unit = Unit::new("1", "Pass-ok with changes");
        unit.status = Status::InProgress;
        unit.verify = Some("true".to_string());
        unit.checkpoint = Some(checkpoint);
        write_unit(&mana_dir, &unit);

        let result = close(
            &mana_dir,
            "1",
            CloseOpts {
                reason: None,
                force: false,
                defer_verify: false,
            },
        )
        .unwrap();

        match result {
            CloseOutcome::Closed(r) => assert_eq!(r.unit.status, Status::Closed),
            _ => panic!("Expected Closed outcome"),
        }
    }

    #[test]
    fn close_with_failing_verify() {
        let (_dir, mana_dir) = setup_mana_dir();
        let mut unit = Unit::new("1", "Task");
        unit.verify = Some("false".to_string());
        write_unit(&mana_dir, &unit);

        let result = close(
            &mana_dir,
            "1",
            CloseOpts {
                reason: None,
                force: false,
                defer_verify: false,
            },
        )
        .unwrap();

        match result {
            CloseOutcome::VerifyFailed(r) => {
                assert_eq!(r.unit.status, Status::Open);
                assert_eq!(r.unit.attempts, 1);
            }
            _ => panic!("Expected VerifyFailed outcome"),
        }
    }

    #[test]
    fn close_force_skips_verify() {
        let (_dir, mana_dir) = setup_mana_dir();
        let mut unit = Unit::new("1", "Task");
        unit.verify = Some("false".to_string());
        write_unit(&mana_dir, &unit);

        let result = close(
            &mana_dir,
            "1",
            CloseOpts {
                reason: None,
                force: true,
                defer_verify: false,
            },
        )
        .unwrap();

        match result {
            CloseOutcome::Closed(r) => {
                assert_eq!(r.unit.status, Status::Closed);
                assert!(r.unit.is_archived);
                assert_eq!(r.unit.attempts, 0);
            }
            _ => panic!("Expected Closed outcome"),
        }
    }

    #[test]
    fn close_feature_returns_requires_human() {
        let (_dir, mana_dir) = setup_mana_dir();
        let mut unit = Unit::new("1", "Feature");
        unit.feature = true;
        write_unit(&mana_dir, &unit);

        let result = close(
            &mana_dir,
            "1",
            CloseOpts {
                reason: None,
                force: false,
                defer_verify: false,
            },
        )
        .unwrap();

        assert!(matches!(result, CloseOutcome::FeatureRequiresHuman { .. }));
    }

    #[test]
    fn close_nonexistent_unit() {
        let (_dir, mana_dir) = setup_mana_dir();
        let result = close(
            &mana_dir,
            "99",
            CloseOpts {
                reason: None,
                force: false,
                defer_verify: false,
            },
        );
        assert!(result.is_err());
    }

    // =====================================================================
    // close_failed() tests
    // =====================================================================

    #[test]
    fn close_failed_marks_unit_as_failed() {
        let (_dir, mana_dir) = setup_mana_dir();
        let mut unit = Unit::new("1", "Task");
        unit.status = Status::InProgress;
        unit.claimed_by = Some("agent-1".to_string());
        unit.attempt_log.push(crate::unit::AttemptRecord {
            num: 1,
            outcome: AttemptOutcome::Abandoned,
            notes: None,
            agent: Some("agent-1".to_string()),
            started_at: Some(Utc::now()),
            finished_at: None,
            autonomy_observation: None,
        });
        write_unit(&mana_dir, &unit);

        let result = close_failed(&mana_dir, "1", Some("blocked".to_string())).unwrap();
        assert_eq!(result.status, Status::Open);
        assert!(result.claimed_by.is_none());
        assert_eq!(result.attempt_log[0].outcome, AttemptOutcome::Failed);
        assert!(result.attempt_log[0].finished_at.is_some());
    }

    // =====================================================================
    // all_children_closed() tests
    // =====================================================================

    #[test]
    fn all_children_closed_when_no_children() {
        let (_dir, mana_dir) = setup_mana_dir();
        let unit = Unit::new("1", "Parent");
        write_unit(&mana_dir, &unit);

        assert!(all_children_closed(&mana_dir, "1").unwrap());
    }

    #[test]
    fn all_children_closed_when_some_open() {
        let (_dir, mana_dir) = setup_mana_dir();
        let parent = Unit::new("1", "Parent");
        write_unit(&mana_dir, &parent);

        let mut child1 = Unit::new("1.1", "Child 1");
        child1.parent = Some("1".to_string());
        child1.status = Status::Closed;
        write_unit(&mana_dir, &child1);

        let mut child2 = Unit::new("1.2", "Child 2");
        child2.parent = Some("1".to_string());
        write_unit(&mana_dir, &child2);

        assert!(!all_children_closed(&mana_dir, "1").unwrap());
    }

    // =====================================================================
    // auto_close_parents() tests
    // =====================================================================

    #[test]
    fn auto_close_parents_when_all_children_closed() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();
        let parent = Unit::new("1", "Parent");
        write_unit(&mana_dir, &parent);

        let mut child = Unit::new("1.1", "Child");
        child.parent = Some("1".to_string());
        write_unit(&mana_dir, &child);

        // Close the child first
        let _ = close(
            &mana_dir,
            "1.1",
            CloseOpts {
                reason: None,
                force: false,
                defer_verify: false,
            },
        )
        .unwrap();

        // Parent should be auto-closed
        let parent_archived = find_archived_unit(&mana_dir, "1");
        assert!(parent_archived.is_ok());
        let p = Unit::from_file(parent_archived.unwrap()).unwrap();
        assert_eq!(p.status, Status::Closed);
        assert!(p.close_reason.as_ref().unwrap().contains("Auto-closed"));
    }

    #[test]
    fn auto_close_skips_feature_parents() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();
        let mut parent = Unit::new("1", "Feature Parent");
        parent.feature = true;
        write_unit(&mana_dir, &parent);

        let mut child = Unit::new("1.1", "Child");
        child.parent = Some("1".to_string());
        write_unit(&mana_dir, &child);

        let _ = close(
            &mana_dir,
            "1.1",
            CloseOpts {
                reason: None,
                force: false,
                defer_verify: false,
            },
        )
        .unwrap();

        // Feature parent should still be open
        let parent_still_open = find_unit_file(&mana_dir, "1");
        assert!(parent_still_open.is_ok());
        let p = Unit::from_file(parent_still_open.unwrap()).unwrap();
        assert_eq!(p.status, Status::Open);
    }

    // =====================================================================
    // archive_unit() tests
    // =====================================================================

    #[test]
    fn archive_unit_moves_and_marks() {
        let (_dir, mana_dir) = setup_mana_dir();
        let mut unit = Unit::new("1", "Task");
        unit.status = Status::Closed;
        let slug = title_to_slug(&unit.title);
        let unit_path = mana_dir.join(format!("1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        let archive_path = archive_unit(&mana_dir, &mut unit, &unit_path).unwrap();
        assert!(archive_path.exists());
        assert!(!unit_path.exists());
        assert!(unit.is_archived);
    }

    // =====================================================================
    // record_failure() tests
    // =====================================================================

    #[test]
    fn record_failure_increments_attempts() {
        let mut unit = Unit::new("1", "Task");
        let failure = VerifyFailure {
            exit_code: Some(1),
            output: "error".to_string(),
            timed_out: false,
            duration_secs: 1.0,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            agent: None,
        };
        record_failure(&mut unit, &failure);
        assert_eq!(unit.attempts, 1);
        assert_eq!(unit.history.len(), 1);
        assert_eq!(unit.history[0].result, RunResult::Fail);
        let disposition = unit.autonomy_disposition.expect("attempt pressure should be derived");
        assert_eq!(disposition.attempt_pressure, crate::unit::AttemptPressure::WithinBudget);
        assert_eq!(disposition.continuation_budget, Some(2));
    }

    #[test]
    fn record_failure_timeout() {
        let mut unit = Unit::new("1", "Task");
        let failure = VerifyFailure {
            exit_code: None,
            output: "timed out".to_string(),
            timed_out: true,
            duration_secs: 30.0,
            started_at: Utc::now(),
            finished_at: Utc::now(),
            agent: None,
        };
        record_failure(&mut unit, &failure);
        assert_eq!(unit.history[0].result, RunResult::Timeout);
    }

    // =====================================================================
    // process_on_fail() tests
    // =====================================================================

    #[test]
    fn process_on_fail_retry_releases_claim() {
        let mut unit = Unit::new("1", "Task");
        unit.on_fail = Some(OnFailAction::Retry {
            max: Some(5),
            delay_secs: None,
        });
        unit.attempts = 1;
        unit.claimed_by = Some("agent-1".to_string());
        unit.claimed_at = Some(Utc::now());

        let result = process_on_fail(&mut unit);
        assert!(matches!(result, OnFailActionTaken::Retry { .. }));
        assert!(unit.claimed_by.is_none());
        let disposition = unit.autonomy_disposition.expect("attempt pressure should be present");
        assert_eq!(disposition.attempt_pressure, crate::unit::AttemptPressure::WithinBudget);
        assert_eq!(disposition.continuation_budget, Some(4));
    }

    #[test]
    fn process_on_fail_escalate_sets_priority() {
        let mut unit = Unit::new("1", "Task");
        unit.on_fail = Some(OnFailAction::Escalate {
            priority: Some(0),
            message: None,
        });
        unit.priority = 2;
        unit.history.push(RunRecord {
            attempt: 1,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            duration_secs: Some(1.0),
            agent: None,
            result: RunResult::Fail,
            exit_code: Some(1),
            tokens: None,
            cost: None,
            output_snippet: None,
            autonomy_observation: None,
        });

        let result = process_on_fail(&mut unit);
        assert!(matches!(result, OnFailActionTaken::Escalated));
        assert_eq!(unit.priority, 0);
        assert!(unit.labels.contains(&"escalated".to_string()));
        let disposition = unit.autonomy_disposition.expect("attempt pressure should be present");
        assert_eq!(disposition.attempt_pressure, crate::unit::AttemptPressure::Exhausted);
        assert!(disposition
            .blockers
            .contains(&crate::unit::AutonomyBlockerCode::AttemptBudgetExhausted));
    }

    #[test]
    fn process_on_fail_none() {
        let mut unit = Unit::new("1", "Task");
        let result = process_on_fail(&mut unit);
        assert!(matches!(result, OnFailActionTaken::None));
    }

    // =====================================================================
    // check_circuit_breaker() tests
    // =====================================================================

    #[test]
    fn circuit_breaker_zero_disabled() {
        let (_dir, mana_dir) = setup_mana_dir();
        let mut unit = Unit::new("1", "Task");
        let result = check_circuit_breaker(&mana_dir, &mut unit, "1", 0).unwrap();
        assert!(!result.tripped);
        let disposition = unit.autonomy_disposition.expect("attempt pressure should be present");
        assert_eq!(disposition.attempt_pressure, crate::unit::AttemptPressure::WithinBudget);
        assert_eq!(disposition.continuation_budget, Some(3));
    }

    #[test]
    fn circuit_breaker_sets_tripped_attempt_pressure() {
        let (_dir, mana_dir) = setup_mana_dir();
        let mut root = Unit::new("1", "Root");
        root.attempts = 2;
        write_unit(&mana_dir, &root);

        let mut child = Unit::new("1.1", "Child");
        child.parent = Some("1".to_string());
        child.attempts = 1;
        write_unit(&mana_dir, &child);

        let mut loaded_child = Unit::from_file(find_unit_file(&mana_dir, "1.1").unwrap()).unwrap();
        let result = check_circuit_breaker(&mana_dir, &mut loaded_child, "1", 3).unwrap();
        assert!(result.tripped);
        let disposition = loaded_child
            .autonomy_disposition
            .expect("attempt pressure should be present");
        assert_eq!(disposition.attempt_pressure, crate::unit::AttemptPressure::CircuitBreakerTripped);
        assert!(disposition
            .blockers
            .contains(&crate::unit::AutonomyBlockerCode::CircuitBreakerTripped));
        assert_eq!(disposition.continuation_budget, Some(0));
    }

    // =====================================================================
    // Helper tests
    // =====================================================================

    #[test]
    fn truncate_to_char_boundary_ascii() {
        let s = "hello world";
        assert_eq!(truncate_to_char_boundary(s, 5), 5);
    }

    #[test]
    fn truncate_to_char_boundary_multibyte() {
        let s = "😀😁😂";
        assert_eq!(truncate_to_char_boundary(s, 5), 4);
    }

    #[test]
    fn truncate_output_short() {
        let output = "line1\nline2\nline3";
        let result = truncate_output(output, 50);
        assert_eq!(result, output);
    }

    #[test]
    fn format_failure_note_includes_exit_code() {
        let note = format_failure_note(1, Some(1), "error message");
        assert!(note.contains("## Attempt 1"));
        assert!(note.contains("Exit code: 1"));
        assert!(note.contains("error message"));
    }

    #[test]
    fn expand_commit_template_substitutes_all_placeholders() {
        let message = expand_commit_template(
            "feat(unit-{id}): {title} [{parent_id}] {labels}",
            "2.3",
            "Ship it",
            Some("2"),
            &["feature".to_string(), "git".to_string()],
        );

        assert_eq!(message, "feat(unit-2.3): Ship it [2] feature,git");
    }

    #[test]
    fn close_auto_commit_uses_default_template_and_includes_index_updates() {
        with_temp_home(|| {
            let config = Config {
                project: "test".to_string(),
                next_id: 100,
                auto_commit: true,
                ..Config::default()
            };
            let (_dir, mana_dir) = setup_git_mana_dir_with_config(config);
            let project_root = mana_dir.parent().unwrap();

            let parent = Unit::new("1", "Parent");
            write_unit(&mana_dir, &parent);

            let mut child = Unit::new("1.1", "Child");
            child.parent = Some("1".to_string());
            write_unit(&mana_dir, &child);

            let result = close(
                &mana_dir,
                "1.1",
                CloseOpts {
                    reason: None,
                    force: false,
                    defer_verify: false,
                },
            )
            .unwrap();

            let close_result = match result {
                CloseOutcome::Closed(result) => result,
                other => panic!("Expected Closed outcome, got {:?}", other),
            };
            let auto_commit = close_result
                .auto_commit_result
                .expect("auto-commit result should be present when enabled");
            assert!(auto_commit.committed);
            assert_eq!(
                auto_commit.message,
                DEFAULT_COMMIT_TEMPLATE
                    .replace("{id}", "1.1")
                    .replace("{title}", "Child")
            );
            assert_eq!(close_result.auto_closed_parents, vec!["1".to_string()]);

            let head_subject = git_stdout(project_root, &["log", "-1", "--pretty=%s"]);
            assert_eq!(head_subject.trim(), "feat(unit-1.1): Child");

            let changed_files =
                git_stdout(project_root, &["show", "--name-only", "--format=", "HEAD"]);
            assert!(
                changed_files.contains(".mana/index.yaml"),
                "{changed_files}"
            );
            assert!(changed_files.contains("1-parent.md"), "{changed_files}");
            assert!(changed_files.contains("1.1-child.md"), "{changed_files}");
        });
    }

    // =====================================================================
    // close_defer tests — deferred verify via defer_verify: true
    // =====================================================================

    /// With defer_verify: true, close() skips the verify command and sets
    /// status to AwaitingVerify instead of Closed.
    #[test]
    fn close_defer_skips_verify() {
        let (_dir, mana_dir) = setup_mana_dir();
        let mut unit = Unit::new("1", "Task");
        // A failing verify — should NOT be run when defer_verify is true.
        unit.verify = Some("false".to_string());
        write_unit(&mana_dir, &unit);

        let outcome = close(
            &mana_dir,
            "1",
            CloseOpts {
                reason: None,
                force: false,
                defer_verify: true,
            },
        )
        .unwrap();

        // Unit should be AwaitingVerify, not Closed, and no verify failure recorded.
        match outcome {
            CloseOutcome::DeferredVerify { .. } => {}
            other => panic!("Expected DeferredVerify outcome, got {:?}", other),
        }

        // Confirm the on-disk state reflects AwaitingVerify.
        let saved = Unit::from_file(
            find_unit_file(&mana_dir, "1").expect("unit file should still be in active dir"),
        )
        .unwrap();
        assert_eq!(saved.status, Status::AwaitingVerify);
        let disposition = saved
            .autonomy_disposition
            .expect("deferred verify should persist autonomy disposition");
        assert_eq!(disposition.verify, VerifyPosture::Deferred);
        assert!(disposition
            .blockers
            .contains(&AutonomyBlockerCode::VerifyDeferred));
        // No verify was run — attempts counter stays at 0.
        assert_eq!(saved.attempts, 0);
    }

    /// With defer_verify: true, the returned outcome is DeferredVerify containing
    /// the correct unit ID.
    #[test]
    fn close_defer_returns_outcome() {
        let (_dir, mana_dir) = setup_mana_dir();
        let unit = Unit::new("42", "Deferred Task");
        write_unit(&mana_dir, &unit);

        let outcome = close(
            &mana_dir,
            "42",
            CloseOpts {
                reason: None,
                force: false,
                defer_verify: true,
            },
        )
        .unwrap();

        match outcome {
            CloseOutcome::DeferredVerify { unit_id } => {
                assert_eq!(unit_id, "42");
            }
            other => panic!("Expected DeferredVerify outcome, got {:?}", other),
        }
    }

    #[test]
    fn close_verify_frozen_violation_persists_autonomy_gate() {
        let (_dir, mana_dir) = setup_mana_dir();
        let mut unit = Unit::new("1", "Frozen verify");
        unit.verify = Some("true".to_string());
        let mut hasher = Sha256::new();
        hasher.update("false".as_bytes());
        unit.verify_hash = Some(format!("{:x}", hasher.finalize()));
        write_unit(&mana_dir, &unit);

        let outcome = close(
            &mana_dir,
            "1",
            CloseOpts {
                reason: None,
                force: false,
                defer_verify: false,
            },
        )
        .unwrap();

        match outcome {
            CloseOutcome::VerifyFrozenViolation { unit_id, .. } => {
                assert_eq!(unit_id, "1");
            }
            other => panic!("Expected VerifyFrozenViolation outcome, got {:?}", other),
        }

        let saved = Unit::from_file(find_unit_file(&mana_dir, "1").unwrap()).unwrap();
        let disposition = saved
            .autonomy_disposition
            .expect("frozen violation should persist autonomy disposition");
        assert_eq!(disposition.verify, VerifyPosture::FrozenViolation);
        assert!(disposition
            .blockers
            .contains(&AutonomyBlockerCode::VerifyFrozenViolation));
    }

    #[test]
    fn close_defer_normal_unchanged() {
        let (_dir, mana_dir) = setup_mana_dir();
        let mut unit = Unit::new("1", "Task");
        unit.verify = Some("false".to_string());
        write_unit(&mana_dir, &unit);

        let outcome = close(
            &mana_dir,
            "1",
            CloseOpts {
                reason: None,
                force: false,
                defer_verify: false,
            },
        )
        .unwrap();

        match outcome {
            CloseOutcome::VerifyFailed(r) => {
                assert_eq!(r.unit.status, Status::Open);
                assert_eq!(r.unit.attempts, 1);
                let disposition = r
                    .unit
                    .autonomy_disposition
                    .expect("verify failure should persist autonomy disposition");
                assert_eq!(disposition.verify, VerifyPosture::Failed);
                assert!(disposition
                    .blockers
                    .contains(&AutonomyBlockerCode::VerifyFailed));
            }
            other => panic!("Expected VerifyFailed outcome, got {:?}", other),
        }
    }
}
