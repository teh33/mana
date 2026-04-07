//! MCP tool definitions and handlers.
//!
//! Each tool maps to a units operation. Handlers work directly with
//! Unit/Index types to avoid stdout pollution from CLI commands.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{json, Value};

use crate::blocking::check_blocked;
use crate::config::Config;
use crate::discovery::find_unit_file;
use crate::index::{Index, IndexEntry};
use crate::mcp::protocol::ToolDefinition;
use crate::unit::{Status, Unit, UnitKind};
use crate::util::{natural_cmp, title_to_slug};

/// Return all MCP tool definitions.
pub fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "list_units".to_string(),
            description: "List units with optional status and priority filters".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["open", "in_progress", "closed"],
                        "description": "Filter by status"
                    },
                    "priority": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 4,
                        "description": "Filter by priority (0-4, where P0 is highest)"
                    },
                    "parent": {
                        "type": "string",
                        "description": "Filter by parent unit ID"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "show_unit".to_string(),
            description: "Get full unit details including description, acceptance criteria, verify command, and history".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Unit ID"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "ready_units".to_string(),
            description: "Get units ready to work on (open dispatchable jobs with all dependencies resolved)".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: "create_unit".to_string(),
            description: "Create a new unit (task/spec for agents)".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Unit title"
                    },
                    "description": {
                        "type": "string",
                        "description": "Full description / agent context (markdown)"
                    },
                    "verify": {
                        "type": "string",
                        "description": "Shell command that must exit 0 to close the unit"
                    },
                    "parent": {
                        "type": "string",
                        "description": "Parent unit ID (creates a child unit)"
                    },
                    "priority": {
                        "type": "integer",
                        "minimum": 0,
                        "maximum": 4,
                        "description": "Priority 0-4 (P0 highest, default P2)"
                    },
                    "acceptance": {
                        "type": "string",
                        "description": "Acceptance criteria"
                    },
                    "deps": {
                        "type": "string",
                        "description": "Comma-separated dependency unit IDs"
                    }
                },
                "required": ["title"]
            }),
        },
        ToolDefinition {
            name: "claim_unit".to_string(),
            description: "Claim a unit for work (sets status to in_progress)".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Unit ID to claim"
                    },
                    "by": {
                        "type": "string",
                        "description": "Who is claiming (agent name or user)"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "close_unit".to_string(),
            description: "Close a unit (runs verify gate first if configured). Returns error if verify fails.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Unit ID to close"
                    },
                    "force": {
                        "type": "boolean",
                        "description": "Skip verify command (force close)",
                        "default": false
                    },
                    "reason": {
                        "type": "string",
                        "description": "Close reason"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "verify_unit".to_string(),
            description: "Run a unit's verify command without closing it. Returns pass/fail and output.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Unit ID to verify"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "context_unit".to_string(),
            description: "Get assembled context for a unit (reads files referenced in description)".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Unit ID"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "status".to_string(),
            description: "Project status overview: claimed, ready jobs, epics, and blocked units".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: "tree".to_string(),
            description: "Hierarchical unit tree showing parent-child relationships and status".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Root unit ID (shows full tree if omitted)"
                    }
                }
            }),
        },
    ]
}

// ---------------------------------------------------------------------------
// Tool Handlers
// ---------------------------------------------------------------------------

/// Dispatch a tool call to the appropriate handler.
pub fn handle_tool_call(name: &str, args: &Value, mana_dir: &Path) -> Value {
    let result = match name {
        "list_units" => handle_list_units(args, mana_dir),
        "show_unit" => handle_show_unit(args, mana_dir),
        "ready_units" => handle_ready_units(mana_dir),
        "create_unit" => handle_create_unit(args, mana_dir),
        "claim_unit" => handle_claim_unit(args, mana_dir),
        "close_unit" => handle_close_unit(args, mana_dir),
        "verify_unit" => handle_verify_unit(args, mana_dir),
        "context_unit" => handle_context_unit(args, mana_dir),
        "status" => handle_status(mana_dir),
        "tree" => handle_tree(args, mana_dir),
        _ => Err(anyhow::anyhow!("Unknown tool: {}", name)),
    };

    match result {
        Ok(text) => json!({
            "content": [{ "type": "text", "text": text }]
        }),
        Err(e) => json!({
            "content": [{ "type": "text", "text": format!("Error: {}", e) }],
            "isError": true
        }),
    }
}

