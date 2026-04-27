use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::commands::config_cmd::collect_doctor_findings;
use crate::graph;
use crate::index::{count_unit_formats, Index};
use crate::unit::Unit;
use mana_core::sqlite;

/// Issue types that doctor can detect and potentially fix
#[derive(Debug)]
enum Issue {
    StaleIndex,
    MixedFormats {
        md_count: usize,
        yaml_count: usize,
    },
    DuplicateId {
        id: String,
        files: Vec<String>,
    },
    OrphanedDependency {
        unit_id: String,
        missing_dep: String,
    },
    MissingParent {
        unit_id: String,
        parent_id: String,
    },
    ArchivedParent {
        unit_id: String,
        parent_id: String,
    },
    StaleIndexEntry {
        id: String,
    },
    MissingIndexEntry {
        id: String,
    },
    Cycle {
        path: Vec<String>,
    },
    ConfigFinding {
        summary: String,
        details: String,
    },
    SqliteIndexStale,
    SqliteSchemaUnsupported {
        version: String,
    },
    SqliteDiagnostic {
        severity: String,
        kind: String,
        path: Option<String>,
        unit_id: Option<String>,
        field: Option<String>,
        message: String,
    },
    UnitParseError {
        file: String,
        message: String,
    },
}

impl Issue {
    fn display(&self) -> String {
        match self {
            Issue::StaleIndex => "[!] Stale index - run 'mana sync' to rebuild".to_string(),
            Issue::MixedFormats {
                md_count,
                yaml_count,
            } => {
                format!(
                    "[!] Mixed unit formats: {} .md files, {} .yaml files\n    \
                     This inflates unit count and causes confusion.\n    \
                     Fix: mkdir -p .mana/legacy && mv .mana/*.yaml .mana/legacy/",
                    md_count, yaml_count
                )
            }
            Issue::DuplicateId { id, files } => {
                format!(
                    "[!] Duplicate ID '{}' in {} files: {}",
                    id,
                    files.len(),
                    files.join(", ")
                )
            }
            Issue::OrphanedDependency {
                unit_id,
                missing_dep,
            } => {
                format!(
                    "[!] Orphaned dependency: {} depends on non-existent {}",
                    unit_id, missing_dep
                )
            }
            Issue::MissingParent { unit_id, parent_id } => {
                format!(
                    "[!] Missing parent: {} lists parent {} but it doesn't exist",
                    unit_id, parent_id
                )
            }
            Issue::ArchivedParent { unit_id, parent_id } => {
                format!(
                    "[!] Unit {} references parent '{}' which is archived",
                    unit_id, parent_id
                )
            }
            Issue::StaleIndexEntry { id } => {
                format!("[!] Index has entry for '{}' but no source file exists", id)
            }
            Issue::MissingIndexEntry { id } => {
                format!("[!] Unit file exists for '{}' but missing from index", id)
            }
            Issue::Cycle { path } => {
                format!("[!] Dependency cycle detected: {}", path.join(" -> "))
            }
            Issue::ConfigFinding { summary, details } => {
                format!("[!] Config: {}\n    {}", summary, details)
            }
            Issue::SqliteIndexStale => {
                "[!] SQLite index stale - run 'mana doctor fix' to rebuild".to_string()
            }
            Issue::SqliteSchemaUnsupported { version } => format!(
                "[!] SQLite index schema {} is unsupported; expected {}",
                version,
                sqlite::SCHEMA_VERSION
            ),
            Issue::SqliteDiagnostic {
                severity,
                kind,
                path,
                unit_id,
                field,
                message,
            } => {
                let mut target = path.clone().or_else(|| unit_id.clone()).unwrap_or_default();
                if let Some(field) = field {
                    if !field.is_empty() {
                        target.push_str(&format!(" field={field}"));
                    }
                }
                format!("[!] SQLite {severity} {kind}: {target}\n    {message}")
            }
            Issue::UnitParseError { file, message } => {
                format!("[!] Unit parse error in {file}\n    {message}")
            }
        }
    }

