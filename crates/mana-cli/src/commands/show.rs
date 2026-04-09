use std::path::Path;

use anyhow::Result;
use mana_core::ops::show as ops_show;
use mana_core::ops::context::{summarize_child_units, ChildSummary};
use termimad::MadSkin;

use crate::unit::{RunRecord, Unit};

/// Default number of history entries to show without `--history`.
const DEFAULT_HISTORY_LIMIT: usize = 10;

/// Maximum lines of outputs JSON to display before truncating.
const MAX_OUTPUT_LINES: usize = 50;

/// Handle `mana show <id>` command
/// - Default: render beautifully with metadata header and markdown formatting
/// - --json: deserialize and re-serialize as JSON
/// - --short: one-line summary "{id}. {title} [{status}]"
/// - --history: show all history entries (default: last 10)
pub fn cmd_show(id: &str, json: bool, short: bool, history: bool, mana_dir: &Path) -> Result<()> {
    let result = ops_show::get(mana_dir, id)?;
    let unit = result.unit;
    let child_summaries = summarize_child_units(mana_dir, id);

    if short {
        println!("{}", format_short(&unit));
    } else if json {
        let json_str = serde_json::to_string_pretty(&unit)?;
        println!("{}", json_str);
    } else {
        render_unit(&unit, &child_summaries, history)?;
    }

    Ok(())
}

fn render_sibling_comparison(child_summaries: &[ChildSummary]) {
    if child_summaries.len() < 2 {
        return;
    }

    println!("\n**Sibling Comparison**");
    for child in child_summaries {
        let mut line = format!(
            "- {} [{}] attempts={}: {}",
            child.id, child.status, child.attempts, child.title
        );
        if let Some(outcome) = &child.recent_outcome {
            line.push_str(&format!(" recent={}", outcome));
        }
        println!("{}", line);
        if let Some(summary) = &child.summary {
            println!("  summary: {}", summary);
        }
    }
}

/// Render a unit beautifully with metadata header and formatted markdown body
fn render_unit(
    unit: &Unit,
    child_summaries: &[mana_core::ops::context::ChildSummary],
    show_all_history: bool,
) -> Result<()> {
    let skin = MadSkin::default();

    // Print metadata header
    println!("{}", render_metadata_header(unit));

    // Print title as emphasized header
    println!("\n*{}*\n", unit.title);

    // Print description with markdown formatting if it exists
    if let Some(description) = &unit.description {
        let formatted = skin.term_text(description);
        println!("{}", formatted);
    }

    // Print acceptance criteria
    if let Some(acceptance) = &unit.acceptance {
        println!("\n**Acceptance Criteria**");
        let formatted = skin.term_text(acceptance);
        println!("{}", formatted);
    }

    // Print verify command
    if let Some(verify) = &unit.verify {
        println!("\n**Verify Command**");
        println!("```");
        println!("{}", verify);
        println!("```");
    }

    // Print design notes
    if let Some(design) = &unit.design {
        println!("\n**Design**");
        let formatted = skin.term_text(design);
        println!("{}", formatted);
    }

    // Print decisions
    if !unit.decisions.is_empty() {
        println!("\n**Decisions ({} unresolved):**", unit.decisions.len());
        for (i, decision) in unit.decisions.iter().enumerate() {
            println!("  {}: {}", i, decision);
        }
    }

    // Print notes
    if let Some(notes) = &unit.notes {
        println!("\n**Notes**");
        let formatted = skin.term_text(notes);
        println!("{}", formatted);
    }

    if !child_summaries.is_empty() {
        println!("\n**Child Job Summaries**");
        for child in child_summaries {
            let mut line = format!("- {} [{}] attempts={}", child.id, child.status, child.attempts);
            if let Some(outcome) = &child.recent_outcome {
                line.push_str(&format!(" recent={}", outcome));
            }
            line.push_str(&format!(": {}", child.title));
            println!("{}", line);
            if let Some(summary) = &child.summary {
                println!("  summary: {}", summary);
            }
            if let Some(follow_up) = &child.follow_up {
                println!("  follow-up: {}", follow_up);
            }
        }
        render_sibling_comparison(child_summaries);
    }

    // Print outputs
    if let Some(outputs) = &unit.outputs {
        println!("\n**Outputs**");
        println!("```");
        let pretty = serde_json::to_string_pretty(outputs).unwrap_or_else(|_| outputs.to_string());
        let lines: Vec<&str> = pretty.lines().collect();
        if lines.len() > MAX_OUTPUT_LINES {
            for line in &lines[..MAX_OUTPUT_LINES] {
                println!("{}", line);
            }
            println!("... (truncated)");
        } else {
            print!("{}", pretty);
            if !pretty.ends_with('\n') {
                println!();
            }
        }
        println!("```");
    }

    // Print history section if non-empty
    if !unit.history.is_empty() {
        let limit = if show_all_history {
            unit.history.len()
        } else {
            DEFAULT_HISTORY_LIMIT
        };
        println!("\n{}", render_history(&unit.history, limit));
    }

    Ok(())
}

