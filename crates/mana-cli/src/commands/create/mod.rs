use std::path::Path;
use std::process::Command as ShellCommand;

use anyhow::{anyhow, Context, Result};
use mana_core::ops::create;
use mana_core::verify_lint::{lint_verify, VerifyLintLevel};

use crate::commands::claim::cmd_claim;
use crate::index::Index;
use crate::project::suggest_verify_command;
use crate::unit::{validate_priority, OnFailAction, UnitKind};
use crate::util::{find_similar_titles, DEFAULT_SIMILARITY_THRESHOLD};

/// Create arguments structure for organizing all the parameters passed to create.
pub struct CreateArgs {
    pub title: String,
    pub description: Option<String>,
    pub acceptance: Option<String>,
    pub notes: Option<String>,
    pub design: Option<String>,
    pub verify: Option<String>,
    pub priority: Option<u8>,
    pub labels: Option<String>,
    pub assignee: Option<String>,
    pub deps: Option<String>,
    pub parent: Option<String>,
    pub produces: Option<String>,
    pub requires: Option<String>,
    /// Comma-separated file paths relevant to this unit.
    pub paths: Option<String>,
    /// Action on verify failure
    pub on_fail: Option<OnFailAction>,
    /// Skip fail-first check (allow verify to already pass)
    pub pass_ok: bool,
    /// Claim the unit immediately after creation
    pub claim: bool,
    /// Who is claiming (used with claim)
    pub by: Option<String>,
    /// Timeout in seconds for the verify command (kills process on expiry).
    pub verify_timeout: Option<u64>,
    /// Mark as a product feature (human-only close, no verify gate required).
    pub feature: bool,
    /// Unresolved decisions that block autonomous execution.
    pub decisions: Vec<String>,
    /// Mark the new unit as an epic instead of a job.
    pub epic: bool,
    /// Skip duplicate title check
    pub force: bool,
}

/// Assign a child ID for a parent unit.
/// Scans .mana/ for {parent_id}.{N}-*.md, finds highest N, returns "{parent_id}.{N+1}".
pub fn assign_child_id(mana_dir: &Path, parent_id: &str) -> Result<String> {
    create::assign_child_id(mana_dir, parent_id)
}

/// Parse an `--on-fail` CLI string into an `OnFailAction`.
///
/// Accepted formats:
/// - `retry` → Retry { max: None, delay_secs: None }
/// - `retry:5` → Retry { max: Some(5), delay_secs: None }
/// - `escalate` → Escalate { priority: None, message: None }
/// - `escalate:P0` or `escalate:0` → Escalate { priority: Some(0), message: None }
pub(crate) fn lint_verify_command(verify_cmd: Option<&str>, force: bool) -> Result<()> {
    let Some(verify_cmd) = verify_cmd else {
        return Ok(());
    };

    let findings = lint_verify(verify_cmd);
    if findings.is_empty() {
        return Ok(());
    }

    let error_count = findings
        .iter()
        .filter(|finding| finding.level == VerifyLintLevel::Error)
        .count();

    for finding in &findings {
        let label = match finding.level {
            VerifyLintLevel::Error => "verify lint error",
            VerifyLintLevel::Warning => "verify lint warning",
        };
        eprintln!("{}: {}", label, finding.message);
    }

    if error_count > 0 && !force {
        anyhow::bail!(
            "Refusing to create unit: verify command has {} lint error(s). Use --force to create anyway.",
            error_count
        );
    }

    if error_count > 0 {
        eprintln!("Proceeding despite verify lint errors because --force was used.");
    }

    Ok(())
}

pub fn parse_on_fail(s: &str) -> Result<OnFailAction> {
    create::parse_on_fail(s)
}

