//! MCP resource definitions and handlers.

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::json;

use crate::discovery::find_unit_file;
use crate::index::Index;
use crate::mcp::protocol::{ResourceContent, ResourceDefinition};
use crate::unit::{Unit, UnitKind};

/// Return static resource definitions.
pub fn resource_definitions() -> Vec<ResourceDefinition> {
    vec![
        ResourceDefinition {
            uri: "units://status".to_string(),
            name: "Project Status".to_string(),
            description: Some(
                "Current project status: claimed, ready jobs, epics, and blocked units".to_string(),
            ),
            mime_type: Some("application/json".to_string()),
        },
        ResourceDefinition {
            uri: "units://rules".to_string(),
            name: "Project Rules".to_string(),
            description: Some("Project rules from RULES.md (if it exists)".to_string()),
            mime_type: Some("text/markdown".to_string()),
        },
    ]
}

/// Handle a resource read request.
pub fn handle_resource_read(uri: &str, mana_dir: &Path) -> Result<Vec<ResourceContent>> {
    if uri == "units://status" {
        return read_status_resource(mana_dir);
    }

    if uri == "units://rules" {
        return read_rules_resource(mana_dir);
    }

    // units://unit/{id}
    if let Some(id) = uri.strip_prefix("units://unit/") {
        return read_unit_resource(id, mana_dir);
    }

    anyhow::bail!("Unknown resource URI: {}", uri)
}

fn read_status_resource(mana_dir: &Path) -> Result<Vec<ResourceContent>> {
    let index = Index::load_or_rebuild(mana_dir)?;

    let mut claimed = 0u32;
    let mut ready = 0u32;
    let mut epics = 0u32;
    let mut goals = 0u32;
    let mut blocked = 0u32;
    let mut closed = 0u32;
    let mut features = 0u32;

    for entry in &index.units {
        match entry.status {
            crate::unit::Status::InProgress | crate::unit::Status::AwaitingVerify => claimed += 1,
            crate::unit::Status::Closed => closed += 1,
            crate::unit::Status::Open => {
                if entry.feature {
                    features += 1;
                } else if entry.kind == UnitKind::Epic {
                    epics += 1;
                } else if entry.has_verify {
                    let is_blocked = entry.dependencies.iter().any(|dep_id| {
                        index
                            .units
                            .iter()
                            .find(|e| &e.id == dep_id)
                            .is_none_or(|e| e.status != crate::unit::Status::Closed)
                    });
                    if is_blocked {
                        blocked += 1;
                    } else {
                        ready += 1;
                    }
                } else {
                    goals += 1;
                }
            }
        }
    }

    let text = serde_json::to_string_pretty(&json!({
        "total": index.units.len(),
        "claimed": claimed,
        "ready": ready,
        "epics": epics,
        "features": features,
        "goals": goals,
        "blocked": blocked,
        "closed": closed,
    }))?;

    Ok(vec![ResourceContent {
        uri: "units://status".to_string(),
        mime_type: Some("application/json".to_string()),
        text,
    }])
}

fn read_rules_resource(mana_dir: &Path) -> Result<Vec<ResourceContent>> {
    let project_dir = mana_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project root"))?;

    let rules_path = project_dir.join("RULES.md");
    if !rules_path.exists() {
        return Ok(vec![ResourceContent {
            uri: "units://rules".to_string(),
            mime_type: Some("text/markdown".to_string()),
            text: "No RULES.md found in project root.".to_string(),
        }]);
    }

    let text = std::fs::read_to_string(&rules_path).context("Failed to read RULES.md")?;

    Ok(vec![ResourceContent {
        uri: "units://rules".to_string(),
        mime_type: Some("text/markdown".to_string()),
        text,
    }])
}

fn read_unit_resource(id: &str, mana_dir: &Path) -> Result<Vec<ResourceContent>> {
    crate::util::validate_unit_id(id)?;
    let unit_path = find_unit_file(mana_dir, id)?;
    let unit = Unit::from_file(&unit_path)?;

    let text = serde_json::to_string_pretty(&unit).context("Failed to serialize unit")?;

    Ok(vec![ResourceContent {
        uri: format!("units://unit/{}", id),
        mime_type: Some("application/json".to_string()),
        text,
    }])
}