/// Render metadata header with ID, status, priority, and dates
fn render_metadata_header(unit: &Unit) -> String {
    let separator = "━".repeat(40);
    let status_str = format!("Status: {}", unit.status);
    let priority_str = format!("Priority: P{}", unit.priority);

    let header_line = format!("  ID: {}  |  {}  |  {}", unit.id, status_str, priority_str);

    // Build metadata details with optional fields
    let mut details = Vec::new();

    if let Some(parent) = &unit.parent {
        details.push(format!("Parent: {}", parent));
    }

    if !unit.dependencies.is_empty() {
        details.push(format!("Dependencies: {}", unit.dependencies.join(", ")));
    }

    if let Some(assignee) = &unit.assignee {
        details.push(format!("Assignee: {}", assignee));
    }

    if !unit.labels.is_empty() {
        details.push(format!("Labels: {}", unit.labels.join(", ")));
    }

    // Format dates nicely
    let created = unit.created_at.format("%Y-%m-%d %H:%M:%S UTC");
    let updated = unit.updated_at.format("%Y-%m-%d %H:%M:%S UTC");
    details.push(format!("Created: {}", created));
    details.push(format!("Updated: {}", updated));

    if let Some(closed_at) = unit.closed_at {
        let closed = closed_at.format("%Y-%m-%d %H:%M:%S UTC");
        details.push(format!("Closed: {}", closed));
    }

    if let Some(reason) = &unit.close_reason {
        details.push(format!("Close reason: {}", reason));
    }

    // Show claim information
    if let Some(claimed_by) = &unit.claimed_by {
        details.push(format!("Claimed by: {}", claimed_by));
    }
    if let Some(claimed_at) = unit.claimed_at {
        let claimed = claimed_at.format("%Y-%m-%d %H:%M:%S UTC");
        details.push(format!("Claimed at: {}", claimed));
    }

    let mut output = String::new();
    output.push_str(&separator);
    output.push('\n');
    output.push_str(&header_line);
    output.push('\n');
    output.push_str(&separator);

    if !details.is_empty() {
        output.push_str("\n\n");
        output.push_str(&details.join("\n"));
    }

    output
}

/// Format a duration in seconds to a human-readable string.
///
/// - Under 60s: `12.3s`
/// - Under 3600s: `2m 15s`
/// - 3600s+: `1h 5m`
fn format_duration(secs: f64) -> String {
    if secs < 60.0 {
        format!("{:.1}s", secs)
    } else if secs < 3600.0 {
        let mins = (secs / 60.0).floor() as u64;
        let remainder = (secs % 60.0).round() as u64;
        format!("{}m {}s", mins, remainder)
    } else {
        let hours = (secs / 3600.0).floor() as u64;
        let remainder_mins = ((secs % 3600.0) / 60.0).round() as u64;
        format!("{}h {}m", hours, remainder_mins)
    }
}

/// Format a token count with `k` suffix for thousands.
///
/// - Under 1000: `500`
/// - Exact thousands (e.g. 12000): `12k`
/// - Otherwise: `8.2k`
fn format_tokens(tokens: u64) -> String {
    if tokens < 1000 {
        tokens.to_string()
    } else if tokens % 1000 == 0 {
        format!("{}k", tokens / 1000)
    } else {
        // Round to nearest hundred for one decimal place
        let k = tokens as f64 / 1000.0;
        format!("{:.1}k", k)
    }
}

/// Format a cost as `$X.XX`, or empty string if `None`.
fn format_cost(cost: f64) -> String {
    format!("${:.2}", cost)
}

/// Truncate a string to `max_len` characters, appending `…` if truncated.
fn truncate_agent(agent: &str, max_len: usize) -> String {
    if agent.len() <= max_len {
        agent.to_string()
    } else {
        let mut s = agent[..max_len - 1].to_string();
        s.push('…');
        s
    }
}

