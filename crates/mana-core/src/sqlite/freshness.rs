use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFileMetadata {
    pub path: String,
    pub unit_id: Option<String>,
    pub kind: SourceFileKind,
    pub hash: Option<String>,
    pub mtime: Option<i64>,
    pub size: Option<i64>,
    pub status: SourceFileStatus,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
    pub error_field: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFileKind {
    Unit,
    Archive,
    Config,
    Other,
}

impl SourceFileKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unit => "unit",
            Self::Archive => "archive",
            Self::Config => "config",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFileStatus {
    Valid,
    InvalidParse,
    InvalidSchema,
    Missing,
    Stale,
    Archived,
}

impl SourceFileStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::InvalidParse => "invalid_parse",
            Self::InvalidSchema => "invalid_schema",
            Self::Missing => "missing",
            Self::Stale => "stale",
            Self::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Stale,
    Missing,
}

pub fn record_source_file(conn: &Connection, metadata: &SourceFileMetadata) -> Result<()> {
    conn.execute(
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

pub fn source_freshness(
    conn: &Connection,
    path: &str,
    hash: Option<&str>,
    mtime: Option<i64>,
    size: Option<i64>,
) -> Result<Freshness> {
    let row = conn
        .query_row(
            "SELECT hash, mtime, size, status FROM source_files WHERE path = ?1",
            [path],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?;

    let Some((stored_hash, stored_mtime, stored_size, status)) = row else {
        return Ok(Freshness::Missing);
    };

    if status != SourceFileStatus::Valid.as_str() {
        return Ok(Freshness::Stale);
    }

    if stored_hash.as_deref() == hash && stored_mtime == mtime && stored_size == size {
        Ok(Freshness::Fresh)
    } else {
        Ok(Freshness::Stale)
    }
}

pub fn source_file_metadata(path: &Path, unit_id: Option<String>) -> Result<SourceFileMetadata> {
    source_file_metadata_with_kind(path, unit_id, SourceFileKind::Unit, SourceFileStatus::Valid)
}

pub(crate) fn source_file_metadata_with_kind(
    path: &Path,
    unit_id: Option<String>,
    kind: SourceFileKind,
    status: SourceFileStatus,
) -> Result<SourceFileMetadata> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read source file metadata: {}", path.display()))?;
    let content = fs::read(path)
        .with_context(|| format!("failed to read source file: {}", path.display()))?;

    Ok(SourceFileMetadata {
        path: path.display().to_string(),
        unit_id,
        kind,
        hash: Some(content_hash(&content)),
        mtime: metadata.modified().ok().and_then(system_time_to_unix_secs),
        size: i64::try_from(metadata.len()).ok(),
        status,
        error_kind: None,
        error_message: None,
        error_field: None,
    })
}

pub(crate) fn invalid_source_file_metadata(
    path: &Path,
    kind: SourceFileKind,
    status: SourceFileStatus,
    error_message: String,
) -> Result<SourceFileMetadata> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to read source file metadata: {}", path.display()))?;
    let content = fs::read(path)
        .with_context(|| format!("failed to read source file: {}", path.display()))?;

    Ok(SourceFileMetadata {
        path: path.display().to_string(),
        unit_id: None,
        kind,
        hash: Some(content_hash(&content)),
        mtime: metadata.modified().ok().and_then(system_time_to_unix_secs),
        size: i64::try_from(metadata.len()).ok(),
        status,
        error_kind: Some("parse".to_string()),
        error_message: Some(error_message),
        error_field: Some("frontmatter".to_string()),
    })
}

fn content_hash(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    format!("{:x}", hasher.finalize())
}

fn system_time_to_unix_secs(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
}
