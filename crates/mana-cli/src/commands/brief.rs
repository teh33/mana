use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::discovery::find_unit_file;
use crate::index::{Index, IndexEntry};
use crate::unit::{Status, Unit};
use crate::util::natural_cmp;

/// Print a concise, read-only operational brief for a unit subtree.
pub fn cmd_brief(mana_dir: &Path, id: &str, json: bool) -> Result<()> {
    let brief = if json {
        render_brief_json(mana_dir, id)?
    } else {
        render_brief(mana_dir, id)?
    };
    print!("{brief}");
    Ok(())
}

/// Render a concise, read-only operational brief for a unit subtree.
pub fn render_brief(mana_dir: &Path, id: &str) -> Result<String> {
    let brief = build_brief(mana_dir, id)?;
    Ok(format_brief(&brief))
}

/// Render a structured JSON operational brief for a unit subtree.
pub fn render_brief_json(mana_dir: &Path, id: &str) -> Result<String> {
    let brief = build_brief(mana_dir, id)?;
    serde_json::to_string_pretty(&BriefJson::from(&brief)).context("Failed to serialize brief JSON")
}

#[derive(Debug, Clone)]
struct Brief {
    root: Unit,
    children: Vec<IndexEntry>,
    open_work: Vec<IndexEntry>,
    in_progress_work: Vec<IndexEntry>,
    blocked_by_dependencies: Vec<DependencyBlock>,
    concerns: Vec<String>,
    next_actions: Vec<String>,
}

#[derive(Debug, Clone)]
struct DependencyBlock {
    unit: IndexEntry,
    open_dependencies: Vec<String>,
}

