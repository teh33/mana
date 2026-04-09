use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Open,
    InProgress,
    /// Agent has finished work; runner will run the verify command on its behalf.
    AwaitingVerify,
    Closed,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Status::Open => write!(f, "open"),
            Status::InProgress => write!(f, "in_progress"),
            Status::AwaitingVerify => write!(f, "awaiting_verify"),
            Status::Closed => write!(f, "closed"),
        }
    }
}

// ---------------------------------------------------------------------------
// Autonomy disposition / observation vocabulary
// ---------------------------------------------------------------------------

/// Top-level scheduler-facing autonomy outcome for a unit's current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyDispositionKind {
    Eligible,
    Blocked,
    RequiresHuman,
    Unknown,
}

/// Typed blocker codes explaining why a unit is not autonomously eligible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyBlockerCode {
    UnresolvedDecision,
    HumanCloseRequired,
    ApprovalRequired,
    ReviewRequired,
    ReviewPending,
    ReviewRejected,
    VerifyAbsent,
    VerifyDeferred,
    VerifyFailed,
    VerifyFrozenViolation,
    VerifyQualityUnknown,
    VisibilityMissing,
    AttemptBudgetExhausted,
    CircuitBreakerTripped,
    RiskTooHigh,
    PolicyUnknown,
}

/// Current review / approval posture for autonomy gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Unknown,
    NotRequired,
    Required,
    Pending,
    Approved,
    Rejected,
}

/// Current verify posture for scheduler-facing autonomy gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyPosture {
    Unknown,
    NotApplicable,
    Absent,
    Deferred,
    Passing,
    Failed,
    FrozenViolation,
    QualityUnknown,
}

/// Whether the unit has enough durable visibility/context for autonomous work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityState {
    Unknown,
    Satisfied,
    Missing,
    NotApplicable,
}

/// Normalized retry / attempt pressure relevant to autonomy gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptPressure {
    Unknown,
    WithinBudget,
    NearLimit,
    Exhausted,
    CircuitBreakerTripped,
}

/// Normalized current risk band for autonomous continuation decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskBand {
    Unknown,
    Low,
    Normal,
    High,
    Critical,
}

/// Provenance of the current autonomy disposition or observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyProvenance {
    Unknown,
    AttemptObservation,
    CloseEvidence,
    Mixed,
}

/// Current scheduler-facing autonomy answer persisted on a unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutonomyDisposition {
    pub kind: AutonomyDispositionKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<AutonomyBlockerCode>,
    pub review: ReviewState,
    pub verify: VerifyPosture,
    pub visibility: VisibilityState,
    pub attempt_pressure: AttemptPressure,
    pub risk: RiskBand,
    pub provenance: AutonomyProvenance,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_budget: Option<u32>,
}

/// Attempt- or verify-scoped autonomy observation retained as evidence.
///
/// This is intentionally limited to typed, durable visibility evidence that
/// `imp` can publish upstream for later policy evaluation in `mana-core`.
/// It must not carry raw confidence, confidence thresholds, model self-report,
/// scheduler commands, or free-text heuristic export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutonomyObservation {
    pub visibility: VisibilityState,
    pub provenance: AutonomyProvenance,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_attempt: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation_budget_delta: Option<i32>,
}

impl Default for AutonomyDisposition {
    fn default() -> Self {
        Self {
            kind: AutonomyDispositionKind::Unknown,
            blockers: Vec::new(),
            review: ReviewState::Unknown,
            verify: VerifyPosture::Unknown,
            visibility: VisibilityState::Unknown,
            attempt_pressure: AttemptPressure::Unknown,
            risk: RiskBand::Unknown,
            provenance: AutonomyProvenance::Unknown,
            continuation_budget: None,
        }
    }
}

impl AutonomyDisposition {
    pub fn unknown() -> Self {
        Self::default()
    }
}

/// Normalized attempt-pressure derivation produced from durable retry state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptPressureEvaluation {
    pub pressure: AttemptPressure,
    pub blockers: Vec<AutonomyBlockerCode>,
    pub continuation_budget: Option<u32>,
    pub budget_limit: u32,
    pub recent_failure_streak: u32,
}

/// Resolve the scheduler-facing attempt budget from durable unit policy.
///
/// `on_fail.retry.max` overrides the unit-level `max_attempts`; other on-fail
/// actions keep using the unit-level budget.
pub fn effective_attempt_budget(max_attempts: u32, on_fail: Option<&OnFailAction>) -> u32 {
    match on_fail {
        Some(OnFailAction::Retry { max: Some(max), .. }) => *max,
        _ => max_attempts,
    }
}

