use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use mana_core::ops::list as ops_list;

use crate::blocking::check_blocked;
use crate::config::resolve_identity;
use crate::index::{Index, IndexEntry};
use crate::unit::Status;
use crate::util::natural_cmp;

/// List units with optional filtering.
/// - Default: tree-format with status indicators
/// - --status: filter by status (open, in_progress, closed)
/// - --priority: filter by priority (0-4)
/// - --parent: show only children of this parent
/// - --label: filter by label
/// - --assignee: filter by assignee
/// - --all: include closed units (default excludes closed)
/// - --json: JSON array output
/// - Shows [!] for blocked units
///
/// When --status closed is specified, also searches archived units.
#[allow(clippy::too_many_arguments)]
pub fn cmd_list(
    status_filter: Option<&str>,
    priority_filter: Option<u8>,
    parent_filter: Option<&str>,
    label_filter: Option<&str>,
    assignee_filter: Option<&str>,
    mine: bool,
    all: bool,
    json: bool,
    ids: bool,
    format_str: Option<&str>,
    mana_dir: &Path,
) -> Result<()> {
    let current_user = if mine {
        let user = resolve_identity(mana_dir);
        if user.is_none() {
            anyhow::bail!(
                "Cannot use --mine: no identity configured.\n\
                 Set one with: mana config set user <name>"
            );
        }
        user
    } else {
        None
    };

    let filtered = ops_list::list(
        mana_dir,
        &ops_list::ListParams {
            status: status_filter.map(str::to_string),
            priority: priority_filter,
            parent: parent_filter.map(str::to_string),
            label: label_filter.map(str::to_string),
            assignee: assignee_filter.map(str::to_string),
            current_user: current_user.clone(),
            include_closed: all,
        },
    )?;

    if json {
        let json_str = serde_json::to_string_pretty(&filtered)?;
        println!("{}", json_str);
    } else if ids {
        for entry in &filtered {
            println!("{}", entry.id);
        }
    } else if let Some(fmt) = format_str {
        for entry in &filtered {
            let line = fmt
                .replace("{id}", &entry.id)
                .replace("{title}", &entry.title)
                .replace("{status}", &format!("{}", entry.status))
                .replace("{priority}", &format!("P{}", entry.priority))
                .replace("{handle}", entry.handle.as_deref().unwrap_or(""))
                .replace("{parent}", entry.parent.as_deref().unwrap_or(""))
                .replace("{assignee}", entry.assignee.as_deref().unwrap_or(""))
                .replace("{labels}", &entry.labels.join(","))
                .replace("\\t", "\t")
                .replace("\\n", "\n");
            println!("{}", line);
        }
    } else {
        let index = Index::load_or_rebuild(mana_dir)?;
        let include_archived = status_filter == Some("closed") || all;
        let combined_index = if include_archived {
            let mut all_units = index.units.clone();
            if let Ok(archived) = Index::collect_archived(mana_dir) {
                all_units.extend(archived);
            }
            Index { units: all_units }
        } else {
            index.clone()
        };

        let tree = render_tree(&filtered, &combined_index);
        println!("{}", tree);
        println!("Legend: [ ] open  [-] in_progress  [x] closed  [!] blocked  [?] has decisions");
    }

    Ok(())
}

/// Render units as a hierarchical tree.
/// - Root units have no parent
/// - Children indented 2 spaces per level
/// - Status: [ ] open, [-] in_progress, [x] closed, [!] blocked
fn render_tree(entries: &[IndexEntry], index: &Index) -> String {
    let mut output = String::new();

    // Build parent -> children map
    let mut children_map: HashMap<Option<String>, Vec<&IndexEntry>> = HashMap::new();
    for entry in entries {
        children_map
            .entry(entry.parent.clone())
            .or_default()
            .push(entry);
    }

    // Sort children by id within each parent
    for children in children_map.values_mut() {
        children.sort_by(|a, b| natural_cmp(&a.id, &b.id));
    }

    // Render root entries
    if let Some(roots) = children_map.get(&None) {
        for root in roots {
            render_entry(&mut output, root, 0, &children_map, index);
        }
    }

    output
}

