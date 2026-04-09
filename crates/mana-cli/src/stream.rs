use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RunRuntimeInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// JSON-line events emitted by `mana run --json-stream` for programmatic consumers.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(dead_code)]
pub enum StreamEvent {
    RunStart {
        parent_id: String,
        total_units: usize,
        total_rounds: usize,
        units: Vec<UnitInfo>,
        #[serde(skip_serializing_if = "Option::is_none")]
        runtime: Option<RunRuntimeInfo>,
    },
    /// Emitted at run start with the full execution plan and detected file overlaps.
    RunPlan {
        parent_id: String,
        waves: Vec<RoundPlan>,
        file_overlaps: Vec<FileOverlapInfo>,
        total_units: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        runtime: Option<RunRuntimeInfo>,
    },
    RoundStart {
        round: usize,
        total_rounds: usize,
        unit_count: usize,
    },
    UnitStart {
        id: String,
        title: String,
        round: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_overlaps: Option<Vec<FileOverlapInfo>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        attempt: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        priority: Option<u8>,
    },
    /// Emitted when a unit becomes ready because a dependency just completed.
    UnitReady {
        id: String,
        title: String,
        unblocked_by: String,
    },
    UnitThinking {
        id: String,
        text: String,
    },
    UnitTool {
        id: String,
        tool_name: String,
        tool_count: usize,
        file_path: Option<String>,
    },
    UnitTokens {
        id: String,
        input_tokens: u64,
        output_tokens: u64,
        cache_read: u64,
        cache_write: u64,
        cost: f64,
    },
    UnitDone {
        id: String,
        success: bool,
        duration_secs: u64,
        error: Option<String>,
        total_tokens: Option<u64>,
        total_cost: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_count: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        turns: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        failure_summary: Option<String>,
    },
    RoundEnd {
        round: usize,
        success_count: usize,
        failed_count: usize,
    },
    RunEnd {
        /// Backward-compatible alias for `total_closed`.
        total_success: usize,
        total_closed: usize,
        /// Backward-compatible aggregate: failed + abandoned.
        total_failed: usize,
        total_abandoned: usize,
        total_awaiting_verify: usize,
        total_skipped: usize,
        duration_secs: u64,
    },
    BatchVerify {
        commands_run: usize,
        passed: Vec<String>,
        failed: Vec<String>,
    },
    VerifyGroupRun {
        command: String,
        unit_ids: Vec<String>,
        success: bool,
    },
    DryRun {
        parent_id: String,
        rounds: Vec<RoundPlan>,
        /// IDs of units on the critical path, in order.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        critical_path: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        runtime: Option<RunRuntimeInfo>,
    },
    Error {
        message: String,
    },
}

/// Metadata about a single unit within a run.
#[derive(Debug, Clone, Serialize)]
pub struct UnitInfo {
    pub id: String,
    pub title: String,
    pub round: usize,
}

/// Describes which units will execute in a given round (used by `DryRun`).
#[derive(Debug, Clone, Serialize)]
pub struct RoundPlan {
    pub round: usize,
    pub units: Vec<UnitInfo>,
    /// Maximum number of units that can actually run concurrently (accounting for file conflicts).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_concurrency: Option<usize>,
    /// File conflicts within this round: each entry is (file_path, [conflicting_unit_ids]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<(String, Vec<String>)>,
}

/// Describes a file overlap between two units that may run concurrently.
#[derive(Debug, Clone, Serialize)]
pub struct FileOverlapInfo {
    pub unit_id: String,
    pub other_unit_id: String,
    pub shared_files: Vec<String>,
}

/// Write a single JSON line to stdout for the given event.
pub fn emit(event: &StreamEvent) {
    if let Some(sink) = current_sink() {
        sink(event.clone());
        return;
    }

    if let Ok(json) = serde_json::to_string(event) {
        println!("{json}");
    }
}

/// Install an in-process sink for stream events.
///
/// While a sink is installed, [`emit`] delivers cloned events to it instead of
/// printing JSON lines to stdout. Dropping the returned guard restores the
/// default stdout behavior.
pub fn install_sink(sink: StreamSink) -> StreamSinkGuard {
    let cell = stream_sink();
    let mut guard = cell.lock().unwrap();
    *guard = Some(sink);
    StreamSinkGuard
}

