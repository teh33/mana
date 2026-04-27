use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mana_core::discovery::{find_archived_unit, find_unit_file};
use mana_core::index::{Index, IndexEntry};
use mana_core::unit::Unit;
use mana_core::util::validate_unit_id;

#[derive(Debug, Clone)]
pub struct SearchMatch {
    pub mana_dir: PathBuf,
    pub entry: IndexEntry,
    pub archived: bool,
}

/// Search for an exact unit ID across mana projects under the user's home directory.
///
/// This is intentionally independent of the current project so `mana search <id>`
/// can locate units even when run outside the project that owns them.
pub fn cmd_search(id: &str, current_dir: &Path, json: bool) -> Result<()> {
    validate_unit_id(id)?;
    let _ = current_dir;

    let matches = search_unit_id(id)?;
    if matches.is_empty() {
        anyhow::bail!("Unit {id} not found in mana system");
    }

    if json {
        let json_matches: Vec<_> = matches
            .iter()
            .map(|m| {
                serde_json::json!({
                    "id": m.entry.id,
                    "title": m.entry.title,
                    "status": m.entry.status,
                    "priority": m.entry.priority,
                    "parent": m.entry.parent,
                    "archived": m.archived,
                    "mana_dir": m.mana_dir,
                    "project_dir": m.mana_dir.parent().unwrap_or(&m.mana_dir),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json_matches)?);
        return Ok(());
    }

    for m in matches {
        let archived = if m.archived { " archived" } else { "" };
        println!(
            "{} [{}{}] P{} {}",
            m.entry.id, m.entry.status, archived, m.entry.priority, m.entry.title
        );
        println!("  mana: {}", m.mana_dir.display());
        if let Some(project_dir) = m.mana_dir.parent() {
            println!("  project: {}", project_dir.display());
        }
    }

    Ok(())
}

fn search_unit_id(id: &str) -> Result<Vec<SearchMatch>> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("Cannot determine home directory for system-wide mana search")?;

    search_unit_id_under(id, &home)
}

fn search_unit_id_under(id: &str, root: &Path) -> Result<Vec<SearchMatch>> {
    let mut seen = HashSet::new();
    let mut mana_dirs = Vec::new();
    collect_mana_dirs(root, &mut seen, &mut mana_dirs, 5)?;

    let mut matches = Vec::new();
    for mana_dir in mana_dirs {
        if let Some(m) = search_mana_dir(id, &mana_dir)? {
            matches.push(m);
        }
    }

    Ok(matches)
}

fn collect_mana_dirs(
    dir: &Path,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<PathBuf>,
    remaining_depth: usize,
) -> Result<()> {
    if should_skip_dir(dir) {
        return Ok(());
    }

    let mana_dir = dir.join(".mana");
    if mana_dir.is_dir() {
        let canonical = mana_dir.canonicalize().unwrap_or(mana_dir);
        if seen.insert(canonical.clone()) {
            out.push(canonical);
        }
    }

    if remaining_depth == 0 {
        return Ok(());
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("Failed to read {}", dir.display())),
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_mana_dirs(&path, seen, out, remaining_depth - 1)?;
        }
    }

    Ok(())
}

fn should_skip_dir(dir: &Path) -> bool {
    matches!(
        dir.file_name().and_then(|n| n.to_str()),
        Some(
            ".cache"
                | ".direnv"
                | ".git"
                | ".npm"
                | ".rustup"
                | ".vscode"
                | "Library"
                | "Applications"
                | "Desktop"
                | "Documents"
                | "Downloads"
                | "Movies"
                | "Music"
                | "Pictures"
                | "target"
                | "node_modules"
        )
    )
}

fn search_mana_dir(id: &str, mana_dir: &Path) -> Result<Option<SearchMatch>> {
    if let Ok(index) = Index::load_or_rebuild(mana_dir) {
        if let Some(entry) = index.units.into_iter().find(|entry| entry.id == id) {
            return Ok(Some(SearchMatch {
                mana_dir: mana_dir.to_path_buf(),
                entry,
                archived: false,
            }));
        }
    }

    if let Ok(path) = find_unit_file(mana_dir, id) {
        let unit = Unit::from_file(path)?;
        return Ok(Some(SearchMatch {
            mana_dir: mana_dir.to_path_buf(),
            entry: IndexEntry::from(&unit),
            archived: false,
        }));
    }

    if let Ok(path) = find_archived_unit(mana_dir, id) {
        let unit = Unit::from_file(path)?;
        return Ok(Some(SearchMatch {
            mana_dir: mana_dir.to_path_buf(),
            entry: IndexEntry::from(&unit),
            archived: true,
        }));
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use mana_core::unit::Unit;
    use tempfile::TempDir;

    use super::search_unit_id_under;

    #[test]
    fn search_unit_id_finds_nested_project_unit_from_root() {
        let dir = TempDir::new().unwrap();
        let root_mana = dir.path().join(".mana");
        let child = dir.path().join("child");
        let child_mana = child.join(".mana");
        fs::create_dir(&root_mana).unwrap();
        fs::create_dir_all(&child_mana).unwrap();

        let unit = Unit::new("42.1", "Nested task");
        unit.to_file(child_mana.join("42.1-nested-task.md"))
            .unwrap();

        let matches = search_unit_id_under("42.1", dir.path()).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].entry.title, "Nested task");
        assert_eq!(matches[0].mana_dir, child_mana.canonicalize().unwrap());
    }
}