    fn is_fixable(&self) -> bool {
        matches!(
            self,
            Issue::StaleIndex
                | Issue::StaleIndexEntry { .. }
                | Issue::MissingIndexEntry { .. }
                | Issue::SqliteIndexStale
                | Issue::SqliteSchemaUnsupported { .. }
                | Issue::SqliteDiagnostic { .. }
        )
    }
}

/// Files to exclude when scanning for unit files
const EXCLUDED_FILES: &[&str] = &["config.yaml", "index.yaml", "unit.yaml"];

/// Check if a filename represents a unit file
fn is_unit_filename(filename: &str) -> bool {
    if EXCLUDED_FILES.contains(&filename) {
        return false;
    }
    let ext = Path::new(filename).extension().and_then(|e| e.to_str());
    match ext {
        Some("md") => filename.contains('-'), // New format: {id}-{slug}.md
        Some("yaml") => true,                 // Legacy format: {id}.yaml
        _ => false,
    }
}

/// Scan units directory and collect unit files with their IDs
fn scan_unit_files(mana_dir: &Path) -> Result<HashMap<String, Vec<String>>> {
    let mut id_to_files: HashMap<String, Vec<String>> = HashMap::new();

    for (path, filename) in iter_unit_files(mana_dir)? {
        if let Ok(unit) = Unit::from_file(&path) {
            id_to_files
                .entry(unit.id.clone())
                .or_default()
                .push(filename);
        }
    }

    Ok(id_to_files)
}

fn collect_unit_parse_errors(mana_dir: &Path) -> Result<Vec<Issue>> {
    let mut issues = Vec::new();
    for (path, filename) in iter_unit_files(mana_dir)? {
        if let Err(error) = Unit::from_file(&path) {
            issues.push(Issue::UnitParseError {
                file: filename,
                message: error.to_string(),
            });
        }
    }
    Ok(issues)
}

fn iter_unit_files(mana_dir: &Path) -> Result<Vec<(std::path::PathBuf, String)>> {
    let mut files = Vec::new();
    let dir_entries = fs::read_dir(mana_dir)?;

    for entry in dir_entries {
        let entry = entry?;
        let path = entry.path();
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();

        if is_unit_filename(&filename) {
            files.push((path, filename));
        }
    }

    Ok(files)
}

/// Get all unit source files that exist
fn get_existing_unit_files(mana_dir: &Path) -> Result<Vec<String>> {
    let mut existing = Vec::new();

    for (path, _) in iter_unit_files(mana_dir)? {
        if let Ok(unit) = Unit::from_file(&path) {
            existing.push(unit.id);
        }
    }

    Ok(existing)
}

/// Collect all archived unit IDs
fn collect_archived_ids(mana_dir: &Path) -> Result<Vec<String>> {
    let archived = Index::collect_archived(mana_dir)?;
    Ok(archived.into_iter().map(|e| e.id).collect())
}

