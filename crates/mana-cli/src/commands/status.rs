use std::path::Path;
use std::process::Command;

use anyhow::Result;
use serde::Serialize;

use crate::blocking::{check_blocked, check_scope_warning, BlockReason};
use crate::index::{ArchiveIndex, Index, IndexEntry};
use crate::unit::{Status, UnitType};
use crate::util::natural_cmp;

/// Agent status parsed from claimed_by field
#[derive(Debug, Clone, Serialize)]
pub struct AgentStatus {
    pub pid: u32,
    pub alive: bool,
}

/// Parse claimed_by field for agent info (e.g., "spro:12345" -> Some(AgentStatus))
fn parse_agent_claim(claimed_by: &Option<String>) -> Option<AgentStatus> {
    let claim = claimed_by.as_ref()?;
    if !claim.starts_with("spro:") {
        return None;
    }
    let pid_str = claim.strip_prefix("spro:")?;
    let pid: u32 = pid_str.parse().ok()?;
    let alive = is_pid_alive(pid);
    Some(AgentStatus { pid, alive })
}

/// Check if a process with the given PID is alive
fn is_pid_alive(pid: u32) -> bool {
    // Use kill -0 to check if process exists (doesn't send a signal)
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Format agent status for display
fn format_agent_status(entry: &IndexEntry) -> String {
    match parse_agent_claim(&entry.claimed_by) {
        Some(agent) if agent.alive => format!("spro:{} ●", agent.pid),
        Some(agent) => format!("spro:{} ✗", agent.pid),
        None => entry.claimed_by.clone().unwrap_or_else(|| "-".to_string()),
    }
}

/// Entry with agent status for JSON output
#[derive(Serialize)]
struct StatusEntry {
    #[serde(flatten)]
    entry: IndexEntry,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<AgentStatus>,
}

impl StatusEntry {
    fn from_entry(entry: IndexEntry) -> Self {
        let agent = parse_agent_claim(&entry.claimed_by);
        Self { entry, agent }
    }
}

/// Blocked entry with reason for JSON output
#[derive(Serialize)]
struct BlockedEntry {
    #[serde(flatten)]
    entry: IndexEntry,
    block_reason: String,
}

/// JSON output structure for status command
#[derive(Serialize)]
struct StatusOutput {
    claimed: Vec<StatusEntry>,
    ready: Vec<IndexEntry>,
    epics: Vec<IndexEntry>,
    goals: Vec<IndexEntry>,
    blocked: Vec<BlockedEntry>,
}

/// Show complete work picture: claimed work, ready tasks, epics, and blocked units
pub fn cmd_status(json: bool, mana_dir: &Path) -> Result<()> {
    let index = Index::load_or_rebuild(mana_dir)?;

    // Separate units into categories
    let mut features: Vec<&IndexEntry> = Vec::new();
    let mut claimed: Vec<&IndexEntry> = Vec::new();
    let mut ready: Vec<&IndexEntry> = Vec::new();
    let mut epics: Vec<&IndexEntry> = Vec::new();
    let mut goals: Vec<&IndexEntry> = Vec::new();
    let mut blocked: Vec<(&IndexEntry, BlockReason)> = Vec::new();

    for entry in &index.units {
        if entry.feature {
            features.push(entry);
            continue;
        }
        match entry.status {
            Status::InProgress | Status::AwaitingVerify => {
                claimed.push(entry);
            }
            Status::Open => {
                if let Some(reason) = check_blocked(entry, &index) {
                    blocked.push((entry, reason));
                } else if entry.kind == UnitType::Epic {
                    epics.push(entry);
                } else if entry.has_verify {
                    ready.push(entry);
                } else {
                    goals.push(entry);
                }
            }
            Status::Closed => {}
        }
    }

    sort_units(&mut features);
    sort_units(&mut claimed);
    sort_units(&mut ready);
    sort_units(&mut epics);
    sort_units(&mut goals);
    blocked.sort_by(|(a, _), (b, _)| match a.priority.cmp(&b.priority) {
        std::cmp::Ordering::Equal => natural_cmp(&a.id, &b.id),
        other => other,
    });

    if json {
        let output = StatusOutput {
            claimed: claimed
                .into_iter()
                .cloned()
                .map(StatusEntry::from_entry)
                .collect(),
            ready: ready.into_iter().cloned().collect(),
            epics: epics.into_iter().cloned().collect(),
            goals: goals.into_iter().cloned().collect(),
            blocked: blocked
                .iter()
                .map(|(e, reason)| BlockedEntry {
                    entry: (*e).clone(),
                    block_reason: reason.to_string(),
                })
                .collect(),
        };
        let json_str = serde_json::to_string_pretty(&output)?;
        println!("{}", json_str);
    } else {
        // Features section (only shown if features exist)
        if !features.is_empty() {
            let archive = ArchiveIndex::load(mana_dir).unwrap_or(ArchiveIndex { units: vec![] });
            let closed_features = features
                .iter()
                .filter(|f| f.status == Status::Closed)
                .count();
            println!("## Features ({}/{})", closed_features, features.len());
            for feature in &features {
                // Count children from both active index and archive
                let active_children: Vec<&IndexEntry> = index
                    .units
                    .iter()
                    .filter(|b| b.parent.as_deref() == Some(&feature.id))
                    .collect();
                let archived_children: Vec<&IndexEntry> = archive
                    .units
                    .iter()
                    .filter(|b| b.parent.as_deref() == Some(&feature.id))
                    .collect();
                let active_count = active_children.len();
                let archived_count = archived_children.len();
                let total = active_count + archived_count;
                let closed = active_children
                    .iter()
                    .filter(|b| b.status == Status::Closed)
                    .count()
                    + archived_count; // All archived units are closed

                let progress = if total == 0 {
                    "not decomposed".to_string()
                } else {
                    format!("{}/{}", closed, total)
                };

                let indicator = if feature.status == Status::Closed {
                    "✓"
                } else if total > 0 && closed == total {
                    "★"
                } else {
                    "○"
                };

                let suffix = if total > 0 && closed == total && feature.status != Status::Closed {
                    " — ready for review"
                } else {
                    ""
                };

                println!(
                    "  {}  {} {} ({}{})",
                    indicator, feature.id, feature.title, progress, suffix
                );
            }
            println!();
        }

        println!("## Claimed ({})", claimed.len());
        if claimed.is_empty() {
            println!("  (none)");
        } else {
            for entry in claimed {
                let agent_str = format_agent_status(entry);
                println!("  {} [-] {} ({})", entry.id, entry.title, agent_str);
            }
        }
        println!();

        println!("## Ready Jobs ({})", ready.len());
        if ready.is_empty() {
            println!("  (none)");
        } else {
            for entry in ready {
                let warning = check_scope_warning(entry)
                    .map(|w| format!("  (⚠ {})", w))
                    .unwrap_or_default();
                println!("  {} [ ] {}{}", entry.id, entry.title, warning);
            }
        }
        println!();

        println!("## Epics ({})", epics.len());
        if epics.is_empty() {
            println!("  (none)");
        } else {
            for entry in epics {
                println!("  {} [~] {}", entry.id, entry.title);
            }
        }
        println!();

        println!("## Goals (need decomposition) ({})", goals.len());
        if goals.is_empty() {
            println!("  (none)");
        } else {
            for entry in goals {
                println!("  {} [?] {}", entry.id, entry.title);
            }
        }
        println!();

        println!("## Blocked ({})", blocked.len());
        if blocked.is_empty() {
            println!("  (none)");
        } else {
            for (entry, reason) in &blocked {
                println!("  {} [!] {}  ({})", entry.id, entry.title, reason);
            }
        }
    }

    Ok(())
}

fn sort_units(units: &mut Vec<&IndexEntry>) {
    units.sort_by(|a, b| match a.priority.cmp(&b.priority) {
        std::cmp::Ordering::Equal => natural_cmp(&a.id, &b.id),
        other => other,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::{Status, Unit, UnitType};
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_mana_dir() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        fs::create_dir(&mana_dir).unwrap();
        (dir, mana_dir)
    }

    #[test]
    fn test_status_shows_epics_section() {
        let (_dir, mana_dir) = setup_test_mana_dir();

        let mut epic = Unit::new("1", "Epic parent");
        epic.kind = UnitType::Epic;
        epic.to_file(mana_dir.join("1.yaml")).unwrap();

        let mut task = Unit::new("2", "Ready task");
        task.kind = UnitType::Task;
        task.verify = Some("true".to_string());
        task.to_file(mana_dir.join("2.yaml")).unwrap();

        let index = Index::load_or_rebuild(&mana_dir).unwrap();
        let epic_entry = index.units.iter().find(|e| e.id == "1").unwrap();
        let job_entry = index.units.iter().find(|e| e.id == "2").unwrap();

        assert_eq!(epic_entry.kind, UnitType::Epic);
        assert_eq!(epic_entry.status, Status::Open);
        assert_eq!(job_entry.kind, UnitType::Task);
        assert!(job_entry.has_verify);
    }
}
