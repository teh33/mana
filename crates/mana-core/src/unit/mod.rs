//! Core unit data model.
//!
//! A [`Unit`] is the fundamental work item in mana. Units are stored as
//! Markdown files with YAML frontmatter (`.mana/{id}-{slug}.md`) and carry
//! everything an agent needs to perform and verify a single piece of work:
//! title, description, verify command, dependency links, attempt history,
//! and lifecycle metadata.
//!
//! ## File format
//!
//! ```text
//! ---
//! id: '42'
//! title: Fix the login bug
//! status: open
//! priority: 2
//! created_at: '2026-01-01T00:00:00Z'
//! updated_at: '2026-01-01T00:00:00Z'
//! verify: cargo test --test login
//! ---
//!
//! ## Description
//!
//! The login flow fails when the session cookie expires mid-request.
//! ```
//!
//! ## Reading and writing
//!
//! ```rust,no_run
//! use mana_core::unit::Unit;
//! use std::path::Path;
//!
//! // Read from file
//! let unit = Unit::from_file(Path::new(".mana/42-fix-login-bug.md")).unwrap();
//!
//! // Modify and write back
//! let mut unit = unit;
//! unit.notes = Some("Root cause: token expiry not checked".to_string());
//! unit.to_file(Path::new(".mana/42-fix-login-bug.md")).unwrap();
//! ```

use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use crate::util::{atomic_write, validate_unit_id};

pub mod types;
pub use types::*;

// ---------------------------------------------------------------------------
// Priority Validation
// ---------------------------------------------------------------------------

/// Validate that priority is in the valid range (0-4, P0-P4).
pub fn validate_priority(priority: u8) -> Result<()> {
    if priority > 4 {
        return Err(anyhow::anyhow!(
            "Invalid priority: {}. Priority must be in range 0-4 (P0-P4)",
            priority
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit Kind
// ---------------------------------------------------------------------------

/// Explicit schema kind for a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitKind {
    Epic,
    Job,
    Fact,
}

// ---------------------------------------------------------------------------
// Unit
// ---------------------------------------------------------------------------

/// A single unit of work managed by mana.
///
/// Units live on disk as Markdown files with YAML frontmatter.
/// All fields are serializable; optional fields are omitted from YAML
/// when `None` or empty to keep files readable.
///
/// Most callers should construct units via [`Unit::try_new`] and mutate
/// them through the high-level API functions in [`crate::api`] rather than
/// building them directly.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Unit {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    pub status: Status,
    #[serde(default = "default_priority")]
    pub priority: u8,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub design: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub close_reason: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,

    // -- verification & claim fields --
    /// Shell command that must exit 0 to close the unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
    /// Optional fast verify command to run before the full verify gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_fast: Option<String>,
    /// Whether this unit was created with --fail-first (enforced TDD).
    /// Records that the verify command was proven to fail before creation.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fail_first: bool,
    /// Git commit SHA recorded when work began for the current attempt.
    /// Used for diff/review baselines and to detect no-op closes when a unit's
    /// verify command already passed before work started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
    /// SHA-256 hash of the verify command at claim time.
    /// Used to detect if the judge was changed after work began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_hash: Option<String>,
    /// How many times the verify command has been run.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attempts: u32,
    /// Maximum verify attempts before escalation (default 3).
    #[serde(
        default = "default_max_attempts",
        skip_serializing_if = "is_default_max_attempts"
    )]
    pub max_attempts: u32,
    /// Agent or user currently holding a claim on this unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    /// When the claim was acquired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<DateTime<Utc>>,

    /// Whether this unit has been moved to the archive.
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_archived: bool,

    /// Artifacts this unit produces (types, functions, files).
    /// Used by decompose skill for dependency inference.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub produces: Vec<String>,

    /// Artifacts this unit requires from other units.
    /// Maps to dependencies via sibling produces.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,

    /// Declarative action to execute when verify fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_fail: Option<OnFailAction>,

    /// Declarative actions to execute when this unit is closed.
    /// Runs after archive and post-close hook. Failures warn but don't revert.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on_close: Vec<OnCloseAction>,

    /// Structured history of verification runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<RunRecord>,

    /// Structured output from verify commands (arbitrary JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<serde_json::Value>,

    /// Maximum agent loops for this unit (overrides config default, 0 = unlimited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_loops: Option<u32>,

    /// Timeout in seconds for the verify command (overrides config default).
    /// If the verify command exceeds this limit, it is killed and treated as failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_timeout: Option<u64>,

    // -- Memory system fields --
    /// Explicit schema kind for public mana vocabulary.
    pub kind: UnitKind,

    /// Unit type: 'task' (default) or 'fact' (verified knowledge).
    #[serde(
        default = "default_unit_type",
        skip_serializing_if = "is_default_unit_type"
    )]
    pub unit_type: String,

    /// Unix timestamp of last successful verify (for staleness detection).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_verified: Option<DateTime<Utc>>,

    /// When this fact becomes stale (created_at + TTL). Only meaningful for facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_after: Option<DateTime<Utc>>,

    /// File paths this unit is relevant to (for context relevance scoring).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,

    /// Structured attempt tracking: [{num, outcome, notes}].
    /// Tracks claim→close cycles for episodic memory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempt_log: Vec<AttemptRecord>,

    /// Identity of who created this unit (resolved from config/git/env).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,

    /// Whether this unit is a feature (product-level goal, human-only close).
    #[serde(default, skip_serializing_if = "is_false")]
    pub feature: bool,

    /// Unresolved decisions that block autonomous execution.
    /// Each entry is a question that must be answered before an agent starts work.
    /// Empty list means no blocking decisions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,

    /// Current derived scheduler-facing autonomy disposition.
    /// This stores the canonical durable answer without duplicating raw confidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy_disposition: Option<AutonomyDisposition>,
    /// Override model for this unit. Takes precedence over config-level model settings.
    /// Used as `{model}` substitution in command templates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

fn default_priority() -> u8 {
    2
}