#[derive(Debug, Serialize)]
struct BriefJson {
    root: RootJson,
    goal: Option<String>,
    acceptance: Option<String>,
    progress: ProgressJson,
    active_work: Vec<WorkJson>,
    open_work: Vec<WorkJson>,
    dependency_blockers: Vec<DependencyBlockJson>,
    concerns: Vec<String>,
    next_actions: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RootJson {
    id: String,
    title: String,
    status: String,
    priority: u8,
    kind: String,
}

#[derive(Debug, Serialize)]
struct ProgressJson {
    closed: usize,
    in_progress: usize,
    open: usize,
    total_descendants: usize,
}

#[derive(Debug, Serialize)]
struct WorkJson {
    id: String,
    title: String,
    status: String,
    priority: u8,
    claimed_by: Option<String>,
}

#[derive(Debug, Serialize)]
struct DependencyBlockJson {
    id: String,
    title: String,
    open_dependencies: Vec<String>,
}

impl From<&IndexEntry> for WorkJson {
    fn from(entry: &IndexEntry) -> Self {
        Self {
            id: entry.id.clone(),
            title: entry.title.clone(),
            status: entry.status.to_string(),
            priority: entry.priority,
            claimed_by: entry.claimed_by.clone(),
        }
    }
}

impl From<&Brief> for BriefJson {
    fn from(brief: &Brief) -> Self {
        let closed = brief
            .children
            .iter()
            .filter(|entry| entry.status == Status::Closed)
            .count();

        Self {
            root: RootJson {
                id: brief.root.id.clone(),
                title: brief.root.title.clone(),
                status: brief.root.status.to_string(),
                priority: brief.root.priority,
                kind: format!("{:?}", brief.root.kind).to_lowercase(),
            },
            goal: first_meaningful_line(brief.root.description.as_deref()),
            acceptance: first_meaningful_line(brief.root.acceptance.as_deref()),
            progress: ProgressJson {
                closed,
                in_progress: brief.in_progress_work.len(),
                open: brief.open_work.len(),
                total_descendants: brief.children.len(),
            },
            active_work: brief.in_progress_work.iter().map(WorkJson::from).collect(),
            open_work: brief.open_work.iter().map(WorkJson::from).collect(),
            dependency_blockers: brief
                .blocked_by_dependencies
                .iter()
                .map(|block| DependencyBlockJson {
                    id: block.unit.id.clone(),
                    title: block.unit.title.clone(),
                    open_dependencies: block.open_dependencies.clone(),
                })
                .collect(),
            concerns: brief.concerns.clone(),
            next_actions: brief.next_actions.clone(),
        }
    }
}

fn build_brief(mana_dir: &Path, id: &str) -> Result<Brief> {
    let index = Index::load_or_rebuild(mana_dir)?;
    let root_path = find_unit_file(mana_dir, id).with_context(|| format!("Unit not found: {id}"))?;
    let root = Unit::from_file(&root_path).with_context(|| format!("Failed to load unit: {id}"))?;

    let descendant_ids = descendant_ids(&index, id);
    let mut children: Vec<_> = index
        .units
        .iter()
        .filter(|entry| descendant_ids.contains(&entry.id))
        .cloned()
        .collect();
    children.sort_by(|a, b| natural_cmp(&a.id, &b.id));

    let open_work: Vec<_> = children
        .iter()
        .filter(|entry| entry.status == Status::Open)
        .cloned()
        .collect();
    let in_progress_work: Vec<_> = children
        .iter()
        .filter(|entry| matches!(entry.status, Status::InProgress | Status::AwaitingVerify))
        .cloned()
        .collect();

    let blocked_by_dependencies = find_dependency_blocks(&index, &children);
    let concerns = infer_concerns(&root, &children, &blocked_by_dependencies);
    let next_actions = recommend_next_actions(&open_work, &in_progress_work, &blocked_by_dependencies, &concerns);

    Ok(Brief {
        root,
        children,
        open_work,
        in_progress_work,
        blocked_by_dependencies,
        concerns,
        next_actions,
    })
}

fn descendant_ids(index: &Index, root_id: &str) -> HashSet<String> {
    let mut ids = HashSet::new();
    let mut stack = vec![root_id.to_string()];

    while let Some(parent_id) = stack.pop() {
        for child in index
            .units
            .iter()
            .filter(|entry| entry.parent.as_deref() == Some(parent_id.as_str()))
        {
            if ids.insert(child.id.clone()) {
                stack.push(child.id.clone());
            }
        }
    }

    ids
}

fn find_dependency_blocks(index: &Index, entries: &[IndexEntry]) -> Vec<DependencyBlock> {
    entries
        .iter()
        .filter(|entry| entry.status != Status::Closed)
        .filter_map(|entry| {
            let open_dependencies: Vec<_> = entry
                .dependencies
                .iter()
                .filter_map(|dep_id| {
                    index
                        .units
                        .iter()
                        .find(|candidate| candidate.id == *dep_id)
                        .filter(|candidate| candidate.status != Status::Closed)
                        .map(|candidate| format!("{} ({})", candidate.id, candidate.status))
                })
                .collect();

            if open_dependencies.is_empty() {
                None
            } else {
                Some(DependencyBlock {
                    unit: entry.clone(),
                    open_dependencies,
                })
            }
        })
        .collect()
}

fn infer_concerns(
    root: &Unit,
    entries: &[IndexEntry],
    blocked_by_dependencies: &[DependencyBlock],
) -> Vec<String> {
    let mut concerns = Vec::new();

    if !root.decisions.is_empty() {
        concerns.push(format!(
            "{} unresolved decision(s) on root unit",
            root.decisions.len()
        ));
    }

    if root.verify.is_none() && root.status != Status::Closed && root.is_dispatchable_task() {
        concerns.push("root unit is dispatchable but has no verify gate".to_string());
    }

    let weak_verify = entries
        .iter()
        .filter(|entry| entry.status == Status::Closed && !entry.has_verify)
        .count();
    if weak_verify > 0 {
        concerns.push(format!(
            "{weak_verify} closed descendant(s) have no verify gate in the index"
        ));
    }

    let claimed = entries
        .iter()
        .filter(|entry| entry.claimed_by.is_some() && entry.status != Status::Closed)
        .count();
    if claimed > 0 {
        concerns.push(format!("{claimed} active descendant(s) are claimed/in progress"));
    }

    if !blocked_by_dependencies.is_empty() {
        concerns.push(format!(
            "{} descendant(s) are waiting on open dependencies",
            blocked_by_dependencies.len()
        ));
    }

    let retry_pressure = entries.iter().filter(|entry| entry.attempts >= 3).count();
    if retry_pressure > 0 {
        concerns.push(format!(
            "{retry_pressure} descendant(s) have reached at least 3 attempts"
        ));
    }

    concerns
}

fn recommend_next_actions(
    open_work: &[IndexEntry],
    in_progress_work: &[IndexEntry],
    blocked_by_dependencies: &[DependencyBlock],
    concerns: &[String],
) -> Vec<String> {
    let mut actions = Vec::new();

    if let Some(entry) = in_progress_work.first() {
        actions.push(format!(
            "Finish or unblock {} — it is already {}{}",
            entry.id,
            entry.status,
            entry
                .claimed_by
                .as_ref()
                .map(|claim| format!(" and claimed by {claim}"))
                .unwrap_or_default()
        ));
    }

    let blocked_ids: HashSet<_> = blocked_by_dependencies
        .iter()
        .map(|block| block.unit.id.as_str())
        .collect();
    if let Some(entry) = open_work
        .iter()
        .filter(|entry| !blocked_ids.contains(entry.id.as_str()))
        .min_by(|a, b| a.priority.cmp(&b.priority).then_with(|| natural_cmp(&a.id, &b.id)))
    {
        actions.push(format!(
            "Work {} next — highest-priority open descendant without open dependencies",
            entry.id
        ));
    }

    if let Some(block) = blocked_by_dependencies.first() {
        actions.push(format!(
            "Resolve dependencies for {} — waiting on {}",
            block.unit.id,
            block.open_dependencies.join(", ")
        ));
    }

    if !concerns.is_empty() {
        actions.push("Review concerns before broadening scope or closing the parent".to_string());
    }

    if actions.is_empty() {
        actions.push("No active child work found; define the next executable slice or close the parent if acceptance is satisfied".to_string());
    }

    actions
}

fn first_meaningful_line(text: Option<&str>) -> Option<String> {
    text?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("---"))
        .map(|line| line.trim_start_matches("Goal:").trim().to_string())
}

