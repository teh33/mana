use chrono::Utc;

use crate::discovery::{find_archived_unit, find_unit_file};
use crate::graph;
use crate::unit::{OnFailAction, RunRecord, RunResult, Unit};

use anyhow::Result;
use std::path::Path;

use super::verify::{format_failure_note, truncate_output};

/// What kind of on_fail action was processed.
pub(crate) enum OnFailActionTaken {
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
pub(crate) struct CircuitBreakerResult {
    pub tripped: bool,
    pub subtree_total: u32,
    pub max_loops: u32,
}

/// Metadata about a verify failure, used by record_failure.
pub(crate) struct FailureRecord {
    pub exit_code: Option<i32>,
    pub output: String,
    pub timed_out: bool,
    pub duration_secs: f64,
    pub started_at: chrono::DateTime<Utc>,
    pub finished_at: chrono::DateTime<Utc>,
    pub agent: Option<String>,
}

/// Record a verify failure on a unit. Updates attempts, notes, history.
/// Does not save — caller decides when to write.
pub(crate) fn record_failure(unit: &mut Unit, failure: &FailureRecord) {
    unit.attempts += 1;
    unit.updated_at = Utc::now();

    // Append failure to notes (backward compat)
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
}

/// Process on_fail actions. Mutates unit (release claim for retry, escalate priority).
/// Returns what action was taken for display purposes.
pub(crate) fn process_on_fail(unit: &mut Unit) -> OnFailActionTaken {
    let on_fail = match &unit.on_fail {
        Some(action) => action.clone(),
        None => return OnFailActionTaken::None,
    };

    match on_fail {
        OnFailAction::Retry { max, delay_secs } => {
            let max_retries = max.unwrap_or(unit.max_attempts);
            if unit.attempts < max_retries {
                // Release claim so bw/deli can pick it up
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
    }
}

/// Check circuit breaker for a unit. If subtree attempts exceed max_loops, trips the breaker.
///
/// The unit must already have been saved to disk before calling this (so subtree count
/// includes the current attempt). If the breaker trips, the unit is mutated (label + P0)
/// but NOT saved — caller decides when to write.
pub(crate) fn check_circuit_breaker(
    mana_dir: &Path,
    unit: &mut Unit,
    root_id: &str,
    max_loops: u32,
) -> Result<CircuitBreakerResult> {
    if max_loops == 0 {
        return Ok(CircuitBreakerResult {
            tripped: false,
            subtree_total: 0,
            max_loops: 0,
        });
    }

    let subtree_total = graph::count_subtree_attempts(mana_dir, root_id)?;
    if subtree_total >= max_loops {
        // Trip circuit breaker
        if !unit.labels.contains(&"circuit-breaker".to_string()) {
            unit.labels.push("circuit-breaker".to_string());
        }
        unit.priority = 0;
        Ok(CircuitBreakerResult {
            tripped: true,
            subtree_total,
            max_loops,
        })
    } else {
        Ok(CircuitBreakerResult {
            tripped: false,
            subtree_total,
            max_loops,
        })
    }
}

/// Resolve the effective max_loops for a unit, considering root parent overrides.
pub(crate) fn resolve_max_loops(
    mana_dir: &Path,
    unit: &Unit,
    root_id: &str,
    config_max: u32,
) -> u32 {
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
