//! Rebuildable SQLite index for canonical mana unit files.
//!
//! This module owns only derived state. Canonical mana data remains in the
//! human-editable Markdown/YAML unit files; the SQLite database can be deleted
//! and rebuilt from those files.

mod freshness;
mod query;
mod rebuild;
mod schema;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

pub use freshness::{
    source_file_metadata, Freshness, SourceFileKind, SourceFileMetadata, SourceFileStatus,
};
pub use rebuild::RebuildReport;

pub const SCHEMA_VERSION: i64 = 1;
const INDEX_FILENAME: &str = "index.sqlite";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticRow {
    pub severity: String,
    pub kind: String,
    pub source_path: Option<String>,
    pub unit_id: Option<String>,
    pub field: Option<String>,
    pub message: String,
}

pub struct Index {
    conn: Connection,
}

impl Index {
    pub fn open(mana_dir: &Path) -> Result<Self> {
        fs::create_dir_all(mana_dir)
            .with_context(|| format!("failed to create mana dir: {}", mana_dir.display()))?;
        let db_path = database_path(mana_dir);
        let conn = Connection::open(&db_path)
            .with_context(|| format!("failed to open SQLite index: {}", db_path.display()))?;
        let index = Self { conn };
        index.initialize(mana_dir)?;
        Ok(index)
    }

    pub fn rebuild(mana_dir: &Path) -> Result<RebuildReport> {
        let mut index = Self::open(mana_dir)?;
        index.rebuild_from_canonical_files(mana_dir)
    }

    pub fn database_path(mana_dir: &Path) -> PathBuf {
        database_path(mana_dir)
    }

    pub fn schema_version(&self) -> Result<i64> {
        let version = self
            .get_meta("schema_version")?
            .ok_or_else(|| anyhow!("SQLite index schema_version metadata is missing"))?;
        version
            .parse::<i64>()
            .context("SQLite index schema_version metadata is invalid")
    }

    pub fn is_stale(&self) -> Result<bool> {
        Ok(self.get_meta("stale")?.is_some_and(|value| value == "true"))
    }

    pub fn mark_stale(&self, reason: &str) -> Result<()> {
        self.set_meta("stale", "true")?;
        self.set_meta("stale_reason", reason)
    }

    pub fn mark_fresh(&self) -> Result<()> {
        self.set_meta("stale", "false")?;
        self.set_meta("stale_reason", "")
    }

    pub fn record_source_file(&self, metadata: &SourceFileMetadata) -> Result<()> {
        freshness::record_source_file(&self.conn, metadata)
    }

    pub fn source_freshness(
        &self,
        path: &str,
        hash: Option<&str>,
        mtime: Option<i64>,
        size: Option<i64>,
    ) -> Result<Freshness> {
        freshness::source_freshness(&self.conn, path, hash, mtime, size)
    }

    pub fn source_status(&self, path: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT status FROM source_files WHERE path = ?1",
                [path],
                |row| row.get(0),
            )
            .optional()
            .with_context(|| format!("failed to read source status: {path}"))
    }

    pub fn unit_exists(&self, id: &str) -> Result<bool> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM units WHERE id = ?1", [id], |row| {
                    row.get(0)
                })?;
        Ok(count > 0)
    }

    pub fn diagnostic_count(&self) -> Result<usize> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM index_diagnostics", [], |row| {
                    row.get(0)
                })?;
        usize::try_from(count).context("diagnostic count overflow")
    }

    pub fn diagnostics(&self) -> Result<Vec<DiagnosticRow>> {
        let mut statement = self.conn.prepare(
            "SELECT severity, kind, source_path, unit_id, field, message FROM index_diagnostics ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(DiagnosticRow {
                severity: row.get(0)?,
                kind: row.get(1)?,
                source_path: row.get(2)?,
                unit_id: row.get(3)?,
                field: row.get(4)?,
                message: row.get(5)?,
            })
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to collect SQLite diagnostics")
    }

    fn initialize(&self, mana_dir: &Path) -> Result<()> {
        self.conn.execute_batch(schema::SCHEMA_SQL)?;
        self.set_meta("schema_version", &SCHEMA_VERSION.to_string())?;
        self.set_meta("mana_root", &mana_dir.display().to_string())?;
        self.set_meta("stale", "false")?;
        self.set_meta("stale_reason", "")?;
        Ok(())
    }

    fn get_meta(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM index_meta WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .with_context(|| format!("failed to read SQLite index metadata: {key}"))
    }

    fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO index_meta (key, value) VALUES (?1, ?2)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value
            "#,
            params![key, value],
        )?;
        Ok(())
    }

    pub(crate) fn connection(&self) -> &Connection {
        &self.conn
    }

    pub(crate) fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

