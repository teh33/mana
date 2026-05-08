use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::discovery::{find_archived_unit, find_unit_file};
use crate::index::Index;
use crate::unit::{Status, Unit};

pub const FACTS_FILE: &str = "facts.mana";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FactSheetStatus {
    Draft,
    Spec,
    InProgress,
    Verified,
    Stale,
    Rejected,
}

impl FactSheetStatus {
    pub fn parse(token: &str) -> Option<Self> {
        match token.strip_prefix('@').unwrap_or(token) {
            "draft" => Some(Self::Draft),
            "spec" => Some(Self::Spec),
            "in_progress" => Some(Self::InProgress),
            "verified" => Some(Self::Verified),
            "stale" => Some(Self::Stale),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Spec => "spec",
            Self::InProgress => "in_progress",
            Self::Verified => "verified",
            Self::Stale => "stale",
            Self::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactSheetFact {
    pub text: String,
    pub status: FactSheetStatus,
    pub unit_ref: Option<String>,
    pub anchor: Option<String>,
    pub section: Vec<String>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FactSheetDiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactSheetDiagnostic {
    pub line: Option<usize>,
    pub severity: FactSheetDiagnosticSeverity,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactSheetParseResult {
    pub facts: Vec<FactSheetFact>,
    pub diagnostics: Vec<FactSheetDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactSheetCheckEntry {
    pub fact: FactSheetFact,
    pub passed: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactSheetCheckResult {
    pub path: PathBuf,
    pub facts: Vec<FactSheetFact>,
    pub diagnostics: Vec<FactSheetDiagnostic>,
    pub entries: Vec<FactSheetCheckEntry>,
}

impl FactSheetCheckResult {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == FactSheetDiagnosticSeverity::Error)
            || self.entries.iter().any(|e| !e.passed)
    }
}

pub fn facts_path_from_mana_dir(mana_dir: &Path) -> Result<PathBuf> {
    let project_root = mana_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot determine project root from mana dir"))?;
    Ok(project_root.join(FACTS_FILE))
}

pub fn parse_facts_sheet(content: &str) -> FactSheetParseResult {
    let mut facts = Vec::new();
    let mut diagnostics = Vec::new();
    let mut section_stack: Vec<(usize, String)> = Vec::new();
    let mut anchors: HashMap<String, usize> = HashMap::new();

    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim();

        if line.is_empty() || line.starts_with("//") {
            continue;
        }

        if let Some((depth, title)) = parse_heading(line) {
            while section_stack.last().is_some_and(|(d, _)| *d >= depth) {
                section_stack.pop();
            }
            section_stack.push((depth, title));
            continue;
        }

        if !line.starts_with("- ") {
            diagnostics.push(error(
                Some(line_no),
                "expected a fact line starting with '- ' or a Markdown heading",
            ));
            continue;
        }

        let Some(fact) = parse_fact_line(&line[2..], line_no, &section_stack, &mut diagnostics)
        else {
            continue;
        };

        if let Some(anchor) = &fact.anchor {
            if let Some(first_line) = anchors.insert(anchor.clone(), line_no) {
                diagnostics.push(error(
                    Some(line_no),
                    format!("duplicate fact anchor '{{{anchor}}}' first used on line {first_line}"),
                ));
            }
        }

        facts.push(fact);
    }

    FactSheetParseResult { facts, diagnostics }
}

pub fn check_facts_sheet(mana_dir: &Path) -> Result<FactSheetCheckResult> {
    let path = facts_path_from_mana_dir(mana_dir)?;
    if !path.exists() {
        return Ok(FactSheetCheckResult {
            path,
            facts: Vec::new(),
            diagnostics: Vec::new(),
            entries: Vec::new(),
        });
    }

    let content = fs::read_to_string(&path)?;
    let parsed = parse_facts_sheet(&content);
    let index = Index::load_or_rebuild(mana_dir)?;

    let mut entries = Vec::new();
    let mut diagnostics = parsed.diagnostics.clone();

    for fact in &parsed.facts {
        if let Some(unit_ref) = &fact.unit_ref {
            match load_backing_unit(mana_dir, &index, unit_ref) {
                Ok(Some(unit)) => {
                    if fact.status == FactSheetStatus::Verified && unit.status != Status::Closed {
                        entries.push(FactSheetCheckEntry {
                            fact: fact.clone(),
                            passed: false,
                            message: Some(format!(
                                "@verified fact references unit {unit_ref}, but that unit is {}",
                                unit.status
                            )),
                        });
                    } else {
                        entries.push(FactSheetCheckEntry {
                            fact: fact.clone(),
                            passed: true,
                            message: None,
                        });
                    }
                }
                Ok(None) => {
                    entries.push(FactSheetCheckEntry {
                        fact: fact.clone(),
                        passed: false,
                        message: Some(format!("referenced Mana unit {unit_ref} was not found")),
                    });
                }
                Err(err) => diagnostics.push(error(
                    Some(fact.line),
                    format!("failed to load referenced Mana unit {unit_ref}: {err}"),
                )),
            }
        } else {
            entries.push(FactSheetCheckEntry {
                fact: fact.clone(),
                passed: true,
                message: None,
            });
        }
    }

    Ok(FactSheetCheckResult {
        path,
        facts: parsed.facts,
        diagnostics,
        entries,
    })
}

fn parse_fact_line(
    content: &str,
    line: usize,
    section_stack: &[(usize, String)],
    diagnostics: &mut Vec<FactSheetDiagnostic>,
) -> Option<FactSheetFact> {
    let mut words: Vec<&str> = content.split_whitespace().collect();
    if words.is_empty() {
        diagnostics.push(error(Some(line), "fact line is empty"));
        return None;
    }

    let mut anchor = None;
    if let Some(last) = words.last().copied() {
        if last.starts_with('{') || last.ends_with('}') {
            if is_valid_anchor_token(last) {
                anchor = Some(last[1..last.len() - 1].to_string());
                words.pop();
            } else {
                diagnostics.push(error(Some(line), format!("malformed fact anchor '{last}'")));
                return None;
            }
        }
    }

    let mut unit_ref = None;
    if let Some(last) = words.last().copied() {
        if looks_like_unit_ref(last) {
            unit_ref = Some(last.to_string());
            words.pop();
        }
    }

    let status_positions: Vec<usize> = words
        .iter()
        .enumerate()
        .filter_map(|(idx, word)| FactSheetStatus::parse(word).map(|_| idx))
        .collect();

    if status_positions.is_empty() {
        diagnostics.push(error(
            Some(line),
            "fact line must contain one status: @draft, @spec, @in_progress, @verified, @stale, or @rejected",
        ));
        return None;
    }

    if status_positions.len() > 1 {
        diagnostics.push(error(
            Some(line),
            "fact line must contain exactly one status",
        ));
        return None;
    }

    let status_idx = status_positions[0];
    let status = FactSheetStatus::parse(words[status_idx]).expect("status checked above");
    words.remove(status_idx);

    if words.iter().any(|word| word.starts_with('@')) {
        diagnostics.push(error(
            Some(line),
            "unknown @status or extra @tag in fact line",
        ));
        return None;
    }

    let text = words.join(" ").trim().to_string();
    if text.is_empty() {
        diagnostics.push(error(Some(line), "fact text is empty"));
        return None;
    }

    Some(FactSheetFact {
        text,
        status,
        unit_ref,
        anchor,
        section: section_stack
            .iter()
            .map(|(_, title)| title.clone())
            .collect(),
        line,
    })
}

fn parse_heading(line: &str) -> Option<(usize, String)> {
    if !line.starts_with('#') {
        return None;
    }
    let depth = line.chars().take_while(|c| *c == '#').count();
    let title = line[depth..].trim();
    if title.is_empty() {
        return None;
    }
    Some((depth, title.to_string()))
}

fn is_valid_anchor_token(token: &str) -> bool {
    let Some(inner) = token.strip_prefix('{').and_then(|s| s.strip_suffix('}')) else {
        return false;
    };
    !inner.is_empty()
        && inner
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

fn looks_like_unit_ref(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    !parts.is_empty()
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
}

fn load_backing_unit(mana_dir: &Path, index: &Index, unit_ref: &str) -> Result<Option<Unit>> {
    let Some(entry) = index.units.iter().find(|entry| entry.id == unit_ref) else {
        return Ok(None);
    };

    let path = if entry.status == Status::Closed {
        find_archived_unit(mana_dir, unit_ref).ok()
    } else {
        find_unit_file(mana_dir, unit_ref).ok()
    };

    path.map(Unit::from_file).transpose()
}

fn error(line: Option<usize>, message: impl Into<String>) -> FactSheetDiagnostic {
    FactSheetDiagnostic {
        line,
        severity: FactSheetDiagnosticSeverity::Error,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ops::create::{create, CreateParams};
    use std::fs;
    use tempfile::TempDir;

    fn setup_mana_dir() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        Config {
            project: "test".to_string(),
            next_id: 1,
            auto_close_parent: true,
            run: None,
            plan: None,
            max_loops: 10,
            max_concurrent: 4,
            poll_interval: 30,
            extends: vec![],
            rules_file: None,
            file_locking: false,
            worktree: false,
            on_close: None,
            on_fail: None,
            verify_timeout: None,
            review: None,
            user: None,
            user_email: None,
            auto_commit: false,
            commit_template: None,
            research: None,
            run_model: None,
            plan_model: None,
            review_model: None,
            research_model: None,
            batch_verify: false,
            memory_reserve_mb: 0,
            notify: None,
        }
        .save(&mana_dir)
        .unwrap();
        (dir, mana_dir)
    }

    #[test]
    fn parse_single_line_facts_with_sections_refs_and_anchors() {
        let content = "# architecture\n\n- SQLite mirrors Mana files for fast agent reads @verified 247.1.2.7 {sqlite-mirror}\n## context\n- Imp reads relevant facts from Mana APIs @spec\n";
        let parsed = parse_facts_sheet(content);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        assert_eq!(parsed.facts.len(), 2);
        assert_eq!(
            parsed.facts[0].text,
            "SQLite mirrors Mana files for fast agent reads"
        );
        assert_eq!(parsed.facts[0].status, FactSheetStatus::Verified);
        assert_eq!(parsed.facts[0].unit_ref.as_deref(), Some("247.1.2.7"));
        assert_eq!(parsed.facts[0].anchor.as_deref(), Some("sqlite-mirror"));
        assert_eq!(parsed.facts[1].section, vec!["architecture", "context"]);
    }

    #[test]
    fn parse_rejects_unknown_status() {
        let parsed = parse_facts_sheet("- Mana is great @done\n");
        assert_eq!(parsed.facts.len(), 0);
        assert!(parsed.diagnostics[0].message.contains("one status"));
    }

    #[test]
    fn parse_rejects_duplicate_anchors() {
        let parsed = parse_facts_sheet("- One fact @spec {same}\n- Another fact @draft {same}\n");
        assert_eq!(parsed.facts.len(), 2);
        assert!(parsed
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("duplicate fact anchor")));
    }

    #[test]
    fn missing_facts_file_checks_cleanly() {
        let (_dir, mana_dir) = setup_mana_dir();
        let checked = check_facts_sheet(&mana_dir).unwrap();
        assert!(checked.facts.is_empty());
        assert!(!checked.has_errors());
    }

    #[test]
    fn check_verified_fact_fails_for_open_backing_unit() {
        let (dir, mana_dir) = setup_mana_dir();
        let created = create(
            &mana_dir,
            CreateParams {
                title: "Open backing unit".to_string(),
                description: None,
                acceptance: None,
                notes: None,
                design: None,
                verify: Some("test -f .mana/config.yaml".to_string()),
                priority: Some(2),
                labels: vec![],
                assignee: None,
                dependencies: vec![],
                parent: None,
                produces: vec![],
                requires: vec![],
                paths: vec![],
                on_fail: None,
                fail_first: false,
                feature: false,
                kind: None,
                verify_timeout: None,
                decisions: vec![],
                force: false,
            },
        )
        .unwrap();

        fs::write(
            dir.path().join(FACTS_FILE),
            format!(
                "- This fact is backed by open work @verified {}\n",
                created.unit.id
            ),
        )
        .unwrap();

        let checked = check_facts_sheet(&mana_dir).unwrap();
        assert!(checked.has_errors());
        assert_eq!(checked.entries.len(), 1);
        assert!(!checked.entries[0].passed);
    }
}