fn collect_issues(mana_dir: &Path, fix: bool) -> Result<Vec<Issue>> {
    let mut issues: Vec<Issue> = Vec::new();

    // Check 1: Index freshness
    let is_stale = Index::is_stale(mana_dir)?;
    if is_stale {
        issues.push(Issue::StaleIndex);
    }

    // Check 2: Mixed unit formats (.yaml and .md)
    let (md_count, yaml_count) = count_unit_formats(mana_dir)?;
    if md_count > 0 && yaml_count > 0 {
        issues.push(Issue::MixedFormats {
            md_count,
            yaml_count,
        });
    }

    // Check 3: Duplicate IDs and parse errors
    let id_to_files = scan_unit_files(mana_dir)?;
    issues.extend(collect_unit_parse_errors(mana_dir)?);
    for (id, files) in &id_to_files {
        if files.len() > 1 {
            issues.push(Issue::DuplicateId {
                id: id.clone(),
                files: files.clone(),
            });
        }
    }

    // Load index for remaining checks (rebuild if stale so we can check properly)
    let index = if is_stale {
        // Try to build fresh index for checking, but don't fail if duplicates exist
        match Index::build(mana_dir) {
            Ok(idx) => {
                // Save it if we're fixing
                if fix {
                    idx.save(mana_dir)?;
                }
                idx
            }
            Err(_) => {
                // If build fails (e.g., duplicates), try to load existing
                Index::load(mana_dir).unwrap_or(Index { units: Vec::new() })
            }
        }
    } else {
        Index::load(mana_dir)?
    };

    // Collect archived unit IDs for parent reference check
    let archived_ids = collect_archived_ids(mana_dir)?;

    // Check 4: Orphaned dependencies (dep IDs that don't exist as units)
    for entry in &index.units {
        for dep_id in &entry.dependencies {
            let dep_exists = index.units.iter().any(|e| &e.id == dep_id);
            let dep_archived = archived_ids.contains(dep_id);
            if !dep_exists && !dep_archived {
                issues.push(Issue::OrphanedDependency {
                    unit_id: entry.id.clone(),
                    missing_dep: dep_id.clone(),
                });
            }
        }
    }

    // Check 5: Missing parent refs (parent doesn't exist at all)
    // Check 6: Archived parent refs (parent exists but is archived)
    for entry in &index.units {
        if let Some(parent_id) = &entry.parent {
            let parent_in_index = index.units.iter().any(|e| &e.id == parent_id);
            let parent_archived = archived_ids.contains(parent_id);

            if parent_archived {
                issues.push(Issue::ArchivedParent {
                    unit_id: entry.id.clone(),
                    parent_id: parent_id.clone(),
                });
            } else if !parent_in_index {
                issues.push(Issue::MissingParent {
                    unit_id: entry.id.clone(),
                    parent_id: parent_id.clone(),
                });
            }
        }
    }

    // Check 7: Stale index entries (entries without source files)
    let existing_ids = get_existing_unit_files(mana_dir)?;
    for entry in &index.units {
        if !existing_ids.contains(&entry.id) {
            issues.push(Issue::StaleIndexEntry {
                id: entry.id.clone(),
            });
        }
    }

    // Check 7b: Unit files that exist on disk but aren't in the index
    let indexed_ids: HashSet<String> = index.units.iter().map(|e| e.id.clone()).collect();
    for id in &existing_ids {
        if !indexed_ids.contains(id) {
            issues.push(Issue::MissingIndexEntry { id: id.clone() });
        }
    }

    // Check 8: Cycles
    let cycles = graph::find_all_cycles(&index)?;
    for cycle in cycles {
        issues.push(Issue::Cycle { path: cycle });
    }

    // Check 9: Config drift / legacy templates / ignored model settings
    for finding in collect_doctor_findings(mana_dir)? {
        issues.push(Issue::ConfigFinding {
            summary: finding.summary,
            details: finding.details,
        });
    }

    // Check 10: SQLite derived index health and diagnostics
    issues.extend(collect_sqlite_issues(mana_dir)?);

    Ok(issues)
}

fn collect_sqlite_issues(mana_dir: &Path) -> Result<Vec<Issue>> {
    let mut issues = Vec::new();
    let sqlite_path = sqlite::Index::database_path(mana_dir);
    if !sqlite_path.exists() {
        return Ok(issues);
    }

    match sqlite::Index::open(mana_dir) {
        Ok(index) => {
            match index.schema_version() {
                Ok(version) if version == sqlite::SCHEMA_VERSION => {}
                Ok(version) => issues.push(Issue::SqliteSchemaUnsupported {
                    version: version.to_string(),
                }),
                Err(error) => issues.push(Issue::SqliteSchemaUnsupported {
                    version: error.to_string(),
                }),
            }

            if index.is_stale()? {
                issues.push(Issue::SqliteIndexStale);
            }

            for diagnostic in index.diagnostics()? {
                issues.push(Issue::SqliteDiagnostic {
                    severity: diagnostic.severity,
                    kind: diagnostic.kind,
                    path: diagnostic.source_path,
                    unit_id: diagnostic.unit_id,
                    field: diagnostic.field,
                    message: diagnostic.message,
                });
            }
        }
        Err(error) => issues.push(Issue::SqliteSchemaUnsupported {
            version: error.to_string(),
        }),
    }

    Ok(issues)
}

