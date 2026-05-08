use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::handle::normalize_handle;
use crate::index::Index;
use crate::unit::Unit;

/// A unit resolved by ID or by project-scoped human handle.
#[derive(Debug)]
pub struct ResolvedUnit {
    pub unit: Unit,
    pub path: PathBuf,
}

/// Resolve an active unit reference as an ID first, then as a unique handle.
///
/// Handles are intentionally project-scoped aliases. Ambiguous handles return a
/// clear error listing matching IDs so callers can ask for a more precise ref.
pub fn resolve_unit(mana_dir: &Path, reference: &str) -> Result<ResolvedUnit> {
    let path = match crate::discovery::find_unit_file(mana_dir, reference) {
        Ok(path) => path,
        Err(id_error) => match resolve_unit_path_by_handle(mana_dir, reference) {
            Ok(path) => path,
            Err(handle_error) => {
                let handle_message = handle_error.to_string();
                if handle_message.contains("ambiguous") {
                    return Err(handle_error);
                }
                return Err(handle_error).with_context(|| {
                    format!("Unit not found by ID or handle: {reference} ({id_error})")
                });
            }
        },
    };

    let unit = Unit::from_file(&path)
        .with_context(|| format!("Failed to load unit: {}", path.display()))?;
    Ok(ResolvedUnit { unit, path })
}

/// Resolve an active unit path by unique handle.
pub fn resolve_unit_path_by_handle(mana_dir: &Path, handle: &str) -> Result<PathBuf> {
    let query = normalize_handle(handle);
    if query.is_empty() {
        return Err(anyhow!("Handle cannot be empty"));
    }

    let index = Index::load_or_rebuild(mana_dir)?;
    let matches: Vec<_> = index
        .units
        .iter()
        .filter(|entry| {
            let normalized = entry.handle.as_deref().map(normalize_handle);
            normalized.as_deref() == Some(query.as_str())
        })
        .collect();

    match matches.as_slice() {
        [] => Err(anyhow!("No unit with handle '{handle}'")),
        [entry] => crate::discovery::find_unit_file(mana_dir, &entry.id),
        many => {
            let choices = many
                .iter()
                .map(|entry| format!("  {} — {}", entry.id, entry.title))
                .collect::<Vec<_>>()
                .join("\n");
            Err(anyhow!(
                "Handle '{handle}' is ambiguous; use a unit ID instead:\n{choices}"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ops::create::{create, CreateParams};
    use crate::unit::Unit;
    use std::fs;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        Config::default().save(&mana_dir).unwrap();
        (dir, mana_dir)
    }

    #[test]
    fn resolve_unit_finds_unique_handle() {
        let (_dir, mana_dir) = setup();
        create(
            &mana_dir,
            CreateParams {
                title: "Implement SQLite-derived index for mana agent context assembly".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let resolved = resolve_unit(&mana_dir, "sqlite derived index").unwrap();
        assert_eq!(resolved.unit.id, "1");
        assert_eq!(
            resolved.unit.handle.as_deref(),
            Some("sqlite derived index")
        );
    }

    #[test]
    fn resolve_unit_reports_ambiguous_handle() {
        let (_dir, mana_dir) = setup();
        let mut first = Unit::new("1", "First title");
        first.handle = Some("shared handle".to_string());
        first.to_file(mana_dir.join("1-first-title.md")).unwrap();
        let mut second = Unit::new("2", "Second title");
        second.handle = Some("shared handle".to_string());
        second.to_file(mana_dir.join("2-second-title.md")).unwrap();
        Index::build(&mana_dir).unwrap().save(&mana_dir).unwrap();

        let error = resolve_unit(&mana_dir, "shared handle")
            .unwrap_err()
            .to_string();
        assert!(error.contains("ambiguous"));
        assert!(error.contains("1 — First title"));
        assert!(error.contains("2 — Second title"));
    }
}
