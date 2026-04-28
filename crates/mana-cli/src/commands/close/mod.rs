#[allow(dead_code)]
mod worktree_merge;

use std::path::Path;

use anyhow::{anyhow, Context, Result};

use crate::unit::Unit;

// Re-export core close ops for use in tests
use mana_core::ops::close::{
    self as ops_close, AutoCommitResult, CloseOpts, CloseOutcome, CloseWarning,
    OnCloseActionResult, OnFailActionTaken,
};

// These imports are used by test modules via `use super::*`
#[allow(unused_imports)]
use crate::index::Index;
#[allow(unused_imports)]
use crate::unit::{RunResult, Status};
#[allow(unused_imports)]
use chrono::Utc;
#[allow(unused_imports)]
use mana_core::ops::close::truncate_to_char_boundary;

#[cfg(test)]
use std::fs;

fn print_close_warnings(unit_id: &str, warnings: &[CloseWarning]) {
    for warning in warnings {
        match warning {
            CloseWarning::PreCloseHookError { message } => {
                eprintln!(
                    "Warning: pre-close hook error for unit {}: {}",
                    unit_id, message
                );
            }
            CloseWarning::PostCloseHookRejected => {
                eprintln!(
                    "Warning: post-close hook returned non-zero for unit {}",
                    unit_id
                );
            }
            CloseWarning::PostCloseHookError { message } => {
                eprintln!(
                    "Warning: post-close hook error for unit {}: {}",
                    unit_id, message
                );
            }
            CloseWarning::WorktreeCleanupFailed { message } => {
                eprintln!(
                    "Warning: failed to clean up worktree for unit {}: {}",
                    unit_id, message
                );
            }
            CloseWarning::VerifyChanged => {
                eprintln!(
                    "⚠ Warning: verify command was changed since claim for unit {} (--force overrode)",
                    unit_id
                );
            }
        }
    }
}

fn print_on_close_results(unit_id: &str, results: &[OnCloseActionResult]) {
    for result in results {
        match result {
            OnCloseActionResult::RanCommand {
                command,
                success,
                exit_code,
                error,
            } => {
                if *success {
                    eprintln!("on_close: ran `{}`", command);
                } else if let Some(error) = error {
                    eprintln!(
                        "on_close run command error for unit {} (`{}`): {}",
                        unit_id, command, error
                    );
                } else {
                    eprintln!(
                        "on_close run command failed for unit {} (`{}`) with exit {}",
                        unit_id,
                        command,
                        exit_code.unwrap_or(-1)
                    );
                }
            }
            OnCloseActionResult::Notified { message } => {
                println!("[unit {}] {}", unit_id, message);
            }
            OnCloseActionResult::Skipped { command } => {
                eprintln!(
                    "on_close: skipping `{}` for unit {} (not trusted — run `mana trust` to enable)",
                    command,
                    unit_id
                );
            }
        }
    }
}

fn print_auto_commit_result(result: &AutoCommitResult) {
    if result.committed {
        eprintln!("auto_commit: {}", result.message);
    }
    if let Some(warning) = &result.warning {
        eprintln!("Warning: {}", warning);
    }
}