// ---------------------------------------------------------------------------
// Individual Handlers
// ---------------------------------------------------------------------------

fn handle_list_units(args: &Value, mana_dir: &Path) -> Result<String> {
    let index = Index::load_or_rebuild(mana_dir)?;

    let status_filter = args
        .get("status")
        .and_then(|v| v.as_str())
        .and_then(crate::util::parse_status);

    let priority_filter = args
        .get("priority")
        .and_then(|v| v.as_u64())
        .map(|v| v as u8);

    let parent_filter = args.get("parent").and_then(|v| v.as_str());

    let filtered: Vec<&IndexEntry> = index
        .units
        .iter()
        .filter(|entry| {
            if let Some(status) = status_filter {
                if entry.status != status {
                    return false;
                }
            } else if entry.status == Status::Closed {
                // Exclude closed by default
                return false;
            }
            if let Some(priority) = priority_filter {
                if entry.priority != priority {
                    return false;
                }
            }
            if let Some(parent) = parent_filter {
                if entry.parent.as_deref() != Some(parent) {
                    return false;
                }
            }
            true
        })
        .collect();

    let entries: Vec<Value> = filtered
        .iter()
        .map(|e| {
            json!({
                "id": e.id,
                "title": e.title,
                "status": format!("{}", e.status),
                "priority": format!("P{}", e.priority),
                "parent": e.parent,
                "has_verify": e.has_verify,
                "claimed_by": e.claimed_by,
            })
        })
        .collect();

    serde_json::to_string_pretty(&json!({ "units": entries, "count": entries.len() }))
        .context("Failed to serialize unit list")
}

fn handle_show_unit(args: &Value, mana_dir: &Path) -> Result<String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required parameter: id"))?;

    crate::util::validate_unit_id(id)?;
    let unit_path = find_unit_file(mana_dir, id)?;
    let unit = Unit::from_file(&unit_path)?;

    serde_json::to_string_pretty(&unit).context("Failed to serialize unit")
}

fn handle_ready_units(mana_dir: &Path) -> Result<String> {
    let index = Index::load_or_rebuild(mana_dir)?;

    let mut ready: Vec<&IndexEntry> = index
        .units
        .iter()
        .filter(|entry| {
            entry.kind == UnitKind::Job
                && entry.has_verify
                && entry.status == Status::Open
                && check_blocked(entry, &index).is_none()
        })
        .collect();

    ready.sort_by(|a, b| match a.priority.cmp(&b.priority) {
        std::cmp::Ordering::Equal => natural_cmp(&a.id, &b.id),
        other => other,
    });

    let entries: Vec<Value> = ready
        .iter()
        .map(|e| {
            json!({
                "id": e.id,
                "title": e.title,
                "priority": format!("P{}", e.priority),
            })
        })
        .collect();

    serde_json::to_string_pretty(&json!({ "ready": entries, "count": entries.len() }))
        .context("Failed to serialize ready units")
}