/// Create a new unit.
///
/// If `args.parent` is given, assign a child ID ({parent_id}.{next_child}).
/// Otherwise, use the next sequential ID from config and increment it.
/// Returns the created unit ID on success.
pub fn cmd_create(mana_dir: &Path, args: CreateArgs) -> Result<String> {
    if let Some(priority) = args.priority {
        validate_priority(priority)?;
    }

    if args.claim && args.parent.is_none() && args.acceptance.is_none() && args.verify.is_none() {
        anyhow::bail!(
            "Unit must have validation criteria: provide --acceptance or --verify (or both)\n\
             Hint: parent/goal units (without --claim) don't require this."
        );
    }

    // Verify lint is handled by mana_core::ops::create::create() at the library level.
    // All consumers (CLI, imp, MCP) get it automatically.

    if !args.pass_ok {
        if let Some(verify_cmd) = args.verify.as_ref() {
            let project_root = mana_dir
                .parent()
                .ok_or_else(|| anyhow!("Cannot determine project root"))?;

            eprintln!("Running verify (must fail): {}", verify_cmd);

            let status = ShellCommand::new("sh")
                .args(["-c", verify_cmd])
                .current_dir(project_root)
                .status()
                .with_context(|| format!("Failed to execute verify command: {}", verify_cmd))?;

            if status.success() {
                anyhow::bail!(
                    "Cannot create unit: verify command already passes!\n\n\
                     The test must FAIL on current code to prove it tests something real.\n\
                     Either:\n\
                     - The test doesn't actually test the new behavior\n\
                     - The feature is already implemented\n\
                     - The test is a no-op (assert True)\n\n\
                     Use --pass-ok / -p to skip this check."
                );
            }

            eprintln!("✓ Verify failed as expected - test is real");
        }
    }

    if !args.force {
        if let Ok(index) = Index::load_or_rebuild(mana_dir) {
            let similar = find_similar_titles(&index, &args.title, DEFAULT_SIMILARITY_THRESHOLD);
            if !similar.is_empty() {
                let mut msg = String::from("Similar unit(s) already exist:\n");
                for s in &similar {
                    msg.push_str(&format!(
                        "  [{}] {} (similarity: {:.0}%)\n",
                        s.id,
                        s.title,
                        s.score * 100.0
                    ));
                }
                msg.push_str("\nUse --force to create anyway.");
                anyhow::bail!(msg);
            }
        }
    }

    let has_verify = args.verify.is_some();
    let title = args.title.clone();
    let params = create::CreateParams {
        title: args.title,
        description: args.description,
        acceptance: args.acceptance,
        notes: args.notes,
        design: args.design,
        verify: args.verify,
        priority: args.priority,
        labels: split_csv(args.labels),
        assignee: args.assignee,
        dependencies: split_csv(args.deps),
        parent: args.parent,
        produces: split_csv(args.produces),
        requires: split_csv(args.requires),
        paths: split_csv(args.paths),
        on_fail: args.on_fail,
        fail_first: !args.pass_ok && has_verify,
        feature: args.feature,
        kind: if args.epic {
            Some(UnitKind::Epic)
        } else {
            None
        },
        verify_timeout: args.verify_timeout,
        decisions: args.decisions,
        force: args.force,
    };

    let result = create::create(mana_dir, params)?;
    let unit_id = result.unit.id.clone();

    eprintln!("Created unit {}: {}", unit_id, title);

    if !has_verify {
        let project_dir = mana_dir
            .parent()
            .ok_or_else(|| anyhow!("Failed to determine project directory"))?;
        if let Some(suggested) = suggest_verify_command(project_dir) {
            eprintln!(
                "Tip: Consider adding a verify command: --verify \"{}\"",
                suggested
            );
        }
    }

    if args.claim {
        cmd_claim(mana_dir, &unit_id, args.by, true)?;
    }

    Ok(unit_id)
}

/// Create a new unit that automatically depends on @latest (the most recently updated unit).
///
/// This enables sequential chaining:
/// ```bash
/// mana create "Step 1" -p
/// mana create next "Step 2" --verify "cargo test step2"
/// mana create next "Step 3" --verify "cargo test step3"
/// ```
///
/// If `args.deps` already contains dependencies, @latest is prepended.
/// Returns the created unit ID on success.
pub fn cmd_create_next(mana_dir: &Path, args: CreateArgs) -> Result<String> {
    // Resolve @latest — find the most recently updated unit
    let index = Index::load(mana_dir).or_else(|_| Index::build(mana_dir))?;
    let latest_id = index
        .units
        .iter()
        .max_by_key(|e| e.updated_at)
        .map(|e| e.id.clone())
        .ok_or_else(|| {
            anyhow!(
                "No previous unit found. 'mana create next' requires at least one existing unit.\n\
                 Use 'mana create' for the first unit in a chain."
            )
        })?;

    // Merge @latest dep with any explicit deps
    let merged_deps = match args.deps {
        Some(ref d) => Some(format!("{},{}", latest_id, d)),
        None => Some(latest_id.clone()),
    };

    eprintln!("⛓ Chained after unit {} (@latest)", latest_id);

    let new_args = CreateArgs {
        deps: merged_deps,
        ..args
    };

    cmd_create(mana_dir, new_args)
}

fn split_csv(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests;
