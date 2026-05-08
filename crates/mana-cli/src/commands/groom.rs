use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::discovery::find_unit_file;
use crate::index::{Index, IndexEntry};
use crate::unit::{Status, Unit};

pub fn cmd_groom(mana_dir: &Path, id: &str, dry_run: bool, json: bool) -> Result<()> {
    if !dry_run {
        anyhow::bail!("mana groom is proposal-only for now; rerun with --dry-run");
    }

    let report = build_groom_report(mana_dir, id)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_groom_report(&report);
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct GroomReport {
    root: GroomRoot,
    proposals: Vec<GroomProposal>,
}

#[derive(Debug, Serialize)]
struct GroomRoot {
    id: String,
    title: String,
    status: String,
}

#[derive(Debug, Serialize)]
struct GroomProposal {
    kind: String,
    unit_id: Option<String>,
    title: Option<String>,
    reason: String,
    recommended_action: String,
    confidence: String,
    requires_human_approval: bool,
    imp_action: String,
    suggested_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    related_units: Vec<RelatedUnit>,
}

#[derive(Debug, Clone, Serialize)]
struct RelatedUnit {
    id: String,
    title: String,
    status: String,
    reason: String,
    score: u32,
}

fn build_groom_report(mana_dir: &Path, id: &str) -> Result<GroomReport> {
    let index = Index::load_or_rebuild(mana_dir)?;
    let root_path =
        find_unit_file(mana_dir, id).with_context(|| format!("Unit not found: {id}"))?;
    let root = Unit::from_file(&root_path).with_context(|| format!("Failed to load unit: {id}"))?;
    let descendants = descendants(&index, id);
    let mut proposals = Vec::new();

    if !root.decisions.is_empty() {
        proposals.push(GroomProposal {
            kind: "triage_decisions".to_string(),
            unit_id: Some(root.id.clone()),
            title: Some(root.title.clone()),
            reason: format!("root unit has {} unresolved decision(s)", root.decisions.len()),
            recommended_action: "Review root decisions; resolve accepted/historical entries or convert real blockers into focused decision tasks.".to_string(),
            confidence: "high".to_string(),
            requires_human_approval: true,
            imp_action: imp_action_for_kind("triage_decisions").to_string(),
            suggested_commands: suggested_commands_for_kind("triage_decisions", &root.id),
            related_units: Vec::new(),
        });
    }

    for entry in &descendants {
        match entry.status {
            Status::Open => {
                if let Some(reason) = stale_reason_for_entry(mana_dir, entry) {
                    proposals.push(GroomProposal {
                        kind: "review_stale_open_work".to_string(),
                        unit_id: Some(entry.id.clone()),
                        title: Some(entry.title.clone()),
                        reason,
                        recommended_action: "Review whether this unit should be marked superseded, rewritten, reparented, or closed with an explicit replacement.".to_string(),
                        confidence: "medium".to_string(),
                        requires_human_approval: true,
                        imp_action: imp_action_for_kind("review_stale_open_work").to_string(),
                        suggested_commands: suggested_commands_for_kind("review_stale_open_work", &entry.id),
                        related_units: related_units_for_stale_entry(entry, &descendants),
                    });
                }

                let open_deps: Vec<_> = entry
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
                if !open_deps.is_empty() {
                    proposals.push(GroomProposal {
                        kind: "resolve_dependency_blocker".to_string(),
                        unit_id: Some(entry.id.clone()),
                        title: Some(entry.title.clone()),
                        reason: format!("waits on open dependencies: {}", open_deps.join(", ")),
                        recommended_action: "Prioritize the dependency, or review whether the blocked work is stale/superseded.".to_string(),
                        confidence: "high".to_string(),
                        requires_human_approval: false,
                        imp_action: imp_action_for_kind("resolve_dependency_blocker").to_string(),
                        suggested_commands: suggested_commands_for_kind("resolve_dependency_blocker", &entry.id),
                        related_units: Vec::new(),
                    });
                }
            }
            Status::InProgress | Status::AwaitingVerify => {
                proposals.push(GroomProposal {
                    kind: "finish_or_release_claim".to_string(),
                    unit_id: Some(entry.id.clone()),
                    title: Some(entry.title.clone()),
                    reason: entry
                        .claimed_by
                        .as_ref()
                        .map(|claim| {
                            format!("active work is {} and claimed by {claim}", entry.status)
                        })
                        .unwrap_or_else(|| format!("active work is {}", entry.status)),
                    recommended_action:
                        "Finish, block with evidence, or release the claim if work has stopped."
                            .to_string(),
                    confidence: "high".to_string(),
                    requires_human_approval: false,
                    imp_action: imp_action_for_kind("finish_or_release_claim").to_string(),
                    suggested_commands: suggested_commands_for_kind(
                        "finish_or_release_claim",
                        &entry.id,
                    ),
                    related_units: Vec::new(),
                });
            }
            Status::Closed => {
                if let Some(reason) = concern_reason_for_entry(mana_dir, entry) {
                    proposals.push(GroomProposal {
                        kind: "review_closed_with_concerns".to_string(),
                        unit_id: Some(entry.id.clone()),
                        title: Some(entry.title.clone()),
                        reason,
                        recommended_action: "Confirm whether the concern is resolved; create a follow-up task or annotate the resolution.".to_string(),
                        confidence: "medium".to_string(),
                        requires_human_approval: true,
                        imp_action: imp_action_for_kind("triage_decisions").to_string(),
                        suggested_commands: suggested_commands_for_kind("triage_decisions", &root.id),
                        related_units: Vec::new(),
                    });
                }
            }
        }
    }

    Ok(GroomReport {
        root: GroomRoot {
            id: root.id,
            title: root.title,
            status: root.status.to_string(),
        },
        proposals,
    })
}

fn descendants(index: &Index, root_id: &str) -> Vec<IndexEntry> {
    let mut ids = std::collections::HashSet::new();
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

    let mut entries: Vec<_> = index
        .units
        .iter()
        .filter(|entry| ids.contains(&entry.id))
        .cloned()
        .collect();
    entries.sort_by(|a, b| crate::util::natural_cmp(&a.id, &b.id));
    entries
}

fn imp_action_for_kind(kind: &str) -> &'static str {
    match kind {
        "triage_decisions" => "pm_triage_decisions",
        "review_stale_open_work" => "pm_review_stale_work",
        "resolve_dependency_blocker" => "pm_resolve_dependency_blocker",
        "finish_or_release_claim" => "worker_finish_or_release_claim",
        "review_closed_with_concerns" => "review_completion_concerns",
        _ => "pm_review_proposal",
    }
}