/// Render the history table from a slice of `RunRecord`.
///
/// Shows the most recent `limit` entries. Includes a totals line at the bottom.
fn render_history(history: &[RunRecord], limit: usize) -> String {
    let total = history.len();
    let entries: &[RunRecord] = if total > limit {
        &history[total - limit..]
    } else {
        history
    };

    let mut out = String::from("**History**\n");

    // Table header
    out.push_str("  #  Result     Duration  Agent         Exit  Tokens  Cost\n");

    for record in entries {
        let attempt = format!("{:>3}", record.attempt);
        let result = format!("{:<9}", format!("{:?}", record.result).to_lowercase());
        let duration = record
            .duration_secs
            .map(format_duration)
            .unwrap_or_else(|| "-".to_string());
        let duration_col = format!("{:<8}", duration);
        let agent = record
            .agent
            .as_deref()
            .map(|a| truncate_agent(a, 12))
            .unwrap_or_else(|| "-".to_string());
        let agent_col = format!("{:<12}", agent);
        let exit = record
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "-".to_string());
        let exit_col = format!("{:<4}", exit);
        let tokens = record
            .tokens
            .map(format_tokens)
            .unwrap_or_else(|| "-".to_string());
        let tokens_col = format!("{:<6}", tokens);
        let cost = record
            .cost
            .map(format_cost)
            .unwrap_or_else(|| "-".to_string());

        out.push_str(&format!(
            "  {} {}  {}  {}  {}  {}  {}\n",
            attempt, result, duration_col, agent_col, exit_col, tokens_col, cost
        ));
    }

    // Totals
    let total_duration: f64 = history.iter().filter_map(|r| r.duration_secs).sum();
    let total_tokens: u64 = history.iter().filter_map(|r| r.tokens).sum();
    let total_cost: f64 = history.iter().filter_map(|r| r.cost).sum();

    let mut totals_parts = vec![format!("{} attempts", total)];
    if total_duration > 0.0 {
        totals_parts.push(format_duration(total_duration));
    }
    if total_tokens > 0 {
        totals_parts.push(format!("{} tokens", format_tokens(total_tokens)));
    }
    if total_cost > 0.0 {
        totals_parts.push(format_cost(total_cost));
    }

    if total > limit {
        out.push_str(&format!(
            "  ... ({} earlier entries hidden)\n",
            total - limit
        ));
    }
    out.push_str(&format!("  Total: {}", totals_parts.join(", ")));

    out
}