/// Recursively render an entry and its children
fn render_entry(
    output: &mut String,
    entry: &IndexEntry,
    depth: u32,
    children_map: &HashMap<Option<String>, Vec<&IndexEntry>>,
    index: &Index,
) {
    let indent = "  ".repeat(depth as usize);
    let (status_indicator, reason_suffix) = get_status_indicator(entry, index);
    let handle_suffix = entry
        .handle
        .as_ref()
        .map(|handle| format!(" ({handle})"))
        .unwrap_or_default();
    output.push_str(&format!(
        "{}{} {}. {}{}{}\n",
        indent, status_indicator, entry.id, entry.title, handle_suffix, reason_suffix
    ));

    // Render children
    if let Some(children) = children_map.get(&Some(entry.id.clone())) {
        for child in children {
            render_entry(output, child, depth + 1, children_map, index);
        }
    }
}

/// Get status indicator and optional suffix for an entry.
/// Returns (indicator, suffix) where suffix is e.g. " (waiting on 3.1)" or " (⚠ oversized)".
fn get_status_indicator(entry: &IndexEntry, index: &Index) -> (String, String) {
    if let Some(reason) = check_blocked(entry, index) {
        ("[!]".to_string(), format!("  ({})", reason))
    } else {
        let indicator = match entry.status {
            Status::Open => "[ ]",
            Status::InProgress | Status::AwaitingVerify => "[-]",
            Status::Closed => "[x]",
        };
        // Scope warnings are non-blocking annotations
        let mut suffix = crate::blocking::check_scope_warning(entry)
            .map(|w| format!("  (⚠ {})", w))
            .unwrap_or_default();
        // Add decisions indicator
        if entry.has_decisions {
            suffix.push_str("  [?]");
        }
        (indicator.to_string(), suffix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::index::{Index, IndexEntry};
    use crate::unit::Status;
    use crate::util::{parse_status, title_to_slug};
    use tempfile::TempDir;

    fn setup_test_units() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();

        // Create some test units
        let unit1 = crate::unit::Unit::new("1", "First task");
        let mut unit2 = crate::unit::Unit::new("2", "Second task");
        unit2.status = Status::InProgress;
        let mut unit3 = crate::unit::Unit::new("3", "Parent task");
        unit3.dependencies = vec!["1".to_string()];

        let mut unit3_1 = crate::unit::Unit::new("3.1", "Subtask");
        unit3_1.parent = Some("3".to_string());

        let slug1 = title_to_slug(&unit1.title);
        let slug2 = title_to_slug(&unit2.title);
        let slug3 = title_to_slug(&unit3.title);
        let slug3_1 = title_to_slug(&unit3_1.title);

        unit1
            .to_file(mana_dir.join(format!("1-{}.md", slug1)))
            .unwrap();
        unit2
            .to_file(mana_dir.join(format!("2-{}.md", slug2)))
            .unwrap();
        unit3
            .to_file(mana_dir.join(format!("3-{}.md", slug3)))
            .unwrap();
        unit3_1
            .to_file(mana_dir.join(format!("3.1-{}.md", slug3_1)))
            .unwrap();

        // Create config
        fs::write(mana_dir.join("config.yaml"), "project: test\nnext_id: 4\n").unwrap();

        (dir, mana_dir)
    }

    #[test]
    fn parse_status_valid() {
        assert_eq!(parse_status("open"), Some(Status::Open));
        assert_eq!(parse_status("in_progress"), Some(Status::InProgress));
        assert_eq!(parse_status("closed"), Some(Status::Closed));
    }

    #[test]
    fn parse_status_invalid() {
        assert_eq!(parse_status("invalid"), None);
        assert_eq!(parse_status(""), None);
    }

    #[test]
    fn blocked_by_open_dependency() {
        let index = Index::build(&setup_test_units().1).unwrap();
        let entry = index.units.iter().find(|e| e.id == "3").unwrap();
        // unit 3 depends on unit 1 which is open, so unit 3 is blocked
        assert!(check_blocked(entry, &index).is_some());
    }

    #[test]
    fn not_blocked_when_no_dependencies() {
        let index = Index::build(&setup_test_units().1).unwrap();
        let entry = index.units.iter().find(|e| e.id == "1").unwrap();
        // unit 1 has no deps — unscoped units are no longer blocked
        let reason = check_blocked(entry, &index);
        assert!(reason.is_none(), "should not be blocked: {:?}", reason);
    }

    fn make_scoped_entry(id: &str, status: Status) -> IndexEntry {
        IndexEntry {
            handle: None,
            id: id.to_string(),
            title: "Test".to_string(),
            status,
            priority: 2,
            parent: None,
            dependencies: Vec::new(),
            labels: Vec::new(),
            assignee: None,
            updated_at: chrono::Utc::now(),
            produces: vec!["Artifact".to_string()],
            requires: Vec::new(),
            has_verify: true,
            verify: None,
            created_at: chrono::Utc::now(),
            claimed_by: None,
            attempts: 0,
            paths: vec!["src/test.rs".to_string()],
            kind: crate::unit::UnitType::Task,
            feature: false,
            has_decisions: false,
        }
    }

    #[test]
    fn status_indicator_open() {
        let entry = make_scoped_entry("1", Status::Open);
        let index = Index {
            units: vec![entry.clone()],
        };
        assert_eq!(
            get_status_indicator(&entry, &index),
            ("[ ]".to_string(), String::new())
        );
    }

    #[test]
    fn status_indicator_in_progress() {
        let entry = make_scoped_entry("1", Status::InProgress);
        let index = Index {
            units: vec![entry.clone()],
        };
        assert_eq!(
            get_status_indicator(&entry, &index),
            ("[-]".to_string(), String::new())
        );
    }

    #[test]
    fn status_indicator_closed() {
        let entry = make_scoped_entry("1", Status::Closed);
        let index = Index {
            units: vec![entry.clone()],
        };
        assert_eq!(
            get_status_indicator(&entry, &index),
            ("[x]".to_string(), String::new())
        );
    }

    #[test]
    fn status_indicator_oversized_shows_warning() {
        let mut entry = make_scoped_entry("1", Status::Open);
        entry.produces = vec!["A".into(), "B".into(), "C".into(), "D".into()];
        let index = Index {
            units: vec![entry.clone()],
        };
        let (indicator, suffix) = get_status_indicator(&entry, &index);
        // Not blocked — still shows [ ] with a warning suffix
        assert_eq!(indicator, "[ ]");
        assert!(suffix.contains("oversized"));
    }

    #[test]
    fn status_indicator_unscoped_no_warning() {
        let mut entry = make_scoped_entry("1", Status::Open);
        entry.produces = Vec::new();
        entry.paths = Vec::new();
        let index = Index {
            units: vec![entry.clone()],
        };
        let (indicator, suffix) = get_status_indicator(&entry, &index);
        // Unscoped is totally fine — no warning, no blocking
        assert_eq!(indicator, "[ ]");
        assert!(suffix.is_empty());
    }

    #[test]
    fn render_tree_hierarchy() {
        let (_dir, mana_dir) = setup_test_units();
        let index = Index::build(&mana_dir).unwrap();
        let tree = render_tree(&index.units, &index);

        // Should contain entries
        assert!(tree.contains("1. First task"));
        assert!(tree.contains("2. Second task"));
        assert!(tree.contains("3. Parent task"));
        assert!(tree.contains("3.1. Subtask"));

        // 3.1 should be indented (child of 3)
        let lines: Vec<&str> = tree.lines().collect();
        let line_3 = lines.iter().find(|l| l.contains("3. Parent task")).unwrap();
        let line_3_1 = lines.iter().find(|l| l.contains("3.1. Subtask")).unwrap();

        // 3.1 should have more indentation than 3
        let indent_3 = line_3.len() - line_3.trim_start().len();
        let indent_3_1 = line_3_1.len() - line_3_1.trim_start().len();
        assert!(indent_3_1 > indent_3);
    }
}
