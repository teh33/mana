use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use crate::unit::{AttemptOutcome, Status};

use super::Index;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepProviderRow {
    pub artifact: String,
    pub unit_id: String,
    pub unit_title: String,
    pub status: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildSummaryRow {
    pub id: String,
    pub title: String,
    pub status: String,
    pub attempts: usize,
    pub recent_outcome: Option<String>,
    pub summary: Option<String>,
    pub follow_up: Option<String>,
}

impl Index {
    pub fn invalid_relevant_diagnostic(&self, unit_id: &str) -> Result<Option<String>> {
        self.connection()
            .query_row(
                r#"
                SELECT message
                FROM index_diagnostics
                WHERE severity = 'error'
                  AND kind IN ('parse', 'schema')
                  AND (
                    unit_id = ?1
                    OR source_path IN (
                        SELECT path FROM source_files WHERE unit_id = ?1 OR path LIKE '%' || ?1 || '-%'
                    )
                  )
                ORDER BY id
                LIMIT 1
                "#,
                [unit_id],
                |row| row.get(0),
            )
            .optional()
            .with_context(|| format!("failed to query invalid diagnostics for unit {unit_id}"))
    }

    pub fn dependency_providers(
        &self,
        unit_id: &str,
        parent_id: Option<&str>,
        required_artifacts: &[String],
    ) -> Result<Vec<DepProviderRow>> {
        if required_artifacts.is_empty() {
            return Ok(Vec::new());
        }

        let mut providers = Vec::new();
        for artifact in required_artifacts {
            let row = self
                .connection()
                .query_row(
                    r#"
                    SELECT u.id, u.title, u.status, u.description
                    FROM unit_artifacts a
                    JOIN units u ON u.id = a.unit_id
                    WHERE a.direction = 'produces'
                      AND a.artifact = ?1
                      AND u.id != ?2
                      AND (?3 IS NULL OR u.parent = ?3)
                    ORDER BY u.id
                    LIMIT 1
                    "#,
                    params![artifact, unit_id, parent_id],
                    |row| {
                        Ok(DepProviderRow {
                            artifact: artifact.clone(),
                            unit_id: row.get(0)?,
                            unit_title: row.get(1)?,
                            status: row.get(2)?,
                            description: row.get(3)?,
                        })
                    },
                )
                .optional()?;

            if let Some(row) = row {
                providers.push(row);
            }
        }

        Ok(providers)
    }

    pub fn child_summaries(&self, parent_id: &str) -> Result<Vec<ChildSummaryRow>> {
        let mut statement = self.connection().prepare(
            r#"
            SELECT id, title, status, notes, close_reason, outputs_json, verify
            FROM units
            WHERE parent = ?1
            ORDER BY id
            "#,
        )?;
        let rows = statement.query_map([parent_id], |row| {
            let id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let status: String = row.get(2)?;
            let notes: Option<String> = row.get(3)?;
            let close_reason: Option<String> = row.get(4)?;
            let outputs_json: Option<String> = row.get(5)?;
            let verify: Option<String> = row.get(6)?;
            Ok((id, title, status, notes, close_reason, outputs_json, verify))
        })?;

        let mut summaries = Vec::new();
        for row in rows {
            let (id, title, status, notes, close_reason, outputs_json, verify) = row?;
            let attempts = self.attempt_count(&id)?;
            let recent_outcome = self
                .latest_attempt_outcome(&id)?
                .or_else(|| status_implied_outcome(&status));
            let summary = summarize_text(close_reason.as_deref())
                .or_else(|| summarize_text(notes.as_deref()))
                .or_else(|| {
                    self.latest_attempt_notes(&id)
                        .ok()
                        .flatten()
                        .and_then(|notes| summarize_text(Some(&notes)))
                })
                .or_else(|| summarize_text(outputs_json.as_deref()));
            let follow_up = self.child_follow_up(&id, &status, verify.as_deref())?;

            summaries.push(ChildSummaryRow {
                id,
                title,
                status,
                attempts,
                recent_outcome,
                summary,
                follow_up,
            });
        }

        summaries.sort_by(|a, b| crate::util::natural_cmp(&a.id, &b.id));
        Ok(summaries)
    }

    fn attempt_count(&self, unit_id: &str) -> Result<usize> {
        let count: i64 = self.connection().query_row(
            "SELECT COUNT(*) FROM unit_attempts WHERE unit_id = ?1",
            [unit_id],
            |row| row.get(0),
        )?;
        usize::try_from(count).context("attempt count overflow")
    }

    fn latest_attempt_outcome(&self, unit_id: &str) -> Result<Option<String>> {
        self.connection()
            .query_row(
                r#"
                SELECT outcome
                FROM unit_attempts
                WHERE unit_id = ?1
                ORDER BY attempt_index DESC
                LIMIT 1
                "#,
                [unit_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map(|value| value.flatten().and_then(parse_attempt_outcome))
            .context("failed to query latest attempt outcome")
    }

    fn latest_attempt_notes(&self, unit_id: &str) -> Result<Option<String>> {
        self.connection()
            .query_row(
                r#"
                SELECT notes
                FROM unit_attempts
                WHERE unit_id = ?1 AND notes IS NOT NULL AND TRIM(notes) != ''
                ORDER BY attempt_index DESC
                LIMIT 1
                "#,
                [unit_id],
                |row| row.get(0),
            )
            .optional()
            .context("failed to query latest attempt notes")
    }

    fn child_follow_up(
        &self,
        unit_id: &str,
        status: &str,
        verify: Option<&str>,
    ) -> Result<Option<String>> {
        let decisions: i64 = self.connection().query_row(
            "SELECT COUNT(*) FROM unit_decisions WHERE unit_id = ?1",
            [unit_id],
            |row| row.get(0),
        )?;
        if decisions > 0 {
            return Ok(Some(format!("{} unresolved decision(s)", decisions)));
        }

        if status != Status::Closed.to_string() {
            if verify.is_some() {
                return Ok(Some("still needs completion/verify".to_string()));
            }
            return Ok(Some("still open".to_string()));
        }

        Ok(None)
    }
}

fn status_implied_outcome(status: &str) -> Option<String> {
    if status == Status::Closed.to_string() {
        Some("success".to_string())
    } else if status == Status::AwaitingVerify.to_string() {
        Some("awaiting_verify".to_string())
    } else if status == Status::InProgress.to_string() {
        Some("in_progress".to_string())
    } else {
        None
    }
}

fn parse_attempt_outcome(value: String) -> Option<String> {
    let outcome: AttemptOutcome = serde_json::from_str(&value).ok()?;
    Some(match outcome {
        AttemptOutcome::Success => "success".to_string(),
        AttemptOutcome::Failed => "failed".to_string(),
        AttemptOutcome::Abandoned => "abandoned".to_string(),
    })
}

fn summarize_text(text: Option<&str>) -> Option<String> {
    let text = text?.trim();
    if text.is_empty() {
        return None;
    }

    let single_line = text.lines().find(|line| !line.trim().is_empty())?.trim();
    let mut summary = single_line.chars().take(140).collect::<String>();
    if single_line.chars().count() > 140 {
        summary.push('…');
    }
    Some(summary)
}