pub(crate) fn timestamp_now() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn database_path(mana_dir: &Path) -> PathBuf {
    mana_dir.join(INDEX_FILENAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::{Status, Unit};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn initializes_schema_and_metadata() {
        let dir = tempdir().unwrap();
        let index = Index::open(dir.path()).unwrap();

        assert_eq!(index.schema_version().unwrap(), SCHEMA_VERSION);
        assert!(!index.is_stale().unwrap());
        assert!(Index::database_path(dir.path()).exists());
    }

    #[test]
    fn marks_index_stale_and_fresh() {
        let dir = tempdir().unwrap();
        let index = Index::open(dir.path()).unwrap();

        index.mark_stale("test failure").unwrap();
        assert!(index.is_stale().unwrap());

        index.mark_fresh().unwrap();
        assert!(!index.is_stale().unwrap());
    }

    #[test]
    fn records_source_metadata_and_detects_freshness() {
        let dir = tempdir().unwrap();
        let unit_path = dir.path().join("1-test.md");
        fs::write(&unit_path, "---\nid: '1'\ntitle: Test\n---\n").unwrap();

        let index = Index::open(dir.path()).unwrap();
        let metadata = source_file_metadata(&unit_path, Some("1".to_string())).unwrap();
        index.record_source_file(&metadata).unwrap();

        assert_eq!(
            index
                .source_freshness(
                    &metadata.path,
                    metadata.hash.as_deref(),
                    metadata.mtime,
                    metadata.size,
                )
                .unwrap(),
            Freshness::Fresh
        );
        assert_eq!(
            index
                .source_freshness(
                    &metadata.path,
                    Some("different"),
                    metadata.mtime,
                    metadata.size
                )
                .unwrap(),
            Freshness::Stale
        );
        assert_eq!(
            index
                .source_freshness("missing.md", None, None, None)
                .unwrap(),
            Freshness::Missing
        );
    }
    #[test]
    fn rebuilds_valid_units_and_child_tables() {
        let dir = tempdir().unwrap();
        let unit_path = dir.path().join("1-test.md");
        fs::write(
            &unit_path,
            r#"---
id: 1
title: Test unit
status: open
priority: 2
kind: task
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-01T00:00:00Z"
labels: [alpha, beta]
paths: [src/lib.rs]
dependencies: [0]
produces: [artifact-a]
requires: [artifact-b]
decisions:
  - Choose SQLite as derived index
---
Human-readable body.
"#,
        )
        .unwrap();

        let mut index = Index::open(dir.path()).unwrap();
        let report = index.rebuild_from_canonical_files(dir.path()).unwrap();

        assert_eq!(report.valid_units, 1);
        assert_eq!(report.invalid_files, 0);
        assert!(index.unit_exists("1").unwrap());
        assert_eq!(index.diagnostic_count().unwrap(), 0);
        assert_eq!(
            index
                .source_status(unit_path.display().to_string().as_str())
                .unwrap(),
            Some("valid".to_string())
        );
    }

    #[test]
    fn rebuild_records_invalid_yaml_without_valid_unit_row() {
        let dir = tempdir().unwrap();
        let unit_path = dir.path().join("1-bad.md");
        fs::write(
            &unit_path,
            "---\nid: 1\ntitle: [unterminated\n---\nBroken body.\n",
        )
        .unwrap();

        let mut index = Index::open(dir.path()).unwrap();
        let report = index.rebuild_from_canonical_files(dir.path()).unwrap();

        assert_eq!(report.valid_units, 0);
        assert_eq!(report.invalid_files, 1);
        assert!(!index.unit_exists("1").unwrap());
        assert_eq!(index.diagnostic_count().unwrap(), 1);
        assert_eq!(
            index
                .source_status(unit_path.display().to_string().as_str())
                .unwrap(),
            Some("invalid_parse".to_string())
        );
    }

    #[test]
    fn rebuild_removes_stale_rows_after_source_becomes_invalid() {
        let dir = tempdir().unwrap();
        let unit_path = dir.path().join("1-test.md");
        let mut unit = Unit::new("1".to_string(), "Test".to_string());
        unit.status = Status::Open;
        unit.to_file(&unit_path).unwrap();

        let mut index = Index::open(dir.path()).unwrap();
        let first_report = index.rebuild_from_canonical_files(dir.path()).unwrap();
        assert_eq!(first_report.valid_units, 1);
        assert!(index.unit_exists("1").unwrap());

        fs::write(
            &unit_path,
            "---\nid: 1\ntitle: Test\ncreated_at: \"not-a-date\"\nupdated_at: \"2026-01-01T00:00:00Z\"\n---\n",
        )
        .unwrap();
        let second_report = index.rebuild_from_canonical_files(dir.path()).unwrap();

        assert_eq!(second_report.valid_units, 0);
        assert_eq!(second_report.invalid_files, 1);
        assert!(!index.unit_exists("1").unwrap());
        assert_eq!(index.diagnostic_count().unwrap(), 1);
    }
}