/// Derive typed attempt pressure from durable retry state.
///
/// Rules:
/// - circuit-breaker label is a hard tripped state
/// - escalate-on-fail plus any recent failure means no autonomous retry budget remains
/// - attempts at or beyond the effective budget are exhausted
/// - one remaining attempt or a streak of recent failed/abandoned outcomes is near-limit
/// - otherwise the unit remains within budget
pub fn derive_attempt_pressure(
    attempts: u32,
    max_attempts: u32,
    on_fail: Option<&OnFailAction>,
    labels: &[String],
    attempt_log: &[AttemptRecord],
    history: &[RunRecord],
) -> AttemptPressureEvaluation {
    let budget_limit = effective_attempt_budget(max_attempts, on_fail);
    let recent_failure_streak = recent_failure_streak(attempt_log, history);
    let circuit_breaker_tripped = labels.iter().any(|label| label == "circuit-breaker");
    let escalate_on_failure = matches!(on_fail, Some(OnFailAction::Escalate { .. }));

    let (pressure, blockers, continuation_budget) = if circuit_breaker_tripped {
        (
            AttemptPressure::CircuitBreakerTripped,
            vec![AutonomyBlockerCode::CircuitBreakerTripped],
            Some(0),
        )
    } else if escalate_on_failure && recent_failure_streak > 0 {
        (
            AttemptPressure::Exhausted,
            vec![AutonomyBlockerCode::AttemptBudgetExhausted],
            Some(0),
        )
    } else {
        let remaining = budget_limit.saturating_sub(attempts);
        if attempts >= budget_limit {
            (
                AttemptPressure::Exhausted,
                vec![AutonomyBlockerCode::AttemptBudgetExhausted],
                Some(0),
            )
        } else if remaining <= 1 || recent_failure_streak >= 2 {
            (AttemptPressure::NearLimit, Vec::new(), Some(remaining))
        } else {
            (AttemptPressure::WithinBudget, Vec::new(), Some(remaining))
        }
    };

    AttemptPressureEvaluation {
        pressure,
        blockers,
        continuation_budget,
        budget_limit,
        recent_failure_streak,
    }
}

fn recent_failure_streak(attempt_log: &[AttemptRecord], history: &[RunRecord]) -> u32 {
    recent_attempt_failure_streak(attempt_log).max(recent_verify_failure_streak(history))
}

fn recent_attempt_failure_streak(attempt_log: &[AttemptRecord]) -> u32 {
    let mut streak = 0;
    for attempt in attempt_log.iter().rev() {
        if attempt.finished_at.is_none() {
            continue;
        }
        if matches!(attempt.outcome, AttemptOutcome::Success) {
            break;
        }
        streak += 1;
    }
    streak
}

fn recent_verify_failure_streak(history: &[RunRecord]) -> u32 {
    let mut streak = 0;
    for run in history.iter().rev() {
        if matches!(run.result, RunResult::Pass) {
            break;
        }
        streak += 1;
    }
    streak
}

// ---------------------------------------------------------------------------
// RunResult / RunRecord (verification history)
// ---------------------------------------------------------------------------

/// Outcome of a verification run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunResult {
    Pass,
    Fail,
    Timeout,
    Cancelled,
}

/// A single verification run record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub attempt: u32,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub result: RunResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_snippet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autonomy_observation: Option<AutonomyObservation>,
}

// ---------------------------------------------------------------------------
// OnCloseAction
// ---------------------------------------------------------------------------

/// Declarative action to run when a unit's verify command fails.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum OnFailAction {
    /// Retry with optional max attempts and delay.
    Retry {
        #[serde(skip_serializing_if = "Option::is_none")]
        max: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        delay_secs: Option<u64>,
    },
    /// Bump priority and add message.
    Escalate {
        #[serde(skip_serializing_if = "Option::is_none")]
        priority: Option<u8>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

/// Declarative actions to run when a unit is closed.
/// Processed after the unit is archived and post-close hook fires.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum OnCloseAction {
    /// Run a shell command in the project root.
    Run { command: String },
    /// Print a notification message.
    Notify { message: String },
}

// ---------------------------------------------------------------------------
// AttemptRecord (for memory system attempt tracking)
// ---------------------------------------------------------------------------

/// Outcome of a claim→close cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    Success,
    Failed,
    Abandoned,
}

/// A single attempt record (claim→close cycle).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub num: u32,
    pub outcome: AttemptOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autonomy_observation: Option<AutonomyObservation>,
}

// ---------------------------------------------------------------------------
// Durable approval / promotion schema
// ---------------------------------------------------------------------------

/// Durable outcome of the review gate policy applied before approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewGateOutcome {
    Skipped,
    Optional,
    Mandatory,
}

impl std::fmt::Display for ReviewGateOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReviewGateOutcome::Skipped => write!(f, "skipped"),
            ReviewGateOutcome::Optional => write!(f, "optional"),
            ReviewGateOutcome::Mandatory => write!(f, "mandatory"),
        }
    }
}

/// Approval-layer decision vocabulary.
///
/// This remains distinct from `mana-review`'s `ReviewDecision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approved,
    Withheld,
    Denied,
}

impl std::fmt::Display for ApprovalDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApprovalDecision::Approved => write!(f, "approved"),
            ApprovalDecision::Withheld => write!(f, "withheld"),
            ApprovalDecision::Denied => write!(f, "denied"),
        }
    }
}

