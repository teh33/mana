use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::config::Config;
use crate::ctx_assembler::{extract_paths, read_file};
use crate::discovery::find_unit_file;
use crate::index::Index;
use crate::unit::{AttemptOutcome, Status, Unit};

// ─── Result types ────────────────────────────────────────────────────────────

/// Information about a sibling unit that produces an artifact this unit requires.
pub struct DepProvider {
    pub artifact: String,
    pub unit_id: String,
    pub unit_title: String,
    pub status: String,
    pub description: Option<String>,
}

/// A file referenced by the unit with its content and structural summary.
pub struct FileEntry {
    pub path: String,
    pub content: Option<String>,
    pub structure: Option<String>,
}

/// Concise parent-oriented summary of a direct child unit.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChildSummary {
    pub id: String,
    pub title: String,
    pub status: String,
    pub attempts: usize,
    pub recent_outcome: Option<String>,
    pub summary: Option<String>,
    pub follow_up: Option<String>,
}

/// Fully assembled agent context for a unit.
pub struct AgentContext {
    pub unit: Unit,
    pub rules: Option<String>,
    pub attempt_notes: Option<String>,
    pub dep_providers: Vec<DepProvider>,
    pub child_summaries: Vec<ChildSummary>,
    pub files: Vec<FileEntry>,
}

// ─── Core operations ─────────────────────────────────────────────────────────

/// Assemble full agent context for a unit — the structured data needed
/// to build any output format (text, JSON, agent prompt).
///
/// Loads the unit, resolves dependency context, merges file paths from
/// explicit `unit.paths` and regex-extracted paths from the description,
/// reads file contents, and extracts structural summaries.
pub fn assemble_agent_context(mana_dir: &Path, id: &str) -> Result<AgentContext> {
    let unit_path =
        find_unit_file(mana_dir, id).context(format!("Could not find unit with ID: {}", id))?;

    let unit = Unit::from_file(&unit_path).context(format!(
        "Failed to parse unit from: {}",
        unit_path.display()
    ))?;

    let project_dir = mana_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Invalid .mana/ path: {}", mana_dir.display()))?;

    let paths = merge_paths(&unit);
    let rules = load_rules(mana_dir);
    let attempt_notes = format_attempt_notes(&unit);
    let dep_providers = resolve_dependency_context(mana_dir, &unit);
    let child_summaries = summarize_child_units(mana_dir, &unit.id);

    let canonical_base = project_dir
        .canonicalize()
        .context("Cannot canonicalize project dir")?;

    let mut files: Vec<FileEntry> = Vec::new();
    for path_str in &paths {
        let full_path = project_dir.join(path_str);
        let canonical = full_path.canonicalize().ok();

        let in_bounds = canonical
            .as_ref()
            .map(|c| c.starts_with(&canonical_base))
            .unwrap_or(false);

        let content = if let Some(ref c) = canonical {
            if in_bounds {
                read_file(c).ok()
            } else {
                None
            }
        } else {
            None
        };

        let structure = content
            .as_deref()
            .and_then(|c| extract_file_structure(path_str, c));

        files.push(FileEntry {
            path: path_str.clone(),
            content,
            structure,
        });
    }

    Ok(AgentContext {
        unit,
        rules,
        attempt_notes,
        dep_providers,
        child_summaries,
        files,
    })
}

// ─── Rules loading ───────────────────────────────────────────────────────────

/// Load project rules from the configured rules file.
///
/// Returns `None` if the file doesn't exist or is empty.
pub fn load_rules(mana_dir: &Path) -> Option<String> {
    let config = Config::load_with_extends(mana_dir).ok()?;
    let rules_path = config.rules_path(mana_dir);

    let content = std::fs::read_to_string(&rules_path).ok()?;
    let trimmed = content.trim();

    if trimmed.is_empty() {
        return None;
    }

    let line_count = content.lines().count();
    if line_count > 1000 {
        eprintln!(
            "Warning: RULES.md is very large ({} lines). Consider trimming it.",
            line_count
        );
    }

    Some(content)
}

