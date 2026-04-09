use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Return the log directory path, creating it if needed.
pub fn log_dir() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("units")
        .join("logs");
    std::fs::create_dir_all(&dir).context("Failed to create units log directory")?;
    Ok(dir)
}

/// Find the most recent log file for a unit.
///
/// Log files follow the pattern `{unit_id}-{timestamp}.log` in the log directory.
/// The unit_id in filenames has dots replaced with underscores for filesystem safety.
pub fn find_latest_log(unit_id: &str) -> Result<Option<PathBuf>> {
    let dir = log_dir()?;
    let logs = find_all_logs_in(unit_id, &dir)?;
    Ok(logs.into_iter().last())
}

/// Find all log files for a unit, sorted oldest to newest.
pub fn find_all_logs(unit_id: &str) -> Result<Vec<PathBuf>> {
    let dir = log_dir()?;
    find_all_logs_in(unit_id, &dir)
}

/// Find all logs for a unit in a specific directory.
fn find_all_logs_in(unit_id: &str, dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    // Unit IDs may contain dots (e.g. "5.1"), which get encoded as underscores
    // in filenames. Match both the raw id and underscore-encoded form.
    let safe_id = unit_id.replace('.', "_");

    let mut logs: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // Match patterns: {unit_id}-*.log or {safe_id}-*.log
            (name.starts_with(&format!("{}-", unit_id))
                || name.starts_with(&format!("{}-", safe_id)))
                && name.ends_with(".log")
        })
        .collect();

    // Sort by filename (timestamp is embedded, so lexicographic = chronological)
    logs.sort();
    Ok(logs)
}

/// View agent output from log files.
///
/// Default: print the latest log file.
/// `--follow`: exec into `tail -f` for live following.
/// `--all`: print all logs with headers.
pub fn cmd_logs(mana_dir: &Path, id: &str, follow: bool, all: bool) -> Result<()> {
    let _ = mana_dir; // Used for validation context only

    if all {
        return show_all_logs(id);
    }

    // Also check agents.json for log_path hint
    let log_path = find_log_path(id)?;

    match log_path {
        Some(path) => {
            if follow {
                follow_log(&path)
            } else {
                print_log(&path)
            }
        }
        None => {
            anyhow::bail!(
                "No logs for unit {}. Has it been dispatched through the runtime path yet (legacy `mana run` or preferred `imp run <id>`)?",
                id
            );
        }
    }
}

/// Try to find a log path — first from agents.json, then from filesystem search.
fn find_log_path(unit_id: &str) -> Result<Option<PathBuf>> {
    // Check agents.json for a log_path hint
    if let Ok(agents) = super::agents::load_agents() {
        if let Some(entry) = agents.get(unit_id) {
            if let Some(ref log_path) = entry.log_path {
                let path = PathBuf::from(log_path);
                if path.exists() {
                    return Ok(Some(path));
                }
            }
        }
    }

    // Fall back to filesystem search
    find_latest_log(unit_id)
}

/// Print a log file to stdout.
fn print_log(path: &Path) -> Result<()> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    print!("{}", contents);
    Ok(())
}

/// Follow a log file with tail -f. Replaces the current process.
fn follow_log(path: &Path) -> Result<()> {
    let status = std::process::Command::new("tail")
        .args(["-f", &path.display().to_string()])
        .status()
        .context("Failed to exec tail -f")?;

    if !status.success() {
        anyhow::bail!("tail exited with code {}", status.code().unwrap_or(-1));
    }
    Ok(())
}

/// Show all log files for a unit with headers.
fn show_all_logs(unit_id: &str) -> Result<()> {
    let logs = find_all_logs(unit_id)?;

    if logs.is_empty() {
        anyhow::bail!(
            "No logs for unit {}. Has it been dispatched through the runtime path yet (legacy `mana run` or preferred `imp run <id>`)?",
            unit_id
        );
    }

    for (i, path) in logs.iter().enumerate() {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        if i > 0 {
            println!();
        }
        println!("═══ {} ═══", filename);
        println!();

        match std::fs::read_to_string(path) {
            Ok(contents) => print!("{}", contents),
            Err(e) => eprintln!("  (error reading {}: {})", path.display(), e),
        }
    }

    println!();
    println!("{} log file(s) for unit {}", logs.len(), unit_id);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_dir_creates_directory() {
        let dir = log_dir().unwrap();
        assert!(dir.exists());
    }

    #[test]
    fn find_latest_log_returns_none_for_unknown() {
        // For a unit ID that's very unlikely to have logs
        let result = find_latest_log("nonexistent_unit_99999").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn find_all_logs_in_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let logs = find_all_logs_in("5.1", dir.path()).unwrap();
        assert!(logs.is_empty());
    }

    #[test]
    fn find_all_logs_in_matches_unit_id() {
        let dir = tempfile::tempdir().unwrap();

        // Create some log files
        std::fs::write(dir.path().join("5_1-20260223-100000.log"), "log 1").unwrap();
        std::fs::write(dir.path().join("5_1-20260223-110000.log"), "log 2").unwrap();
        std::fs::write(dir.path().join("5_2-20260223-100000.log"), "other unit").unwrap();
        std::fs::write(dir.path().join("unrelated.txt"), "not a log").unwrap();

        let logs = find_all_logs_in("5.1", dir.path()).unwrap();
        assert_eq!(logs.len(), 2);

        // Should be sorted chronologically
        assert!(logs[0]
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("100000"));
        assert!(logs[1]
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("110000"));
    }

    #[test]
    fn find_all_logs_in_matches_raw_id() {
        let dir = tempfile::tempdir().unwrap();

        // Some systems might use the raw ID with dots
        std::fs::write(dir.path().join("5.1-20260223-100000.log"), "log 1").unwrap();

        let logs = find_all_logs_in("5.1", dir.path()).unwrap();
        assert_eq!(logs.len(), 1);
    }

    #[test]
    fn find_latest_log_returns_most_recent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("8-20260223-080000.log"), "early").unwrap();
        std::fs::write(dir.path().join("8-20260223-120000.log"), "later").unwrap();

        let logs = find_all_logs_in("8", dir.path()).unwrap();
        assert_eq!(logs.len(), 2);
        let latest = logs.last().unwrap();
        assert!(latest
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("120000"));
    }

    #[test]
    fn find_all_logs_nonexistent_dir() {
        let path = Path::new("/tmp/definitely_not_a_real_mana_dir_xyz");
        let logs = find_all_logs_in("1", path).unwrap();
        assert!(logs.is_empty());
    }
}
