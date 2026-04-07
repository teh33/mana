use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::discovery::{archive_path_for_unit, find_unit_file};
use crate::index::{ArchiveIndex, Index};
use crate::output::Output;
use crate::unit::{Status, Unit};
use crate::util::title_to_slug;

/// A record of one unit that was (or would be) archived during tidy.
/// We collect these so we can print a summary at the end.
struct TidiedUnit {
    id: String,
    title: String,
    archive_path: String,
}

/// A record of one unit that was (or would be) released during tidy.
struct ReleasedUnit {
    id: String,
    title: String,
    reason: String,
}

/// A record of one unit that was auto-closed because its verify gate passed.
struct SweptUnit {
    id: String,
    title: String,
}

/// A record of one unit whose verify gate failed during sweep.
struct SweptFailure {
    id: String,
    title: String,
    reason: String,
}

/// Format a chrono Duration as a human-readable string like "3 days ago"
/// or "2 hours ago".
fn format_duration(duration: chrono::Duration) -> String {
    let secs = duration.num_seconds();
    if secs < 0 {
        return "just now".to_string();
    }
    let minutes = secs / 60;
    let hours = minutes / 60;
    let days = hours / 24;

    if days > 0 {
        format!("claimed {} day(s) ago", days)
    } else if hours > 0 {
        format!("claimed {} hour(s) ago", hours)
    } else if minutes > 0 {
        format!("claimed {} minute(s) ago", minutes)
    } else {
        "claimed just now".to_string()
    }
}