// ─── Attempt notes ───────────────────────────────────────────────────────────

/// Format the attempt_log and notes field into a combined notes string.
///
/// Returns `None` if there are no attempt notes and no unit notes.
pub fn format_attempt_notes(unit: &Unit) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();

    if let Some(ref notes) = unit.notes {
        let trimmed = notes.trim();
        if !trimmed.is_empty() {
            parts.push(format!("Unit notes:\n{}", trimmed));
        }
    }

    let attempt_entries: Vec<String> = unit
        .attempt_log
        .iter()
        .filter_map(|a| {
            let notes = a.notes.as_deref()?.trim();
            if notes.is_empty() {
                return None;
            }
            let outcome = match a.outcome {
                AttemptOutcome::Success => "success",
                AttemptOutcome::Failed => "failed",
                AttemptOutcome::Abandoned => "abandoned",
            };
            let agent_str = a
                .agent
                .as_deref()
                .map(|ag| format!(" ({})", ag))
                .unwrap_or_default();
            Some(format!(
                "Attempt #{}{} [{}]: {}",
                a.num, agent_str, outcome, notes
            ))
        })
        .collect();

    if !attempt_entries.is_empty() {
        parts.push(attempt_entries.join("\n"));
    }

    if parts.is_empty() {
        return None;
    }

    Some(parts.join("\n\n"))
}

// ─── Dependency context ──────────────────────────────────────────────────────

/// Resolve dependency context: find sibling units that produce artifacts
/// this unit requires, and load their descriptions.
pub fn resolve_dependency_context(mana_dir: &Path, unit: &Unit) -> Vec<DepProvider> {
    if unit.requires.is_empty() {
        return Vec::new();
    }

    let index = match Index::load_or_rebuild(mana_dir) {
        Ok(idx) => idx,
        Err(_) => return Vec::new(),
    };

    let mut providers = Vec::new();

    for required in &unit.requires {
        let producer = index
            .units
            .iter()
            .find(|e| e.id != unit.id && e.parent == unit.parent && e.produces.contains(required));

        if let Some(entry) = producer {
            let desc = find_unit_file(mana_dir, &entry.id)
                .ok()
                .and_then(|p| Unit::from_file(&p).ok())
                .and_then(|b| b.description.clone());

            providers.push(DepProvider {
                artifact: required.clone(),
                unit_id: entry.id.clone(),
                unit_title: entry.title.clone(),
                status: format!("{}", entry.status),
                description: desc,
            });
        }
    }

    providers
}

/// Summarize direct child jobs for parent-oriented views.
pub fn summarize_child_units(mana_dir: &Path, parent_id: &str) -> Vec<ChildSummary> {
    let index = match Index::load_or_rebuild(mana_dir) {
        Ok(idx) => idx,
        Err(_) => return Vec::new(),
    };

    let mut children: Vec<_> = index
        .units
        .iter()
        .filter(|entry| entry.parent.as_deref() == Some(parent_id))
        .cloned()
        .collect();
    children.sort_by(|a, b| crate::util::natural_cmp(&a.id, &b.id));

    children
        .into_iter()
        .map(|entry| {
            let full_unit = find_unit_file(mana_dir, &entry.id)
                .ok()
                .and_then(|path| Unit::from_file(path).ok());

            let recent_outcome = full_unit
                .as_ref()
                .and_then(latest_attempt_outcome)
                .or_else(|| status_implied_outcome(entry.status));
            let summary = full_unit.as_ref().and_then(summarize_child_signal);
            let follow_up = full_unit.as_ref().and_then(summarize_child_follow_up);

            ChildSummary {
                id: entry.id,
                title: entry.title,
                status: entry.status.to_string(),
                attempts: full_unit
                    .as_ref()
                    .map(|unit| unit.attempt_log.len())
                    .unwrap_or(0),
                recent_outcome,
                summary,
                follow_up,
            }
        })
        .collect()
}