/// Lifecycle state of the durable approval record itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Active,
    Superseded,
}

/// Who or what made a durable approval or promotion decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSource {
    Human,
    HumanRole,
    PolicyEngine,
    Agent,
    ServiceAccount,
    MixedWorkflow,
}

/// Mana-owned snapshot of a risk flag used in approval provenance.
///
/// This intentionally stores durable risk lineage without depending on the
/// `mana-review` crate, which already depends on `mana-core`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RiskFlagRecord {
    /// Stable kind/code, typically derived from a review-layer flag vocabulary.
    pub code: String,
    /// Human-readable explanation for the flag.
    pub message: String,
    /// Related file paths, if any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

/// Durable provenance block for an approval decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalProvenance {
    /// Unit-level durable linkage.
    pub unit_id: String,
    /// Attempt-scoped provenance when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    /// Run-scoped provenance when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Candidate lineage consumed by the approval decision.
    pub candidate_ref: String,
    /// Evidence bundle consumed by the approval decision.
    pub evidence_bundle_ref: String,
    /// Gate policy or version used.
    pub gate_policy_ref: String,
    /// Review-gate outcome applied at decision time.
    pub review_gate_outcome: ReviewGateOutcome,
    /// Risk level snapshot, usually derived from `mana-review`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<String>,
    /// Risk flag snapshot, usually derived from `mana-review`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risk_flags: Vec<RiskFlagRecord>,
    /// Verify result refs considered by the gate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verify_refs: Vec<String>,
    /// Diff/scope evidence ref, e.g. a CloseEvidence-style artifact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_scope_ref: Option<String>,
    /// Review records considered when present.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_refs: Vec<String>,
    /// Review decision lineage when present.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_decision_refs: Vec<String>,
    /// Who or what made the approval decision.
    pub decision_source: DecisionSource,
    /// Concrete actor identity when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Timestamp for the durable decision.
    pub recorded_at: DateTime<Utc>,
}

/// Durable approval record: a post-evidence gate decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRecord {
    /// Stable identity for the approval record.
    pub approval_id: String,
    /// What durable subject was under approval.
    pub subject_ref: String,
    /// Unit-level durable linkage.
    pub unit_id: String,
    /// Attempt-scoped provenance when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    /// Run-scoped provenance when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Reference to the candidate output being evaluated.
    pub candidate_ref: String,
    /// Reference to the durable evidence bundle consumed by the gate.
    pub evidence_bundle_ref: String,
    /// Approval-layer outcome.
    pub decision: ApprovalDecision,
    /// Lifecycle state of this approval record.
    pub state: ApprovalState,
    /// Who or what made the decision.
    pub decision_source: DecisionSource,
    /// Concrete actor identity when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Timestamp of the approval decision.
    pub approved_at: DateTime<Utc>,
    /// Gate policy or version used.
    pub gate_policy_ref: String,
    /// Review gate outcome applied at the approval layer.
    pub review_gate_outcome: ReviewGateOutcome,
    /// Durable risk level snapshot, typically derived from `mana-review`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<String>,
    /// Durable risk flags snapshot, typically derived from `mana-review`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub risk_flags: Vec<RiskFlagRecord>,
    /// Verify result refs considered by the gate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verify_refs: Vec<String>,
    /// Diff/scope evidence ref, e.g. a CloseEvidence-style artifact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_scope_ref: Option<String>,
    /// Review refs considered by the gate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_refs: Vec<String>,
    /// Review decision lineage considered by the gate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_decision_refs: Vec<String>,
    /// Short explanation of why the approval decision was made.
    pub basis: String,
    /// Mandatory provenance block for cold auditability.
    pub provenance: ApprovalProvenance,
}

impl ApprovalRecord {
    /// Validate schema-level invariants for approval lineage.
    pub fn validate(&self) -> Result<(), String> {
        if self.review_gate_outcome == ReviewGateOutcome::Mandatory {
            if self.review_refs.is_empty() {
                return Err(
                    "mandatory review gate outcome requires non-empty review_refs".to_string(),
                );
            }
            if self.review_decision_refs.is_empty() {
                return Err(
                    "mandatory review gate outcome requires non-empty review_decision_refs"
                        .to_string(),
                );
            }
        }

        if self.provenance.review_gate_outcome == ReviewGateOutcome::Mandatory {
            if self.provenance.review_refs.is_empty() {
                return Err(
                    "mandatory review gate outcome requires non-empty provenance.review_refs"
                        .to_string(),
                );
            }
            if self.provenance.review_decision_refs.is_empty() {
                return Err(
                    "mandatory review gate outcome requires non-empty provenance.review_decision_refs"
                        .to_string(),
                );
            }
        }

        if self.candidate_ref != self.provenance.candidate_ref {
            return Err("approval candidate_ref must match provenance.candidate_ref".to_string());
        }

        if self.evidence_bundle_ref != self.provenance.evidence_bundle_ref {
            return Err(
                "approval evidence_bundle_ref must match provenance.evidence_bundle_ref"
                    .to_string(),
            );
        }

        if self.gate_policy_ref != self.provenance.gate_policy_ref {
            return Err("approval gate_policy_ref must match provenance.gate_policy_ref".to_string());
        }

        if self.review_gate_outcome != self.provenance.review_gate_outcome {
            return Err(
                "approval review_gate_outcome must match provenance.review_gate_outcome"
                    .to_string(),
            );
        }

        Ok(())
    }
}

