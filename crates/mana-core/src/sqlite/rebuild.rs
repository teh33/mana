use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Transaction};

use crate::unit::{Unit, UnitType};

use super::freshness::{
    invalid_source_file_metadata, source_file_metadata_with_kind, SourceFileKind,
    SourceFileMetadata, SourceFileStatus,
};
use super::Index;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildReport {
    pub valid_units: usize,
    pub invalid_files: usize,
}

impl Index {
    pub fn rebuild_from_canonical_files(&mut self, mana_dir: &Path) -> Result<RebuildReport> {
        let files = discover_unit_files(mana_dir)?;
        let tx = self.connection_mut().transaction()?;

        clear_indexed_rows(&tx)?;

        let mut report = RebuildReport {
            valid_units: 0,
            invalid_files: 0,
        };

        for source in files {
            match Unit::from_file(&source.path) {
                Ok(mut unit) => {
                    unit.is_archived = source.is_archived;
                    let metadata = source_file_metadata_with_kind(
                        &source.path,
                        Some(unit.id.clone()),
                        if source.is_archived {
                            SourceFileKind::Archive
                        } else {
                            SourceFileKind::Unit
                        },
                        SourceFileStatus::Valid,
                    )?;
                    record_source_file_tx(&tx, &metadata)?;
                    insert_unit_tx(&tx, &unit, &metadata)?;
                    report.valid_units += 1;
                }
                Err(error) => {
                    let metadata = invalid_source_file_metadata(
                        &source.path,
                        if source.is_archived {
                            SourceFileKind::Archive
                        } else {
                            SourceFileKind::Unit
                        },
                        SourceFileStatus::InvalidParse,
                        error.to_string(),
                    )?;
                    record_source_file_tx(&tx, &metadata)?;
                    insert_diagnostic_tx(
                        &tx,
                        "error",
                        "parse",
                        Some(&metadata.path),
                        None,
                        Some("frontmatter"),
                        metadata
                            .error_message
                            .as_deref()
                            .unwrap_or("failed to parse unit file"),
                    )?;
                    report.invalid_files += 1;
                }
            }
        }

        set_meta_tx(&tx, "last_full_rebuild_at", &super::timestamp_now())?;
        set_meta_tx(&tx, "stale", "false")?;
        set_meta_tx(&tx, "stale_reason", "")?;
        tx.commit()?;

        Ok(report)
    }
}

#[derive(Debug, Clone)]
struct UnitSourceFile {
    path: PathBuf,
    is_archived: bool,
}