/// Check if a process matching a pattern is running (excluding our own PID).
fn pgrep_running(pattern: &str) -> bool {
    let current_pid = std::process::id();
    if let Ok(output) = std::process::Command::new("pgrep")
        .args(["-f", pattern])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if let Ok(pid) = line.trim().parse::<u32>() {
                    if pid != current_pid {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Check if any agent processes are currently running.
///
/// Uses the configured `run` command to determine what process pattern to
/// look for. Falls back to checking for common agent patterns if no run
/// command is configured.
fn has_running_agents() -> bool {
    // If a run command is configured, extract the binary name and search for it
    if let Ok(config) = crate::config::Config::load_with_extends(std::path::Path::new(".mana")) {
        if let Some(ref run_cmd) = config.run {
            // Extract the first word (binary name) from the run template
            if let Some(binary) = run_cmd.split_whitespace().next() {
                if pgrep_running(binary) {
                    return true;
                }
            }
            // Also search for the full command pattern (with {id} stripped)
            let pattern = run_cmd.replace("{id}", "");
            if pgrep_running(pattern.trim()) {
                return true;
            }
            return false;
        }
    }

    // No run command configured — check common agent patterns as fallback
    pgrep_running("pi -p units") || pgrep_running("deli spawn") || pgrep_running("claude")
}

/// Tidy the units directory: archive closed units, release stale in-progress
/// units, and rebuild the index.
///
/// Delegates to `cmd_tidy_inner` with the real agent-detection function.
pub fn cmd_tidy(mana_dir: &Path, dry_run: bool, out: &Output) -> Result<()> {
    cmd_tidy_inner(mana_dir, dry_run, has_running_agents, out)
}

/// Inner implementation of tidy, with an injectable agent-check function
/// for testability.
///
/// This is a housekeeping command that catches state inconsistencies:
///
/// - **Closed units not archived:** units whose status was set to "closed"
///   via `mana update --status closed` (which bypasses the close command's
///   archiving logic), units closed before archiving was added, or files
///   edited by hand.
///
/// - **Stale in-progress units:** units whose status is "in_progress" but
///   no agent is actually working on them. This happens when an agent
///   crashes without releasing its claim, when `deli spawn` is killed, or
///   when files are edited by hand. These are released back to "open".
///
/// The steps are:
/// 1. Build a fresh index from disk so we see every unit, even if the
///    cached index is stale.
/// 2. Walk through the index looking for units with status == Closed
///    that are still sitting in the main .mana/ directory (is_archived
///    is false).
/// 3. For each one, compute its archive path (using closed_at if available,
///    otherwise today's date) and move it there.
/// 4. Check for in-progress units that appear stale (no running agent
///    processes detected) and release them back to open.
/// 5. Rebuild and save the index one final time so it reflects the new
///    state.
///
/// With `dry_run = true` we report what would change without touching
/// any files.
fn cmd_tidy_inner(
    mana_dir: &Path,
    dry_run: bool,
    check_agents: fn() -> bool,
    out: &Output,
) -> Result<()> {
    // Step 1 — Build a fresh index so we're working from the truth on disk,
    // not a potentially stale cache.
    let index = Index::build(mana_dir).context("Failed to build index")?;

    // Step 2 — Find every closed unit that's still in the main directory.
    // We filter on two things:
    //   • status == Closed  (the unit is done)
    //   • find_unit_file succeeds (the file is still in .mana/, not archive/)
    //
    // We also skip units that have open children — archiving them would
    // orphan the children's parent reference without the parent being
    // findable in the main directory.
    let closed: Vec<&crate::index::IndexEntry> = index
        .units
        .iter()
        .filter(|entry| entry.status == Status::Closed)
        .collect();

    let mut tidied: Vec<TidiedUnit> = Vec::new();
    let mut skipped_parent_ids: Vec<String> = Vec::new();

    for entry in &closed {
        // Double-check the file actually exists in the main directory.
        // If find_unit_file fails, it's either already archived or
        // something weird — either way, nothing for us to do.
        let unit_path = match find_unit_file(mana_dir, &entry.id) {
            Ok(path) => path,
            Err(_) => continue,
        };

        // Load the full unit so we can read closed_at, slug, etc.
        let mut unit = Unit::from_file(&unit_path)
            .with_context(|| format!("Failed to load unit: {}", entry.id))?;

        // Safety check: if this unit is already marked archived, skip it.
        // (Shouldn't happen since it's in the main dir, but be defensive.)
        if unit.is_archived {
            continue;
        }

        // Guard: don't archive a parent whose children are still open.
        // We check by looking for any unit in the index that lists this
        // unit as its parent and isn't closed yet.
        let has_open_children = index
            .units
            .iter()
            .any(|b| b.parent.as_deref() == Some(entry.id.as_str()) && b.status != Status::Closed);

        if has_open_children {
            skipped_parent_ids.push(entry.id.clone());
            continue;
        }

        // Pick the date for the archive subdirectory.
        // Prefer closed_at (when the unit was actually finished) because
        // that groups archived units by *completion* month.  Fall back to
        // updated_at (always present) if closed_at was never set — this
        // happens for units that were closed via `mana update --status closed`
        // which doesn't set closed_at.
        let archive_date = unit
            .closed_at
            .unwrap_or(unit.updated_at)
            .with_timezone(&chrono::Local)
            .date_naive();

        // Build the target path under .mana/archive/YYYY/MM/<id>-<slug>.<ext>
        let slug = unit
            .slug
            .clone()
            .unwrap_or_else(|| title_to_slug(&unit.title));
        let ext = unit_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("md");
        let archive_path = archive_path_for_unit(mana_dir, &entry.id, &slug, ext, archive_date);

        // Record what we're about to do (for the summary).
        // We store the archive path relative to .mana/ to keep output tidy.
        let relative = archive_path.strip_prefix(mana_dir).unwrap_or(&archive_path);
        tidied.push(TidiedUnit {
            id: entry.id.clone(),
            title: entry.title.clone(),
            archive_path: relative.display().to_string(),
        });

        // In dry-run mode we stop here — no file moves.
        if dry_run {
            continue;
        }

        // Step 3 — Actually move the unit.
        // Create the archive directory tree (archive/YYYY/MM) if needed.
        if let Some(parent) = archive_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create archive directory for unit {}", entry.id)
            })?;
        }

        // Move the file from .mana/<id>-<slug>.md → .mana/archive/YYYY/MM/…
        std::fs::rename(&unit_path, &archive_path)
            .with_context(|| format!("Failed to move unit {} to archive", entry.id))?;

        // Mark the unit as archived and persist. This sets is_archived = true
        // in the YAML front-matter so other commands (unarchive, list --all)
        // know this unit lives in the archive.
        unit.is_archived = true;
        unit.to_file(&archive_path)
            .with_context(|| format!("Failed to save archived unit: {}", entry.id))?;
    }

    // Step 4 — Release stale in-progress units.
    //
    // An in-progress unit is "stale" if no agent process is currently
    // running that could be working on it. We check for running `pi`
    // and `deli spawn` processes. If none are found, all in-progress
    // units are considered stale and released back to open.
    //
    // If agents ARE running, we skip this step entirely because we
    // can't reliably determine which units they're working on.
    let in_progress: Vec<&crate::index::IndexEntry> = index
        .units
        .iter()
        .filter(|entry| entry.status == Status::InProgress)
        .collect();

    let mut released: Vec<ReleasedUnit> = Vec::new();

    if !in_progress.is_empty() {
        let agents_running = check_agents();

        if agents_running {
            // Agents are running — we can't safely release in-progress
            // units because one of them might be actively being worked on.
            // Just report them.
            out.warn(&format!(
                "{} in-progress unit(s) found, but agent processes are running — skipping release",
                in_progress.len()
            ));
        } else {
            // No agents running — all in-progress units are stale.
            for entry in &in_progress {
                let unit_path = match find_unit_file(mana_dir, &entry.id) {
                    Ok(path) => path,
                    Err(_) => continue,
                };

                let mut unit = match Unit::from_file(&unit_path) {
                    Ok(b) => b,
                    Err(_) => continue,
                };

                // Build a human-readable reason for the release.
                let reason = if let Some(claimed_at) = unit.claimed_at {
                    let age = Utc::now().signed_duration_since(claimed_at);
                    format_duration(age)
                } else {
                    "never properly claimed".to_string()
                };

                released.push(ReleasedUnit {
                    id: entry.id.clone(),
                    title: entry.title.clone(),
                    reason,
                });

                if dry_run {
                    continue;
                }

                // Release the unit: set status to Open, clear claim fields.
                let now = Utc::now();
                unit.status = Status::Open;
                unit.claimed_by = None;
                unit.claimed_at = None;
                unit.updated_at = now;

                unit.to_file(&unit_path)
                    .with_context(|| format!("Failed to release stale unit: {}", entry.id))?;
            }
        }
    }

    // Step 5 — Sweep: verify open units and close those that pass.
    //
    // Find every open unit that has a verify command. Run it. If it passes,
    // close the unit (which archives it, auto-closes parents, etc.).
    // This catches units that were implemented but never formally closed.
    let sweep_index = Index::build(mana_dir).context("Failed to rebuild index for sweep")?;

    let verifiable_open: Vec<&crate::index::IndexEntry> = sweep_index
        .units
        .iter()
        .filter(|entry| {
            entry.status == Status::Open
                && entry.kind == crate::unit::UnitKind::Job
                && entry.has_verify
        })
        .collect();

    let mut swept: Vec<SweptUnit> = Vec::new();
    let mut sweep_failed: Vec<SweptFailure> = Vec::new();

    for entry in &verifiable_open {
        if dry_run {
            // In dry-run, run verify but don't close
            use mana_core::ops::verify::run_verify;
            match run_verify(mana_dir, &entry.id) {
                Ok(Some(result)) if result.passed => {
                    swept.push(SweptUnit {
                        id: entry.id.clone(),
                        title: entry.title.clone(),
                    });
                }
                Ok(Some(result)) => {
                    let reason = if result.timed_out {
                        "timed out".to_string()
                    } else {
                        format!("exit {}", result.exit_code.unwrap_or(-1))
                    };
                    sweep_failed.push(SweptFailure {
                        id: entry.id.clone(),
                        title: entry.title.clone(),
                        reason,
                    });
                }
                _ => {}
            }
            continue;
        }

        use mana_core::ops::close::{self as ops_close, CloseOpts, CloseOutcome};
        match ops_close::close(
            mana_dir,
            &entry.id,
            CloseOpts {
                reason: Some("verify passed (tidy sweep)".to_string()),
                force: false,
                defer_verify: false,
            },
        ) {
            Ok(CloseOutcome::Closed(result)) => {
                swept.push(SweptUnit {
                    id: entry.id.clone(),
                    title: result.unit.title.clone(),
                });
            }
            Ok(CloseOutcome::VerifyFailed(result)) => {
                let reason = if result.timed_out {
                    "timed out".to_string()
                } else {
                    format!("exit {}", result.exit_code.unwrap_or(-1))
                };
                sweep_failed.push(SweptFailure {
                    id: entry.id.clone(),
                    title: entry.title.clone(),
                    reason,
                });
            }
            Ok(_) => {} // deferred, hook rejected, etc — skip silently
            Err(e) => {
                sweep_failed.push(SweptFailure {
                    id: entry.id.clone(),
                    title: entry.title.clone(),
                    reason: e.to_string(),
                });
            }
        }
    }

    // Step 6 — Rebuild the index one final time.
    // After moving files around, releasing stale units, and sweeping,
    // the old index is stale, so we rebuild from disk.
    let final_index = Index::build(mana_dir).context("Failed to rebuild index after tidy")?;
    final_index.save(mana_dir).context("Failed to save index")?;

    // Step 6b — Rebuild the archive index too, since units were moved into archive.
    if !dry_run && (!tidied.is_empty() || !swept.is_empty()) {
        let archive_index =
            ArchiveIndex::build(mana_dir).context("Failed to rebuild archive index after tidy")?;
        archive_index
            .save(mana_dir)
            .context("Failed to save archive index")?;
    }

    // ── Print results ────────────────────────────────────────────────

    let archive_verb = if dry_run { "Would archive" } else { "Archived" };
    let release_verb = if dry_run { "Would release" } else { "Released" };

    if tidied.is_empty()
        && skipped_parent_ids.is_empty()
        && released.is_empty()
        && swept.is_empty()
        && sweep_failed.is_empty()
    {
        out.info("Nothing to tidy — all units look good.");
    }

    if !tidied.is_empty() {
        out.info(&format!("{} {} unit(s):", archive_verb, tidied.len()));
        for t in &tidied {
            out.info(&format!("  → {}. {} → {}", t.id, t.title, t.archive_path));
        }
    }

    if !released.is_empty() {
        out.info(&format!(
            "{} {} stale in-progress unit(s):",
            release_verb,
            released.len()
        ));
        for r in &released {
            out.info(&format!("  → {}. {} ({})", r.id, r.title, r.reason));
        }
    }

    let sweep_verb = if dry_run { "Would close" } else { "Closed" };

    if !swept.is_empty() {
        out.info(&format!(
            "{} {} unit(s) (verify passed):",
            sweep_verb,
            swept.len()
        ));
        for s in &swept {
            out.info(&format!("  ✓ {}. {}", s.id, s.title));
        }
    }

    if !sweep_failed.is_empty() {
        out.info(&format!(
            "Verify failed for {} unit(s):",
            sweep_failed.len()
        ));
        for f in &sweep_failed {
            out.info(&format!("  ✗ {}. {} ({})", f.id, f.title, f.reason));
        }
    }

    if !skipped_parent_ids.is_empty() {
        out.warn(&format!(
            "Skipped {} closed parent(s) with open children: {}",
            skipped_parent_ids.len(),
            skipped_parent_ids.join(", ")
        ));
    }

    out.info(&format!(
        "Index rebuilt: {} unit(s) indexed.",
        final_index.units.len()
    ));

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::Output;
    use crate::unit::Unit;
    use crate::util::title_to_slug;
    use std::fs;
    use tempfile::TempDir;

    /// Create a .mana/ directory and return (TempDir guard, path).
    fn setup() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        (dir, mana_dir)
    }

    /// Mock: no agents running (for testing stale-release behavior).
    fn no_agents() -> bool {
        false
    }

    /// Mock: agents are running (for testing skip behavior).
    fn agents_running() -> bool {
        true
    }

    /// Helper: write a unit to the main .mana/ directory.
    fn write_unit(mana_dir: &Path, unit: &Unit) {
        let slug = title_to_slug(&unit.title);
        let path = mana_dir.join(format!("{}-{}.md", unit.id, slug));
        unit.to_file(path).unwrap();
    }

    // ── Basic behaviour ────────────────────────────────────────────

    #[test]
    fn tidy_archives_closed_units() {
        let (_dir, mana_dir) = setup();

        let mut unit = Unit::new("1", "Done task");
        unit.status = Status::Closed;
        unit.closed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &unit);

        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();

        // Should no longer be in main directory
        assert!(find_unit_file(&mana_dir, "1").is_err());
        // Should be in archive
        let archived = crate::discovery::find_archived_unit(&mana_dir, "1");
        assert!(archived.is_ok());
        let archived_unit = Unit::from_file(archived.unwrap()).unwrap();
        assert!(archived_unit.is_archived);
    }

    #[test]
    fn tidy_leaves_open_units_alone() {
        let (_dir, mana_dir) = setup();

        let unit = Unit::new("1", "Open task");
        write_unit(&mana_dir, &unit);

        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();

        // Should still be in main directory
        assert!(find_unit_file(&mana_dir, "1").is_ok());
    }

    #[test]
    fn tidy_idempotent() {
        let (_dir, mana_dir) = setup();

        let mut unit = Unit::new("1", "Done task");
        unit.status = Status::Closed;
        unit.closed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &unit);

        // First tidy archives it
        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();
        // Second tidy should be a no-op (no panic, no error)
        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();

        let archived = crate::discovery::find_archived_unit(&mana_dir, "1");
        assert!(archived.is_ok());
    }

    // ── Dry-run ────────────────────────────────────────────────────

    #[test]
    fn tidy_dry_run_does_not_move_files() {
        let (_dir, mana_dir) = setup();

        let mut unit = Unit::new("1", "Done task");
        unit.status = Status::Closed;
        unit.closed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &unit);

        cmd_tidy_inner(&mana_dir, true, no_agents, &Output::new()).unwrap();

        // File should still be in main directory (dry-run)
        assert!(find_unit_file(&mana_dir, "1").is_ok());
    }

    // ── Skips parents with open children ───────────────────────────

    #[test]
    fn tidy_skips_closed_parent_with_open_children() {
        let (_dir, mana_dir) = setup();

        // Parent is closed
        let mut parent = Unit::new("1", "Parent");
        parent.status = Status::Closed;
        parent.closed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &parent);

        // Child is still open
        let mut child = Unit::new("1.1", "Child");
        child.parent = Some("1".to_string());
        write_unit(&mana_dir, &child);

        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();

        // Parent should NOT be archived because child is still open
        assert!(find_unit_file(&mana_dir, "1").is_ok());
        // Child should still be in main dir
        assert!(find_unit_file(&mana_dir, "1.1").is_ok());
    }

    #[test]
    fn tidy_archives_parent_when_all_children_closed() {
        let (_dir, mana_dir) = setup();

        // Parent is closed
        let mut parent = Unit::new("1", "Parent");
        parent.status = Status::Closed;
        parent.closed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &parent);

        // Child is also closed
        let mut child = Unit::new("1.1", "Child");
        child.parent = Some("1".to_string());
        child.status = Status::Closed;
        child.closed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &child);

        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();

        // Both should be archived
        assert!(find_unit_file(&mana_dir, "1").is_err());
        assert!(find_unit_file(&mana_dir, "1.1").is_err());
        assert!(crate::discovery::find_archived_unit(&mana_dir, "1").is_ok());
        assert!(crate::discovery::find_archived_unit(&mana_dir, "1.1").is_ok());
    }

    // ── Uses closed_at for archive path ────────────────────────────

    #[test]
    fn tidy_uses_closed_at_for_archive_date() {
        let (_dir, mana_dir) = setup();

        let mut unit = Unit::new("1", "January task");
        unit.status = Status::Closed;
        // Force a specific closed_at date
        unit.closed_at = Some(
            chrono::DateTime::parse_from_rfc3339("2025-06-15T12:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        );
        write_unit(&mana_dir, &unit);

        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();

        let archived = crate::discovery::find_archived_unit(&mana_dir, "1").unwrap();
        // The archive path should contain 2025/06 (from closed_at)
        let path_str = archived.display().to_string();
        assert!(
            path_str.contains("2025") && path_str.contains("06"),
            "Expected archive under 2025/06, got: {}",
            path_str
        );
    }

    // ── Mixed open and closed ──────────────────────────────────────

    #[test]
    fn tidy_handles_mix_of_open_closed_and_in_progress() {
        let (_dir, mana_dir) = setup();

        let open_unit = Unit::new("1", "Still open");
        write_unit(&mana_dir, &open_unit);

        let mut closed_unit = Unit::new("2", "Already done");
        closed_unit.status = Status::Closed;
        closed_unit.closed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &closed_unit);

        let mut in_progress = Unit::new("3", "Working on it");
        in_progress.status = Status::InProgress;
        write_unit(&mana_dir, &in_progress);

        // With no agents running, in_progress units get released
        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();

        // Open unit untouched
        let b1 = Unit::from_file(find_unit_file(&mana_dir, "1").unwrap()).unwrap();
        assert_eq!(b1.status, Status::Open);

        // Closed unit archived
        assert!(find_unit_file(&mana_dir, "2").is_err());
        assert!(crate::discovery::find_archived_unit(&mana_dir, "2").is_ok());

        // In-progress unit released (no agents running)
        let b3 = Unit::from_file(find_unit_file(&mana_dir, "3").unwrap()).unwrap();
        assert_eq!(b3.status, Status::Open);
    }

    #[test]
    fn tidy_skips_in_progress_when_agents_running() {
        let (_dir, mana_dir) = setup();

        let mut unit = Unit::new("1", "Active WIP");
        unit.status = Status::InProgress;
        unit.claimed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &unit);

        // With agents running, in_progress units are NOT released
        cmd_tidy_inner(&mana_dir, false, agents_running, &Output::new()).unwrap();

        let updated = Unit::from_file(find_unit_file(&mana_dir, "1").unwrap()).unwrap();
        assert_eq!(updated.status, Status::InProgress);
        assert!(updated.claimed_at.is_some());
    }

    // ── Stale in-progress units ──────────────────────────────────

    #[test]
    fn tidy_releases_stale_in_progress_units() {
        let (_dir, mana_dir) = setup();

        // Create an in-progress unit with a stale claim (old claimed_at, no running agent)
        let mut unit = Unit::new("1", "Stale WIP");
        unit.status = Status::InProgress;
        unit.claimed_at = Some(
            chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        );
        write_unit(&mana_dir, &unit);

        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();

        // Unit should be released back to open
        let updated = Unit::from_file(find_unit_file(&mana_dir, "1").unwrap()).unwrap();
        assert_eq!(updated.status, Status::Open);
        assert!(updated.claimed_by.is_none());
        assert!(updated.claimed_at.is_none());
    }

    #[test]
    fn tidy_releases_in_progress_unit_without_claimed_at() {
        let (_dir, mana_dir) = setup();

        // Create a unit that was manually set to in_progress without proper claiming
        let mut unit = Unit::new("1", "Manually set WIP");
        unit.status = Status::InProgress;
        // No claimed_at, no claimed_by — definitely stale
        write_unit(&mana_dir, &unit);

        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();

        let updated = Unit::from_file(find_unit_file(&mana_dir, "1").unwrap()).unwrap();
        assert_eq!(updated.status, Status::Open);
    }

    #[test]
    fn tidy_dry_run_does_not_release_stale_units() {
        let (_dir, mana_dir) = setup();

        let mut unit = Unit::new("1", "Stale WIP");
        unit.status = Status::InProgress;
        unit.claimed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &unit);

        cmd_tidy_inner(&mana_dir, true, no_agents, &Output::new()).unwrap();

        // Unit should still be in_progress (dry-run)
        let updated = Unit::from_file(find_unit_file(&mana_dir, "1").unwrap()).unwrap();
        assert_eq!(updated.status, Status::InProgress);
        assert!(updated.claimed_at.is_some());
    }

    #[test]
    fn tidy_handles_mix_of_stale_and_closed() {
        let (_dir, mana_dir) = setup();

        // An open unit — untouched
        let open_unit = Unit::new("1", "Open");
        write_unit(&mana_dir, &open_unit);

        // A closed unit — archived
        let mut closed_unit = Unit::new("2", "Closed");
        closed_unit.status = Status::Closed;
        closed_unit.closed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &closed_unit);

        // A stale in-progress unit — released
        let mut stale_unit = Unit::new("3", "Stale WIP");
        stale_unit.status = Status::InProgress;
        stale_unit.claimed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &stale_unit);

        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();

        // Open unit untouched
        let b1 = Unit::from_file(find_unit_file(&mana_dir, "1").unwrap()).unwrap();
        assert_eq!(b1.status, Status::Open);

        // Closed unit archived
        assert!(find_unit_file(&mana_dir, "2").is_err());
        assert!(crate::discovery::find_archived_unit(&mana_dir, "2").is_ok());

        // Stale in-progress unit released
        let b3 = Unit::from_file(find_unit_file(&mana_dir, "3").unwrap()).unwrap();
        assert_eq!(b3.status, Status::Open);
        assert!(b3.claimed_at.is_none());
        assert!(b3.claimed_by.is_none());
    }

    #[test]
    fn tidy_releases_in_progress_with_claimed_by() {
        let (_dir, mana_dir) = setup();

        // Unit was claimed by an agent that no longer exists
        let mut unit = Unit::new("1", "Agent crashed");
        unit.status = Status::InProgress;
        unit.claimed_by = Some("agent-42".to_string());
        unit.claimed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &unit);

        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();

        let updated = Unit::from_file(find_unit_file(&mana_dir, "1").unwrap()).unwrap();
        assert_eq!(updated.status, Status::Open);
        assert!(updated.claimed_by.is_none());
        assert!(updated.claimed_at.is_none());
    }

    // ── Empty project ──────────────────────────────────────────────

    #[test]
    fn tidy_empty_project() {
        let (_dir, mana_dir) = setup();
        // Should succeed with nothing to do
        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();
    }

    // ── Index is rebuilt ───────────────────────────────────────────

    #[test]
    fn tidy_rebuilds_index() {
        let (_dir, mana_dir) = setup();

        let open_unit = Unit::new("1", "Open");
        write_unit(&mana_dir, &open_unit);

        let mut closed_unit = Unit::new("2", "Closed");
        closed_unit.status = Status::Closed;
        closed_unit.closed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &closed_unit);

        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();

        // Index should only contain the open unit (closed was archived)
        let index = Index::load(&mana_dir).unwrap();
        assert_eq!(index.units.len(), 1);
        assert_eq!(index.units[0].id, "1");
    }

    #[test]
    fn tidy_updates_archive_yaml() {
        let (_dir, mana_dir) = setup();

        // Create two closed units
        let mut unit1 = Unit::new("1", "Done first");
        unit1.status = Status::Closed;
        unit1.closed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &unit1);

        let mut unit2 = Unit::new("2", "Done second");
        unit2.status = Status::Closed;
        unit2.closed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &unit2);

        cmd_tidy_inner(&mana_dir, false, no_agents, &Output::new()).unwrap();

        // archive.yaml should exist and contain both archived units
        assert!(mana_dir.join("archive.yaml").exists());
        let archive = ArchiveIndex::load(&mana_dir).unwrap();
        assert_eq!(archive.units.len(), 2);
        let ids: Vec<&str> = archive.units.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"1"));
        assert!(ids.contains(&"2"));
    }

    #[test]
    fn tidy_dry_run_does_not_create_archive_yaml() {
        let (_dir, mana_dir) = setup();

        let mut unit = Unit::new("1", "Done task");
        unit.status = Status::Closed;
        unit.closed_at = Some(chrono::Utc::now());
        write_unit(&mana_dir, &unit);

        cmd_tidy_inner(&mana_dir, true, no_agents, &Output::new()).unwrap();

        // archive.yaml should NOT be created in dry-run mode
        assert!(!mana_dir.join("archive.yaml").exists());
    }
}