fn suggested_commands_for_kind(kind: &str, unit_id: &str) -> Vec<String> {
    match kind {
        "triage_decisions" => vec![
            format!("mana show {unit_id}"),
            format!("mana brief {unit_id} --json"),
        ],
        "review_stale_open_work" => vec![
            format!("mana show {unit_id}"),
            format!("mana tree {unit_id}"),
            format!("mana groom {unit_id} --dry-run --json"),
        ],
        "resolve_dependency_blocker" => vec![
            format!("mana show {unit_id}"),
            format!("mana deps {unit_id}"),
        ],
        "finish_or_release_claim" => vec![
            format!("mana show {unit_id}"),
            format!("mana verify {unit_id}"),
        ],
        "review_closed_with_concerns" => vec![
            format!("mana show {unit_id}"),
            format!("mana verify {unit_id}"),
        ],
        _ => vec![format!("mana show {unit_id}")],
    }
}
fn related_units_for_stale_entry(
    entry: &IndexEntry,
    candidates: &[IndexEntry],
) -> Vec<RelatedUnit> {
    let entry_tokens = title_tokens(&entry.title);
    let mut scored: Vec<_> = candidates
        .iter()
        .filter(|candidate| candidate.id != entry.id)
        .filter_map(|candidate| {
            let mut score = 0;
            let mut reasons = Vec::new();

            if candidate.parent == entry.parent && candidate.parent.is_some() {
                score += 4;
                reasons.push("same parent".to_string());
            }
            if candidate.id.starts_with(&format!("{}.", entry.id)) {
                score += 5;
                reasons.push("descendant of stale unit".to_string());
            }
            if candidate.updated_at > entry.updated_at || candidate.created_at > entry.created_at {
                score += 2;
                reasons.push("newer unit".to_string());
            }
            let shared_labels: Vec<_> = candidate
                .labels
                .iter()
                .filter(|label| entry.labels.contains(*label))
                .cloned()
                .collect();
            if !shared_labels.is_empty() {
                score += 2 + shared_labels.len() as u32;
                reasons.push(format!("shared label(s): {}", shared_labels.join(", ")));
            }
            let overlap = title_tokens(&candidate.title)
                .intersection(&entry_tokens)
                .count();
            if overlap > 0 {
                score += overlap as u32;
                reasons.push(format!("{overlap} shared title token(s)"));
            }
            if matches!(
                candidate.status,
                Status::Closed | Status::InProgress | Status::AwaitingVerify
            ) && candidate.has_verify
            {
                score += 2;
                reasons.push("concrete verified/active task".to_string());
            }

            (score > 0).then(|| RelatedUnit {
                id: candidate.id.clone(),
                title: candidate.title.clone(),
                status: candidate.status.to_string(),
                reason: reasons.join("; "),
                score,
            })
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| crate::util::natural_cmp(&a.id, &b.id))
    });
    scored.truncate(5);
    scored
}

