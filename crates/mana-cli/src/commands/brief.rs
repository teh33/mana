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
    scope: ScopeSummary,
    current_truth: CurrentTruthSummary,
    management_signals: ManagementSignals,
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
    scope: ScopeJson,
    current_truth: CurrentTruthJson,
    management_signals: ManagementSignalsJson,
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

#[derive(Debug, Clone, Default)]
struct ScopeSummary {
    included: Vec<String>,
    excluded: Vec<String>,
    deferred: Vec<String>,
}

impl ScopeSummary {
    fn is_empty(&self) -> bool {
        self.included.is_empty() && self.excluded.is_empty() && self.deferred.is_empty()
    }
}

#[derive(Debug, Serialize)]
struct ScopeJson {
    included: Vec<String>,
    excluded: Vec<String>,
    deferred: Vec<String>,
}

impl From<&ScopeSummary> for ScopeJson {
    fn from(scope: &ScopeSummary) -> Self {
        Self {
            included: scope.included.clone(),
            excluded: scope.excluded.clone(),
            deferred: scope.deferred.clone(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct CurrentTruthSummary {
    product_identity: Vec<String>,
    stack: Vec<String>,
    deployment: Vec<String>,
    principles: Vec<String>,
    constraints: Vec<String>,
    current_state: Vec<String>,
    non_goals: Vec<String>,
}

impl CurrentTruthSummary {
    fn is_empty(&self) -> bool {
        self.product_identity.is_empty()
            && self.stack.is_empty()
            && self.deployment.is_empty()
            && self.principles.is_empty()
            && self.constraints.is_empty()
            && self.current_state.is_empty()
            && self.non_goals.is_empty()
    }
}

#[derive(Debug, Serialize)]
struct CurrentTruthJson {
    product_identity: Vec<String>,
    stack: Vec<String>,
    deployment: Vec<String>,
    principles: Vec<String>,
    constraints: Vec<String>,
    current_state: Vec<String>,
    non_goals: Vec<String>,
}

impl From<&CurrentTruthSummary> for CurrentTruthJson {
    fn from(current_truth: &CurrentTruthSummary) -> Self {
        Self {
            product_identity: current_truth.product_identity.clone(),
            stack: current_truth.stack.clone(),
            deployment: current_truth.deployment.clone(),
            principles: current_truth.principles.clone(),
            constraints: current_truth.constraints.clone(),
            current_state: current_truth.current_state.clone(),
            non_goals: current_truth.non_goals.clone(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ManagementSignals {
    unresolved_decisions: usize,
    likely_stale_open_work: Vec<WorkJsonSource>,
    closed_with_concerns: Vec<WorkJsonSource>,
    claimed_work: Vec<WorkJsonSource>,
    dependency_blockers: usize,
}

impl ManagementSignals {
    fn is_empty(&self) -> bool {
        self.unresolved_decisions == 0
            && self.likely_stale_open_work.is_empty()
            && self.closed_with_concerns.is_empty()
            && self.claimed_work.is_empty()
            && self.dependency_blockers == 0
    }
}

#[derive(Debug, Clone)]
struct WorkJsonSource {
    entry: IndexEntry,
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct ManagementSignalsJson {
    unresolved_decisions: usize,
    likely_stale_open_work: Vec<SignalWorkJson>,
    closed_with_concerns: Vec<SignalWorkJson>,
    claimed_work: Vec<SignalWorkJson>,
    dependency_blockers: usize,
}

#[derive(Debug, Serialize)]
struct SignalWorkJson {
    id: String,
    title: String,
    status: String,
    priority: u8,
    claimed_by: Option<String>,
    reason: Option<String>,
}

impl From<&WorkJsonSource> for SignalWorkJson {
    fn from(source: &WorkJsonSource) -> Self {
        Self {
            id: source.entry.id.clone(),
            title: source.entry.title.clone(),
            status: source.entry.status.to_string(),
            priority: source.entry.priority,
            claimed_by: source.entry.claimed_by.clone(),
            reason: source.reason.clone(),
        }
    }
}

impl From<&ManagementSignals> for ManagementSignalsJson {
    fn from(signals: &ManagementSignals) -> Self {
        Self {
            unresolved_decisions: signals.unresolved_decisions,
            likely_stale_open_work: signals
                .likely_stale_open_work
                .iter()
                .map(SignalWorkJson::from)
                .collect(),
            closed_with_concerns: signals
                .closed_with_concerns
                .iter()
                .map(SignalWorkJson::from)
                .collect(),
            claimed_work: signals
                .claimed_work
                .iter()
                .map(SignalWorkJson::from)
                .collect(),
            dependency_blockers: signals.dependency_blockers,
        }
    }
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
            scope: ScopeJson::from(&brief.scope),
            current_truth: CurrentTruthJson::from(&brief.current_truth),
            management_signals: ManagementSignalsJson::from(&brief.management_signals),
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
    let root_path =
        find_unit_file(mana_dir, id).with_context(|| format!("Unit not found: {id}"))?;
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

    let scope = extract_scope(&root);
    let current_truth = extract_current_truth(&root);
    let management_signals =
        infer_management_signals(mana_dir, &root, &children, &blocked_by_dependencies);
    let next_actions = recommend_next_actions(
        &open_work,
        &in_progress_work,
        &blocked_by_dependencies,
        &concerns,
        &management_signals,
    );

    Ok(Brief {
        root,
        children,
        open_work,
        in_progress_work,
        blocked_by_dependencies,
        concerns,
        next_actions,
        scope,
        current_truth,
        management_signals,
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
        concerns.push(format!(
            "{claimed} active descendant(s) are claimed/in progress"
        ));
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
    management_signals: &ManagementSignals,
) -> Vec<String> {
    let mut actions = Vec::new();

    if management_signals.unresolved_decisions > 0 {
        actions.push(format!(
            "Triage {} unresolved root decision(s) before broad orchestration",
            management_signals.unresolved_decisions
        ));
    }

    if !management_signals.likely_stale_open_work.is_empty() {
        actions.push(format!(
            "Groom {} likely stale/superseded open descendant(s) before assigning new workers",
            management_signals.likely_stale_open_work.len()
        ));
    }

    if !management_signals.closed_with_concerns.is_empty() {
        actions.push(format!(
            "Review {} closed descendant(s) with concern-like completion evidence and create follow-ups if needed",
            management_signals.closed_with_concerns.len()
        ));
    }

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
    let stale_ids: HashSet<_> = management_signals
        .likely_stale_open_work
        .iter()
        .map(|source| source.entry.id.as_str())
        .collect();
    if let Some(entry) = open_work
        .iter()
        .filter(|entry| !blocked_ids.contains(entry.id.as_str()))
        .filter(|entry| !stale_ids.contains(entry.id.as_str()))
        .min_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| natural_cmp(&a.id, &b.id))
        })
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

fn infer_management_signals(
    mana_dir: &Path,
    root: &Unit,
    entries: &[IndexEntry],
    blocked_by_dependencies: &[DependencyBlock],
) -> ManagementSignals {
    let likely_stale_open_work = entries
        .iter()
        .filter(|entry| entry.status == Status::Open)
        .filter_map(|entry| stale_signal_for_entry(mana_dir, entry))
        .collect();

    let closed_with_concerns = entries
        .iter()
        .filter(|entry| entry.status == Status::Closed)
        .filter_map(|entry| closed_concern_signal_for_entry(mana_dir, entry))
        .collect();

    let claimed_work = entries
        .iter()
        .filter(|entry| {
            entry.claimed_by.is_some()
                || matches!(entry.status, Status::InProgress | Status::AwaitingVerify)
        })
        .map(|entry| WorkJsonSource {
            entry: entry.clone(),
            reason: entry
                .claimed_by
                .as_ref()
                .map(|claim| format!("claimed by {claim}")),
        })
        .collect();

    ManagementSignals {
        unresolved_decisions: root.decisions.len(),
        likely_stale_open_work,
        closed_with_concerns,
        claimed_work,
        dependency_blockers: blocked_by_dependencies.len(),
    }
}

fn stale_signal_for_entry(mana_dir: &Path, entry: &IndexEntry) -> Option<WorkJsonSource> {
    let mut haystack = entry.title.clone();
    if let Ok(path) = find_unit_file(mana_dir, &entry.id) {
        if let Ok(unit) = Unit::from_file(&path) {
            append_optional_text(&mut haystack, unit.description.as_deref());
            append_optional_text(&mut haystack, unit.notes.as_deref());
            append_optional_text(&mut haystack, unit.close_reason.as_deref());
        }
    }

    stale_reason(&haystack).map(|reason| WorkJsonSource {
        entry: entry.clone(),
        reason: Some(reason.to_string()),
    })
}

fn closed_concern_signal_for_entry(mana_dir: &Path, entry: &IndexEntry) -> Option<WorkJsonSource> {
    let path = find_unit_file(mana_dir, &entry.id).ok()?;
    let unit = Unit::from_file(&path).ok()?;
    let mut haystack = String::new();
    append_optional_text(&mut haystack, unit.close_reason.as_deref());
    append_optional_text(&mut haystack, unit.notes.as_deref());

    concern_reason(&haystack).map(|reason| WorkJsonSource {
        entry: entry.clone(),
        reason: Some(reason.to_string()),
    })
}

fn append_optional_text(haystack: &mut String, text: Option<&str>) {
    if let Some(text) = text {
        haystack.push('\n');
        haystack.push_str(text);
    }
}

fn stale_reason(text: &str) -> Option<&'static str> {
    let normalized = text.to_lowercase();
    for (needle, reason) in [
        ("superseded", "mentions superseded"),
        ("stale", "mentions stale"),
        ("do not execute", "warns not to execute"),
        ("scope changed", "mentions scope changed"),
        ("needs revision", "mentions needs revision"),
    ] {
        if normalized.contains(needle) {
            return Some(reason);
        }
    }
    None
}

fn concern_reason(text: &str) -> Option<&'static str> {
    let normalized = text.to_lowercase();
    for (needle, reason) in [
        ("concern", "mentions concern"),
        ("blocked", "mentions blocked"),
        ("partial", "mentions partial"),
        ("deferred", "mentions deferred"),
        ("unverified", "mentions unverified"),
        ("could not", "mentions could not"),
    ] {
        if normalized.contains(needle) {
            return Some(reason);
        }
    }
    None
}

fn extract_current_truth(root: &Unit) -> CurrentTruthSummary {
    let mut current_truth = CurrentTruthSummary::default();
    for text in [
        root.description.as_deref(),
        root.acceptance.as_deref(),
        root.notes.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        extract_current_truth_from_text(text, &mut current_truth);
    }
    current_truth
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CurrentTruthBucket {
    ProductIdentity,
    Stack,
    Deployment,
    Principles,
    Constraints,
    CurrentState,
    NonGoals,
}

fn extract_current_truth_from_text(text: &str, current_truth: &mut CurrentTruthSummary) {
    let mut bucket: Option<CurrentTruthBucket> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(next_bucket) = recognized_current_truth_heading(line) {
            bucket = Some(next_bucket);
            continue;
        }

        if is_markdown_heading(line) && bucket.is_some() {
            bucket = None;
            continue;
        }

        if let Some(current_bucket) = bucket {
            if let Some(item) = bullet_item(line) {
                push_current_truth_item(current_truth, current_bucket, item);
            }
        }
    }
}

fn recognized_current_truth_heading(line: &str) -> Option<CurrentTruthBucket> {
    let normalized = normalized_heading(line);

    match normalized.as_str() {
        "product identity" | "product" | "identity" => Some(CurrentTruthBucket::ProductIdentity),
        "settled stack" | "stack" | "technology stack" | "tech stack" => {
            Some(CurrentTruthBucket::Stack)
        }
        "deployment default" | "deployment" | "default deployment" => {
            Some(CurrentTruthBucket::Deployment)
        }
        "core principle" | "core principles" | "principle" | "principles" => {
            Some(CurrentTruthBucket::Principles)
        }
        "confirmed constraints" | "constraints" | "requirements" => {
            Some(CurrentTruthBucket::Constraints)
        }
        "current state" | "state" | "status" => Some(CurrentTruthBucket::CurrentState),
        "out of scope" | "non-goals" | "non goals" | "excluded" | "not v1" | "not in v1" => {
            Some(CurrentTruthBucket::NonGoals)
        }
        _ => None,
    }
}

fn push_current_truth_item(
    current_truth: &mut CurrentTruthSummary,
    bucket: CurrentTruthBucket,
    item: String,
) {
    let target = match bucket {
        CurrentTruthBucket::ProductIdentity => &mut current_truth.product_identity,
        CurrentTruthBucket::Stack => &mut current_truth.stack,
        CurrentTruthBucket::Deployment => &mut current_truth.deployment,
        CurrentTruthBucket::Principles => &mut current_truth.principles,
        CurrentTruthBucket::Constraints => &mut current_truth.constraints,
        CurrentTruthBucket::CurrentState => &mut current_truth.current_state,
        CurrentTruthBucket::NonGoals => &mut current_truth.non_goals,
    };

    if !target.contains(&item) {
        target.push(item);
    }
}

fn extract_scope(root: &Unit) -> ScopeSummary {
    let mut scope = ScopeSummary::default();
    for text in [
        root.description.as_deref(),
        root.acceptance.as_deref(),
        root.notes.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        extract_scope_from_text(text, &mut scope);
    }
    scope
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScopeBucket {
    Included,
    Excluded,
    Deferred,
}

fn extract_scope_from_text(text: &str, scope: &mut ScopeSummary) {
    let mut bucket: Option<ScopeBucket> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(next_bucket) = recognized_scope_heading(line) {
            bucket = Some(next_bucket);
            continue;
        }

        if is_markdown_heading(line) && bucket.is_some() {
            bucket = None;
            continue;
        }

        if let Some(current_bucket) = bucket {
            if let Some(item) = bullet_item(line) {
                push_scope_item(scope, current_bucket, item);
            }
        }
    }
}

fn recognized_scope_heading(line: &str) -> Option<ScopeBucket> {
    let normalized = normalized_heading(line);

    match normalized.as_str() {
        "scope" | "current scope" | "in scope" | "included" | "v1 scope" => {
            Some(ScopeBucket::Included)
        }
        "out of scope" | "non-goals" | "non goals" | "excluded" | "not v1" | "not in v1" => {
            Some(ScopeBucket::Excluded)
        }
        "deferred" | "future" | "later" | "future path" | "future paths" => {
            Some(ScopeBucket::Deferred)
        }
        _ => None,
    }
}

fn normalized_heading(line: &str) -> String {
    line.trim_start_matches('#')
        .trim()
        .trim_end_matches(':')
        .to_lowercase()
}

fn is_markdown_heading(line: &str) -> bool {
    line.starts_with('#') || (line.ends_with(':') && bullet_item(line).is_none())
}

fn bullet_item(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let item = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("• "))
        .or_else(|| {
            let (number, rest) = trimmed.split_once(". ")?;
            number.chars().all(|c| c.is_ascii_digit()).then_some(rest)
        })?;

    let item = item.trim();
    (!item.is_empty()).then(|| item.to_string())
}

fn push_scope_item(scope: &mut ScopeSummary, bucket: ScopeBucket, item: String) {
    let target = match bucket {
        ScopeBucket::Included => &mut scope.included,
        ScopeBucket::Excluded => &mut scope.excluded,
        ScopeBucket::Deferred => &mut scope.deferred,
    };

    if !target.contains(&item) {
        target.push(item);
    }
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

    if !brief.current_truth.is_empty() {
        output.push_str("\nCurrent truth:\n");
        append_truth_items(
            &mut output,
            "product identity",
            &brief.current_truth.product_identity,
        );
        append_truth_items(&mut output, "stack", &brief.current_truth.stack);
        append_truth_items(&mut output, "deployment", &brief.current_truth.deployment);
        append_truth_items(&mut output, "principles", &brief.current_truth.principles);
        append_truth_items(&mut output, "constraints", &brief.current_truth.constraints);
        append_truth_items(
            &mut output,
            "current state",
            &brief.current_truth.current_state,
        );
        append_truth_items(&mut output, "non-goals", &brief.current_truth.non_goals);
    }

    if !brief.scope.is_empty() {
        output.push_str("\nScope:\n");
        if !brief.scope.included.is_empty() {
            output.push_str("- included:\n");
            for item in &brief.scope.included {
                output.push_str(&format!("  - {item}\n"));
            }
        }
        if !brief.scope.excluded.is_empty() {
            output.push_str("- excluded:\n");
            for item in &brief.scope.excluded {
                output.push_str(&format!("  - {item}\n"));
            }
        }
        if !brief.scope.deferred.is_empty() {
            output.push_str("- deferred:\n");
            for item in &brief.scope.deferred {
                output.push_str(&format!("  - {item}\n"));
            }
        }
    }

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
            output.push_str(&format!(
                "- {} {} [P{}]\n",
                entry.id, entry.title, entry.priority
            ));
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

    if !brief.management_signals.is_empty() {
        output.push_str("\nManagement signals:\n");
        if brief.management_signals.unresolved_decisions > 0 {
            output.push_str(&format!(
                "- {} unresolved root decision(s)\n",
                brief.management_signals.unresolved_decisions
            ));
        }
        if !brief.management_signals.likely_stale_open_work.is_empty() {
            output.push_str(&format!(
                "- {} open descendant(s) look stale or superseded\n",
                brief.management_signals.likely_stale_open_work.len()
            ));
        }
        if !brief.management_signals.closed_with_concerns.is_empty() {
            output.push_str(&format!(
                "- {} closed descendant(s) mention concerns/partial verification\n",
                brief.management_signals.closed_with_concerns.len()
            ));
        }
        if !brief.management_signals.claimed_work.is_empty() {
            output.push_str(&format!(
                "- {} active descendant claim(s) or in-progress unit(s)\n",
                brief.management_signals.claimed_work.len()
            ));
        }
        if brief.management_signals.dependency_blockers > 0 {
            output.push_str(&format!(
                "- {} dependency blocker(s)\n",
                brief.management_signals.dependency_blockers
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

fn append_truth_items(output: &mut String, label: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }

    output.push_str(&format!("- {label}:\n"));
    for item in items {
        output.push_str(&format!("  - {item}\n"));
    }
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
        assert!(brief
            .concerns
            .iter()
            .any(|concern| concern.contains("waiting on open dependencies")));
    }
    #[test]
    fn brief_extracts_scope_sections_from_realistic_prose() {
        let mut root = unit("1", "Root", Status::Open, None);
        root.description = Some(
            "Goal: Build the thing.\n\nCurrent scope:\n- local-first POS/CRM\n- Pi builder seam\n\nNon-goals:\n- real payment processing\n- cloud backend\n\nFuture paths:\n1. hosted sync\n2. Shopify connector\n\nOther heading:\n- should not be captured"
                .to_string(),
        );

        let scope = extract_scope(&root);
        assert_eq!(
            scope.included,
            vec!["local-first POS/CRM", "Pi builder seam"]
        );
        assert_eq!(
            scope.excluded,
            vec!["real payment processing", "cloud backend"]
        );
        assert_eq!(scope.deferred, vec!["hosted sync", "Shopify connector"]);
    }

    #[test]
    fn brief_json_includes_scope_summary() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        let mut root = unit("1", "Root", Status::Open, None);
        root.description = Some(
            "Goal: Root\n\nIn scope:\n- brief JSON\n\nOut of scope:\n- schema migration"
                .to_string(),
        );
        root.to_file(&mana_dir.join("1-root.md")).unwrap();
        Index::build(mana_dir).unwrap().save(mana_dir).unwrap();

        let json: serde_json::Value =
            serde_json::from_str(&render_brief_json(mana_dir, "1").unwrap()).unwrap();
        assert_eq!(json["scope"]["included"][0], "brief JSON");
        assert_eq!(json["scope"]["excluded"][0], "schema migration");
        assert!(json["scope"]["deferred"].as_array().unwrap().is_empty());
    }
    #[test]
    fn brief_extracts_current_truth_from_335_style_prose() {
        let mut root = unit("1", "Root", Status::Open, None);
        root.description = Some(
            "Build a starter business system.\n\nProduct identity:\n- A local-first POS/CRM app that works out of the box.\n- Pi-powered builder/admin agent is the differentiator.\n\nSettled stack:\n- Kotlin backend/capability server.\n- TypeScript + React desktop/admin shell.\n\nDeployment default:\n- Single desktop machine, local-first, local DB.\n\nCore principle:\n- Kotlin remains API/capability-surfaced.\n\nConfirmed constraints:\n- POS and CRM are first-class starter capabilities.\n\nCurrent state:\n- Project has initial scaffold work in progress.\n\nOut of scope:\n- Real payment processing."
                .to_string(),
        );

        let current_truth = extract_current_truth(&root);
        assert_eq!(current_truth.product_identity.len(), 2);
        assert_eq!(current_truth.stack.len(), 2);
        assert_eq!(
            current_truth.deployment,
            vec!["Single desktop machine, local-first, local DB."]
        );
        assert_eq!(
            current_truth.principles,
            vec!["Kotlin remains API/capability-surfaced."]
        );
        assert_eq!(
            current_truth.constraints,
            vec!["POS and CRM are first-class starter capabilities."]
        );
        assert_eq!(
            current_truth.current_state,
            vec!["Project has initial scaffold work in progress."]
        );
        assert_eq!(current_truth.non_goals, vec!["Real payment processing."]);
    }

    #[test]
    fn brief_json_includes_current_truth_summary() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        let mut root = unit("1", "Root", Status::Open, None);
        root.description =
            Some("Goal: Root\n\nProduct identity:\n- Useful app\n\nStack:\n- Rust CLI".to_string());
        root.to_file(&mana_dir.join("1-root.md")).unwrap();
        Index::build(mana_dir).unwrap().save(mana_dir).unwrap();

        let json: serde_json::Value =
            serde_json::from_str(&render_brief_json(mana_dir, "1").unwrap()).unwrap();
        assert_eq!(json["current_truth"]["product_identity"][0], "Useful app");
        assert_eq!(json["current_truth"]["stack"][0], "Rust CLI");
    }

    #[test]
    fn brief_management_signals_detect_stale_and_closed_concerns() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        let mut root = unit("1", "Root", Status::Open, None);
        root.decisions = vec!["Choose API shape".to_string()];
        let mut stale = unit("1.1", "Stale open task", Status::Open, Some("1"));
        stale.notes = Some("Scope changed; do not execute unchanged.".to_string());
        let mut closed = unit("1.2", "Closed with caveat", Status::Closed, Some("1"));
        closed.close_reason = Some("Partial completion; UI deferred.".to_string());
        let mut claimed = unit("1.3", "Claimed work", Status::InProgress, Some("1"));
        claimed.claimed_by = Some("imp".to_string());
        let dep = unit("1.4", "Dependency", Status::Open, Some("1"));
        let mut blocked = unit("1.5", "Blocked", Status::Open, Some("1"));
        blocked.dependencies = vec!["1.4".to_string()];

        for unit in [&root, &stale, &closed, &claimed, &dep, &blocked] {
            unit.to_file(&mana_dir.join(format!("{}-unit.md", unit.id)))
                .unwrap();
        }
        Index::build(mana_dir).unwrap().save(mana_dir).unwrap();

        let brief = build_brief(mana_dir, "1").unwrap();
        assert_eq!(brief.management_signals.unresolved_decisions, 1);
        assert_eq!(brief.management_signals.likely_stale_open_work.len(), 1);
        assert_eq!(brief.management_signals.closed_with_concerns.len(), 1);
        assert_eq!(brief.management_signals.claimed_work.len(), 1);
        assert_eq!(brief.management_signals.dependency_blockers, 1);
    }

    #[test]
    fn brief_json_includes_management_signals() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        let mut root = unit("1", "Root", Status::Open, None);
        root.decisions = vec!["Resolve blocker".to_string()];
        root.to_file(&mana_dir.join("1-root.md")).unwrap();
        Index::build(mana_dir).unwrap().save(mana_dir).unwrap();

        let json: serde_json::Value =
            serde_json::from_str(&render_brief_json(mana_dir, "1").unwrap()).unwrap();
        assert_eq!(json["management_signals"]["unresolved_decisions"], 1);
        assert_eq!(json["management_signals"]["dependency_blockers"], 0);
    }

    #[test]
    fn brief_next_actions_prioritize_management_signals_and_skip_stale_work() {
        let tmp = TempDir::new().unwrap();
        let mana_dir = tmp.path();

        let mut root = unit("1", "Root", Status::Open, None);
        root.decisions = vec!["Resolve product mode".to_string()];
        let mut stale = unit("1.1", "Stale high-priority task", Status::Open, Some("1"));
        stale.priority = 0;
        stale.notes = Some("Superseded by newer implementation slice.".to_string());
        let mut fresh = unit("1.2", "Fresh executable task", Status::Open, Some("1"));
        fresh.priority = 1;
        let mut closed = unit("1.3", "Closed caveat", Status::Closed, Some("1"));
        closed.close_reason = Some("Done with concern: partial verification.".to_string());

        for unit in [&root, &stale, &fresh, &closed] {
            unit.to_file(&mana_dir.join(format!("{}-unit.md", unit.id)))
                .unwrap();
        }
        Index::build(mana_dir).unwrap().save(mana_dir).unwrap();

        let brief = build_brief(mana_dir, "1").unwrap();
        assert!(brief.next_actions[0].contains("Triage 1 unresolved"));
        assert!(brief
            .next_actions
            .iter()
            .any(|action| action.contains("Groom 1 likely stale")));
        assert!(brief
            .next_actions
            .iter()
            .any(|action| action.contains("Review 1 closed")));
        assert!(brief
            .next_actions
            .iter()
            .any(|action| action.contains("Work 1.2 next")));
        assert!(!brief
            .next_actions
            .iter()
            .any(|action| action.contains("Work 1.1 next")));
    }
}