fn latest_attempt_outcome(unit: &Unit) -> Option<String> {
    unit.attempt_log
        .last()
        .map(|attempt| match attempt.outcome {
            AttemptOutcome::Success => "success".to_string(),
            AttemptOutcome::Failed => "failed".to_string(),
            AttemptOutcome::Abandoned => "abandoned".to_string(),
        })
}

fn status_implied_outcome(status: Status) -> Option<String> {
    match status {
        Status::Closed => Some("success".to_string()),
        Status::AwaitingVerify => Some("awaiting_verify".to_string()),
        Status::InProgress => Some("in_progress".to_string()),
        Status::Open => None,
    }
}

fn summarize_child_signal(unit: &Unit) -> Option<String> {
    if let Some(summary) = summarize_text(unit.close_reason.as_deref()) {
        return Some(summary);
    }
    if let Some(summary) = summarize_text(unit.notes.as_deref()) {
        return Some(summary);
    }
    if let Some(summary) = summarize_text(
        unit.attempt_log
            .iter()
            .rev()
            .find_map(|attempt| attempt.notes.as_deref()),
    ) {
        return Some(summary);
    }
    unit.outputs
        .as_ref()
        .and_then(|outputs| summarize_text(Some(&outputs.to_string())))
}

fn summarize_child_follow_up(unit: &Unit) -> Option<String> {
    if !unit.decisions.is_empty() {
        return Some(format!("{} unresolved decision(s)", unit.decisions.len()));
    }

    if unit.status != Status::Closed {
        if unit.verify.is_some() {
            return Some("still needs completion/verify".to_string());
        }
        return Some("still open".to_string());
    }

    None
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

// ─── Path merging ────────────────────────────────────────────────────────────

/// Merge explicit `unit.paths` with paths regex-extracted from the description.
/// Explicit paths come first, then regex-extracted paths fill gaps.
pub fn merge_paths(unit: &Unit) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();

    for p in &unit.paths {
        if seen.insert(p.clone()) {
            result.push(p.clone());
        }
    }

    let description = unit.description.as_deref().unwrap_or("");
    for p in extract_paths(description) {
        if seen.insert(p.clone()) {
            result.push(p);
        }
    }

    result
}

// ─── Structure extraction ────────────────────────────────────────────────────

/// Extract a structural summary (signatures, imports) from file content.
///
/// Dispatches to language-specific extractors based on file extension.
/// Returns `None` for unrecognized file types or when no structure is found.
pub fn extract_file_structure(path: &str, content: &str) -> Option<String> {
    let ext = Path::new(path).extension()?.to_str()?;

    let lines: Vec<String> = match ext {
        "rs" => extract_rust_structure(content),
        "ts" | "tsx" => extract_ts_structure(content),
        "py" => extract_python_structure(content),
        _ => return None,
    };

    if lines.is_empty() {
        return None;
    }

    Some(lines.join("\n"))
}

fn extract_rust_structure(content: &str) -> Vec<String> {
    let mut result = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
        {
            continue;
        }

        if trimmed.starts_with("use ") {
            result.push(trimmed.to_string());
            continue;
        }

        let is_decl = trimmed.starts_with("pub fn ")
            || trimmed.starts_with("pub async fn ")
            || trimmed.starts_with("pub(crate) fn ")
            || trimmed.starts_with("pub(crate) async fn ")
            || trimmed.starts_with("fn ")
            || trimmed.starts_with("async fn ")
            || trimmed.starts_with("pub struct ")
            || trimmed.starts_with("pub(crate) struct ")
            || trimmed.starts_with("struct ")
            || trimmed.starts_with("pub enum ")
            || trimmed.starts_with("pub(crate) enum ")
            || trimmed.starts_with("enum ")
            || trimmed.starts_with("pub trait ")
            || trimmed.starts_with("pub(crate) trait ")
            || trimmed.starts_with("trait ")
            || trimmed.starts_with("pub type ")
            || trimmed.starts_with("type ")
            || trimmed.starts_with("impl ")
            || trimmed.starts_with("pub const ")
            || trimmed.starts_with("pub(crate) const ")
            || trimmed.starts_with("const ")
            || trimmed.starts_with("pub static ")
            || trimmed.starts_with("static ");

        if is_decl {
            let sig = trimmed.trim_end_matches('{').trim_end();
            result.push(sig.to_string());
        }
    }

    result
}