/// Kind of durable promotion that happened after approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionKind {
    AcceptCandidate,
    CloseUnit,
    RegisterArtifact,
    PublishResult,
    PromoteFact,
    UnblockDependencies,
}

/// Structured effect of a promotion step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionEffect {
    /// What kind of durable effect occurred.
    pub kind: String,
    /// Target affected by the effect when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<String>,
    /// Human-readable summary of the effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Durable provenance block for a promotion event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionProvenance {
    /// Approval record that authorized the transition.
    pub approval_ref: String,
    /// Evidence bundle retained for lineage.
    pub evidence_bundle_ref: String,
    /// Review linkage when applicable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_refs: Vec<String>,
    /// Candidate lineage consumed by the promotion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_ref: Option<String>,
    /// Who or what applied the promotion.
    pub promotion_source: DecisionSource,
    /// Concrete actor identity when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub promoted_by: Option<String>,
    /// Timestamp for the promotion event.
    pub promoted_at: DateTime<Utc>,
}

/// Durable promotion record: an already-approved state transition that happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionRecord {
    /// Stable identity for the promotion event.
    pub promotion_id: String,
    /// What durable subject was promoted.
    pub subject_ref: String,
    /// Unit-level durable linkage.
    pub unit_id: String,
    /// Approval record that authorized the promotion.
    pub approval_ref: String,
    /// Evidence bundle retained for lineage and auditability.
    pub evidence_bundle_ref: String,
    /// Review linkage when applicable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review_refs: Vec<String>,
    /// Kind of promotion that occurred.
    pub promotion_kind: PromotionKind,
    /// Prior durable state.
    pub from_state: String,
    /// Resulting durable state.
    pub to_state: String,
    /// Structured durable consequences of the promotion.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<PromotionEffect>,
    /// Timestamp when promotion was applied.
    pub promoted_at: DateTime<Utc>,
    /// Who or what applied the promotion.
    pub promotion_source: DecisionSource,
    /// Concrete actor identity when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub promoted_by: Option<String>,
    /// Mandatory provenance block for cold auditability.
    pub provenance: PromotionProvenance,
}

impl PromotionRecord {
    /// Validate schema-level invariants for promotion lineage.
    pub fn validate(&self) -> Result<(), String> {
        if self.approval_ref != self.provenance.approval_ref {
            return Err("promotion approval_ref must match provenance.approval_ref".to_string());
        }

        if self.evidence_bundle_ref != self.provenance.evidence_bundle_ref {
            return Err(
                "promotion evidence_bundle_ref must match provenance.evidence_bundle_ref"
                    .to_string(),
            );
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_serializes_as_lowercase() {
        let open = serde_yml::to_string(&Status::Open).unwrap();
        let in_progress = serde_yml::to_string(&Status::InProgress).unwrap();
        let closed = serde_yml::to_string(&Status::Closed).unwrap();

        assert_eq!(open.trim(), "open");
        assert_eq!(in_progress.trim(), "in_progress");
        assert_eq!(closed.trim(), "closed");
    }

    #[test]
    fn awaiting_verify_serializes_as_snake_case() {
        let yaml = serde_yml::to_string(&Status::AwaitingVerify).unwrap();
        assert_eq!(yaml.trim(), "awaiting_verify");
    }

    #[test]
    fn awaiting_verify_deserializes_from_snake_case() {
        let status: Status = serde_yml::from_str("awaiting_verify").unwrap();
        assert_eq!(status, Status::AwaitingVerify);
    }

    #[test]
    fn awaiting_verify_display() {
        assert_eq!(Status::AwaitingVerify.to_string(), "awaiting_verify");
    }

    #[test]
    fn autonomy_blocker_code_serializes_as_snake_case() {
        let yaml = serde_yml::to_string(&AutonomyBlockerCode::VerifyFrozenViolation).unwrap();
        assert_eq!(yaml.trim(), "verify_frozen_violation");
    }

    #[test]
    fn autonomy_disposition_round_trip_with_budget() {
        let disposition = AutonomyDisposition {
            kind: AutonomyDispositionKind::RequiresHuman,
            blockers: vec![
                AutonomyBlockerCode::HumanCloseRequired,
                AutonomyBlockerCode::ReviewPending,
            ],
            review: ReviewState::Pending,
            verify: VerifyPosture::Deferred,
            visibility: VisibilityState::Satisfied,
            attempt_pressure: AttemptPressure::NearLimit,
            risk: RiskBand::High,
            provenance: AutonomyProvenance::Mixed,
            continuation_budget: Some(1),
        };

        let yaml = serde_yml::to_string(&disposition).unwrap();
        let restored: AutonomyDisposition = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored, disposition);
        assert!(yaml.contains("kind: requires_human"));
        assert!(yaml.contains("human_close_required"));
        assert!(yaml.contains("review_pending"));
        assert!(yaml.contains("continuation_budget: 1"));
    }

    #[test]
    fn autonomy_disposition_omits_empty_optional_fields() {
        let disposition = AutonomyDisposition {
            kind: AutonomyDispositionKind::Unknown,
            blockers: Vec::new(),
            review: ReviewState::Unknown,
            verify: VerifyPosture::Unknown,
            visibility: VisibilityState::Unknown,
            attempt_pressure: AttemptPressure::Unknown,
            risk: RiskBand::Unknown,
            provenance: AutonomyProvenance::Unknown,
            continuation_budget: None,
        };

        let yaml = serde_yml::to_string(&disposition).unwrap();
        let restored: AutonomyDisposition = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored, disposition);
        assert!(!yaml.contains("blockers:"));
        assert!(!yaml.contains("continuation_budget:"));
    }

