use std::path::Path;

use anyhow::{anyhow, Result};
use mana_core::ops::{fact, fact_sheet};
use serde::Serialize;

/// Create a verified fact (convenience wrapper around create with unit_type=fact).
///
/// Facts require a verify command — that's the point. If you can't write a
/// verify command, the knowledge belongs in agents.md, not in `mana fact`.
pub fn cmd_fact(
    mana_dir: &Path,
    title: String,
    verify: String,
    description: Option<String>,
    paths: Option<String>,
    ttl_days: Option<i64>,
    pass_ok: bool,
) -> Result<String> {
    if verify.trim().is_empty() {
        return Err(anyhow!(
            "Facts require a verify command. If you can't write one, \
             this belongs in agents.md, not mana fact."
        ));
    }

    let result = fact::create_fact(
        mana_dir,
        fact::FactParams {
            title,
            verify,
            description,
            paths,
            ttl_days,
            pass_ok,
        },
    )?;

    eprintln!("Created fact {}: {}", result.unit_id, result.unit.title);
    Ok(result.unit_id)
}

/// Verify all facts and report staleness.
///
/// Re-runs verify commands for all units with unit_type=fact.
/// Reports which facts are stale (past their stale_after date)
/// and which have failing verify commands.
///
/// Suspect propagation: facts that require artifacts from failing/stale facts
/// are marked as suspect (up to depth 3).
pub fn cmd_verify_facts(mana_dir: &Path, json: bool) -> Result<()> {
    // Run the lightweight fact-sheet checker first so `mana verify-facts` remains
    // compatible while the fact surface evolves toward `mana fact check`.
    let sheet_result = fact_sheet::check_facts_sheet(mana_dir)?;
    let result = fact::verify_facts(mana_dir)?;

    if json {
        let output = FactCheckJson {
            fact_sheet: &sheet_result,
            fact_units: &result,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print_fact_sheet_check(&sheet_result);
        print_fact_unit_check(&result);
    }

    if sheet_result.has_errors() {
        anyhow::bail!("fact sheet check failed");
    }

    if result.failing_count > 0 {
        anyhow::bail!("{} fact(s) failed verification", result.failing_count);
    }

    Ok(())
}

#[derive(Serialize)]
struct FactCheckJson<'a> {
    fact_sheet: &'a fact_sheet::FactSheetCheckResult,
    fact_units: &'a fact::VerifyFactsResult,
}

fn print_fact_unit_check(result: &fact::VerifyFactsResult) {
    for entry in &result.entries {
        if entry.stale {
            eprintln!("⚠ STALE: [{}] \"{}\"", entry.id, entry.title);
        }

        match (entry.verify_passed, entry.error.as_deref()) {
            (Some(true), _) => println!("  ✓ [{}] \"{}\"", entry.id, entry.title),
            (Some(false), Some(error)) => {
                eprintln!("  ✗ ERROR: [{}] \"{}\" — {}", entry.id, entry.title, error)
            }
            (Some(false), None) => eprintln!(
                "  ✗ FAILING: [{}] \"{}\" — verify command returned non-zero",
                entry.id, entry.title
            ),
            (None, _) => {}
        }
    }

    for (id, title) in &result.suspect_entries {
        eprintln!(
            "  ⚠ SUSPECT: [{}] \"{}\" — requires artifact from invalid fact",
            id, title
        );
    }

    println!();
    println!(
        "Facts: {} total, {} verified, {} stale, {} failing, {} suspect",
        result.total_facts,
        result.verified_count,
        result.stale_count,
        result.failing_count,
        result.suspect_count
    );
}

fn print_fact_sheet_check(result: &fact_sheet::FactSheetCheckResult) {
    if result.facts.is_empty() && result.diagnostics.is_empty() && result.entries.is_empty() {
        return;
    }

    println!("Fact sheet: {}", result.path.display());

    for diagnostic in &result.diagnostics {
        let line = diagnostic
            .line
            .map(|line| format!("line {line}: "))
            .unwrap_or_default();
        eprintln!("  ✗ {}{}", line, diagnostic.message);
    }

    for entry in &result.entries {
        if entry.passed {
            println!(
                "  ✓ line {} [{}] {}",
                entry.fact.line,
                entry.fact.status.as_str(),
                entry.fact.text
            );
        } else if let Some(message) = &entry.message {
            eprintln!(
                "  ✗ line {} [{}] {} — {}",
                entry.fact.line,
                entry.fact.status.as_str(),
                entry.fact.text,
                message
            );
        }
    }

    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use chrono::Utc;

    use crate::config::Config;
    use crate::discovery::find_unit_file;
    use crate::unit::Unit;
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
    fn create_fact_sets_unit_type() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let id = cmd_fact(
            &mana_dir,
            "Auth uses RS256".to_string(),
            "grep -q RS256 src/auth.rs".to_string(),
            None,
            None,
            None,
            true, // pass_ok since file doesn't exist
        )
        .unwrap();

        let unit_path = find_unit_file(&mana_dir, &id).unwrap();
        let unit = Unit::from_file(&unit_path).unwrap();

        assert_eq!(unit.unit_type, "fact");
        assert!(unit.labels.contains(&"fact".to_string()));
        assert!(unit.stale_after.is_some());
        assert!(unit.verify.is_some());
    }

    #[test]
    fn create_fact_with_paths() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let id = cmd_fact(
            &mana_dir,
            "Config file format".to_string(),
            "grep -q 'project: test' .mana/config.yaml".to_string(),
            None,
            Some("src/config.rs, src/main.rs".to_string()),
            None,
            true,
        )
        .unwrap();

        let unit_path = find_unit_file(&mana_dir, &id).unwrap();
        let unit = Unit::from_file(&unit_path).unwrap();

        assert_eq!(unit.paths, vec!["src/config.rs", "src/main.rs"]);
    }

    #[test]
    fn create_fact_with_custom_ttl() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let id = cmd_fact(
            &mana_dir,
            "Short-lived fact".to_string(),
            "grep -q 'project: test' .mana/config.yaml".to_string(),
            None,
            None,
            Some(7), // 7 days
            true,
        )
        .unwrap();

        let unit_path = find_unit_file(&mana_dir, &id).unwrap();
        let unit = Unit::from_file(&unit_path).unwrap();

        // stale_after should be ~7 days from now
        let stale = unit.stale_after.unwrap();
        let diff = stale - Utc::now();
        assert!(diff.num_days() >= 6 && diff.num_days() <= 7);
    }

    #[test]
    fn create_fact_requires_verify() {
        let (_dir, mana_dir) = setup_mana_dir_with_config();

        let result = cmd_fact(
            &mana_dir,
            "No verify fact".to_string(),
            "  ".to_string(), // empty verify
            None,
            None,
            None,
            true,
        );

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("verify command"));
    }
}