/// Format a unit as a one-line summary
fn format_short(unit: &Unit) -> String {
    format!("{}. {} [{}]", unit.id, unit.title, unit.status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::{RunRecord, RunResult};
    use crate::util::title_to_slug;
    use chrono::Utc;
    use tempfile::TempDir;

    // ------------------------------------------------------------------
    // cmd_show integration tests
    // ------------------------------------------------------------------

    #[test]
    fn show_renders_beautifully_default() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        std::fs::create_dir(&mana_dir).unwrap();

        let unit = Unit::new("1", "Test unit");
        let slug = title_to_slug(&unit.title);
        let unit_path = mana_dir.join(format!("1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        let result = cmd_show("1", false, false, false, &mana_dir);
        assert!(result.is_ok());
    }

    #[test]
    fn show_json() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        std::fs::create_dir(&mana_dir).unwrap();

        let unit = Unit::new("1", "Test unit");
        let slug = title_to_slug(&unit.title);
        let unit_path = mana_dir.join(format!("1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        let result = cmd_show("1", true, false, false, &mana_dir);
        assert!(result.is_ok());
    }

    #[test]
    fn show_short() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        std::fs::create_dir(&mana_dir).unwrap();

        let unit = Unit::new("1", "Test unit");
        let slug = title_to_slug(&unit.title);
        let unit_path = mana_dir.join(format!("1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        let result = cmd_show("1", false, true, false, &mana_dir);
        assert!(result.is_ok());
    }

    #[test]
    fn show_archived_unit() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        let archive_dir = mana_dir.join("archive/2026/04");
        std::fs::create_dir_all(&archive_dir).unwrap();

        let mut unit = Unit::new("1", "Archived unit");
        unit.is_archived = true;
        let slug = title_to_slug(&unit.title);
        let unit_path = archive_dir.join(format!("1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        let result = cmd_show("1", false, false, false, &mana_dir);
        assert!(result.is_ok());
    }

    #[test]
    fn show_not_found() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        std::fs::create_dir(&mana_dir).unwrap();

        let result = cmd_show("999", false, false, false, &mana_dir);
        assert!(result.is_err());
    }

    #[test]
    fn format_short_test() {
        let unit = Unit::new("42", "My task");
        let formatted = format_short(&unit);
        assert_eq!(formatted, "42. My task [open]");
    }

    #[test]
    fn metadata_header_includes_id_and_status() {
        let unit = Unit::new("1", "Test");
        let header = render_metadata_header(&unit);
        assert!(header.contains("ID: 1"));
        assert!(header.contains("Status: open"));
    }

    #[test]
    fn metadata_header_includes_parent_when_set() {
        let mut unit = Unit::new("1.1", "Child task");
        unit.parent = Some("1".to_string());
        let header = render_metadata_header(&unit);
        assert!(header.contains("Parent: 1"));
    }

    #[test]
    fn metadata_header_includes_dependencies() {
        let mut unit = Unit::new("2", "Task");
        unit.dependencies = vec!["1".to_string(), "1.1".to_string()];
        let header = render_metadata_header(&unit);
        assert!(header.contains("Dependencies: 1, 1.1"));
    }

    #[test]
    fn render_unit_with_description() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        std::fs::create_dir(&mana_dir).unwrap();

        let mut unit = Unit::new("1", "Test unit");
        unit.description = Some("# Description\n\nThis is test markdown.".to_string());
        let slug = title_to_slug(&unit.title);
        let unit_path = mana_dir.join(format!("1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        let result = cmd_show("1", false, false, false, &mana_dir);
        assert!(result.is_ok());
    }

    #[test]
    fn show_works_with_hierarchical_ids() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        std::fs::create_dir(&mana_dir).unwrap();

        let unit = Unit::new("11.1", "Hierarchical unit");
        let slug = title_to_slug(&unit.title);
        let unit_path = mana_dir.join(format!("11.1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        let result = cmd_show("11.1", false, false, false, &mana_dir);
        assert!(result.is_ok());
    }

    // ------------------------------------------------------------------
    // Duration formatting
    // ------------------------------------------------------------------

    #[test]
    fn history_format_duration_seconds() {
        assert_eq!(format_duration(0.0), "0.0s");
        assert_eq!(format_duration(12.3), "12.3s");
        assert_eq!(format_duration(59.9), "59.9s");
    }

    #[test]
    fn history_format_duration_minutes() {
        assert_eq!(format_duration(60.0), "1m 0s");
        assert_eq!(format_duration(135.0), "2m 15s");
        assert_eq!(format_duration(3599.0), "59m 59s");
    }

    #[test]
    fn history_format_duration_hours() {
        assert_eq!(format_duration(3600.0), "1h 0m");
        assert_eq!(format_duration(3900.0), "1h 5m");
        assert_eq!(format_duration(7200.0), "2h 0m");
    }

    // ------------------------------------------------------------------
    // Token formatting
    // ------------------------------------------------------------------

    #[test]
    fn history_format_tokens_small() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(500), "500");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn history_format_tokens_thousands() {
        assert_eq!(format_tokens(1000), "1k");
        assert_eq!(format_tokens(8200), "8.2k");
        assert_eq!(format_tokens(12400), "12.4k");
        assert_eq!(format_tokens(12000), "12k");
    }

    // ------------------------------------------------------------------
    // Cost formatting
    // ------------------------------------------------------------------

    #[test]
    fn history_format_cost() {
        assert_eq!(format_cost(0.0), "$0.00");
        assert_eq!(format_cost(0.03), "$0.03");
        assert_eq!(format_cost(1.5), "$1.50");
    }

    // ------------------------------------------------------------------
    // Agent truncation
    // ------------------------------------------------------------------

    #[test]
    fn history_truncate_agent_short() {
        assert_eq!(truncate_agent("pi-abc123", 12), "pi-abc123");
        assert_eq!(truncate_agent("exactly12chr", 12), "exactly12chr");
    }

    #[test]
    fn history_truncate_agent_long() {
        assert_eq!(
            truncate_agent("pi-very-long-agent-name", 12),
            "pi-very-lon…"
        );
    }

    // ------------------------------------------------------------------
    // History rendering
    // ------------------------------------------------------------------

    fn make_record(
        attempt: u32,
        result: RunResult,
        duration: f64,
        agent: &str,
        exit: i32,
        tokens: u64,
        cost: f64,
    ) -> RunRecord {
        RunRecord {
            attempt,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            duration_secs: Some(duration),
            agent: Some(agent.to_string()),
            result,
            exit_code: Some(exit),
            tokens: Some(tokens),
            cost: Some(cost),
            output_snippet: None,
            autonomy_observation: None,
        }
    }

    #[test]
    fn history_not_shown_when_empty() {
        let unit = Unit::new("1", "No history");
        assert!(unit.history.is_empty());
        // render_history is never called when history is empty, but verify it
        // produces a sensible output anyway
        let rendered = render_history(&[], 10);
        assert!(rendered.contains("0 attempts"));
    }

    #[test]
    fn history_displays_formatted_table() {
        let records = vec![
            make_record(1, RunResult::Fail, 12.3, "pi-abc123", 1, 8200, 0.04),
            make_record(2, RunResult::Fail, 8.1, "pi-def456", 1, 6100, 0.03),
            make_record(3, RunResult::Pass, 15.7, "pi-ghi789", 0, 12400, 0.05),
        ];

        let rendered = render_history(&records, 10);

        // Header present
        assert!(rendered.contains("**History**"));
        assert!(rendered.contains("Result"));
        assert!(rendered.contains("Duration"));
        assert!(rendered.contains("Agent"));
        assert!(rendered.contains("Tokens"));

        // Row content
        assert!(rendered.contains("fail"));
        assert!(rendered.contains("pass"));
        assert!(rendered.contains("12.3s"));
        assert!(rendered.contains("8.1s"));
        assert!(rendered.contains("15.7s"));
        assert!(rendered.contains("pi-abc123"));
        assert!(rendered.contains("8.2k"));
        assert!(rendered.contains("6.1k"));
        assert!(rendered.contains("12.4k"));
    }

    #[test]
    fn history_totals_sum_correctly() {
        let records = vec![
            make_record(1, RunResult::Fail, 12.3, "a", 1, 8200, 0.04),
            make_record(2, RunResult::Fail, 8.1, "b", 1, 6100, 0.03),
            make_record(3, RunResult::Pass, 15.7, "c", 0, 12400, 0.05),
        ];

        let rendered = render_history(&records, 10);

        assert!(rendered.contains("3 attempts"));
        // Total duration: 36.1s
        assert!(rendered.contains("36.1s"));
        // Total tokens: 26700 → 26.7k
        assert!(rendered.contains("26.7k tokens"));
        // Total cost: $0.12
        assert!(rendered.contains("$0.12"));
    }

    #[test]
    fn history_limits_entries_default() {
        // Create 15 records, limit to 10. Use exit code 0 to avoid
        // ambiguous substring matches with attempt numbers.
        let records: Vec<RunRecord> = (1..=15)
            .map(|i| make_record(i, RunResult::Fail, 1.0, "agent", 0, 1000, 0.01))
            .collect();

        let rendered = render_history(&records, 10);

        // Should mention hidden entries
        assert!(rendered.contains("5 earlier entries hidden"));
        // Totals are over ALL 15
        assert!(rendered.contains("15 attempts"));

        // Entries 1-5 hidden, 6-15 shown.
        // Check attempt column (right-aligned 3 chars) at line start.
        let data_lines: Vec<&str> = rendered
            .lines()
            .filter(|l| {
                l.starts_with("  ")
                    && !l.starts_with("  #")
                    && !l.starts_with("  Total")
                    && !l.starts_with("  ...")
            })
            .collect();
        assert_eq!(data_lines.len(), 10);
        // First visible attempt is 6
        assert!(data_lines[0].contains("  6 "));
        // Last visible attempt is 15
        assert!(data_lines[9].contains(" 15 "));
    }

    #[test]
    fn history_show_all_flag() {
        let records: Vec<RunRecord> = (1..=15)
            .map(|i| make_record(i, RunResult::Fail, 1.0, "agent", 0, 1000, 0.01))
            .collect();

        // With limit = total, all shown
        let rendered = render_history(&records, 15);
        assert!(!rendered.contains("hidden"));

        let data_lines: Vec<&str> = rendered
            .lines()
            .filter(|l| l.starts_with("  ") && !l.starts_with("  #") && !l.starts_with("  Total"))
            .collect();
        assert_eq!(data_lines.len(), 15);
    }

    #[test]
    fn history_handles_missing_optional_fields() {
        let record = RunRecord {
            attempt: 1,
            started_at: Utc::now(),
            finished_at: None,
            duration_secs: None,
            agent: None,
            result: RunResult::Timeout,
            exit_code: None,
            tokens: None,
            cost: None,
            output_snippet: None,
            autonomy_observation: None,
        };

        let rendered = render_history(&[record], 10);
        assert!(rendered.contains("timeout"));
        // Missing fields show as "-"
        // Count dashes in the row (duration, agent, exit, tokens, cost)
        let row_line = rendered.lines().nth(2).unwrap(); // first data row
        let dashes = row_line.matches(" - ").count() + row_line.matches(" -\n").count();
        assert!(
            dashes >= 3,
            "Expected dashes for missing fields, got line: {}",
            row_line
        );
    }

    #[test]
    fn history_cmd_show_with_history() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        std::fs::create_dir(&mana_dir).unwrap();

        let mut unit = Unit::new("1", "Unit with history");
        unit.history = vec![
            make_record(1, RunResult::Fail, 5.0, "pi-test", 1, 3000, 0.02),
            make_record(2, RunResult::Pass, 3.0, "pi-test", 0, 2000, 0.01),
        ];
        let slug = title_to_slug(&unit.title);
        let unit_path = mana_dir.join(format!("1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        // Without --history flag (still shows, just limited to 10)
        let result = cmd_show("1", false, false, false, &mana_dir);
        assert!(result.is_ok());

        // With --history flag
        let result = cmd_show("1", false, false, true, &mana_dir);
        assert!(result.is_ok());
    }

    // ------------------------------------------------------------------
    // Outputs display
    // ------------------------------------------------------------------

    #[test]
    fn render_unit_accepts_multiple_child_summaries_for_comparison() {
        let unit = Unit::new("1", "Parent");
        let child_summaries = vec![
            ChildSummary {
                id: "1.1".to_string(),
                title: "Attempt A".to_string(),
                status: "open".to_string(),
                attempts: 2,
                recent_outcome: Some("failed".to_string()),
                summary: Some("Tried parser branch A".to_string()),
                follow_up: None,
            },
            ChildSummary {
                id: "1.2".to_string(),
                title: "Attempt B".to_string(),
                status: "closed".to_string(),
                attempts: 1,
                recent_outcome: Some("success".to_string()),
                summary: Some("Fixed it via branch B".to_string()),
                follow_up: None,
            },
        ];

        let result = render_unit(&unit, &child_summaries, false);
        assert!(result.is_ok());
    }

    #[test]
    fn outputs_not_shown_when_none() {
        let unit = Unit::new("1", "No outputs");
        // render_unit prints to stdout; just verify it doesn't panic
        let result = render_unit(&unit, &[], false);
        assert!(result.is_ok());
    }

    #[test]
    fn outputs_shows_pretty_printed_json() {
        let dir = TempDir::new().unwrap();
        let mana_dir = dir.path().join(".mana");
        std::fs::create_dir(&mana_dir).unwrap();

        let mut unit = Unit::new("1", "With outputs");
        unit.outputs = Some(serde_json::json!({
            "coverage": 85.5,
            "files": ["a.rs", "b.rs"]
        }));
        let slug = title_to_slug(&unit.title);
        let unit_path = mana_dir.join(format!("1-{}.md", slug));
        unit.to_file(&unit_path).unwrap();

        let result = cmd_show("1", false, false, false, &mana_dir);
        assert!(result.is_ok());
    }

    #[test]
    fn outputs_long_truncated_at_50_lines() {
        // Build a JSON value that pretty-prints to >50 lines
        let map: serde_json::Map<String, serde_json::Value> = (0..60)
            .map(|i| (format!("key_{}", i), serde_json::json!(i)))
            .collect();
        let big_obj = serde_json::Value::Object(map);
        let pretty = serde_json::to_string_pretty(&big_obj).unwrap();
        let lines: Vec<&str> = pretty.lines().collect();
        assert!(
            lines.len() > MAX_OUTPUT_LINES,
            "test setup: need >50 lines, got {}",
            lines.len()
        );

        let mut unit = Unit::new("1", "Big outputs");
        unit.outputs = Some(big_obj);
        // Just verify render_unit doesn't panic and works
        let result = render_unit(&unit, &[], false);
        assert!(result.is_ok());
    }
}