    #[test]
    fn derive_attempt_pressure_uses_retry_override_budget() {
        let evaluation = derive_attempt_pressure(
            3,
            6,
            Some(&OnFailAction::Retry {
                max: Some(4),
                delay_secs: None,
            }),
            &[],
            &[],
            &[],
        );

        assert_eq!(evaluation.pressure, AttemptPressure::NearLimit);
        assert_eq!(evaluation.budget_limit, 4);
        assert_eq!(evaluation.continuation_budget, Some(1));
        assert!(evaluation.blockers.is_empty());
    }

    #[test]
    fn derive_attempt_pressure_exhausts_at_budget_limit() {
        let evaluation = derive_attempt_pressure(3, 3, None, &[], &[], &[]);

        assert_eq!(evaluation.pressure, AttemptPressure::Exhausted);
        assert_eq!(evaluation.continuation_budget, Some(0));
        assert_eq!(
            evaluation.blockers,
            vec![AutonomyBlockerCode::AttemptBudgetExhausted]
        );
    }

    #[test]
    fn derive_attempt_pressure_escalate_on_fail_exhausts_after_recent_failure() {
        let now = Utc::now();
        let evaluation = derive_attempt_pressure(
            1,
            5,
            Some(&OnFailAction::Escalate {
                priority: Some(0),
                message: None,
            }),
            &[],
            &[],
            &[RunRecord {
                attempt: 1,
                started_at: now,
                finished_at: Some(now),
                duration_secs: Some(1.0),
                agent: None,
                result: RunResult::Fail,
                exit_code: Some(1),
                tokens: None,
                cost: None,
                output_snippet: None,
                autonomy_observation: None,
            }],
        );

        assert_eq!(evaluation.pressure, AttemptPressure::Exhausted);
        assert_eq!(evaluation.recent_failure_streak, 1);
        assert_eq!(evaluation.continuation_budget, Some(0));
    }

    #[test]
    fn derive_attempt_pressure_uses_recent_failure_streak() {
        let now = Utc::now();
        let evaluation = derive_attempt_pressure(
            1,
            5,
            None,
            &[],
            &[
                AttemptRecord {
                    num: 1,
                    outcome: AttemptOutcome::Failed,
                    notes: None,
                    agent: None,
                    started_at: Some(now),
                    finished_at: Some(now),
                    autonomy_observation: None,
                },
                AttemptRecord {
                    num: 2,
                    outcome: AttemptOutcome::Abandoned,
                    notes: None,
                    agent: None,
                    started_at: Some(now),
                    finished_at: Some(now),
                    autonomy_observation: None,
                },
            ],
            &[],
        );

        assert_eq!(evaluation.pressure, AttemptPressure::NearLimit);
        assert_eq!(evaluation.recent_failure_streak, 2);
        assert_eq!(evaluation.continuation_budget, Some(4));
    }

    #[test]
    fn derive_attempt_pressure_trips_from_circuit_breaker_label() {
        let evaluation = derive_attempt_pressure(
            1,
            5,
            None,
            &["circuit-breaker".to_string()],
            &[],
            &[],
        );

        assert_eq!(evaluation.pressure, AttemptPressure::CircuitBreakerTripped);
        assert_eq!(evaluation.continuation_budget, Some(0));
        assert_eq!(
            evaluation.blockers,
            vec![AutonomyBlockerCode::CircuitBreakerTripped]
        );
    }