fn default_max_attempts() -> u32 {
    3
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

fn is_default_max_attempts(v: &u32) -> bool {
    *v == 3
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn default_unit_type() -> String {
    "task".to_string()
}

fn is_default_unit_type(v: &str) -> bool {
    v == "task"
}

fn default_unit_kind() -> UnitKind {
    UnitKind::Epic
}

fn infer_unit_kind(kind: Option<UnitKind>, unit_type: &str, verify: Option<&str>) -> UnitKind {
    kind.unwrap_or_else(|| {
        if unit_type == "fact" {
            UnitKind::Fact
        } else if verify.is_some_and(|command| !command.trim().is_empty()) {
            UnitKind::Job
        } else {
            UnitKind::Epic
        }
    })
}

impl UnitKind {
    pub fn is_dispatchable_job(self) -> bool {
        matches!(self, UnitKind::Job)
    }

    pub fn is_claimable(self) -> bool {
        !matches!(self, UnitKind::Fact)
    }

    pub fn is_epic_like(self, feature: bool) -> bool {
        feature || matches!(self, UnitKind::Epic)
    }
}

impl Unit {
    pub fn is_dispatchable_job(&self) -> bool {
        self.kind.is_dispatchable_job()
            && self.verify.as_ref().is_some_and(|v| !v.trim().is_empty())
    }

    pub fn is_claimable(&self) -> bool {
        self.kind.is_claimable()
    }

    pub fn requires_human_close(&self) -> bool {
        self.feature
    }

    pub fn is_epic_like(&self) -> bool {
        self.kind.is_epic_like(self.feature)
    }
}

#[derive(Debug, Deserialize)]
struct UnitWire {
    id: String,
    title: String,
    #[serde(default)]
    slug: Option<String>,
    status: Status,
    #[serde(default = "default_priority")]
    priority: u8,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,

    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    acceptance: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    design: Option<String>,

    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    assignee: Option<String>,

    #[serde(default)]
    closed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    close_reason: Option<String>,

    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,

    #[serde(default)]
    verify: Option<String>,
    #[serde(default)]
    verify_fast: Option<String>,
    #[serde(default)]
    fail_first: bool,
    #[serde(default)]
    checkpoint: Option<String>,
    #[serde(default)]
    verify_hash: Option<String>,
    #[serde(default)]
    attempts: u32,
    #[serde(default = "default_max_attempts")]
    max_attempts: u32,
    #[serde(default)]
    claimed_by: Option<String>,
    #[serde(default)]
    claimed_at: Option<DateTime<Utc>>,

    #[serde(default)]
    is_archived: bool,

    #[serde(default)]
    produces: Vec<String>,

    #[serde(default)]
    requires: Vec<String>,

    #[serde(default)]
    on_fail: Option<OnFailAction>,

    #[serde(default)]
    on_close: Vec<OnCloseAction>,

    #[serde(default)]
    history: Vec<RunRecord>,

    #[serde(default)]
    outputs: Option<serde_json::Value>,

    #[serde(default)]
    max_loops: Option<u32>,

    #[serde(default)]
    verify_timeout: Option<u64>,

    #[serde(default)]
    kind: Option<UnitKind>,

    #[serde(default = "default_unit_type")]
    unit_type: String,

    #[serde(default)]
    last_verified: Option<DateTime<Utc>>,

    #[serde(default)]
    stale_after: Option<DateTime<Utc>>,

    #[serde(default)]
    paths: Vec<String>,

    #[serde(default)]
    attempt_log: Vec<AttemptRecord>,

    #[serde(default)]
    created_by: Option<String>,

    #[serde(default)]
    feature: bool,

    #[serde(default)]
    decisions: Vec<String>,
    #[serde(default)]
    autonomy_disposition: Option<AutonomyDisposition>,
    #[serde(default)]
    model: Option<String>,
}

impl From<UnitWire> for Unit {
    fn from(raw: UnitWire) -> Self {
        let kind = infer_unit_kind(raw.kind, &raw.unit_type, raw.verify.as_deref());

        Self {
            id: raw.id,
            title: raw.title,
            slug: raw.slug,
            status: raw.status,
            priority: raw.priority,
            created_at: raw.created_at,
            updated_at: raw.updated_at,
            description: raw.description,
            acceptance: raw.acceptance,
            notes: raw.notes,
            design: raw.design,
            labels: raw.labels,
            assignee: raw.assignee,
            closed_at: raw.closed_at,
            close_reason: raw.close_reason,
            parent: raw.parent,
            dependencies: raw.dependencies,
            verify: raw.verify,
            verify_fast: raw.verify_fast,
            fail_first: raw.fail_first,
            checkpoint: raw.checkpoint,
            verify_hash: raw.verify_hash,
            attempts: raw.attempts,
            max_attempts: raw.max_attempts,
            claimed_by: raw.claimed_by,
            claimed_at: raw.claimed_at,
            is_archived: raw.is_archived,
            produces: raw.produces,
            requires: raw.requires,
            on_fail: raw.on_fail,
            on_close: raw.on_close,
            history: raw.history,
            outputs: raw.outputs,
            max_loops: raw.max_loops,
            verify_timeout: raw.verify_timeout,
            kind,
            unit_type: raw.unit_type,
            last_verified: raw.last_verified,
            stale_after: raw.stale_after,
            paths: raw.paths,
            attempt_log: raw.attempt_log,
            created_by: raw.created_by,
            feature: raw.feature,
            decisions: raw.decisions,
            autonomy_disposition: raw.autonomy_disposition,
            model: raw.model,
        }
    }
}

impl<'de> Deserialize<'de> for Unit {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        UnitWire::deserialize(deserializer).map(Unit::from)
    }
}

impl Unit {
    fn push_unique_blocker(blockers: &mut Vec<AutonomyBlockerCode>, blocker: AutonomyBlockerCode) {
        if !blockers.contains(&blocker) {
            blockers.push(blocker);
        }
    }

    /// Create a new unit with sensible defaults.
    /// Returns an error if the ID is invalid.
    pub fn try_new(id: impl Into<String>, title: impl Into<String>) -> Result<Self> {
        let id_str = id.into();
        validate_unit_id(&id_str)?;

        let now = Utc::now();
        Ok(Self {
            id: id_str,
            title: title.into(),
            slug: None,
            status: Status::Open,
            priority: 2,
            created_at: now,
            updated_at: now,
            description: None,
            acceptance: None,
            notes: None,
            design: None,
            labels: Vec::new(),
            assignee: None,
            closed_at: None,
            close_reason: None,
            parent: None,
            dependencies: Vec::new(),
            verify: None,
            verify_fast: None,
            fail_first: false,
            checkpoint: None,
            verify_hash: None,
            attempts: 0,
            max_attempts: 3,
            claimed_by: None,
            claimed_at: None,
            is_archived: false,
            feature: false,
            produces: Vec::new(),
            requires: Vec::new(),
            on_fail: None,
            on_close: Vec::new(),
            history: Vec::new(),
            outputs: None,
            max_loops: None,
            verify_timeout: None,
            kind: default_unit_kind(),
            unit_type: "task".to_string(),
            last_verified: None,
            stale_after: None,
            paths: Vec::new(),
            attempt_log: Vec::new(),
            created_by: None,
            decisions: Vec::new(),
            autonomy_disposition: None,
            model: None,
        })
    }

    /// Create a new unit with sensible defaults.
    /// Panics if the ID is invalid. Prefer `try_new` for fallible construction.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self::try_new(id, title).expect("Invalid unit ID")
    }

    /// Recompute the scheduler-facing autonomy disposition from current durable unit state.
    pub fn refresh_autonomy_disposition(&mut self) {
        let evaluation = derive_attempt_pressure(
            self.attempts,
            self.max_attempts,
            self.on_fail.as_ref(),
            &self.labels,
            &self.attempt_log,
            &self.history,
        );

        let prior = self
            .autonomy_disposition
            .clone()
            .unwrap_or_else(AutonomyDisposition::unknown);

        let review = self.derive_review_state(&prior);
        let approval = self.derive_approval_state(&prior);
        let verify = self.derive_verify_posture(&prior);
        let visibility = prior.visibility;
        let risk = prior.risk;

        let mut blockers = prior.blockers;
        blockers.retain(|blocker| {
            !matches!(
                blocker,
                AutonomyBlockerCode::HumanCloseRequired
                    | AutonomyBlockerCode::ApprovalRequired
                    | AutonomyBlockerCode::ReviewRequired
                    | AutonomyBlockerCode::ReviewPending
                    | AutonomyBlockerCode::ReviewRejected
                    | AutonomyBlockerCode::VerifyAbsent
                    | AutonomyBlockerCode::VerifyDeferred
                    | AutonomyBlockerCode::VerifyFailed
                    | AutonomyBlockerCode::VerifyFrozenViolation
                    | AutonomyBlockerCode::VerifyQualityUnknown
                    | AutonomyBlockerCode::VisibilityMissing
                    | AutonomyBlockerCode::AttemptBudgetExhausted
                    | AutonomyBlockerCode::CircuitBreakerTripped
            )
        });

        if self.requires_human_close() {
            Self::push_unique_blocker(&mut blockers, AutonomyBlockerCode::HumanCloseRequired);
        }

        match review {
            ReviewState::Required => {
                Self::push_unique_blocker(&mut blockers, AutonomyBlockerCode::ReviewRequired)
            }
            ReviewState::Pending => {
                Self::push_unique_blocker(&mut blockers, AutonomyBlockerCode::ReviewPending)
            }
            ReviewState::Rejected => {
                Self::push_unique_blocker(&mut blockers, AutonomyBlockerCode::ReviewRejected)
            }
            ReviewState::Unknown | ReviewState::NotRequired | ReviewState::Approved => {}
        }

        match approval {
            ApprovalState::Required | ApprovalState::Pending | ApprovalState::Rejected => {
                Self::push_unique_blocker(&mut blockers, AutonomyBlockerCode::ApprovalRequired)
            }
            ApprovalState::Unknown | ApprovalState::NotRequired | ApprovalState::Approved => {}
        }

        match verify {
            VerifyPosture::Absent => {
                Self::push_unique_blocker(&mut blockers, AutonomyBlockerCode::VerifyAbsent)
            }
            VerifyPosture::Deferred => {
                Self::push_unique_blocker(&mut blockers, AutonomyBlockerCode::VerifyDeferred)
            }
            VerifyPosture::Failed => {
                Self::push_unique_blocker(&mut blockers, AutonomyBlockerCode::VerifyFailed)
            }
            VerifyPosture::FrozenViolation => {
                Self::push_unique_blocker(&mut blockers, AutonomyBlockerCode::VerifyFrozenViolation)
            }
            VerifyPosture::Weak | VerifyPosture::Unknown => {
                if self.verify_requires_quality_blocker(verify) {
                    Self::push_unique_blocker(&mut blockers, AutonomyBlockerCode::VerifyQualityUnknown)
                }
            }
            VerifyPosture::NotApplicable | VerifyPosture::Satisfied => {}
        }

        if visibility == VisibilityState::Missing {
            Self::push_unique_blocker(&mut blockers, AutonomyBlockerCode::VisibilityMissing);
        }

        for blocker in evaluation.blockers {
            Self::push_unique_blocker(&mut blockers, blocker);
        }

        let kind = if blockers.contains(&AutonomyBlockerCode::HumanCloseRequired) {
            AutonomyDispositionKind::RequiresHuman
        } else if blockers.is_empty() {
            AutonomyDispositionKind::Eligible
        } else {
            AutonomyDispositionKind::Blocked
        };

        let provenance = if review != ReviewState::Unknown
            || approval != ApprovalState::Unknown
            || verify != VerifyPosture::Unknown
            || visibility != VisibilityState::Unknown
            || risk != RiskBand::Unknown
        {
            match prior.provenance {
                AutonomyProvenance::Unknown | AutonomyProvenance::AttemptObservation => {
                    AutonomyProvenance::Mixed
                }
                existing => existing,
            }
        } else {
            match prior.provenance {
                AutonomyProvenance::Unknown => AutonomyProvenance::AttemptObservation,
                existing => existing,
            }
        };

        self.autonomy_disposition = Some(AutonomyDisposition {
            kind,
            blockers,
            review,
            approval,
            verify,
            visibility,
            attempt_pressure: evaluation.pressure,
            risk,
            provenance,
            continuation_budget: evaluation.continuation_budget,
        });
    }

    fn derive_review_state(&self, prior: &AutonomyDisposition) -> ReviewState {
        if self.labels.iter().any(|label| label == "reviewed") {
            ReviewState::Approved
        } else if self.labels.iter().any(|label| label == "rejected") {
            ReviewState::Rejected
        } else if self.labels.iter().any(|label| label == "needs-human-review") {
            ReviewState::Pending
        } else if self.labels.iter().any(|label| label == "review-failed") {
            if self.status == Status::Open {
                ReviewState::Pending
            } else {
                ReviewState::Rejected
            }
        } else if !matches!(prior.review, ReviewState::Unknown) {
            prior.review
        } else {
            ReviewState::Unknown
        }
    }

    fn derive_approval_state(&self, prior: &AutonomyDisposition) -> ApprovalState {
        if !matches!(prior.approval, ApprovalState::Unknown) {
            prior.approval
        } else {
            ApprovalState::Unknown
        }
    }

    fn derive_verify_posture(&self, prior: &AutonomyDisposition) -> VerifyPosture {
        let has_verify = self
            .verify
            .as_ref()
            .is_some_and(|verify| !verify.trim().is_empty());

        if self.is_epic_like() && !has_verify {
            return VerifyPosture::NotApplicable;
        }

        if !has_verify {
            return VerifyPosture::Absent;
        }

        if self.verify_hash_mismatch() {
            return VerifyPosture::FrozenViolation;
        }

        if self.status == Status::AwaitingVerify {
            return VerifyPosture::Deferred;
        }

        if let Some(last_run) = self.history.last() {
            match last_run.result {
                RunResult::Pass => return VerifyPosture::Satisfied,
                RunResult::Fail | RunResult::Timeout => return VerifyPosture::Failed,
                RunResult::Cancelled => {}
            }
        }

        if matches!(prior.verify, VerifyPosture::FrozenViolation) && self.verify_hash_mismatch() {
            return VerifyPosture::FrozenViolation;
        }

        VerifyPosture::Weak
    }

    fn verify_hash_mismatch(&self) -> bool {
        let (Some(stored_hash), Some(verify_cmd)) = (&self.verify_hash, &self.verify) else {
            return false;
        };
        if verify_cmd.trim().is_empty() {
            return false;
        }

        let mut hasher = Sha256::new();
        hasher.update(verify_cmd.as_bytes());
        let current_hash = format!("{:x}", hasher.finalize());
        current_hash != *stored_hash
    }

    fn verify_requires_quality_blocker(&self, posture: VerifyPosture) -> bool {
        !self.is_epic_like() && matches!(posture, VerifyPosture::Weak | VerifyPosture::Unknown)
    }

    /// Get effective max_loops (per-unit override or config default).
    /// A value of 0 means unlimited.
    pub fn effective_max_loops(&self, config_max: u32) -> u32 {
        self.max_loops.unwrap_or(config_max)
    }

    /// Get effective verify_timeout: unit-level override, then config default, then None.
    pub fn effective_verify_timeout(&self, config_timeout: Option<u64>) -> Option<u64> {
        self.verify_timeout.or(config_timeout)
    }

    /// Parse YAML frontmatter and markdown body.
    /// Expects format:
    /// ```text
    /// ---
    /// id: 1
    /// title: Example
    /// status: open
    /// ...
    /// ---
    /// # Markdown body here
    /// ```
    fn parse_frontmatter(content: &str) -> Result<(String, Option<String>)> {
        // Check if content starts with ---
        if !content.starts_with("---\n") && !content.starts_with("---\r\n") {
            // Not frontmatter format, try pure YAML
            return Err(anyhow::anyhow!("Not markdown frontmatter format"));
        }

        // Find the second --- delimiter
        let after_first_delimiter = if let Some(stripped) = content.strip_prefix("---\r\n") {
            stripped
        } else if let Some(stripped) = content.strip_prefix("---\n") {
            stripped
        } else {
            return Err(anyhow::anyhow!("Not markdown frontmatter format"));
        };

        let second_delimiter_pos =
            Self::find_closing_delimiter(after_first_delimiter).ok_or_else(|| {
                anyhow::anyhow!("Markdown frontmatter is missing closing delimiter (---)")
            })?;
        let frontmatter = &after_first_delimiter[..second_delimiter_pos];

        // Skip the closing --- and any whitespace to get the body
        let body_start = second_delimiter_pos + 3;
        let body_raw = &after_first_delimiter[body_start..];

        // Trim leading/trailing whitespace from body
        let body = body_raw.trim();
        let body = (!body.is_empty()).then(|| body.to_string());

        Ok((frontmatter.to_string(), body))
    }

    /// Find the closing `---` delimiter at the start of a line.
    /// A naive `find("---")` matches inside YAML values, corrupting the parse.
    fn find_closing_delimiter(content: &str) -> Option<usize> {
        if content.starts_with("---\n") || content.starts_with("---\r\n") || content == "---" {
            return Some(0);
        }
        let mut search_from = 0;
        while let Some(pos) = content[search_from..].find("\n---") {
            let abs_pos = search_from + pos;
            let delimiter_start = abs_pos + 1;
            let after_dashes = delimiter_start + 3;
            if after_dashes >= content.len()
                || content.as_bytes()[after_dashes] == b'\n'
                || content.as_bytes()[after_dashes] == b'\r'
            {
                return Some(delimiter_start);
            }
            search_from = abs_pos + 1;
        }
        None
    }

    /// Parse a unit from a string (either YAML or Markdown with YAML frontmatter).
    pub fn from_string(content: &str) -> Result<Self> {
        // Try frontmatter format first
        match Self::parse_frontmatter(content) {
            Ok((frontmatter, body)) => {
                // Parse frontmatter as YAML
                let mut unit: Unit = serde_yml::from_str(&frontmatter)?;

                // If there's a body and no description yet, set it
                if let Some(markdown_body) = body {
                    if unit.description.is_none() {
                        unit.description = Some(markdown_body);
                    }
                }

                Ok(unit)
            }
            Err(_) => {
                // Fallback: treat entire content as YAML
                let unit: Unit = serde_yml::from_str(content)?;
                Ok(unit)
            }
        }
    }

    /// Read a unit from a file (supports both YAML and Markdown with YAML frontmatter).
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let contents = std::fs::read_to_string(path.as_ref())?;
        Self::from_string(&contents)
    }

    /// Write this unit to a file.
    /// For `.md` files, writes markdown frontmatter format (YAML between `---` delimiters
    /// with description as the markdown body). For other extensions, writes pure YAML.
    pub fn to_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let is_md = path.extension().and_then(|e| e.to_str()) == Some("md");

        if is_md {
            // Always write frontmatter format for .md files: ---\nYAML\n---\nbody
            let mut frontmatter_unit = self.clone();
            let description = frontmatter_unit.description.take(); // Remove from YAML
            let yaml = serde_yml::to_string(&frontmatter_unit)?;
            let mut content = String::from("---\n");
            content.push_str(yaml.trim_start_matches("---\n").trim_end());
            content.push_str("\n---\n");
            if let Some(desc) = description {
                content.push('\n');
                content.push_str(&desc);
                if !desc.ends_with('\n') {
                    content.push('\n');
                }
            }
            atomic_write(path, &content)?;
        } else {
            let yaml = serde_yml::to_string(self)?;
            atomic_write(path, &yaml)?;
        }
        Ok(())
    }

    /// Calculate SHA256 hash of canonical form.
    ///
    /// Used for optimistic locking. The hash is calculated from a canonical
    /// JSON representation with transient fields cleared.
    pub fn hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let canonical = self.clone();

        // Serialize to JSON (deterministic)
        let json =
            serde_json::to_string(&canonical).expect("Unit serialization to JSON cannot fail");
        let mut hasher = Sha256::new();
        hasher.update(json.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Load unit with version hash for optimistic locking.
    ///
    /// Returns the unit and its content hash as a tuple. The hash can be
    /// compared before saving to detect concurrent modifications.
    pub fn from_file_with_hash(path: impl AsRef<Path>) -> Result<(Self, String)> {
        let unit = Self::from_file(path)?;
        let hash = unit.hash();
        Ok((unit, hash))
    }

    /// Apply a JSON-serialized value to a field by name.
    ///
    /// Used by conflict resolution to set a field to a chosen value.
    /// The value should be JSON-serialized (e.g., `"\"hello\""` for a string).
    ///
    /// # Arguments
    /// * `field` - The field name to update
    /// * `json_value` - JSON-serialized value to apply
    ///
    /// # Returns
    /// * `Ok(())` on success
    /// * `Err` if field is unknown or value cannot be deserialized
    pub fn apply_value(&mut self, field: &str, json_value: &str) -> Result<()> {
        match field {
            "title" => self.title = serde_json::from_str(json_value)?,
            "status" => self.status = serde_json::from_str(json_value)?,
            "priority" => self.priority = serde_json::from_str(json_value)?,
            "description" => self.description = serde_json::from_str(json_value)?,
            "acceptance" => self.acceptance = serde_json::from_str(json_value)?,
            "notes" => self.notes = serde_json::from_str(json_value)?,
            "design" => self.design = serde_json::from_str(json_value)?,
            "assignee" => self.assignee = serde_json::from_str(json_value)?,
            "labels" => self.labels = serde_json::from_str(json_value)?,
            "dependencies" => self.dependencies = serde_json::from_str(json_value)?,
            "parent" => self.parent = serde_json::from_str(json_value)?,
            "verify" => self.verify = serde_json::from_str(json_value)?,
            "produces" => self.produces = serde_json::from_str(json_value)?,
            "requires" => self.requires = serde_json::from_str(json_value)?,
            "claimed_by" => self.claimed_by = serde_json::from_str(json_value)?,
            "close_reason" => self.close_reason = serde_json::from_str(json_value)?,
            "on_fail" => self.on_fail = serde_json::from_str(json_value)?,
            "outputs" => self.outputs = serde_json::from_str(json_value)?,
            "max_loops" => self.max_loops = serde_json::from_str(json_value)?,
            "kind" => self.kind = serde_json::from_str(json_value)?,
            "unit_type" => self.unit_type = serde_json::from_str(json_value)?,
            "last_verified" => self.last_verified = serde_json::from_str(json_value)?,
            "stale_after" => self.stale_after = serde_json::from_str(json_value)?,
            "paths" => self.paths = serde_json::from_str(json_value)?,
            "decisions" => self.decisions = serde_json::from_str(json_value)?,
            "autonomy_disposition" => {
                self.autonomy_disposition = serde_json::from_str(json_value)?
            },
            "model" => self.model = serde_json::from_str(json_value)?,
            _ => return Err(anyhow::anyhow!("Unknown field: {}", field)),
        }
        self.updated_at = Utc::now();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn round_trip_minimal_unit() {
        let unit = Unit::new("1", "My first unit");

        // Serialize
        let yaml = serde_yml::to_string(&unit).unwrap();

        // Deserialize
        let restored: Unit = serde_yml::from_str(&yaml).unwrap();

        assert_eq!(unit, restored);
    }

    #[test]
    fn epic_is_not_dispatchable() {
        let mut unit = Unit::new("1", "Epic");
        unit.kind = UnitKind::Epic;
        unit.verify = Some("cargo test something".to_string());

        assert!(!unit.is_dispatchable_job());
        assert!(unit.is_claimable());
        assert!(unit.is_epic_like());
    }

    #[test]
    fn job_dispatchability_is_explicit() {
        let mut unit = Unit::new("2", "Job");
        unit.kind = UnitKind::Job;
        unit.verify = Some("cargo test job_dispatchability_is_explicit".to_string());

        assert!(unit.is_dispatchable_job());
        assert!(unit.is_claimable());
        assert!(!unit.is_epic_like());

        unit.verify = Some("   ".to_string());
        assert!(!unit.is_dispatchable_job());
    }

    #[test]
    fn feature_semantics_preserve_human_review() {
        let mut unit = Unit::new("3", "Feature epic");
        unit.kind = UnitKind::Epic;
        unit.feature = true;

        assert!(unit.is_epic_like());
        assert!(unit.requires_human_close());
        assert!(!unit.is_dispatchable_job());
    }

    #[test]
    fn kind_round_trip_yaml() {
        let mut unit = Unit::new("1", "Explicit kind");
        unit.kind = UnitKind::Epic;
        unit.verify = Some("cargo test unit::check".to_string());

        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(yaml.contains("kind: epic"));

        let restored: Unit = serde_yml::from_str(&yaml).unwrap();

        assert_eq!(restored.kind, UnitKind::Epic);
        assert_eq!(restored.verify, unit.verify);
    }

    #[test]
    fn kind_infers_from_legacy_fields() {
        let fact_yaml = r#"
id: "1"
title: Legacy fact
status: open
priority: 2
created_at: "2025-01-01T00:00:00Z"
updated_at: "2025-01-01T00:00:00Z"
unit_type: fact
"#;
        let fact: Unit = serde_yml::from_str(fact_yaml).unwrap();
        assert_eq!(fact.kind, UnitKind::Fact);

        let epic_yaml = r#"
id: "2"
title: Legacy epic
status: open
priority: 2
created_at: "2025-01-01T00:00:00Z"
updated_at: "2025-01-01T00:00:00Z"
"#;
        let epic: Unit = serde_yml::from_str(epic_yaml).unwrap();
        assert_eq!(epic.kind, UnitKind::Epic);

        let job_yaml = r#"
id: "3"
title: Legacy job
status: open
priority: 2
created_at: "2025-01-01T00:00:00Z"
updated_at: "2025-01-01T00:00:00Z"
verify: cargo test
"#;
        let job: Unit = serde_yml::from_str(job_yaml).unwrap();
        assert_eq!(job.kind, UnitKind::Job);
    }

    #[test]
    fn round_trip_full_unit() {
        let now = Utc::now();
        let unit = Unit {
            id: "3.2.1".to_string(),
            title: "Implement parser".to_string(),
            slug: None,
            status: Status::InProgress,
            priority: 1,
            created_at: now,
            updated_at: now,
            description: Some("Build a robust YAML parser".to_string()),
            acceptance: Some("All tests pass".to_string()),
            notes: Some("Watch out for edge cases".to_string()),
            design: Some("Use serde_yaml".to_string()),
            labels: vec!["backend".to_string(), "core".to_string()],
            assignee: Some("alice".to_string()),
            closed_at: Some(now),
            close_reason: Some("Done".to_string()),
            parent: Some("3.2".to_string()),
            dependencies: vec!["3.1".to_string()],
            verify: Some("cargo test unit::check".to_string()),
            verify_fast: Some("cargo check -p mana-core".to_string()),
            fail_first: false,
            checkpoint: None,
            verify_hash: None,
            attempts: 1,
            max_attempts: 5,
            claimed_by: Some("agent-7".to_string()),
            claimed_at: Some(now),
            is_archived: false,
            feature: false,
            produces: vec!["Parser".to_string()],
            requires: vec!["Lexer".to_string()],
            on_fail: Some(OnFailAction::Retry {
                max: Some(5),
                delay_secs: None,
            }),
            on_close: vec![
                OnCloseAction::Run {
                    command: "echo done".to_string(),
                },
                OnCloseAction::Notify {
                    message: "Task complete".to_string(),
                },
            ],
            verify_timeout: None,
            history: Vec::new(),
            outputs: Some(serde_json::json!({"key": "value"})),
            max_loops: None,
            kind: UnitKind::Job,
            unit_type: "task".to_string(),
            last_verified: None,
            stale_after: None,
            paths: Vec::new(),
            attempt_log: Vec::new(),
            created_by: Some("alice".to_string()),
            decisions: vec!["JWT or sessions?".to_string()],
            autonomy_disposition: Some(AutonomyDisposition {
                kind: AutonomyDispositionKind::Blocked,
                blockers: vec![
                    AutonomyBlockerCode::UnresolvedDecision,
                    AutonomyBlockerCode::ReviewPending,
                ],
                review: ReviewState::Pending,
                approval: ApprovalState::Pending,
                verify: VerifyPosture::Deferred,
                visibility: VisibilityState::Satisfied,
                attempt_pressure: AttemptPressure::WithinBudget,
                risk: RiskBand::Normal,
                provenance: AutonomyProvenance::Mixed,
                continuation_budget: Some(2),
            }),
            model: Some("claude-sonnet".to_string()),
        };

        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(yaml.contains("autonomy_disposition:"));
        let restored: Unit = serde_yml::from_str(&yaml).unwrap();

        assert_eq!(unit, restored);
    }

    #[test]
    fn optional_fields_omitted_when_none() {
        let unit = Unit::new("1", "Minimal");
        let yaml = serde_yml::to_string(&unit).unwrap();

        assert!(!yaml.contains("description:"));
        assert!(!yaml.contains("acceptance:"));
        assert!(!yaml.contains("notes:"));
        assert!(!yaml.contains("design:"));
        assert!(!yaml.contains("assignee:"));
        assert!(!yaml.contains("closed_at:"));
        assert!(!yaml.contains("close_reason:"));
        assert!(!yaml.contains("parent:"));
        assert!(!yaml.contains("labels:"));
        assert!(!yaml.contains("dependencies:"));
        assert!(!yaml.contains("verify:"));
        assert!(!yaml.contains("verify_fast:"));
        assert!(!yaml.contains("attempts:"));
        assert!(!yaml.contains("max_attempts:"));
        assert!(!yaml.contains("claimed_by:"));
        assert!(!yaml.contains("claimed_at:"));
        assert!(!yaml.contains("is_archived:"));
        assert!(!yaml.contains("on_fail:"));
        assert!(!yaml.contains("on_close:"));
        assert!(!yaml.contains("history:"));
        assert!(!yaml.contains("outputs:"));
        assert!(!yaml.contains("autonomy_disposition:"));
    }

    #[test]
    fn timestamps_serialize_as_iso8601() {
        let unit = Unit::new("1", "Check timestamps");
        let yaml = serde_yml::to_string(&unit).unwrap();

        // ISO 8601 timestamps contain 'T' between date and time
        for line in yaml.lines() {
            if line.starts_with("created_at:") || line.starts_with("updated_at:") {
                let value = line.split_once(':').unwrap().1.trim();
                assert!(value.contains('T'), "timestamp should be ISO 8601: {value}");
            }
        }
    }

    #[test]
    fn file_round_trip() {
        let unit = Unit::new("42", "File I/O test");

        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        // Write
        unit.to_file(&path).unwrap();

        // Read back
        let restored = Unit::from_file(&path).unwrap();
        assert_eq!(unit, restored);

        // Verify the file is valid YAML we can also read raw
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("id: '42'") || raw.contains("id: \"42\""));
        assert!(raw.contains("title: File I/O test") || raw.contains("title: 'File I/O test'"));
        drop(tmp);
    }

    #[test]
    fn defaults_are_correct() {
        let unit = Unit::new("1", "Defaults");
        assert_eq!(unit.status, Status::Open);
        assert_eq!(unit.priority, 2);
        assert_eq!(unit.kind, UnitKind::Epic);
        assert!(unit.labels.is_empty());
        assert!(unit.dependencies.is_empty());
        assert!(unit.description.is_none());
    }

    #[test]
    fn deserialize_with_missing_optional_fields() {
        let yaml = r#"
id: "5"
title: Sparse unit
status: open
priority: 3
created_at: "2025-01-01T00:00:00Z"
updated_at: "2025-01-01T00:00:00Z"
"#;
        let unit: Unit = serde_yml::from_str(yaml).unwrap();
        assert_eq!(unit.id, "5");
        assert_eq!(unit.priority, 3);
        assert_eq!(unit.kind, UnitKind::Epic);
        assert!(unit.description.is_none());
        assert!(unit.labels.is_empty());
        assert!(unit.autonomy_disposition.is_none());
    }

    #[test]
    fn autonomy_disposition_round_trips_on_unit() {
        let mut unit = Unit::new("6", "Autonomy-ready unit");
        unit.autonomy_disposition = Some(AutonomyDisposition {
            kind: AutonomyDispositionKind::Eligible,
            blockers: Vec::new(),
            review: ReviewState::NotRequired,
            approval: ApprovalState::NotRequired,
            verify: VerifyPosture::Satisfied,
            visibility: VisibilityState::Satisfied,
            attempt_pressure: AttemptPressure::WithinBudget,
            risk: RiskBand::Low,
            provenance: AutonomyProvenance::Mixed,
            continuation_budget: Some(3),
        });

        let yaml = serde_yml::to_string(&unit).unwrap();
        let restored: Unit = serde_yml::from_str(&yaml).unwrap();

        assert_eq!(restored.autonomy_disposition, unit.autonomy_disposition);
        assert!(yaml.contains("autonomy_disposition:"));
        assert!(yaml.contains("kind: eligible"));
        assert!(yaml.contains("continuation_budget: 3"));
    }

    #[test]
    fn validate_priority_accepts_valid_range() {
        for priority in 0..=4 {
            assert!(
                validate_priority(priority).is_ok(),
                "Priority {} should be valid",
                priority
            );
        }
    }

    #[test]
    fn validate_priority_rejects_out_of_range() {
        assert!(validate_priority(5).is_err());
        assert!(validate_priority(10).is_err());
        assert!(validate_priority(255).is_err());
    }

    // =====================================================================
    // Tests for Markdown Frontmatter Parsing
    // =====================================================================

    #[test]
    fn test_parse_md_frontmatter() {
        let content = r#"---
id: 11.1
title: Test Unit
status: open
priority: 2
created_at: "2026-01-26T15:00:00Z"
updated_at: "2026-01-26T15:00:00Z"
---

# Description

Test markdown body.
"#;
        let unit = Unit::from_string(content).unwrap();
        assert_eq!(unit.id, "11.1");
        assert_eq!(unit.title, "Test Unit");
        assert_eq!(unit.status, Status::Open);
        assert!(unit.description.is_some());
        assert!(unit.description.as_ref().unwrap().contains("# Description"));
        assert!(unit
            .description
            .as_ref()
            .unwrap()
            .contains("Test markdown body"));
    }

    #[test]
    fn test_parse_md_frontmatter_preserves_metadata_fields() {
        let content = r#"---
id: "2.5"
title: Complex Unit
status: in_progress
priority: 1
created_at: "2026-01-01T10:00:00Z"
updated_at: "2026-01-26T15:00:00Z"
parent: "2"
labels:
  - backend
  - urgent
dependencies:
  - "2.1"
  - "2.2"
---

## Implementation Notes

This is a complex unit with multiple metadata fields.
"#;
        let unit = Unit::from_string(content).unwrap();
        assert_eq!(unit.id, "2.5");
        assert_eq!(unit.title, "Complex Unit");
        assert_eq!(unit.status, Status::InProgress);
        assert_eq!(unit.priority, 1);
        assert_eq!(unit.parent, Some("2".to_string()));
        assert_eq!(
            unit.labels,
            vec!["backend".to_string(), "urgent".to_string()]
        );
        assert_eq!(
            unit.dependencies,
            vec!["2.1".to_string(), "2.2".to_string()]
        );
        assert!(unit.description.is_some());
    }

    #[test]
    fn test_parse_md_frontmatter_empty_body() {
        let content = r#"---
id: "3"
title: No Body Unit
status: open
priority: 2
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-01T00:00:00Z"
---
"#;
        let unit = Unit::from_string(content).unwrap();
        assert_eq!(unit.id, "3");
        assert_eq!(unit.title, "No Body Unit");
        assert!(unit.description.is_none());
    }

    #[test]
    fn test_parse_md_frontmatter_with_body_containing_dashes() {
        let content = r#"---
id: "4"
title: Dashes in Body
status: open
priority: 2
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-01T00:00:00Z"
---

# Section 1

This has --- inside the body, which should not break parsing.

---

More content after a horizontal rule.
"#;
        let unit = Unit::from_string(content).unwrap();
        assert_eq!(unit.id, "4");
        assert!(unit.description.is_some());
        let body = unit.description.as_ref().unwrap();
        assert!(body.contains("---"));
        assert!(body.contains("horizontal rule"));
    }

    #[test]
    fn test_parse_md_frontmatter_with_whitespace_in_body() {
        let content = r#"---
id: "5"
title: Whitespace Test
status: open
priority: 2
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-01T00:00:00Z"
---


   Leading whitespace preserved after trimming newlines.

"#;
        let unit = Unit::from_string(content).unwrap();
        assert_eq!(unit.id, "5");
        assert!(unit.description.is_some());
        let body = unit.description.as_ref().unwrap();
        // Leading newlines trimmed, but content preserved
        assert!(body.contains("Leading whitespace"));
    }

    #[test]
    fn test_fallback_to_yaml_parsing() {
        let yaml_content = r#"
id: "6"
title: Pure YAML Unit
status: open
priority: 3
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-01T00:00:00Z"
description: "This is YAML, not markdown"
"#;
        let unit = Unit::from_string(yaml_content).unwrap();
        assert_eq!(unit.id, "6");
        assert_eq!(unit.title, "Pure YAML Unit");
        assert_eq!(
            unit.description,
            Some("This is YAML, not markdown".to_string())
        );
    }

    #[test]
    fn test_file_round_trip_with_markdown() {
        let content = r#"---
id: "7"
title: File Markdown Test
status: open
priority: 2
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-01T00:00:00Z"
---

# Markdown Body

This is a test of reading markdown from a file.
"#;

        // Use a .md extension to trigger frontmatter write
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("7-test.md");

        // Write markdown content
        std::fs::write(&path, content).unwrap();

        // Read back as unit
        let unit = Unit::from_file(&path).unwrap();
        assert_eq!(unit.id, "7");
        assert_eq!(unit.title, "File Markdown Test");
        assert!(unit.description.is_some());
        assert!(unit
            .description
            .as_ref()
            .unwrap()
            .contains("# Markdown Body"));

        // Write it back — should preserve frontmatter format for .md files
        unit.to_file(&path).unwrap();

        // Verify the file still has frontmatter format
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.starts_with("---\n"),
            "Should start with frontmatter delimiter, got: {}",
            &written[..50.min(written.len())]
        );
        assert!(
            written.contains("# Markdown Body"),
            "Should contain markdown body"
        );
        // Description should NOT be in the YAML frontmatter section
        let parts: Vec<&str> = written.splitn(3, "---").collect();
        assert!(parts.len() >= 3, "Should have frontmatter delimiters");
        let frontmatter_section = parts[1];
        assert!(
            !frontmatter_section.contains("# Markdown Body"),
            "Description should be in body, not frontmatter"
        );

        // Read back one more time to verify full round-trip
        let unit2 = Unit::from_file(&path).unwrap();
        assert_eq!(unit2.id, unit.id);
        assert_eq!(unit2.title, unit.title);
        assert_eq!(unit2.description, unit.description);
    }

    #[test]
    fn test_parse_md_frontmatter_missing_closing_delimiter() {
        let bad_content = r#"---
id: "8"
title: Missing Delimiter
status: open
"#;
        let result = Unit::from_string(bad_content);
        // Should fail because no closing ---
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_md_frontmatter_multiline_fields() {
        let content = r#"---
id: "9"
title: Multiline Test
status: open
priority: 2
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-01T00:00:00Z"
acceptance: |
  - Criterion 1
  - Criterion 2
  - Criterion 3
---

# Implementation

Start implementing...
"#;
        let unit = Unit::from_string(content).unwrap();
        assert_eq!(unit.id, "9");
        assert!(unit.acceptance.is_some());
        let acceptance = unit.acceptance.as_ref().unwrap();
        assert!(acceptance.contains("Criterion 1"));
        assert!(acceptance.contains("Criterion 2"));
        assert!(unit.description.is_some());
    }

    #[test]
    fn test_parse_md_with_crlf_line_endings() {
        let content = "---\r\nid: \"10\"\r\ntitle: CRLF Test\r\nstatus: open\r\npriority: 2\r\ncreated_at: \"2026-01-01T00:00:00Z\"\r\nupdated_at: \"2026-01-01T00:00:00Z\"\r\n---\r\n\r\n# Body\r\n\r\nCRLF line endings.";
        let unit = Unit::from_string(content).unwrap();
        assert_eq!(unit.id, "10");
        assert_eq!(unit.title, "CRLF Test");
        assert!(unit.description.is_some());
    }

    #[test]
    fn test_parse_md_description_does_not_override_yaml_description() {
        let content = r#"---
id: "11"
title: Override Test
status: open
priority: 2
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-01T00:00:00Z"
description: "From YAML metadata"
---

# From Markdown Body

This should not override.
"#;
        let unit = Unit::from_string(content).unwrap();
        // Description from YAML should take precedence
        assert_eq!(unit.description, Some("From YAML metadata".to_string()));
    }

    // =====================================================================
    // Tests for Unit hash methods
    // =====================================================================

    #[test]
    fn test_hash_consistency() {
        let unit1 = Unit::new("1", "Test unit");
        let unit2 = unit1.clone();
        // Same content produces same hash
        assert_eq!(unit1.hash(), unit2.hash());
        // Hash is deterministic
        assert_eq!(unit1.hash(), unit1.hash());
    }

    #[test]
    fn test_hash_changes_with_content() {
        let unit1 = Unit::new("1", "Test unit");
        let unit2 = Unit::new("1", "Different title");
        assert_ne!(unit1.hash(), unit2.hash());
    }

    #[test]
    fn test_from_file_with_hash() {
        let unit = Unit::new("42", "Hash file test");
        let expected_hash = unit.hash();

        let tmp = NamedTempFile::new().unwrap();
        unit.to_file(tmp.path()).unwrap();

        let (loaded, hash) = Unit::from_file_with_hash(tmp.path()).unwrap();
        assert_eq!(loaded, unit);
        assert_eq!(hash, expected_hash);
    }

    // =====================================================================
    // on_close serialization tests
    // =====================================================================

    #[test]
    fn on_close_empty_vec_not_serialized() {
        let unit = Unit::new("1", "No actions");
        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(!yaml.contains("on_close"));
    }

    #[test]
    fn on_close_round_trip_run_action() {
        let mut unit = Unit::new("1", "With run");
        unit.on_close = vec![OnCloseAction::Run {
            command: "echo hi".to_string(),
        }];

        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(yaml.contains("on_close"));
        assert!(yaml.contains("action: run"));
        assert!(yaml.contains("echo hi"));

        let restored: Unit = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.on_close, unit.on_close);
    }

    #[test]
    fn on_close_round_trip_notify_action() {
        let mut unit = Unit::new("1", "With notify");
        unit.on_close = vec![OnCloseAction::Notify {
            message: "Done!".to_string(),
        }];

        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(yaml.contains("action: notify"));
        assert!(yaml.contains("Done!"));

        let restored: Unit = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.on_close, unit.on_close);
    }

    #[test]
    fn on_close_round_trip_multiple_actions() {
        let mut unit = Unit::new("1", "Multiple actions");
        unit.on_close = vec![
            OnCloseAction::Run {
                command: "make deploy".to_string(),
            },
            OnCloseAction::Notify {
                message: "Deployed".to_string(),
            },
            OnCloseAction::Run {
                command: "echo cleanup".to_string(),
            },
        ];

        let yaml = serde_yml::to_string(&unit).unwrap();
        let restored: Unit = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.on_close.len(), 3);
        assert_eq!(restored.on_close, unit.on_close);
    }

    #[test]
    fn on_close_deserialized_from_yaml() {
        let yaml = r#"
id: "1"
title: From YAML
status: open
priority: 2
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-01T00:00:00Z"
on_close:
  - action: run
    command: "cargo test"
  - action: notify
    message: "Tests passed"
"#;
        let unit: Unit = serde_yml::from_str(yaml).unwrap();
        assert_eq!(unit.on_close.len(), 2);
        assert_eq!(
            unit.on_close[0],
            OnCloseAction::Run {
                command: "cargo test".to_string()
            }
        );
        assert_eq!(
            unit.on_close[1],
            OnCloseAction::Notify {
                message: "Tests passed".to_string()
            }
        );
    }

    // =====================================================================
    // RunResult / RunRecord / history tests
    // =====================================================================

    #[test]
    fn history_empty_not_serialized() {
        let unit = Unit::new("1", "No history");
        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(!yaml.contains("history:"));
    }

    #[test]
    fn history_round_trip_yaml() {
        let now = Utc::now();
        let mut unit = Unit::new("1", "With history");
        unit.history = vec![
            RunRecord {
                attempt: 1,
                started_at: now,
                finished_at: Some(now),
                duration_secs: Some(5.2),
                agent: Some("agent-1".to_string()),
                result: RunResult::Fail,
                exit_code: Some(1),
                tokens: None,
                cost: None,
                output_snippet: Some("error: test failed".to_string()),
                autonomy_observation: None,
            },
            RunRecord {
                attempt: 2,
                started_at: now,
                finished_at: Some(now),
                duration_secs: Some(3.1),
                agent: Some("agent-1".to_string()),
                result: RunResult::Pass,
                exit_code: Some(0),
                tokens: Some(12000),
                cost: Some(0.05),
                output_snippet: None,
                autonomy_observation: None,
            },
        ];

        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(yaml.contains("history:"));

        let restored: Unit = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.history.len(), 2);
        assert_eq!(restored.history[0].result, RunResult::Fail);
        assert_eq!(restored.history[1].result, RunResult::Pass);
        assert_eq!(restored.history[0].attempt, 1);
        assert_eq!(restored.history[1].attempt, 2);
        assert_eq!(restored.history, unit.history);
    }

    #[test]
    fn history_deserialized_from_yaml() {
        let yaml = r#"
id: "1"
title: From YAML
status: open
priority: 2
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-01T00:00:00Z"
history:
  - attempt: 1
    started_at: "2026-01-01T00:01:00Z"
    duration_secs: 10.0
    result: timeout
    exit_code: 124
  - attempt: 2
    started_at: "2026-01-01T00:05:00Z"
    finished_at: "2026-01-01T00:05:03Z"
    duration_secs: 3.0
    agent: agent-7
    result: pass
    exit_code: 0
"#;
        let unit: Unit = serde_yml::from_str(yaml).unwrap();
        assert_eq!(unit.history.len(), 2);
        assert_eq!(unit.history[0].result, RunResult::Timeout);
        assert_eq!(unit.history[0].exit_code, Some(124));
        assert_eq!(unit.history[1].result, RunResult::Pass);
        assert_eq!(unit.history[1].agent, Some("agent-7".to_string()));
    }

    // =====================================================================
    // on_fail serialization tests
    // =====================================================================

    #[test]
    fn on_fail_none_not_serialized() {
        let unit = Unit::new("1", "No fail action");
        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(!yaml.contains("on_fail"));
    }

    #[test]
    fn on_fail_retry_round_trip() {
        let mut unit = Unit::new("1", "With retry");
        unit.on_fail = Some(OnFailAction::Retry {
            max: Some(5),
            delay_secs: Some(10),
        });

        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(yaml.contains("on_fail"));
        assert!(yaml.contains("action: retry"));
        assert!(yaml.contains("max: 5"));
        assert!(yaml.contains("delay_secs: 10"));

        let restored: Unit = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.on_fail, unit.on_fail);
    }

    #[test]
    fn on_fail_retry_minimal_round_trip() {
        let mut unit = Unit::new("1", "Retry minimal");
        unit.on_fail = Some(OnFailAction::Retry {
            max: None,
            delay_secs: None,
        });

        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(yaml.contains("action: retry"));
        // Optional fields should be omitted
        assert!(!yaml.contains("max:"));
        assert!(!yaml.contains("delay_secs:"));

        let restored: Unit = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.on_fail, unit.on_fail);
    }

    #[test]
    fn on_fail_escalate_round_trip() {
        let mut unit = Unit::new("1", "With escalate");
        unit.on_fail = Some(OnFailAction::Escalate {
            priority: Some(0),
            message: Some("Needs attention".to_string()),
        });

        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(yaml.contains("action: escalate"));
        assert!(yaml.contains("priority: 0"));
        assert!(yaml.contains("Needs attention"));

        let restored: Unit = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.on_fail, unit.on_fail);
    }

    #[test]
    fn on_fail_escalate_minimal_round_trip() {
        let mut unit = Unit::new("1", "Escalate minimal");
        unit.on_fail = Some(OnFailAction::Escalate {
            priority: None,
            message: None,
        });

        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(yaml.contains("action: escalate"));
        // The on_fail block should not contain priority or message
        // (the unit itself has a top-level priority field, so check within on_fail)
        let on_fail_section = yaml.split("on_fail:").nth(1).unwrap();
        let on_fail_end = on_fail_section
            .find("\non_close:")
            .or_else(|| on_fail_section.find("\nhistory:"))
            .unwrap_or(on_fail_section.len());
        let on_fail_block = &on_fail_section[..on_fail_end];
        assert!(
            !on_fail_block.contains("priority:"),
            "on_fail block should not contain priority"
        );
        assert!(
            !on_fail_block.contains("message:"),
            "on_fail block should not contain message"
        );

        let restored: Unit = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.on_fail, unit.on_fail);
    }

    #[test]
    fn on_fail_deserialized_from_yaml() {
        let yaml = r#"
id: "1"
title: From YAML
status: open
priority: 2
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-01T00:00:00Z"
on_fail:
  action: retry
  max: 3
  delay_secs: 30
"#;
        let unit: Unit = serde_yml::from_str(yaml).unwrap();
        assert_eq!(
            unit.on_fail,
            Some(OnFailAction::Retry {
                max: Some(3),
                delay_secs: Some(30),
            })
        );
    }

    #[test]
    fn on_fail_escalate_deserialized_from_yaml() {
        let yaml = r#"
id: "1"
title: Escalate YAML
status: open
priority: 2
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-01T00:00:00Z"
on_fail:
  action: escalate
  priority: 0
  message: "Critical failure"
"#;
        let unit: Unit = serde_yml::from_str(yaml).unwrap();
        assert_eq!(
            unit.on_fail,
            Some(OnFailAction::Escalate {
                priority: Some(0),
                message: Some("Critical failure".to_string()),
            })
        );
    }

    // =====================================================================
    // outputs field tests
    // =====================================================================

    #[test]
    fn outputs_none_not_serialized() {
        let unit = Unit::new("1", "No outputs");
        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(
            !yaml.contains("outputs:"),
            "outputs field should be omitted when None, got:\n{yaml}"
        );
    }

    #[test]
    fn outputs_round_trip_nested_object() {
        let mut unit = Unit::new("1", "With outputs");
        unit.outputs = Some(serde_json::json!({
            "test_results": {
                "passed": 42,
                "failed": 0,
                "skipped": 3
            },
            "coverage": 87.5
        }));

        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(yaml.contains("outputs"));

        let restored: Unit = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.outputs, unit.outputs);
        let out = restored.outputs.unwrap();
        assert_eq!(out["test_results"]["passed"], 42);
        assert_eq!(out["coverage"], 87.5);
    }

    #[test]
    fn outputs_round_trip_array() {
        let mut unit = Unit::new("1", "Array outputs");
        unit.outputs = Some(serde_json::json!(["artifact1.tar.gz", "artifact2.zip"]));

        let yaml = serde_yml::to_string(&unit).unwrap();
        let restored: Unit = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.outputs, unit.outputs);
        let arr = restored.outputs.unwrap();
        assert_eq!(arr.as_array().unwrap().len(), 2);
        assert_eq!(arr[0], "artifact1.tar.gz");
    }

    #[test]
    fn outputs_round_trip_simple_values() {
        // String value
        let mut unit = Unit::new("1", "String output");
        unit.outputs = Some(serde_json::json!("just a string"));
        let yaml = serde_yml::to_string(&unit).unwrap();
        let restored: Unit = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.outputs, unit.outputs);

        // Number value
        unit.outputs = Some(serde_json::json!(42));
        let yaml = serde_yml::to_string(&unit).unwrap();
        let restored: Unit = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.outputs, unit.outputs);

        // Boolean value
        unit.outputs = Some(serde_json::json!(true));
        let yaml = serde_yml::to_string(&unit).unwrap();
        let restored: Unit = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.outputs, unit.outputs);
    }

    #[test]
    fn max_loops_defaults_to_none() {
        let unit = Unit::new("1", "No max_loops");
        assert_eq!(unit.max_loops, None);
        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(!yaml.contains("max_loops:"));
    }

    #[test]
    fn max_loops_overrides_config_when_set() {
        let mut unit = Unit::new("1", "With max_loops");
        unit.max_loops = Some(5);

        let yaml = serde_yml::to_string(&unit).unwrap();
        assert!(yaml.contains("max_loops: 5"));

        let restored: Unit = serde_yml::from_str(&yaml).unwrap();
        assert_eq!(restored.max_loops, Some(5));
    }

    #[test]
    fn max_loops_effective_returns_unit_value_when_set() {
        let mut unit = Unit::new("1", "Override");
        unit.max_loops = Some(20);
        assert_eq!(unit.effective_max_loops(10), 20);
    }

    #[test]
    fn max_loops_effective_returns_config_value_when_none() {
        let unit = Unit::new("1", "Default");
        assert_eq!(unit.effective_max_loops(10), 10);
        assert_eq!(unit.effective_max_loops(42), 42);
    }

    #[test]
    fn max_loops_zero_means_unlimited() {
        let mut unit = Unit::new("1", "Unlimited");
        unit.max_loops = Some(0);
        assert_eq!(unit.effective_max_loops(10), 0);

        // Config-level zero also works
        let unit2 = Unit::new("2", "Config unlimited");
        assert_eq!(unit2.effective_max_loops(0), 0);
    }

    #[test]
    fn outputs_deserialized_from_yaml() {
        let yaml = r#"
id: "1"
title: Outputs YAML
status: open
priority: 2
created_at: "2026-01-01T00:00:00Z"
updated_at: "2026-01-01T00:00:00Z"
outputs:
  binary: /tmp/build/app
  size_bytes: 1048576
  checksums:
    sha256: abc123
"#;
        let unit: Unit = serde_yml::from_str(yaml).unwrap();
        assert!(unit.outputs.is_some());
        let out = unit.outputs.unwrap();
        assert_eq!(out["binary"], "/tmp/build/app");
        assert_eq!(out["size_bytes"], 1048576);
        assert_eq!(out["checksums"]["sha256"], "abc123");
    }
}
