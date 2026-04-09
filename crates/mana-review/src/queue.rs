//! Review queue — list and rank units awaiting review.
//!
//! Scans `.mana/` for units that are closed and haven't been reviewed,
//! scores them, and returns a ranked queue.

use anyhow::Result;
use std::path::Path;

use crate::risk;
use crate::types::{FileChange, QueueEntry};

/// Build the review queue from the current `.mana/` state.
///
/// Returns units that are closed (verify passed) but haven't been
/// reviewed yet, ranked by risk level (highest first).
pub fn build(mana_dir: &Path, project_root: &Path) -> Result<Vec<QueueEntry>> {
    let index = mana_core::api::load_index(mana_dir)?;

    let mut entries = Vec::new();

    for unit_entry in &index.units {
        // Only show closed units (verify passed)
        if unit_entry.status != mana_core::api::Status::Closed {
            continue;
        }

        // Skip features — they're human-reviewed at a higher level
        if unit_entry.feature {
            continue;
        }

        // Skip units that already have a persisted review record.
        if crate::state::has_review(mana_dir, &unit_entry.id) {
            continue;
        }

        // Load the full unit for risk scoring
        let unit = mana_core::api::get_unit(mana_dir, &unit_entry.id)?;

        let file_changes = get_file_changes_for_unit(project_root, &unit);

        let (risk_level, risk_flags) = risk::score(&unit, &file_changes);

        let total_additions: u32 = file_changes.iter().map(|fc| fc.additions).sum();
        let total_deletions: u32 = file_changes.iter().map(|fc| fc.deletions).sum();

        entries.push(QueueEntry {
            unit_id: unit.id.clone(),
            title: unit.title.clone(),
            risk_level,
            risk_flags,
            attempt: unit.attempts,
            file_count: file_changes.len(),
            additions: total_additions,
            deletions: total_deletions,
        });
    }

    // Sort: Critical first, then High, Normal, Low
    entries.sort_by(|a, b| b.risk_level.cmp(&a.risk_level));

    Ok(entries)
}

/// Get file changes for a unit.
///
/// Uses the unit's checkpoint (if available) to compute the diff
/// against the state before work began.
fn get_file_changes_for_unit(project_root: &Path, unit: &mana_core::unit::Unit) -> Vec<FileChange> {
    let Some(checkpoint) = unit.checkpoint.as_deref() else {
        // No checkpoint means we don't have a trustworthy baseline for this unit.
        return vec![];
    };

    crate::diff::compute(project_root, Some(checkpoint))
        .map(|(_, file_changes)| file_changes)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state;
    use crate::types::{Review, ReviewDecision, RiskFlagKind, RiskLevel};
    use chrono::Utc;
    use mana_core::unit::{Status, Unit};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use tempfile::TempDir;

    fn setup_mana_dir() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path().join(".mana");
        fs::create_dir_all(&mana_dir).unwrap();
        (tmp, mana_dir)
    }

    fn write_unit(mana_dir: &Path, unit: &Unit) {
        unit.to_file(mana_dir.join(format!("{}-test-unit.md", unit.id)))
            .unwrap();
    }

    fn review_for(unit_id: &str) -> Review {
        Review {
            unit_id: unit_id.to_string(),
            attempt: 1,
            decision: ReviewDecision::Approved,
            summary: Some("Looks good".to_string()),
            annotations: vec![],
            reviewed_at: Utc::now(),
            reviewer: "human".to_string(),
        }
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn init_repo(repo: &Path) {
        git(repo, &["init"]);
        git(repo, &["config", "user.name", "Mana Review Tests"]);
        git(repo, &["config", "user.email", "tests@example.com"]);
    }

    #[test]
    fn reviewed_unit_excluded_from_queue() {
        let (_tmp, mana_dir) = setup_mana_dir();

        let mut unit = Unit::new("1", "Reviewed unit");
        unit.status = Status::Closed;
        write_unit(&mana_dir, &unit);
        state::save(&mana_dir, &review_for("1")).unwrap();

        let entries = build(&mana_dir, mana_dir.parent().unwrap()).unwrap();

        assert!(entries.is_empty());
    }

    #[test]
    fn unreviewed_closed_unit_included_when_diff_unavailable() {
        let (_tmp, mana_dir) = setup_mana_dir();

        let mut unit = Unit::new("1", "Closed unit");
        unit.status = Status::Closed;
        unit.checkpoint = Some("deadbeef".to_string());
        write_unit(&mana_dir, &unit);

        let entries = build(&mana_dir, mana_dir.parent().unwrap()).unwrap();

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.unit_id, "1");
        assert_eq!(entry.file_count, 0);
        assert_eq!(entry.additions, 0);
        assert_eq!(entry.deletions, 0);
        assert_eq!(entry.risk_level, RiskLevel::Low);
    }

    #[test]
    fn queue_uses_diff_evidence_for_stats_and_risk() {
        let tmp = TempDir::new().unwrap();
        let project_root = tmp.path();
        init_repo(project_root);

        fs::create_dir_all(project_root.join("src")).unwrap();
        fs::write(project_root.join("src/auth.rs"), "pub fn login() {\n    // v1\n}\n").unwrap();
        git(project_root, &["add", "src/auth.rs"]);
        git(project_root, &["commit", "-m", "base"]);
        let checkpoint = git_stdout(project_root, &["rev-parse", "HEAD"]);

        fs::write(
            project_root.join("src/auth.rs"),
            "pub fn login() {\n    // v2\n}\n\npub fn issue_token() {}\n",
        )
        .unwrap();
        git(project_root, &["add", "src/auth.rs"]);
        git(project_root, &["commit", "-m", "change auth"]);

        let mana_dir = project_root.join(".mana");
        fs::create_dir_all(&mana_dir).unwrap();

        let mut unit = Unit::new("1", "Review auth change");
        unit.status = Status::Closed;
        unit.checkpoint = Some(checkpoint);
        write_unit(&mana_dir, &unit);

        let entries = build(&mana_dir, project_root).unwrap();

        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.unit_id, "1");
        assert_eq!(entry.file_count, 1);
        assert_eq!(entry.additions, 3);
        assert_eq!(entry.deletions, 1);
        assert_eq!(entry.risk_level, RiskLevel::Critical);
        assert!(entry
            .risk_flags
            .iter()
            .any(|flag| flag.kind == RiskFlagKind::SecuritySensitive));
    }
}