/// Shared callback type for in-process stream consumers.
pub type StreamSink = Arc<dyn Fn(StreamEvent) + Send + Sync>;

/// RAII guard returned by [`install_sink`].
pub struct StreamSinkGuard;

impl Drop for StreamSinkGuard {
    fn drop(&mut self) {
        if let Ok(mut guard) = stream_sink().lock() {
            *guard = None;
        }
    }
}

fn stream_sink() -> &'static Mutex<Option<StreamSink>> {
    static STREAM_SINK: OnceLock<Mutex<Option<StreamSink>>> = OnceLock::new();
    STREAM_SINK.get_or_init(|| Mutex::new(None))
}

fn current_sink() -> Option<StreamSink> {
    stream_sink().lock().ok().and_then(|guard| guard.clone())
}

/// Convenience wrapper to emit an `Error` event.
pub fn emit_error(message: &str) {
    emit(&StreamEvent::Error {
        message: message.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_event_serializes_with_type_tag() {
        let event = StreamEvent::RunStart {
            parent_id: "42".into(),
            total_units: 3,
            total_rounds: 2,
            units: vec![UnitInfo {
                id: "42.1".into(),
                title: "first".into(),
                round: 1,
            }],
            runtime: None,
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "run_start");
        assert_eq!(json["parent_id"], "42");
        assert_eq!(json["total_units"], 3);
        assert_eq!(json["units"][0]["id"], "42.1");
    }

    #[test]
    fn stream_unit_done_serializes_optional_fields() {
        let event = StreamEvent::UnitDone {
            id: "1".into(),
            success: true,
            duration_secs: 10,
            error: None,
            total_tokens: Some(500),
            total_cost: Some(0.01),
            tool_count: None,
            turns: None,
            failure_summary: None,
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "unit_done");
        assert!(json["error"].is_null());
        assert_eq!(json["total_tokens"], 500);
        // New optional fields should be absent when None
        assert!(json.get("tool_count").is_none());
        assert!(json.get("turns").is_none());
        assert!(json.get("failure_summary").is_none());
    }

    #[test]
    fn stream_unit_done_with_enriched_fields() {
        let event = StreamEvent::UnitDone {
            id: "1".into(),
            success: false,
            duration_secs: 30,
            error: Some("Exit code 1".into()),
            total_tokens: Some(1000),
            total_cost: Some(0.05),
            tool_count: Some(15),
            turns: Some(3),
            failure_summary: Some("Failed after 15 tool calls, 3 turns. Exit code 1".into()),
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "unit_done");
        assert_eq!(json["tool_count"], 15);
        assert_eq!(json["turns"], 3);
        assert_eq!(
            json["failure_summary"],
            "Failed after 15 tool calls, 3 turns. Exit code 1"
        );
    }

    #[test]
    fn stream_error_event() {
        let event = StreamEvent::Error {
            message: "something broke".into(),
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["message"], "something broke");
    }

    #[test]
    fn run_end_serializes_closed_failed_abandoned_counts() {
        let event = StreamEvent::RunEnd {
            total_success: 2,
            total_closed: 2,
            total_failed: 3,
            total_abandoned: 1,
            total_awaiting_verify: 4,
            total_skipped: 5,
            duration_secs: 42,
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "run_end");
        assert_eq!(json["total_success"], 2);
        assert_eq!(json["total_closed"], 2);
        assert_eq!(json["total_failed"], 3);
        assert_eq!(json["total_abandoned"], 1);
        assert_eq!(json["total_awaiting_verify"], 4);
        assert_eq!(json["total_skipped"], 5);
    }

    #[test]
    fn stream_dry_run_with_round_plans() {
        let event = StreamEvent::DryRun {
            parent_id: "10".into(),
            rounds: vec![
                RoundPlan {
                    round: 1,
                    units: vec![UnitInfo {
                        id: "10.1".into(),
                        title: "a".into(),
                        round: 1,
                    }],
                    effective_concurrency: None,
                    conflicts: vec![],
                },
                RoundPlan {
                    round: 2,
                    units: vec![UnitInfo {
                        id: "10.2".into(),
                        title: "b".into(),
                        round: 2,
                    }],
                    effective_concurrency: None,
                    conflicts: vec![],
                },
            ],
            critical_path: vec![],
            runtime: None,
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "dry_run");
        assert_eq!(json["rounds"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn stream_emit_writes_json_line() {
        // Just ensure emit doesn't panic — stdout capture is not trivial in unit tests
        let event = StreamEvent::RoundEnd {
            round: 1,
            success_count: 2,
            failed_count: 0,
        };
        emit(&event);
    }

    #[test]
    fn stream_emit_error_convenience() {
        emit_error("test error");
    }

    #[test]
    fn stream_run_plan_serializes() {
        let event = StreamEvent::RunPlan {
            parent_id: "5".into(),
            waves: vec![
                RoundPlan {
                    round: 1,
                    units: vec![UnitInfo {
                        id: "5.1".into(),
                        title: "first".into(),
                        round: 1,
                    }],
                    effective_concurrency: None,
                    conflicts: vec![],
                },
                RoundPlan {
                    round: 2,
                    units: vec![UnitInfo {
                        id: "5.2".into(),
                        title: "second".into(),
                        round: 2,
                    }],
                    effective_concurrency: None,
                    conflicts: vec![],
                },
            ],
            file_overlaps: vec![FileOverlapInfo {
                unit_id: "5.1".into(),
                other_unit_id: "5.3".into(),
                shared_files: vec!["src/main.rs".into()],
            }],
            total_units: 3,
            runtime: None,
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "run_plan");
        assert_eq!(json["parent_id"], "5");
        assert_eq!(json["total_units"], 3);
        assert_eq!(json["waves"].as_array().unwrap().len(), 2);
        assert_eq!(json["file_overlaps"].as_array().unwrap().len(), 1);
        assert_eq!(json["file_overlaps"][0]["shared_files"][0], "src/main.rs");
    }

    #[test]
    fn stream_unit_ready_serializes() {
        let event = StreamEvent::UnitReady {
            id: "3".into(),
            title: "Implement parser".into(),
            unblocked_by: "2".into(),
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "unit_ready");
        assert_eq!(json["id"], "3");
        assert_eq!(json["unblocked_by"], "2");
    }

    #[test]
    fn stream_unit_start_with_enriched_fields() {
        let event = StreamEvent::UnitStart {
            id: "1".into(),
            title: "Test".into(),
            round: 1,
            file_overlaps: Some(vec![FileOverlapInfo {
                unit_id: "1".into(),
                other_unit_id: "2".into(),
                shared_files: vec!["lib.rs".into()],
            }]),
            attempt: Some(2),
            priority: Some(1),
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "unit_start");
        assert_eq!(json["attempt"], 2);
        assert_eq!(json["priority"], 1);
        assert_eq!(json["file_overlaps"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn stream_unit_start_omits_none_fields() {
        let event = StreamEvent::UnitStart {
            id: "1".into(),
            title: "Test".into(),
            round: 1,
            file_overlaps: None,
            attempt: None,
            priority: None,
        };
        let json: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "unit_start");
        assert_eq!(json["id"], "1");
        // Optional fields should be absent when None
        assert!(json.get("file_overlaps").is_none());
        assert!(json.get("attempt").is_none());
        assert!(json.get("priority").is_none());
    }

    #[test]
    fn stream_file_overlap_info_serializes() {
        let info = FileOverlapInfo {
            unit_id: "A".into(),
            other_unit_id: "B".into(),
            shared_files: vec!["src/main.rs".into(), "src/lib.rs".into()],
        };
        let json: serde_json::Value = serde_json::to_value(&info).unwrap();
        assert_eq!(json["unit_id"], "A");
        assert_eq!(json["other_unit_id"], "B");
        assert_eq!(json["shared_files"].as_array().unwrap().len(), 2);
    }
}