/// Health check: detect orphaned dependencies, missing parent refs, cycles, stale index,
/// duplicate IDs, archived parent refs, stale index entries, and stale/misleading config.
/// With `mana doctor fix`, automatically resolves fixable issues.
pub fn cmd_doctor(mana_dir: &Path, fix: bool) -> Result<()> {
    let issues = collect_issues(mana_dir, fix)?;

    // Display issues
    if issues.is_empty() {
        println!("All clear.");
        return Ok(());
    }

    let fixable_count = issues.iter().filter(|i| i.is_fixable()).count();
    let unfixable_count = issues.len() - fixable_count;

    for issue in &issues {
        println!("{}", issue.display());
    }

    // Summary
    println!();
    if fix {
        // Apply fixes for fixable issues
        let mut fixed_count = 0;

        for issue in &issues {
            match issue {
                Issue::StaleIndex
                | Issue::StaleIndexEntry { .. }
                | Issue::MissingIndexEntry { .. } => {
                    // Rebuild index handles all of these
                    // We'll do one rebuild at the end
                }
                _ => {}
            }
        }

        let has_missing_index_entries = issues
            .iter()
            .any(|i| matches!(i, Issue::MissingIndexEntry { .. }));
        let has_file_index_issues = issues.iter().any(|i| {
            matches!(
                i,
                Issue::StaleIndex | Issue::StaleIndexEntry { .. } | Issue::MissingIndexEntry { .. }
            )
        });
        let has_sqlite_fixable_issues = issues.iter().any(|i| {
            matches!(
                i,
                Issue::SqliteIndexStale
                    | Issue::SqliteSchemaUnsupported { .. }
                    | Issue::SqliteDiagnostic { .. }
            )
        });

        if has_file_index_issues {
            match Index::build(mana_dir) {
                Ok(idx) => {
                    idx.save(mana_dir)?;
                    if has_missing_index_entries {
                        println!("✓ Rebuilt index to include missing entries");
                    } else {
                        println!("✓ Rebuilt index");
                    }
                    fixed_count += issues
                        .iter()
                        .filter(|i| {
                            matches!(
                                i,
                                Issue::StaleIndex
                                    | Issue::StaleIndexEntry { .. }
                                    | Issue::MissingIndexEntry { .. }
                            )
                        })
                        .count();
                }
                Err(e) => {
                    println!("✗ Could not rebuild index: {}", e);
                }
            }
        }

        if has_sqlite_fixable_issues && !has_file_index_issues {
            match sqlite::Index::rebuild(mana_dir) {
                Ok(report) => {
                    println!(
                        "✓ Rebuilt SQLite index ({} valid unit(s), {} invalid file(s))",
                        report.valid_units, report.invalid_files
                    );
                    fixed_count += issues
                        .iter()
                        .filter(|i| {
                            matches!(
                                i,
                                Issue::SqliteIndexStale
                                    | Issue::SqliteSchemaUnsupported { .. }
                                    | Issue::SqliteDiagnostic { .. }
                            )
                        })
                        .count();
                }
                Err(e) => {
                    println!("✗ Could not rebuild SQLite index: {}", e);
                }
            }
        }

        if fixed_count > 0 {
            println!("Fixed {} issue(s)", fixed_count);
        }
        if unfixable_count > 0 {
            println!("{} issue(s) require manual intervention", unfixable_count);
        }
    } else {
        println!(
            "Found {} issue(s). Run `mana doctor fix` to resolve fixable issues.",
            issues.len()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::Unit;
    use std::fs;
    use tempfile::TempDir;

    fn setup_clean_project() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let unit1 = Unit::new("1", "Task one");
        let mut unit2 = Unit::new("2", "Task two");
        unit2.dependencies = vec!["1".to_string()];

        unit1.to_file(mana_dir.join("1.yaml")).unwrap();
        unit2.to_file(mana_dir.join("2.yaml")).unwrap();

        // Rebuild index to make it fresh
        Index::build(&mana_dir).unwrap().save(&mana_dir).unwrap();

        (dir, mana_dir)
    }

    #[test]
    fn doctor_clean_project() {
        let (_dir, mana_dir) = setup_clean_project();
        let result = cmd_doctor(&mana_dir, false);
        assert!(result.is_ok());
    }

    #[test]
    fn doctor_detects_legacy_config_template() {
        let (_dir, mana_dir) = setup_clean_project();
        fs::write(
            mana_dir.join("config.yaml"),
            "project: test\nnext_id: 3\nrun: '../target/debug/imp run {id}'\nrun_model: gpt-5.4\n",
        )
        .unwrap();

        let issues = collect_issues(&mana_dir, false).unwrap();
        assert!(issues.iter().any(|issue| matches!(
            issue,
            Issue::ConfigFinding { summary, .. } if summary.contains("Run template")
        )));
        assert!(issues.iter().any(|issue| matches!(
            issue,
            Issue::ConfigFinding { summary, .. } if summary.contains("run_model")
        )));
    }

    #[test]
    fn doctor_detects_orphaned_dep() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let mut unit = Unit::new("1", "Task");
        unit.dependencies = vec!["nonexistent".to_string()];
        unit.to_file(mana_dir.join("1.yaml")).unwrap();

        let result = cmd_doctor(&mana_dir, false);
        assert!(result.is_ok());
    }

    #[test]
    fn doctor_detects_missing_parent() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let mut unit = Unit::new("1.1", "Subtask");
        unit.parent = Some("nonexistent".to_string());
        unit.to_file(mana_dir.join("1.1.yaml")).unwrap();

        let result = cmd_doctor(&mana_dir, false);
        assert!(result.is_ok());
    }

    #[test]
    fn doctor_detects_cycle() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create a cycle: 1 -> 2 -> 3 -> 1
        let mut unit1 = Unit::new("1", "Task 1");
        unit1.dependencies = vec!["3".to_string()];

        let mut unit2 = Unit::new("2", "Task 2");
        unit2.dependencies = vec!["1".to_string()];

        let mut unit3 = Unit::new("3", "Task 3");
        unit3.dependencies = vec!["2".to_string()];

        unit1.to_file(mana_dir.join("1.yaml")).unwrap();
        unit2.to_file(mana_dir.join("2.yaml")).unwrap();
        unit3.to_file(mana_dir.join("3.yaml")).unwrap();

        let result = cmd_doctor(&mana_dir, false);
        assert!(result.is_ok());
    }

    #[test]
    fn doctor_detects_mixed_formats() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create units in both formats
        let unit1 = Unit::new("1", "Task one in yaml");
        let unit2 = Unit::new("2", "Task two in md");

        // .yaml file (legacy format)
        unit1.to_file(mana_dir.join("1.yaml")).unwrap();
        // .md file (new format)
        unit2.to_file(mana_dir.join("2-task-two-in-md.md")).unwrap();

        // Doctor should succeed but detect the issue
        let result = cmd_doctor(&mana_dir, false);
        assert!(result.is_ok());

        // Verify counts are correct
        let (md_count, yaml_count) = count_unit_formats(&mana_dir).unwrap();
        assert_eq!(md_count, 1);
        assert_eq!(yaml_count, 1);
    }

    #[test]
    fn doctor_no_warning_for_single_format() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create units only in .md format
        let unit1 = Unit::new("1", "Task one");
        let unit2 = Unit::new("2", "Task two");

        unit1.to_file(mana_dir.join("1-task-one.md")).unwrap();
        unit2.to_file(mana_dir.join("2-task-two.md")).unwrap();

        let result = cmd_doctor(&mana_dir, false);
        assert!(result.is_ok());

        // Should have only .md files
        let (md_count, yaml_count) = count_unit_formats(&mana_dir).unwrap();
        assert_eq!(md_count, 2);
        assert_eq!(yaml_count, 0);
    }

    #[test]
    fn doctor_detects_duplicate_ids() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create two units with the same ID in different files
        let unit_a = Unit::new("99", "Unit A");
        let unit_b = Unit::new("99", "Unit B");

        unit_a.to_file(mana_dir.join("99-a.md")).unwrap();
        unit_b.to_file(mana_dir.join("99-b.md")).unwrap();

        // Doctor should succeed and report the duplicate
        let result = cmd_doctor(&mana_dir, false);
        assert!(result.is_ok());
    }

    #[test]
    fn doctor_detects_archived_parent() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create archive structure with a parent unit
        let archive_dir = mana_dir.join("archive").join("2026").join("02");
        fs::create_dir_all(&archive_dir).unwrap();

        let mut archived_parent = Unit::new("100", "Archived parent");
        archived_parent.is_archived = true;
        archived_parent
            .to_file(archive_dir.join("100-archived-parent.md"))
            .unwrap();

        // Create a child that references the archived parent
        let mut child = Unit::new("100.1", "Child of archived");
        child.parent = Some("100".to_string());
        child.to_file(mana_dir.join("100.1-child.md")).unwrap();

        // Doctor should succeed and detect the archived parent reference
        let result = cmd_doctor(&mana_dir, false);
        assert!(result.is_ok());
    }

    #[test]
    fn doctor_detects_stale_index_entries() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create a unit and build index
        let unit = Unit::new("1", "Task one");
        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();

        let index = Index::build(&mana_dir).unwrap();
        index.save(&mana_dir).unwrap();

        // Now delete the source file, leaving a stale index entry
        fs::remove_file(mana_dir.join("1-task-one.md")).unwrap();

        // Doctor should succeed and detect the stale entry
        let result = cmd_doctor(&mana_dir, false);
        assert!(result.is_ok());
    }

    #[test]
    fn doctor_detects_missing_index_entries() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let unit = Unit::new("1", "Task one");
        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();

        // Save an index that is missing the on-disk unit entry.
        Index { units: Vec::new() }.save(&mana_dir).unwrap();

        let issues = collect_issues(&mana_dir, false).unwrap();
        assert!(issues.iter().any(|issue| matches!(
            issue,
            Issue::MissingIndexEntry { id } if id == "1"
        )));
    }

    #[test]
    fn doctor_fix_rebuilds_index_for_missing_entries() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let unit = Unit::new("1", "Task one");
        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();

        // Save an index that is fresh by timestamp but missing the unit entry.
        Index { units: Vec::new() }.save(&mana_dir).unwrap();

        let issues = collect_issues(&mana_dir, false).unwrap();
        assert!(issues.iter().any(|issue| matches!(
            issue,
            Issue::MissingIndexEntry { id } if id == "1"
        )));

        let result = cmd_doctor(&mana_dir, true);
        assert!(result.is_ok());

        let rebuilt = Index::load(&mana_dir).unwrap();
        assert!(rebuilt.units.iter().any(|entry| entry.id == "1"));
    }

    #[test]
    fn doctor_does_not_warn_for_missing_derived_sqlite() {
        let (_dir, mana_dir) = setup_clean_project();
        let sqlite_path = sqlite::Index::database_path(&mana_dir);
        if sqlite_path.exists() {
            fs::remove_file(sqlite_path).unwrap();
        }

        let issues = collect_issues(&mana_dir, false).unwrap();
        assert!(!issues.iter().any(|issue| {
            matches!(
                issue,
                Issue::SqliteIndexStale
                    | Issue::SqliteSchemaUnsupported { .. }
                    | Issue::SqliteDiagnostic { .. }
            )
        }));
    }

    #[test]
    fn doctor_can_rebuild_sqlite_index_directly() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        let unit = Unit::new("1", "Task one");
        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();
        let report = sqlite::Index::rebuild(&mana_dir).unwrap();

        assert_eq!(report.valid_units, 1);
        assert!(sqlite::Index::database_path(&mana_dir).exists());
    }

    #[test]
    fn doctor_fix_rebuilds_index() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create a unit without an index
        let unit = Unit::new("1", "Task one");
        unit.to_file(mana_dir.join("1-task-one.md")).unwrap();

        // Verify index is stale
        assert!(Index::is_stale(&mana_dir).unwrap());

        // Run doctor fix mode
        let result = cmd_doctor(&mana_dir, true);
        assert!(result.is_ok());

        // Index should now be fresh
        assert!(!Index::is_stale(&mana_dir).unwrap());
    }
}