fn handle_create_unit(args: &Value, mana_dir: &Path) -> Result<String> {
    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required parameter: title"))?;

    let description = args.get("description").and_then(|v| v.as_str());
    let verify = args.get("verify").and_then(|v| v.as_str());
    let parent = args.get("parent").and_then(|v| v.as_str());
    let priority = args
        .get("priority")
        .and_then(|v| v.as_u64())
        .map(|v| v as u8);
    let acceptance = args.get("acceptance").and_then(|v| v.as_str());
    let deps = args.get("deps").and_then(|v| v.as_str());

    if let Some(p) = priority {
        crate::unit::validate_priority(p)?;
    }

    // Determine unit ID
    let mut config = Config::load(mana_dir)?;
    let unit_id = if let Some(parent_id) = parent {
        crate::util::validate_unit_id(parent_id)?;
        crate::commands::create::assign_child_id(mana_dir, parent_id)?
    } else {
        let id = config.increment_id();
        config.save(mana_dir)?;
        id.to_string()
    };

    let slug = title_to_slug(title);
    let mut unit = Unit::try_new(&unit_id, title)?;
    unit.slug = Some(slug.clone());

    if let Some(desc) = description {
        unit.description = Some(desc.to_string());
    }
    if let Some(v) = verify {
        unit.verify = Some(v.to_string());
    }
    if let Some(p) = parent {
        unit.parent = Some(p.to_string());
    }
    if let Some(p) = priority {
        unit.priority = p;
    }
    if let Some(a) = acceptance {
        unit.acceptance = Some(a.to_string());
    }
    if let Some(d) = deps {
        unit.dependencies = d.split(',').map(|s| s.trim().to_string()).collect();
    }

    // Write unit file
    let unit_path = mana_dir.join(format!("{}-{}.md", unit_id, slug));
    unit.to_file(&unit_path)?;

    // Rebuild index
    let index = Index::build(mana_dir)?;
    index.save(mana_dir)?;

    Ok(format!("Created unit {}: {}", unit_id, title))
}

fn handle_claim_unit(args: &Value, mana_dir: &Path) -> Result<String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required parameter: id"))?;
    let by = args.get("by").and_then(|v| v.as_str());

    crate::util::validate_unit_id(id)?;
    let unit_path = find_unit_file(mana_dir, id)?;
    let mut unit = Unit::from_file(&unit_path)?;

    if unit.status != Status::Open {
        anyhow::bail!(
            "Unit {} is {} — only open units can be claimed",
            id,
            unit.status
        );
    }

    let now = Utc::now();
    unit.status = Status::InProgress;
    unit.claimed_by = by.map(|s| s.to_string());
    unit.claimed_at = Some(now);
    unit.updated_at = now;

    unit.to_file(&unit_path)?;

    // Rebuild index
    let index = Index::build(mana_dir)?;
    index.save(mana_dir)?;

    let claimer = by.unwrap_or("anonymous");
    Ok(format!(
        "Claimed unit {}: {} (by {})",
        id, unit.title, claimer
    ))
}

fn handle_close_unit(args: &Value, mana_dir: &Path) -> Result<String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required parameter: id"))?;
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let reason = args.get("reason").and_then(|v| v.as_str());

    crate::util::validate_unit_id(id)?;
    let unit_path = find_unit_file(mana_dir, id)?;
    let mut unit = Unit::from_file(&unit_path)?;

    // Run verify if configured and not forced
    if let Some(ref verify_cmd) = unit.verify {
        if !force {
            let project_root = mana_dir
                .parent()
                .ok_or_else(|| anyhow::anyhow!("Cannot determine project root"))?;

            let output = std::process::Command::new("sh")
                .args(["-c", verify_cmd])
                .current_dir(project_root)
                .output()
                .with_context(|| format!("Failed to execute verify: {}", verify_cmd))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let stdout = String::from_utf8_lossy(&output.stdout);
                let combined = format!("{}{}", stdout, stderr);
                let snippet = if combined.len() > 2000 {
                    format!("...{}", &combined[combined.len() - 2000..])
                } else {
                    combined.to_string()
                };

                unit.attempts += 1;
                unit.updated_at = Utc::now();
                unit.to_file(&unit_path)?;

                // Rebuild index to reflect attempt count
                let index = Index::build(mana_dir)?;
                index.save(mana_dir)?;

                anyhow::bail!(
                    "Verify failed for unit {} (attempt {})\nCommand: {}\nOutput:\n{}",
                    id,
                    unit.attempts,
                    verify_cmd,
                    snippet.trim()
                );
            }
        }
    }

    // Close the unit
    let now = Utc::now();
    unit.status = Status::Closed;
    unit.closed_at = Some(now);
    unit.close_reason = reason.map(|s| s.to_string());
    unit.updated_at = now;

    unit.to_file(&unit_path)?;

    // Archive the unit
    let slug = unit
        .slug
        .clone()
        .unwrap_or_else(|| title_to_slug(&unit.title));
    let ext = unit_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("md");
    let today = chrono::Local::now().naive_local().date();
    let archive_path = crate::discovery::archive_path_for_unit(mana_dir, id, &slug, ext, today);

    if let Some(parent) = archive_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&unit_path, &archive_path)?;

    unit.is_archived = true;
    unit.to_file(&archive_path)?;

    // Rebuild index
    let index = Index::build(mana_dir)?;
    index.save(mana_dir)?;

    // Check auto-close parent
    if let Some(parent_id) = &unit.parent {
        let auto_close = Config::load_with_extends(mana_dir)
            .map(|c| c.auto_close_parent)
            .unwrap_or(true);
        if auto_close {
            if let Ok(true) = all_children_closed(mana_dir, parent_id) {
                let _ = auto_close_parent(mana_dir, parent_id);
            }
        }
    }

    Ok(format!("Closed unit {}: {}", id, unit.title))
}