fn title_tokens(title: &str) -> std::collections::HashSet<String> {
    const STOP_WORDS: &[&str] = &[
        "a",
        "an",
        "and",
        "or",
        "the",
        "to",
        "of",
        "for",
        "with",
        "in",
        "on",
        "by",
        "into",
        "add",
        "build",
        "create",
        "define",
        "implement",
        "make",
        "plan",
        "task",
        "unit",
    ];

    title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_lowercase)
        .filter(|token| token.len() > 2 && !STOP_WORDS.contains(&token.as_str()))
        .collect()
}

fn stale_reason_for_entry(mana_dir: &Path, entry: &IndexEntry) -> Option<String> {
    let mut haystack = entry.title.clone();
    if let Ok(path) = find_unit_file(mana_dir, &entry.id) {
        if let Ok(unit) = Unit::from_file(&path) {
            append_optional_text(&mut haystack, unit.description.as_deref());
            append_optional_text(&mut haystack, unit.notes.as_deref());
        }
    }
    keyword_reason(
        &haystack,
        &[
            ("superseded", "mentions superseded"),
            ("stale", "mentions stale"),
            ("do not execute", "warns not to execute"),
            ("scope changed", "mentions scope changed"),
            ("needs revision", "mentions needs revision"),
        ],
    )
}

fn concern_reason_for_entry(mana_dir: &Path, entry: &IndexEntry) -> Option<String> {
    let path = find_unit_file(mana_dir, &entry.id).ok()?;
    let unit = Unit::from_file(&path).ok()?;

    if let Some(reason) = unit
        .close_reason
        .as_deref()
        .and_then(strong_completion_concern_reason)
    {
        return Some(reason);
    }

    unit.notes
        .as_deref()
        .and_then(strong_completion_concern_reason)
}

fn strong_completion_concern_reason(text: &str) -> Option<String> {
    keyword_reason(
        text,
        &[
            ("done with concern", "mentions done with concern"),
            ("with concern", "mentions concern"),
            ("concern:", "mentions concern"),
            ("blocked", "mentions blocked"),
            ("partial", "mentions partial"),
            ("unverified", "mentions unverified"),
            ("not verified", "mentions not verified"),
            ("could not", "mentions could not"),
            ("failed", "mentions failed"),
            ("warning", "mentions warning"),
            ("unable", "mentions unable"),
            ("follow-up required", "mentions follow-up required"),
        ],
    )
}

fn keyword_reason(text: &str, keywords: &[(&str, &str)]) -> Option<String> {
    let normalized = text.to_lowercase();
    keywords
        .iter()
        .find(|(needle, _)| normalized.contains(*needle))
        .map(|(_, reason)| (*reason).to_string())
}

fn append_optional_text(haystack: &mut String, text: Option<&str>) {
    if let Some(text) = text {
        haystack.push('\n');
        haystack.push_str(text);
    }
}