/// Close one or more units.
///
/// Sets status=closed, closed_at=now, and optionally close_reason.
/// If the unit has a verify command, it must pass before closing (unless force=true).
/// Calls pre-close hook before verify (can block close if hook fails).
/// Auto-closes parent units when all children are closed (if enabled in config).
/// Rebuilds the index.
pub fn cmd_close(
    mana_dir: &Path,
    ids: Vec<String>,
    reason: Option<String>,
    force: bool,
    defer_verify: bool,
) -> Result<()> {
    if ids.is_empty() {
        return Err(anyhow!("At least one unit ID is required"));
    }

    let mut any_closed = false;
    let mut rejected_units = Vec::new();

    for id in &ids {
        let outcome = ops_close::close(
            mana_dir,
            id,
            CloseOpts {
                reason: reason.clone(),
                force,
                defer_verify,
            },
        )?;

        match outcome {
            CloseOutcome::Closed(result) => {
                println!("Closed unit {}: {}", id, result.unit.title);
                any_closed = true;

                // Close evidence warnings
                if let Some(ref evidence) = result.evidence {
                    if evidence.only_mana_changes {
                        eprintln!("⚠ Only .mana/ files changed — no implementation detected");
                    }
                    if evidence.no_path_overlap && !result.unit.paths.is_empty() {
                        eprintln!(
                            "⚠ No changed files overlap with unit paths: {:?}",
                            result.unit.paths
                        );
                    }
                }

                print_on_close_results(&result.unit.id, &result.on_close_results);
                print_close_warnings(&result.unit.id, &result.warnings);
                if let Some(auto_commit_result) = &result.auto_commit_result {
                    print_auto_commit_result(auto_commit_result);
                }

                for parent_id in &result.auto_closed_parents {
                    // Load from archive to get the title
                    if let Ok(archived_path) =
                        crate::discovery::find_archived_unit(mana_dir, parent_id)
                    {
                        if let Ok(parent) = Unit::from_file(&archived_path) {
                            println!("Auto-closed parent unit {}: {}", parent_id, parent.title);
                        }
                    }
                }
            }
            CloseOutcome::VerifyFailed(result) => {
                print_close_warnings(&result.unit.id, &result.warnings);

                // Display detailed failure feedback
                if result.timed_out {
                    println!("✗ Verify timed out for unit {}", id);
                } else {
                    println!("✗ Verify failed for unit {}", id);
                }
                println!();
                println!("Command: {}", result.verify_command);
                if result.timed_out {
                    println!("Timed out after {}s", result.timeout_secs.unwrap_or(0));
                } else if let Some(code) = result.exit_code {
                    println!("Exit code: {}", code);
                }
                if !result.output.is_empty() {
                    println!("Output:");
                    for line in result.output.lines() {
                        println!("  {}", line);
                    }
                }
                println!();
                println!("Attempt {}. Unit remains open.", result.attempt_number);
                println!("Tip: Run `mana verify {}` to test without closing.", id);
                println!("Tip: Use `mana close {} --force` to skip verify.", id);

                // Display on_fail action info
                if let Some(action) = result.on_fail_action_taken {
                    match action {
                        OnFailActionTaken::Retry {
                            attempt,
                            max,
                            delay_secs,
                        } => {
                            println!("on_fail: will retry (attempt {}/{})", attempt, max);
                            if let Some(delay) = delay_secs {
                                println!(
                                    "on_fail: retry delay {}s (enforced by orchestrator)",
                                    delay
                                );
                            }
                        }
                        OnFailActionTaken::RetryExhausted { max } => {
                            println!("on_fail: max retries ({}) exhausted", max);
                        }
                        OnFailActionTaken::Escalated => {
                            // Load unit to get on_fail details
                            if let Some(crate::unit::OnFailAction::Escalate { priority, message }) =
                                &result.unit.on_fail
                            {
                                if let Some(p) = priority {
                                    println!("on_fail: escalated priority → P{}", p);
                                }
                                if let Some(msg) = message {
                                    println!("on_fail: {}", msg);
                                }
                            }
                        }
                        OnFailActionTaken::None => {}
                    }
                }
            }
            CloseOutcome::RejectedByHook { unit_id } => {
                eprintln!("Unit {} rejected by pre-close hook", unit_id);
                rejected_units.push(unit_id);
            }
            CloseOutcome::FeatureRequiresHuman {
                unit_id,
                title,
                warnings,
            } => {
                print_close_warnings(&unit_id, &warnings);

                use std::io::IsTerminal;
                if !std::io::stdin().is_terminal() {
                    println!("Feature \"{}\" requires human review to close.", title);
                    continue;
                }
                eprintln!("Feature: \"{}\" — mark as complete? [y/N] ", title);
                let mut input = String::new();
                std::io::stdin().read_line(&mut input).unwrap_or(0);
                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Skipped feature \"{}\"", title);
                    continue;
                }
                // User confirmed — close with force to bypass verify and feature gate.
                let outcome = ops_close::close(
                    mana_dir,
                    &unit_id,
                    CloseOpts {
                        reason: reason.clone(),
                        force: true,
                        defer_verify: false,
                    },
                );
                match outcome {
                    Ok(CloseOutcome::Closed(result)) => {
                        println!("Closed unit {}: {}", unit_id, result.unit.title);
                        print_on_close_results(&result.unit.id, &result.on_close_results);
                        print_close_warnings(&result.unit.id, &result.warnings);
                        if let Some(auto_commit_result) = &result.auto_commit_result {
                            print_auto_commit_result(auto_commit_result);
                        }
                        any_closed = true;
                    }
                    Ok(other) => {
                        eprintln!("Failed to close feature unit {}: {:?}", unit_id, other);
                    }
                    Err(err) => {
                        eprintln!("Failed to close feature unit {}: {}", unit_id, err);
                    }
                }
            }
            CloseOutcome::CircuitBreakerTripped {
                unit_id,
                total_attempts,
                max,
                warnings,
            } => {
                print_close_warnings(&unit_id, &warnings);
                eprintln!(
                    "⚡ Circuit breaker tripped for unit {} \
                     (subtree total {} >= max_loops {})",
                    unit_id, total_attempts, max
                );
                eprintln!(
                    "Unit {} escalated to P0 with 'circuit-breaker' label. \
                     Manual intervention required.",
                    unit_id
                );
            }
            CloseOutcome::MergeConflict { files, warnings } => {
                print_close_warnings(id, &warnings);
                eprintln!("Merge conflict in files: {:?}", files);
                eprintln!("Resolve conflicts and run `mana close {}` again", id);
            }
            CloseOutcome::DeferredVerify { unit_id } => {
                println!(
                    "Deferred verify for unit {} — status set to awaiting_verify",
                    unit_id
                );
            }
            CloseOutcome::VerifyFrozenViolation { unit_id, warnings } => {
                print_close_warnings(&unit_id, &warnings);
                eprintln!(
                    "✕ Unit {} verify command was changed since claim — judge integrity violated",
                    unit_id
                );
                eprintln!("  The verify command must not change after work begins.");
                eprintln!("  Use --force to override (will be flagged for review).");
            }
        }
    }

    // Report rejected units
    if !rejected_units.is_empty() {
        eprintln!(
            "Failed to close {} unit(s) due to pre-close hook rejection: {}",
            rejected_units.len(),
            rejected_units.join(", ")
        );
    }

    // Rebuild index once after all updates
    if (any_closed || !ids.is_empty()) && mana_dir.exists() {
        let index = Index::build(mana_dir).with_context(|| "Failed to rebuild index")?;
        index
            .save(mana_dir)
            .with_context(|| "Failed to save index")?;
    }

    Ok(())
}

/// Mark an attempt as explicitly failed.
///
/// The unit stays open and the claim is released so another agent can retry.
/// Records the failure in attempt_log for episodic memory.
pub fn cmd_close_failed(mana_dir: &Path, ids: Vec<String>, reason: Option<String>) -> Result<()> {
    if ids.is_empty() {
        return Err(anyhow!("At least one unit ID is required"));
    }

    for id in &ids {
        let result = ops_close::close_failed(mana_dir, id, reason.clone())?;

        let attempt_count = result.attempt_log.len();
        println!(
            "Marked unit {} as failed (attempt #{}): {}",
            id, attempt_count, result.title
        );
        if let Some(ref reason_text) = reason {
            println!("  Reason: {}", reason_text);
        }
        println!("  Unit remains open for retry.");
    }

    Ok(())
}

#[cfg(test)]
#[path = "tests_close.rs"]
mod tests;

#[cfg(test)]
#[path = "tests_verify_timeout.rs"]
mod verify_timeout_tests;