fn discover_unit_files(mana_dir: &Path) -> Result<Vec<UnitSourceFile>> {
    let mut files = Vec::new();
    collect_unit_files_in_dir(mana_dir, false, &mut files)?;

    let archive_dir = mana_dir.join("archive");
    if archive_dir.is_dir() {
        collect_unit_files_recursive(&archive_dir, true, &mut files)?;
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

fn collect_unit_files_in_dir(
    dir: &Path,
    is_archived: bool,
    files: &mut Vec<UnitSourceFile>,
) -> Result<()> {
    for entry in fs::read_dir(dir)
        .with_context(|| format!("failed to read mana directory: {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && is_unit_file(&path) {
            files.push(UnitSourceFile { path, is_archived });
        } else if path.is_dir()
            && path.file_name().and_then(|name| name.to_str()) != Some("archive")
        {
            collect_unit_files_recursive(&path, is_archived, files)?;
        }
    }
    Ok(())
}

fn collect_unit_files_recursive(
    dir: &Path,
    is_archived: bool,
    files: &mut Vec<UnitSourceFile>,
) -> Result<()> {
    for entry in fs::read_dir(dir)
        .with_context(|| format!("failed to read archive directory: {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_unit_files_recursive(&path, is_archived, files)?;
        } else if path.is_file() && is_unit_file(&path) {
            files.push(UnitSourceFile { path, is_archived });
        }
    }
    Ok(())
}

fn is_unit_file(path: &Path) -> bool {
    let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if matches!(
        filename,
        "config.yaml" | "index.yaml" | "unit.yaml" | "archive.yaml"
    ) {
        return false;
    }
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("md") => filename.contains('-'),
        Some("yaml") => true,
        _ => false,
    }
}

fn clear_indexed_rows(tx: &Transaction<'_>) -> Result<()> {
    for table in [
        "index_diagnostics",
        "context_edges",
        "facts",
        "unit_history",
        "unit_attempts",
        "unit_decisions",
        "unit_artifacts",
        "unit_dependencies",
        "unit_paths",
        "unit_labels",
        "units",
        "source_files",
    ] {
        tx.execute(&format!("DELETE FROM {table}"), [])?;
    }
    Ok(())
}

fn record_source_file_tx(tx: &Transaction<'_>, metadata: &SourceFileMetadata) -> Result<()> {
    tx.execute(
        r#"
        INSERT INTO source_files (
            path, unit_id, kind, hash, mtime, size, indexed_at, status,
            error_kind, error_message, error_field
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        ON CONFLICT(path) DO UPDATE SET
            unit_id = excluded.unit_id,
            kind = excluded.kind,
            hash = excluded.hash,
            mtime = excluded.mtime,
            size = excluded.size,
            indexed_at = excluded.indexed_at,
            status = excluded.status,
            error_kind = excluded.error_kind,
            error_message = excluded.error_message,
            error_field = excluded.error_field
        "#,
        params![
            metadata.path,
            metadata.unit_id,
            metadata.kind.as_str(),
            metadata.hash,
            metadata.mtime,
            metadata.size,
            super::timestamp_now(),
            metadata.status.as_str(),
            metadata.error_kind,
            metadata.error_message,
            metadata.error_field,
        ],
    )?;
    Ok(())
}

fn insert_unit_tx(tx: &Transaction<'_>, unit: &Unit, metadata: &SourceFileMetadata) -> Result<()> {
    let source_hash = metadata.hash.clone().unwrap_or_default();
    let indexed_at = super::timestamp_now();
    tx.execute(
        r#"
        INSERT INTO units (
            id, title, slug, status, priority, kind, unit_type, feature,
            created_at, updated_at, closed_at, close_reason, description, acceptance,
            notes, design, parent, assignee, claimed_by, claimed_at, is_archived,
            verify, verify_fast, fail_first, checkpoint, verify_hash, attempts,
            max_attempts, max_loops, verify_timeout, last_verified, stale_after,
            created_by, model, autonomy_disposition, outputs_json, on_fail_json,
            on_close_json, source_path, source_hash, indexed_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
            ?9, ?10, ?11, ?12, ?13, ?14,
            ?15, ?16, ?17, ?18, ?19, ?20, ?21,
            ?22, ?23, ?24, ?25, ?26, ?27,
            ?28, ?29, ?30, ?31, ?32,
            ?33, ?34, ?35, ?36, ?37,
            ?38, ?39, ?40, ?41
        )
        "#,
        params![
            unit.id,
            unit.title,
            unit.slug,
            unit.status.to_string(),
            i64::from(unit.priority),
            unit_type_as_str(unit.kind),
            unit.unit_type,
            unit.feature,
            unit.created_at.to_rfc3339(),
            unit.updated_at.to_rfc3339(),
            unit.closed_at.map(|value| value.to_rfc3339()),
            unit.close_reason,
            unit.description,
            unit.acceptance,
            unit.notes,
            unit.design,
            unit.parent,
            unit.assignee,
            unit.claimed_by,
            unit.claimed_at.map(|value| value.to_rfc3339()),
            unit.is_archived,
            unit.verify,
            unit.verify_fast,
            unit.fail_first,
            unit.checkpoint,
            unit.verify_hash,
            i64::from(unit.attempts),
            i64::from(unit.max_attempts),
            unit.max_loops.map(i64::from),
            unit.verify_timeout
                .and_then(|value| i64::try_from(value).ok()),
            unit.last_verified.map(|value| value.to_rfc3339()),
            unit.stale_after.map(|value| value.to_rfc3339()),
            unit.created_by,
            unit.model,
            json_string(&unit.autonomy_disposition)?,
            json_string(&unit.outputs)?,
            json_string(&unit.on_fail)?,
            json_string(&unit.on_close)?,
            metadata.path,
            source_hash,
            indexed_at,
        ],
    )?;

    insert_strings_tx(tx, "unit_labels", "label", &unit.id, &unit.labels)?;
    insert_strings_tx(tx, "unit_paths", "path", &unit.id, &unit.paths)?;
    insert_strings_tx(
        tx,
        "unit_dependencies",
        "dep_id",
        &unit.id,
        &unit.dependencies,
    )?;
    insert_artifacts_tx(tx, &unit.id, "produces", &unit.produces)?;
    insert_artifacts_tx(tx, &unit.id, "requires", &unit.requires)?;
    insert_decisions_tx(tx, &unit.id, &unit.decisions)?;
    insert_attempts_tx(tx, &unit.id, &unit.attempt_log)?;
    insert_history_tx(tx, &unit.id, &unit.history)?;

    if matches!(unit.kind, UnitType::Fact) || unit.unit_type == "fact" {
        tx.execute(
            "INSERT INTO facts (unit_id, last_verified, stale_after, score_hint) VALUES (?1, ?2, ?3, NULL)",
            params![
                unit.id,
                unit.last_verified.map(|value| value.to_rfc3339()),
                unit.stale_after.map(|value| value.to_rfc3339()),
            ],
        )?;
    }

    Ok(())
}

fn insert_strings_tx(
    tx: &Transaction<'_>,
    table: &str,
    column: &str,
    unit_id: &str,
    values: &[String],
) -> Result<()> {
    let sql = format!("INSERT INTO {table} (unit_id, {column}, position) VALUES (?1, ?2, ?3)");
    for (position, value) in values.iter().enumerate() {
        tx.execute(&sql, params![unit_id, value, position as i64])?;
    }
    Ok(())
}

fn insert_artifacts_tx(
    tx: &Transaction<'_>,
    unit_id: &str,
    direction: &str,
    values: &[String],
) -> Result<()> {
    for (position, value) in values.iter().enumerate() {
        tx.execute(
            "INSERT INTO unit_artifacts (unit_id, direction, artifact, position) VALUES (?1, ?2, ?3, ?4)",
            params![unit_id, direction, value, position as i64],
        )?;
    }
    Ok(())
}

fn insert_decisions_tx(tx: &Transaction<'_>, unit_id: &str, decisions: &[String]) -> Result<()> {
    for (index, decision) in decisions.iter().enumerate() {
        tx.execute(
            "INSERT INTO unit_decisions (unit_id, decision_index, text, resolved) VALUES (?1, ?2, ?3, 0)",
            params![unit_id, index as i64, decision],
        )?;
    }
    Ok(())
}

fn insert_attempts_tx(
    tx: &Transaction<'_>,
    unit_id: &str,
    attempts: &[crate::unit::types::AttemptRecord],
) -> Result<()> {
    for (index, attempt) in attempts.iter().enumerate() {
        tx.execute(
            "INSERT INTO unit_attempts (unit_id, attempt_index, num, outcome, notes, raw_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                unit_id,
                index as i64,
                i64::from(attempt.num),
                json_string(&attempt.outcome)?,
                attempt.notes,
                json_string(attempt)?,
            ],
        )?;
    }
    Ok(())
}

fn insert_history_tx(
    tx: &Transaction<'_>,
    unit_id: &str,
    history: &[crate::unit::types::RunRecord],
) -> Result<()> {
    for (index, record) in history.iter().enumerate() {
        tx.execute(
            "INSERT INTO unit_history (unit_id, history_index, started_at, finished_at, status, exit_code, raw_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                unit_id,
                index as i64,
                record.started_at.to_rfc3339(),
                record.finished_at.map(|value| value.to_rfc3339()),
                json_string(&record.result)?,
                record.exit_code,
                json_string(record)?,
            ],
        )?;
    }
    Ok(())
}

fn insert_diagnostic_tx(
    tx: &Transaction<'_>,
    severity: &str,
    kind: &str,
    source_path: Option<&str>,
    unit_id: Option<&str>,
    field: Option<&str>,
    message: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO index_diagnostics (severity, kind, source_path, unit_id, field, message, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![severity, kind, source_path, unit_id, field, message, super::timestamp_now()],
    )?;
    Ok(())
}

fn set_meta_tx(tx: &Transaction<'_>, key: &str, value: &str) -> Result<()> {
    tx.execute(
        r#"
        INSERT INTO index_meta (key, value) VALUES (?1, ?2)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        "#,
        params![key, value],
    )?;
    Ok(())
}

fn json_string<T: serde::Serialize>(value: &T) -> Result<Option<String>> {
    serde_json::to_string(value)
        .map(Some)
        .context("failed to serialize indexed JSON value")
}

fn unit_type_as_str(kind: UnitType) -> &'static str {
    match kind {
        UnitType::Epic => "epic",
        UnitType::Task => "task",
        UnitType::Fact => "fact",
    }
}