fn handle_verify_unit(args: &Value, mana_dir: &Path) -> Result<String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required parameter: id"))?;

    crate::util::validate_unit_id(id)?;
    let unit_path = find_unit_file(mana_dir, id)?;
    let unit = Unit::from_file(&unit_path)?;

    let verify_cmd = match &unit.verify {
        Some(cmd) => cmd.clone(),
        None => return Ok(format!("Unit {} has no verify command", id)),
    };

    let project_root = mana_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project root"))?;

    let output = std::process::Command::new("sh")
        .args(["-c", &verify_cmd])
        .current_dir(project_root)
        .output()
        .with_context(|| format!("Failed to execute verify: {}", verify_cmd))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let passed = output.status.success();

    Ok(serde_json::to_string_pretty(&json!({
        "id": id,
        "passed": passed,
        "command": verify_cmd,
        "exit_code": output.status.code(),
        "stdout": truncate_str(&stdout, 2000),
        "stderr": truncate_str(&stderr, 2000),
    }))?)
}

fn handle_context_unit(args: &Value, mana_dir: &Path) -> Result<String> {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required parameter: id"))?;

    crate::util::validate_unit_id(id)?;
    let unit_path = find_unit_file(mana_dir, id)?;
    let unit = Unit::from_file(&unit_path)?;

    let project_dir = mana_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project root"))?;

    let description = unit.description.as_deref().unwrap_or("");
    let paths = crate::ctx_assembler::extract_paths(description);

    if paths.is_empty() {
        return Ok(format!("Unit {}: no file paths found in description", id));
    }

    let context = crate::ctx_assembler::assemble_context(paths, project_dir)
        .context("Failed to assemble context")?;

    Ok(context)
}