    #[test]
    fn autonomy_observation_round_trip_omits_optional_fields() {
        let observation = AutonomyObservation {
            visibility: VisibilityState::NotApplicable,
            provenance: AutonomyProvenance::AttemptObservation,
            source_attempt: None,
            observed_at: None,
            continuation_budget_delta: None,
        };

        let yaml = serde_yml::to_string(&observation).unwrap();
        let restored: AutonomyObservation = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored, observation);
        assert!(yaml.contains("visibility: not_applicable"));
        assert!(yaml.contains("provenance: attempt_observation"));
        assert!(!yaml.contains("source_attempt:"));
        assert!(!yaml.contains("observed_at:"));
        assert!(!yaml.contains("continuation_budget_delta:"));
        assert!(!yaml.contains("confidence"));
        assert!(!yaml.contains("threshold"));
        assert!(!yaml.contains("summary:"));
    }

    #[test]
    fn run_result_serializes_as_snake_case() {
        assert_eq!(
            serde_yml::to_string(&RunResult::Pass).unwrap().trim(),
            "pass"
        );
        assert_eq!(
            serde_yml::to_string(&RunResult::Fail).unwrap().trim(),
            "fail"
        );
        assert_eq!(
            serde_yml::to_string(&RunResult::Timeout).unwrap().trim(),
            "timeout"
        );
        assert_eq!(
            serde_yml::to_string(&RunResult::Cancelled).unwrap().trim(),
            "cancelled"
        );
    }

    #[test]
    fn run_record_minimal_round_trip() {
        let now = Utc::now();
        let record = RunRecord {
            attempt: 1,
            started_at: now,
            finished_at: None,
            duration_secs: None,
            agent: None,
            result: RunResult::Pass,
            exit_code: None,
            tokens: None,
            cost: None,
            output_snippet: None,
            autonomy_observation: None,
        };

        let yaml = serde_yml::to_string(&record).unwrap();
        let restored: RunRecord = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(record, restored);

        // Optional fields should be omitted
        assert!(!yaml.contains("finished_at:"));
        assert!(!yaml.contains("duration_secs:"));
        assert!(!yaml.contains("agent:"));
        assert!(!yaml.contains("exit_code:"));
        assert!(!yaml.contains("tokens:"));
        assert!(!yaml.contains("cost:"));
        assert!(!yaml.contains("output_snippet:"));
        assert!(!yaml.contains("autonomy_observation:"));
    }

    #[test]
    fn run_record_full_round_trip() {
        let now = Utc::now();
        let record = RunRecord {
            attempt: 3,
            started_at: now,
            finished_at: Some(now),
            duration_secs: Some(12.5),
            agent: Some("agent-42".to_string()),
            result: RunResult::Fail,
            exit_code: Some(1),
            tokens: Some(5000),
            cost: Some(0.03),
            output_snippet: Some("FAILED: assertion error".to_string()),
            autonomy_observation: Some(AutonomyObservation {
                visibility: VisibilityState::Satisfied,
                provenance: AutonomyProvenance::CloseEvidence,
                source_attempt: Some(3),
                observed_at: Some(now),
                continuation_budget_delta: Some(1),
            }),
        };

        let yaml = serde_yml::to_string(&record).unwrap();
        let restored: RunRecord = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(record, restored);
    }

    #[test]
    fn history_with_cancelled_result() {
        let now = Utc::now();
        let record = RunRecord {
            attempt: 1,
            started_at: now,
            finished_at: None,
            duration_secs: None,
            agent: None,
            result: RunResult::Cancelled,
            exit_code: None,
            tokens: None,
            cost: None,
            output_snippet: None,
            autonomy_observation: None,
        };

        let yaml = serde_yml::to_string(&record).unwrap();
        assert!(yaml.contains("cancelled"));
        let restored: RunRecord = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.result, RunResult::Cancelled);
    }

    #[test]
    fn attempt_record_round_trip_with_autonomy_observation() {
        let now = Utc::now();
        let record = AttemptRecord {
            num: 4,
            outcome: AttemptOutcome::Success,
            notes: Some("completed visible mana-backed follow-up".to_string()),
            agent: Some("imp".to_string()),
            started_at: Some(now),
            finished_at: Some(now),
            autonomy_observation: Some(AutonomyObservation {
                visibility: VisibilityState::Satisfied,
                provenance: AutonomyProvenance::AttemptObservation,
                source_attempt: Some(4),
                observed_at: Some(now),
                continuation_budget_delta: Some(1),
            }),
        };

        let yaml = serde_yml::to_string(&record).unwrap();
        let restored: AttemptRecord = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored, record);
        assert!(yaml.contains("outcome: success"));
        assert!(yaml.contains("autonomy_observation:"));
        assert!(yaml.contains("visibility: satisfied"));
        assert!(yaml.contains("provenance: attempt_observation"));
        assert!(!yaml.contains("confidence"));
        assert!(!yaml.contains("threshold"));
        assert!(!yaml.contains("summary:"));
    }

    #[test]
    fn review_gate_outcome_serializes_as_snake_case() {
        assert_eq!(
            serde_yml::to_string(&ReviewGateOutcome::Skipped)
                .unwrap()
                .trim(),
            "skipped"
        );
        assert_eq!(
            serde_yml::to_string(&ReviewGateOutcome::Optional)
                .unwrap()
                .trim(),
            "optional"
        );
        assert_eq!(
            serde_yml::to_string(&ReviewGateOutcome::Mandatory)
                .unwrap()
                .trim(),
            "mandatory"
        );
    }

    #[test]
    fn approval_record_round_trips_with_provenance() {
        let now = Utc::now();
        let record = ApprovalRecord {
            approval_id: "approval-1".to_string(),
            subject_ref: "unit:45.8".to_string(),
            unit_id: "45.8".to_string(),
            attempt: Some(1),
            run_id: Some("run-1".to_string()),
            candidate_ref: "candidate:run-1".to_string(),
            evidence_bundle_ref: "evidence:run-1".to_string(),
            decision: ApprovalDecision::Approved,
            state: ApprovalState::Active,
            decision_source: DecisionSource::PolicyEngine,
            actor: Some("policy://default".to_string()),
            approved_at: now,
            gate_policy_ref: "policy:review-gate/v1".to_string(),
            review_gate_outcome: ReviewGateOutcome::Optional,
            risk_level: Some("normal".to_string()),
            risk_flags: vec![RiskFlagRecord {
                code: "scope_creep".to_string(),
                message: "change touched one extra file".to_string(),
                files: vec!["mana/crates/mana-core/src/unit/types.rs".to_string()],
            }],
            verify_refs: vec!["verify:run-1".to_string()],
            diff_scope_ref: Some("close-evidence:run-1".to_string()),
            review_refs: Vec::new(),
            review_decision_refs: Vec::new(),
            basis: "evidence complete; optional review not exercised".to_string(),
            provenance: ApprovalProvenance {
                unit_id: "45.8".to_string(),
                attempt: Some(1),
                run_id: Some("run-1".to_string()),
                candidate_ref: "candidate:run-1".to_string(),
                evidence_bundle_ref: "evidence:run-1".to_string(),
                gate_policy_ref: "policy:review-gate/v1".to_string(),
                review_gate_outcome: ReviewGateOutcome::Optional,
                risk_level: Some("normal".to_string()),
                risk_flags: vec![RiskFlagRecord {
                    code: "scope_creep".to_string(),
                    message: "change touched one extra file".to_string(),
                    files: vec!["mana/crates/mana-core/src/unit/types.rs".to_string()],
                }],
                verify_refs: vec!["verify:run-1".to_string()],
                diff_scope_ref: Some("close-evidence:run-1".to_string()),
                review_refs: Vec::new(),
                review_decision_refs: Vec::new(),
                decision_source: DecisionSource::PolicyEngine,
                actor: Some("policy://default".to_string()),
                recorded_at: now,
            },
        };

        record.validate().unwrap();

        let yaml = serde_yml::to_string(&record).unwrap();
        assert!(yaml.contains("evidence_bundle_ref: evidence:run-1"));
        assert!(yaml.contains("review_gate_outcome: optional"));
        assert!(yaml.contains("provenance:"));

        let restored: ApprovalRecord = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(record, restored);
    }

    #[test]
    fn approval_record_rejects_mandatory_review_without_lineage() {
        let now = Utc::now();
        let record = ApprovalRecord {
            approval_id: "approval-2".to_string(),
            subject_ref: "unit:45.8".to_string(),
            unit_id: "45.8".to_string(),
            attempt: Some(2),
            run_id: Some("run-2".to_string()),
            candidate_ref: "candidate:run-2".to_string(),
            evidence_bundle_ref: "evidence:run-2".to_string(),
            decision: ApprovalDecision::Withheld,
            state: ApprovalState::Active,
            decision_source: DecisionSource::Human,
            actor: Some("asher".to_string()),
            approved_at: now,
            gate_policy_ref: "policy:review-gate/v1".to_string(),
            review_gate_outcome: ReviewGateOutcome::Mandatory,
            risk_level: Some("high".to_string()),
            risk_flags: vec![RiskFlagRecord {
                code: "large_diff".to_string(),
                message: "diff exceeded mandatory-review threshold".to_string(),
                files: Vec::new(),
            }],
            verify_refs: vec!["verify:run-2".to_string()],
            diff_scope_ref: Some("close-evidence:run-2".to_string()),
            review_refs: Vec::new(),
            review_decision_refs: Vec::new(),
            basis: "mandatory review still pending".to_string(),
            provenance: ApprovalProvenance {
                unit_id: "45.8".to_string(),
                attempt: Some(2),
                run_id: Some("run-2".to_string()),
                candidate_ref: "candidate:run-2".to_string(),
                evidence_bundle_ref: "evidence:run-2".to_string(),
                gate_policy_ref: "policy:review-gate/v1".to_string(),
                review_gate_outcome: ReviewGateOutcome::Mandatory,
                risk_level: Some("high".to_string()),
                risk_flags: vec![RiskFlagRecord {
                    code: "large_diff".to_string(),
                    message: "diff exceeded mandatory-review threshold".to_string(),
                    files: Vec::new(),
                }],
                verify_refs: vec!["verify:run-2".to_string()],
                diff_scope_ref: Some("close-evidence:run-2".to_string()),
                review_refs: Vec::new(),
                review_decision_refs: Vec::new(),
                decision_source: DecisionSource::Human,
                actor: Some("asher".to_string()),
                recorded_at: now,
            },
        };

        let error = record.validate().unwrap_err();
        assert!(error.contains("mandatory review gate outcome requires non-empty review_refs"));
    }

    #[test]
    fn approval_record_accepts_mandatory_review_with_lineage() {
        let now = Utc::now();
        let record = ApprovalRecord {
            approval_id: "approval-3".to_string(),
            subject_ref: "unit:45.8".to_string(),
            unit_id: "45.8".to_string(),
            attempt: Some(3),
            run_id: Some("run-3".to_string()),
            candidate_ref: "candidate:run-3".to_string(),
            evidence_bundle_ref: "evidence:run-3".to_string(),
            decision: ApprovalDecision::Approved,
            state: ApprovalState::Active,
            decision_source: DecisionSource::MixedWorkflow,
            actor: Some("human+policy".to_string()),
            approved_at: now,
            gate_policy_ref: "policy:review-gate/v1".to_string(),
            review_gate_outcome: ReviewGateOutcome::Mandatory,
            risk_level: Some("high".to_string()),
            risk_flags: vec![RiskFlagRecord {
                code: "security_sensitive".to_string(),
                message: "runtime-boundary file changed".to_string(),
                files: vec!["mana/crates/mana-core/src/unit/types.rs".to_string()],
            }],
            verify_refs: vec!["verify:run-3".to_string()],
            diff_scope_ref: Some("close-evidence:run-3".to_string()),
            review_refs: vec!["review:run-3".to_string()],
            review_decision_refs: vec!["review-decision:approved".to_string()],
            basis: "mandatory skeptical review completed and evidence satisfied gate"
                .to_string(),
            provenance: ApprovalProvenance {
                unit_id: "45.8".to_string(),
                attempt: Some(3),
                run_id: Some("run-3".to_string()),
                candidate_ref: "candidate:run-3".to_string(),
                evidence_bundle_ref: "evidence:run-3".to_string(),
                gate_policy_ref: "policy:review-gate/v1".to_string(),
                review_gate_outcome: ReviewGateOutcome::Mandatory,
                risk_level: Some("high".to_string()),
                risk_flags: vec![RiskFlagRecord {
                    code: "security_sensitive".to_string(),
                    message: "runtime-boundary file changed".to_string(),
                    files: vec!["mana/crates/mana-core/src/unit/types.rs".to_string()],
                }],
                verify_refs: vec!["verify:run-3".to_string()],
                diff_scope_ref: Some("close-evidence:run-3".to_string()),
                review_refs: vec!["review:run-3".to_string()],
                review_decision_refs: vec!["review-decision:approved".to_string()],
                decision_source: DecisionSource::MixedWorkflow,
                actor: Some("human+policy".to_string()),
                recorded_at: now,
            },
        };

        record.validate().unwrap();
    }

    #[test]
    fn promotion_record_round_trips_with_approval_lineage() {
        let now = Utc::now();
        let record = PromotionRecord {
            promotion_id: "promotion-1".to_string(),
            subject_ref: "unit:45.8".to_string(),
            unit_id: "45.8".to_string(),
            approval_ref: "approval-3".to_string(),
            evidence_bundle_ref: "evidence:run-3".to_string(),
            review_refs: vec!["review:run-3".to_string()],
            promotion_kind: PromotionKind::CloseUnit,
            from_state: "awaiting_verify".to_string(),
            to_state: "closed".to_string(),
            effects: vec![PromotionEffect {
                kind: "close_unit".to_string(),
                target_ref: Some("unit:45.8".to_string()),
                summary: Some("unit archived after approval-backed promotion".to_string()),
            }],
            promoted_at: now,
            promotion_source: DecisionSource::PolicyEngine,
            promoted_by: Some("policy://close".to_string()),
            provenance: PromotionProvenance {
                approval_ref: "approval-3".to_string(),
                evidence_bundle_ref: "evidence:run-3".to_string(),
                review_refs: vec!["review:run-3".to_string()],
                candidate_ref: Some("candidate:run-3".to_string()),
                promotion_source: DecisionSource::PolicyEngine,
                promoted_by: Some("policy://close".to_string()),
                promoted_at: now,
            },
        };

        record.validate().unwrap();

        let yaml = serde_yml::to_string(&record).unwrap();
        assert!(yaml.contains("approval_ref: approval-3"));
        assert!(yaml.contains("evidence_bundle_ref: evidence:run-3"));
        assert!(yaml.contains("provenance:"));

        let restored: PromotionRecord = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(record, restored);
    }
}