fn print_groom_report(report: &GroomReport) {
    println!(
        "Grooming proposals for {} {} [{}]:",
        report.root.id, report.root.title, report.root.status
    );

    if report.proposals.is_empty() {
        println!("No grooming proposals found.");
        return;
    }

    for (index, proposal) in report.proposals.iter().enumerate() {
        println!("\n{}. {}", index + 1, proposal.kind.replace('_', " "));
        if let Some(unit_id) = &proposal.unit_id {
            match &proposal.title {
                Some(title) => println!("   - Unit: {unit_id} {title}"),
                None => println!("   - Unit: {unit_id}"),
            }
        }
        println!("   - Reason: {}", proposal.reason);
        println!("   - Recommended action: {}", proposal.recommended_action);
        println!("   - Confidence: {}", proposal.confidence);
        println!(
            "   - Requires human approval: {}",
            proposal.requires_human_approval
        );
        println!("   - Imp action: {}", proposal.imp_action);
        if !proposal.suggested_commands.is_empty() {
            println!("   - Suggested commands:");
            for command in &proposal.suggested_commands {
                println!("     - {command}");
            }
        }
        if !proposal.related_units.is_empty() {
            println!("   - Possible replacements:");
            for related in &proposal.related_units {
                println!(
                    "     - {} {} [{}] — {}",
                    related.id, related.title, related.status, related.reason
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::TempDir;

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
            acceptance: Some("Done when groom can propose cleanup.".to_string()),
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
    fn groom_generates_proposals_from_management_signals() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        let mut root = unit("1", "Root", Status::Open, None);
        root.decisions = vec!["Resolve mode".to_string()];
        let mut stale = unit("1.1", "Stale task", Status::Open, Some("1"));
        stale.notes = Some("Superseded by newer slice.".to_string());
        let dep = unit("1.2", "Dependency", Status::Open, Some("1"));
        let mut blocked = unit("1.3", "Blocked", Status::Open, Some("1"));
        blocked.dependencies = vec!["1.2".to_string()];
        let mut active = unit("1.4", "Active", Status::InProgress, Some("1"));
        active.claimed_by = Some("imp".to_string());
        let mut closed = unit("1.5", "Closed", Status::Closed, Some("1"));
        closed.close_reason = Some("Partial completion with concern.".to_string());

        for unit in [&root, &stale, &dep, &blocked, &active, &closed] {
            unit.to_file(&mana_dir.join(format!("{}-unit.md", unit.id)))
                .unwrap();
        }
        Index::build(mana_dir).unwrap().save(mana_dir).unwrap();

        let report = build_groom_report(mana_dir, "1").unwrap();
        let kinds: Vec<_> = report
            .proposals
            .iter()
            .map(|proposal| proposal.kind.as_str())
            .collect();
        assert!(kinds.contains(&"triage_decisions"));
        assert!(kinds.contains(&"review_stale_open_work"));
        assert!(kinds.contains(&"resolve_dependency_blocker"));
        assert!(kinds.contains(&"finish_or_release_claim"));
        assert!(kinds.contains(&"review_closed_with_concerns"));
        assert!(report
            .proposals
            .iter()
            .all(|proposal| !proposal.imp_action.is_empty()
                && !proposal.suggested_commands.is_empty()));
    }
    #[test]
    fn groom_does_not_flag_strategic_deferred_language_as_concern() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        let root = unit("1", "Root", Status::Open, None);
        let mut closed = unit(
            "1.1",
            "Closed strategic deferral",
            Status::Closed,
            Some("1"),
        );
        closed.close_reason =
            Some("Documented future/deferred hosted backend path; decision accepted.".to_string());

        for unit in [&root, &closed] {
            unit.to_file(&mana_dir.join(format!("{}-unit.md", unit.id)))
                .unwrap();
        }
        Index::build(mana_dir).unwrap().save(mana_dir).unwrap();

        let report = build_groom_report(mana_dir, "1").unwrap();
        assert!(!report
            .proposals
            .iter()
            .any(|proposal| proposal.kind == "review_closed_with_concerns"));
    }

    #[test]
    fn groom_flags_strong_closed_completion_concerns() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        let root = unit("1", "Root", Status::Open, None);
        let mut closed = unit("1.1", "Closed partial", Status::Closed, Some("1"));
        closed.close_reason =
            Some("Partial completion; verify was not verified on this machine.".to_string());

        for unit in [&root, &closed] {
            unit.to_file(&mana_dir.join(format!("{}-unit.md", unit.id)))
                .unwrap();
        }
        Index::build(mana_dir).unwrap().save(mana_dir).unwrap();

        let report = build_groom_report(mana_dir, "1").unwrap();
        assert!(report
            .proposals
            .iter()
            .any(|proposal| proposal.kind == "review_closed_with_concerns"));
    }
    #[test]
    fn groom_adds_related_units_to_stale_work_proposals() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        let root = unit("1", "Root", Status::Open, None);
        let mut stale = unit(
            "1.1",
            "Implement API contract strategy",
            Status::Open,
            Some("1"),
        );
        stale.notes = Some("Superseded by newer implementation slices.".to_string());
        stale.labels = vec!["api".to_string()];
        let mut related = unit(
            "1.2",
            "Implement API client contract",
            Status::Closed,
            Some("1"),
        );
        related.labels = vec!["api".to_string()];
        let unrelated = unit("1.3", "Write wine list", Status::Open, Some("1"));

        for unit in [&root, &stale, &related, &unrelated] {
            unit.to_file(&mana_dir.join(format!("{}-unit.md", unit.id)))
                .unwrap();
        }
        Index::build(mana_dir).unwrap().save(mana_dir).unwrap();

        let report = build_groom_report(mana_dir, "1").unwrap();
        let stale_proposal = report
            .proposals
            .iter()
            .find(|proposal| proposal.kind == "review_stale_open_work")
            .unwrap();
        assert_eq!(stale_proposal.related_units[0].id, "1.2");
        assert!(stale_proposal.related_units.len() <= 5);
    }
}