fn handle_status(mana_dir: &Path) -> Result<String> {
    let index = Index::load_or_rebuild(mana_dir)?;

    let mut claimed = Vec::new();
    let mut ready = Vec::new();
    let mut epics = Vec::new();
    let mut goals = Vec::new();
    let mut blocked: Vec<(&IndexEntry, String)> = Vec::new();

    for entry in &index.units {
        match entry.status {
            Status::InProgress | Status::AwaitingVerify => claimed.push(entry),
            Status::Open => {
                if let Some(reason) = check_blocked(entry, &index) {
                    blocked.push((entry, reason.to_string()));
                } else if entry.feature {
                    goals.push(entry);
                } else if entry.kind == UnitKind::Epic {
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

    let format_entries = |entries: &[&IndexEntry]| -> Vec<Value> {
        entries
            .iter()
            .map(|e| {
                json!({
                    "id": e.id,
                    "title": e.title,
                    "priority": format!("P{}", e.priority),
                    "claimed_by": e.claimed_by,
                })
            })
            .collect()
    };

    let blocked_entries: Vec<Value> = blocked
        .iter()
        .map(|(e, reason)| {
            json!({
                "id": e.id,
                "title": e.title,
                "priority": format!("P{}", e.priority),
                "claimed_by": e.claimed_by,
                "block_reason": reason,
            })
        })
        .collect();

    serde_json::to_string_pretty(&json!({
        "claimed": format_entries(&claimed),
        "ready": format_entries(&ready),
        "epics": format_entries(&epics),
        "goals": format_entries(&goals),
        "blocked": blocked_entries,
        "summary": format!(
            "{} claimed, {} ready, {} epics, {} goals, {} blocked",
            claimed.len(), ready.len(), epics.len(), goals.len(), blocked.len()
        )
    }))
    .context("Failed to serialize status")
}

fn handle_tree(args: &Value, mana_dir: &Path) -> Result<String> {
    let index = Index::load_or_rebuild(mana_dir)?;
    let root_id = args.get("id").and_then(|v| v.as_str());

    let mut output = String::new();

    if let Some(root) = root_id {
        render_subtree(&index, root, "", true, &mut output);
    } else {
        // Find root units (no parent)
        let roots: Vec<&IndexEntry> = index.units.iter().filter(|e| e.parent.is_none()).collect();

        for (i, root) in roots.iter().enumerate() {
            let is_last = i == roots.len() - 1;
            let status_icon = status_icon(root.status);
            output.push_str(&format!("{} {} {}\n", status_icon, root.id, root.title));
            render_children(&index, &root.id, "  ", &mut output);
            if !is_last {
                output.push('\n');
            }
        }
    }

    if output.is_empty() {
        Ok("No units found.".to_string())
    } else {
        Ok(output)
    }
}

// ---------------------------------------------------------------------------
// Helper Functions
// ---------------------------------------------------------------------------

/// Check if all children of a parent unit are closed.
fn all_children_closed(mana_dir: &Path, parent_id: &str) -> Result<bool> {
    let index = Index::load_or_rebuild(mana_dir)?;
    let children: Vec<&IndexEntry> = index
        .units
        .iter()
        .filter(|e| e.parent.as_deref() == Some(parent_id))
        .collect();

    if children.is_empty() {
        return Ok(false);
    }

    Ok(children.iter().all(|c| c.status == Status::Closed))
}

/// Auto-close a parent unit when all children are closed.
fn auto_close_parent(mana_dir: &Path, parent_id: &str) -> Result<()> {
    let unit_path = find_unit_file(mana_dir, parent_id)?;
    let mut unit = Unit::from_file(&unit_path)?;

    if unit.status == Status::Closed {
        return Ok(());
    }

    let now = Utc::now();
    unit.status = Status::Closed;
    unit.closed_at = Some(now);
    unit.close_reason = Some("All children closed".to_string());
    unit.updated_at = now;
    unit.to_file(&unit_path)?;

    // Archive
    let slug = unit
        .slug
        .clone()
        .unwrap_or_else(|| title_to_slug(&unit.title));
    let ext = unit_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("md");
    let today = chrono::Local::now().naive_local().date();
    let archive_path =
        crate::discovery::archive_path_for_unit(mana_dir, parent_id, &slug, ext, today);
    if let Some(parent) = archive_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&unit_path, &archive_path)?;
    unit.is_archived = true;
    unit.to_file(&archive_path)?;

    // Rebuild index
    let index = Index::build(mana_dir)?;
    index.save(mana_dir)?;

    Ok(())
}

fn status_icon(status: Status) -> &'static str {
    match status {
        Status::Open => "[ ]",
        Status::InProgress | Status::AwaitingVerify => "[-]",
        Status::Closed => "[x]",
    }
}

fn render_subtree(index: &Index, id: &str, prefix: &str, _is_last: bool, output: &mut String) {
    if let Some(entry) = index.units.iter().find(|e| e.id == id) {
        let icon = status_icon(entry.status);
        output.push_str(&format!(
            "{}{} {} {}\n",
            prefix, icon, entry.id, entry.title
        ));
        render_children(index, id, &format!("{}  ", prefix), output);
    }
}

fn render_children(index: &Index, parent_id: &str, prefix: &str, output: &mut String) {
    let children: Vec<&IndexEntry> = index
        .units
        .iter()
        .filter(|e| e.parent.as_deref() == Some(parent_id))
        .collect();

    for child in &children {
        let icon = status_icon(child.status);
        output.push_str(&format!(
            "{}{} {} {}\n",
            prefix, icon, child.id, child.title
        ));
        render_children(index, &child.id, &format!("{}  ", prefix), output);
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("...{}", &s[s.len() - max..])
    } else {
        s.to_string()
    }
}