fn format_brief(brief: &Brief) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "{} {} [{} P{}]\n",
        brief.root.id, brief.root.title, brief.root.status, brief.root.priority
    ));

    if let Some(goal) = first_meaningful_line(brief.root.description.as_deref()) {
        output.push_str(&format!("Goal: {goal}\n"));
    }
    if let Some(acceptance) = first_meaningful_line(brief.root.acceptance.as_deref()) {
        output.push_str(&format!("Acceptance: {acceptance}\n"));
    }

    let closed = brief
        .children
        .iter()
        .filter(|entry| entry.status == Status::Closed)
        .count();
    output.push_str(&format!(
        "Progress: {} closed, {} in progress, {} open descendants\n",
        closed,
        brief.in_progress_work.len(),
        brief.open_work.len()
    ));

    if !brief.in_progress_work.is_empty() {
        output.push_str("\nActive work:\n");
        for entry in brief.in_progress_work.iter().take(8) {
            output.push_str(&format!(
                "- {} {} [{}]{}\n",
                entry.id,
                entry.title,
                entry.status,
                entry
                    .claimed_by
                    .as_ref()
                    .map(|claim| format!(" claimed by {claim}"))
                    .unwrap_or_default()
            ));
        }
    }

    if !brief.open_work.is_empty() {
        output.push_str("\nOpen work:\n");
        for entry in brief.open_work.iter().take(10) {
            output.push_str(&format!("- {} {} [P{}]\n", entry.id, entry.title, entry.priority));
        }
        if brief.open_work.len() > 10 {
            output.push_str(&format!("- … {} more\n", brief.open_work.len() - 10));
        }
    }

    if !brief.blocked_by_dependencies.is_empty() {
        output.push_str("\nBlocked by dependencies:\n");
        for block in brief.blocked_by_dependencies.iter().take(8) {
            output.push_str(&format!(
                "- {} waits on {}\n",
                block.unit.id,
                block.open_dependencies.join(", ")
            ));
        }
    }

    if !brief.concerns.is_empty() {
        output.push_str("\nConcerns / risks:\n");
        for concern in &brief.concerns {
            output.push_str(&format!("- {concern}\n"));
        }
    }

    output.push_str("\nRecommended next actions:\n");
    for action in &brief.next_actions {
        output.push_str(&format!("- {action}\n"));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

    use crate::index::Index;
    use crate::unit::{Status, Unit, UnitType};

    fn unit(id: &str, title: &str, status: Status, parent: Option<&str>) -> Unit {
        let now = Utc::now();
        Unit {
            id: id.to_string(),
            title: title.to_string(),
            slug: None,
            status,
            priority: 1,
            created_at: now,
            updated_at: now,
            description: Some(format!("Goal: {title}")),
            acceptance: Some("Done when brief can summarize it.".to_string()),
            notes: None,
            design: None,
            labels: vec![],
            assignee: None,
            closed_at: None,
            close_reason: None,
            parent: parent.map(str::to_string),
            dependencies: vec![],
            verify: Some("true".to_string()),
            verify_fast: None,
            fail_first: false,
            checkpoint: None,
            verify_hash: None,
            attempts: 0,
            max_attempts: 3,
            claimed_by: None,
            claimed_at: None,
            is_archived: false,
            produces: vec![],
            requires: vec![],
            on_fail: None,
            on_close: vec![],
            history: vec![],
            outputs: None,
            max_loops: None,
            verify_timeout: None,
            kind: UnitType::Task,
            unit_type: "task".to_string(),
            last_verified: None,
            stale_after: None,
            paths: vec![],
            attempt_log: vec![],
            created_by: None,
            feature: false,
            decisions: vec![],
            autonomy_disposition: None,
            model: None,
        }
    }

    #[test]
    fn brief_collects_descendants_and_recommends_in_progress_first() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        let root = unit("1", "Root", Status::Open, None);
        let mut child = unit("1.1", "Child", Status::InProgress, Some("1"));
        child.claimed_by = Some("imp".to_string());
        let open = unit("1.2", "Open", Status::Open, Some("1"));

        root.to_file(&mana_dir.join("1-root.md")).unwrap();
        child.to_file(&mana_dir.join("1.1-child.md")).unwrap();
        open.to_file(&mana_dir.join("1.2-open.md")).unwrap();
        Index::build(mana_dir).unwrap().save(mana_dir).unwrap();

        let brief = build_brief(mana_dir, "1").unwrap();
        assert_eq!(brief.children.len(), 2);
        assert_eq!(brief.in_progress_work.len(), 1);
        assert_eq!(brief.open_work.len(), 1);
        assert!(brief.next_actions[0].contains("Finish or unblock 1.1"));
    }

    #[test]
    fn brief_reports_open_dependency_blocks() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        let root = unit("1", "Root", Status::Open, None);
        let dep = unit("1.1", "Dependency", Status::Open, Some("1"));
        let mut blocked = unit("1.2", "Blocked", Status::Open, Some("1"));
        blocked.dependencies = vec!["1.1".to_string()];

        root.to_file(&mana_dir.join("1-root.md")).unwrap();
        dep.to_file(&mana_dir.join("1.1-dependency.md")).unwrap();
        blocked.to_file(&mana_dir.join("1.2-blocked.md")).unwrap();
        Index::build(mana_dir).unwrap().save(mana_dir).unwrap();

        let brief = build_brief(mana_dir, "1").unwrap();
        assert_eq!(brief.blocked_by_dependencies.len(), 1);
        assert!(brief.concerns.iter().any(|concern| concern.contains("waiting on open dependencies")));
    }
}