fn extract_ts_structure(content: &str) -> Vec<String> {
    let mut result = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
        {
            continue;
        }

        if trimmed.starts_with("import ") {
            result.push(trimmed.to_string());
            continue;
        }

        let is_decl = trimmed.starts_with("export function ")
            || trimmed.starts_with("export async function ")
            || trimmed.starts_with("export default function ")
            || trimmed.starts_with("function ")
            || trimmed.starts_with("async function ")
            || trimmed.starts_with("export class ")
            || trimmed.starts_with("export abstract class ")
            || trimmed.starts_with("class ")
            || trimmed.starts_with("export interface ")
            || trimmed.starts_with("interface ")
            || trimmed.starts_with("export type ")
            || trimmed.starts_with("export enum ")
            || trimmed.starts_with("export const ")
            || trimmed.starts_with("export default class ")
            || trimmed.starts_with("export default async function ");

        if is_decl {
            let sig = trimmed.trim_end_matches('{').trim_end();
            result.push(sig.to_string());
        }
    }

    result
}

fn extract_python_structure(content: &str) -> Vec<String> {
    let mut result = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if line.starts_with("import ") || line.starts_with("from ") {
            result.push(trimmed.to_string());
            continue;
        }

        if trimmed.starts_with("def ")
            || trimmed.starts_with("async def ")
            || trimmed.starts_with("class ")
        {
            let sig = trimmed.trim_end_matches(':').trim_end();
            result.push(sig.to_string());
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::{AttemptOutcome, AttemptRecord};
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_env() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        (dir, mana_dir)
    }

    #[test]
    fn assemble_context_basic() {
        let (_dir, mana_dir) = setup_test_env();
        let mut unit = Unit::new("1", "Test unit");
        unit.description = Some("A description with no file paths".to_string());
        let slug = crate::util::title_to_slug(&unit.title);
        let unit_path = mana_dir.join(format!("1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        let ctx = assemble_agent_context(&mana_dir, "1").unwrap();
        assert_eq!(ctx.unit.id, "1");
        assert!(ctx.files.is_empty());
    }

    #[test]
    fn assemble_context_with_files() {
        let (dir, mana_dir) = setup_test_env();
        let project_dir = dir.path();

        let src_dir = project_dir.join("src");
        fs::create_dir(&src_dir).unwrap();
        fs::write(src_dir.join("foo.rs"), "fn main() {}").unwrap();

        let mut unit = Unit::new("1", "Test unit");
        unit.description = Some("Check src/foo.rs for implementation".to_string());
        let slug = crate::util::title_to_slug(&unit.title);
        let unit_path = mana_dir.join(format!("1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        let ctx = assemble_agent_context(&mana_dir, "1").unwrap();
        assert_eq!(ctx.files.len(), 1);
        assert_eq!(ctx.files[0].path, "src/foo.rs");
        assert!(ctx.files[0].content.is_some());
    }

    #[test]
    fn assemble_context_not_found() {
        let (_dir, mana_dir) = setup_test_env();
        let result = assemble_agent_context(&mana_dir, "999");
        assert!(result.is_err());
    }

    #[test]
    fn load_rules_returns_none_when_missing() {
        let (_dir, mana_dir) = setup_test_env();
        fs::write(mana_dir.join("config.yaml"), "project: test\nnext_id: 1\n").unwrap();
        assert!(load_rules(&mana_dir).is_none());
    }

    #[test]
    fn load_rules_returns_none_when_empty() {
        let (_dir, mana_dir) = setup_test_env();
        fs::write(mana_dir.join("config.yaml"), "project: test\nnext_id: 1\n").unwrap();
        fs::write(mana_dir.join("RULES.md"), "   \n\n  ").unwrap();
        assert!(load_rules(&mana_dir).is_none());
    }

    #[test]
    fn load_rules_returns_content() {
        let (_dir, mana_dir) = setup_test_env();
        fs::write(mana_dir.join("config.yaml"), "project: test\nnext_id: 1\n").unwrap();
        fs::write(mana_dir.join("RULES.md"), "# My Rules\nNo unwrap.\n").unwrap();
        let result = load_rules(&mana_dir);
        assert!(result.is_some());
        assert!(result.unwrap().contains("No unwrap."));
    }

    #[test]
    fn format_attempt_notes_empty() {
        let unit = Unit::new("1", "Empty unit");
        assert!(format_attempt_notes(&unit).is_none());
    }

    #[test]
    fn format_attempt_notes_with_data() {
        let mut unit = Unit::new("1", "Test unit");
        unit.attempt_log = vec![AttemptRecord {
            num: 1,
            outcome: AttemptOutcome::Abandoned,
            notes: Some("Tried X, hit bug Y".to_string()),
            agent: Some("pi-agent".to_string()),
            started_at: None,
            finished_at: None,
            autonomy_observation: None,
        }];

        let result = format_attempt_notes(&unit).unwrap();
        assert!(result.contains("Attempt #1"));
        assert!(result.contains("pi-agent"));
        assert!(result.contains("abandoned"));
        assert!(result.contains("Tried X, hit bug Y"));
    }

    #[test]
    fn format_attempt_notes_with_unit_notes() {
        let mut unit = Unit::new("1", "Test unit");
        unit.notes = Some("Watch out for edge cases".to_string());
        let result = format_attempt_notes(&unit).unwrap();
        assert!(result.contains("Watch out for edge cases"));
        assert!(result.contains("Unit notes:"));
    }

    #[test]
    fn format_attempt_notes_skips_whitespace_only() {
        let mut unit = Unit::new("1", "Test unit");
        unit.notes = Some("   ".to_string());
        unit.attempt_log = vec![AttemptRecord {
            num: 1,
            outcome: AttemptOutcome::Abandoned,
            notes: Some("  ".to_string()),
            agent: None,
            started_at: None,
            finished_at: None,
            autonomy_observation: None,
        }];
        assert!(format_attempt_notes(&unit).is_none());
    }

    #[test]
    fn summarize_child_units_includes_recent_outcome_summary_and_follow_up() {
        let (_dir, mana_dir) = setup_test_env();

        let parent = Unit::new("1", "Parent");
        let parent_slug = crate::util::title_to_slug(&parent.title);
        parent
            .to_file(mana_dir.join(format!("1-{}.md", parent_slug)))
            .unwrap();

        let mut child = Unit::new("1.1", "Child job");
        child.parent = Some("1".to_string());
        child.status = Status::Open;
        child.verify = Some("cargo test child".to_string());
        child.notes =
            Some("Investigated parser edge case and found bad separator handling".to_string());
        child.decisions = vec!["Pick parser boundary behavior".to_string()];
        child.attempt_log = vec![AttemptRecord {
            num: 1,
            outcome: AttemptOutcome::Failed,
            notes: Some("Attempted fix A but fixture still fails".to_string()),
            agent: Some("imp".to_string()),
            started_at: None,
            finished_at: None,
            autonomy_observation: None,
        }];
        let child_slug = crate::util::title_to_slug(&child.title);
        child
            .to_file(mana_dir.join(format!("1.1-{}.md", child_slug)))
            .unwrap();

        let summaries = summarize_child_units(&mana_dir, "1");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "1.1");
        assert_eq!(summaries[0].status, "open");
        assert_eq!(summaries[0].attempts, 1);
        assert_eq!(summaries[0].recent_outcome.as_deref(), Some("failed"));
        assert!(summaries[0]
            .summary
            .as_deref()
            .unwrap()
            .contains("Investigated parser edge case"));
        assert_eq!(
            summaries[0].follow_up.as_deref(),
            Some("1 unresolved decision(s)")
        );
    }

    #[test]
    fn summarize_child_units_falls_back_to_closed_status_when_no_attempts_exist() {
        let (_dir, mana_dir) = setup_test_env();

        let parent = Unit::new("2", "Parent");
        let parent_slug = crate::util::title_to_slug(&parent.title);
        parent
            .to_file(mana_dir.join(format!("2-{}.md", parent_slug)))
            .unwrap();

        let mut child = Unit::new("2.1", "Closed child");
        child.parent = Some("2".to_string());
        child.status = Status::Closed;
        child.close_reason = Some("Completed successfully after consolidation".to_string());
        let child_slug = crate::util::title_to_slug(&child.title);
        child
            .to_file(mana_dir.join(format!("2.1-{}.md", child_slug)))
            .unwrap();

        let summaries = summarize_child_units(&mana_dir, "2");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].recent_outcome.as_deref(), Some("success"));
        assert_eq!(summaries[0].attempts, 0);
        assert!(summaries[0]
            .summary
            .as_deref()
            .unwrap()
            .contains("Completed successfully"));
        assert!(summaries[0].follow_up.is_none());
    }

    #[test]
    fn assemble_agent_context_includes_child_summaries() {
        let (_dir, mana_dir) = setup_test_env();

        let mut parent = Unit::new("3", "Parent");
        parent.description = Some("Review child outputs".to_string());
        let parent_slug = crate::util::title_to_slug(&parent.title);
        parent
            .to_file(mana_dir.join(format!("3-{}.md", parent_slug)))
            .unwrap();

        let mut child = Unit::new("3.1", "Child");
        child.parent = Some("3".to_string());
        child.status = Status::Closed;
        child.close_reason = Some("Found root cause and fixed it".to_string());
        let child_slug = crate::util::title_to_slug(&child.title);
        child
            .to_file(mana_dir.join(format!("3.1-{}.md", child_slug)))
            .unwrap();

        let ctx = assemble_agent_context(&mana_dir, "3").unwrap();
        assert_eq!(ctx.child_summaries.len(), 1);
        assert_eq!(ctx.child_summaries[0].id, "3.1");
        assert_eq!(
            ctx.child_summaries[0].recent_outcome.as_deref(),
            Some("success")
        );
    }

    #[test]
    fn merge_paths_deduplicates() {
        let mut unit = Unit::new("1", "Test unit");
        unit.paths = vec!["src/main.rs".to_string()];
        unit.description = Some("Check src/main.rs and src/lib.rs".to_string());
        let paths = merge_paths(&unit);
        assert_eq!(paths, vec!["src/main.rs", "src/lib.rs"]);
    }

    #[test]
    fn extract_rust_structure_basic() {
        let content = "use std::io;\n\npub fn hello() {\n}\n\nstruct Foo {\n}\n";
        let result = extract_file_structure("test.rs", content).unwrap();
        assert!(result.contains("use std::io;"));
        assert!(result.contains("pub fn hello()"));
        assert!(result.contains("struct Foo"));
    }

    #[test]
    fn extract_ts_structure_basic() {
        let content = "import { foo } from 'bar';\n\nexport function hello() {\n}\n";
        let result = extract_file_structure("test.ts", content).unwrap();
        assert!(result.contains("import { foo } from 'bar';"));
        assert!(result.contains("export function hello()"));
    }

    #[test]
    fn extract_python_structure_basic() {
        let content = "import os\n\ndef hello():\n    pass\n";
        let result = extract_file_structure("test.py", content).unwrap();
        assert!(result.contains("import os"));
        assert!(result.contains("def hello()"));
    }
}
