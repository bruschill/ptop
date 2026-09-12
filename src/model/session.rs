use serde::Serialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum SessionStatus {
    /// Model is generating a response (last_user_ts_ms > 0)
    Thinking,
    /// Running a tool (descendant CPU active OR current_task non-empty)
    Executing,
    /// Idle, waiting for user input or permission prompt
    Waiting,
    /// Activity is unavailable, or process ownership is not confirmed.
    Unknown,
    /// Session finished
    Done,
}

impl SessionStatus {
    /// Returns true for states where the agent is actively doing work.
    pub fn is_active(&self) -> bool {
        matches!(self, SessionStatus::Thinking | SessionStatus::Executing)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryPrecision {
    Unknown,
    Inferred,
    Estimated,
    Exact,
}

impl TelemetryPrecision {
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Unknown | Self::Exact => "",
            Self::Inferred => "~",
            Self::Estimated => "≈",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Inferred => "inferred",
            Self::Estimated => "estimated",
            Self::Exact => "exact",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryCompleteness {
    Unknown,
    Partial,
    Complete,
}

impl TelemetryCompleteness {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Partial => "partial",
            Self::Complete => "complete",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentState {
    ProcessOnly,
    Attached,
}

impl AttachmentState {
    pub fn label(self) -> &'static str {
        match self {
            Self::ProcessOnly => "process only",
            Self::Attached => "attached",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentConfidence {
    None,
    Low,
    Medium,
    High,
}

impl AttachmentConfidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceHealth {
    Unavailable,
    Healthy,
    Stale,
    Error,
}

impl SourceHealth {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Healthy => "healthy",
            Self::Stale => "stale",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetVisibility {
    Supported,
    Unavailable,
}

impl FleetVisibility {
    pub fn label(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetRunState {
    Unknown,
    Queued,
    Running,
    Complete,
    Failed,
    Partial,
    Paused,
    Stopped,
    Rejected,
}

impl FleetRunState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Partial => "partial",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
            Self::Rejected => "rejected",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Complete
                | Self::Failed
                | Self::Partial
                | Self::Paused
                | Self::Stopped
                | Self::Rejected
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetRunMode {
    Unknown,
    Single,
    Parallel,
    Chain,
    Workflow,
}

impl FleetRunMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Single => "single",
            Self::Parallel => "parallel",
            Self::Chain => "chain",
            Self::Workflow => "workflow",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetExecution {
    Unknown,
    InProcess,
    Background,
}

impl FleetExecution {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "execution unknown",
            Self::InProcess => "in process",
            Self::Background => "background",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetIdentitySource {
    ChildId,
    WorkflowKey,
    RunId,
    Index,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetProcessTerminalState {
    Pending,
    Observed,
    Unknown,
    NotStarted,
}

impl FleetProcessTerminalState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Observed => "observed",
            Self::Unknown => "unknown",
            Self::NotStarted => "not started",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FleetProcessTerminal {
    pub state: FleetProcessTerminalState,
    pub observed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetUsageAccounting {
    /// Extension aggregate kept separate because Pi transcript tool results may
    /// already include the same child usage.
    SeparateRunAggregate,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FleetUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub reported_cost: Option<f64>,
    pub accounting: FleetUsageAccounting,
}

impl FleetUsage {
    pub fn separate_run_aggregate() -> Self {
        Self {
            input_tokens: None,
            output_tokens: None,
            total_tokens: None,
            reported_cost: None,
            accounting: FleetUsageAccounting::SeparateRunAggregate,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FleetChild {
    pub id: String,
    pub run_id: Option<String>,
    pub identity_source: FleetIdentitySource,
    pub name: String,
    pub state: FleetRunState,
    pub execution: FleetExecution,
    pub model: Option<String>,
    pub current_tool: Option<String>,
    pub activity: Option<String>,
    pub started_at_ms: Option<u64>,
    pub updated_at_ms: Option<u64>,
    pub ended_at_ms: Option<u64>,
    pub usage: FleetUsage,
    pub children: Vec<FleetChild>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FleetRun {
    pub lifecycle_version: Option<u64>,
    pub run_id: String,
    pub parent_run_id: Option<String>,
    pub nested: bool,
    pub mode: FleetRunMode,
    pub state: FleetRunState,
    pub execution: FleetExecution,
    pub runner_pid: Option<u32>,
    pub started_at_ms: Option<u64>,
    pub updated_at_ms: Option<u64>,
    pub ended_at_ms: Option<u64>,
    pub source_updated_at_ms: u64,
    pub stale: bool,
    pub process_terminal: Option<FleetProcessTerminal>,
    pub usage: FleetUsage,
    pub children: Vec<FleetChild>,
    pub omitted_children: u32,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FleetTelemetry {
    pub source_health: SourceHealth,
    pub provenance: String,
    pub foreground_visibility: FleetVisibility,
    pub background_visibility: FleetVisibility,
    pub observed_at_ms: u64,
    pub source_updated_at_ms: Option<u64>,
    pub stale: bool,
    pub retention_days: u64,
    pub scanned_statuses: u32,
    pub malformed_statuses: u32,
    pub unsupported_statuses: u32,
    pub omitted_statuses: u32,
    pub runs: Vec<FleetRun>,
    pub reason: Option<String>,
}

impl FleetTelemetry {
    pub fn unavailable(observed_at_ms: u64, reason: &str) -> Self {
        Self {
            source_health: SourceHealth::Unavailable,
            provenance: "pi-subagents status.json".to_string(),
            foreground_visibility: FleetVisibility::Unavailable,
            background_visibility: FleetVisibility::Supported,
            observed_at_ms,
            source_updated_at_ms: None,
            stale: false,
            retention_days: 30,
            scanned_statuses: 0,
            malformed_statuses: 0,
            unsupported_statuses: 0,
            omitted_statuses: 0,
            runs: Vec::new(),
            reason: Some(reason.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TelemetryMetadata {
    pub precision: TelemetryPrecision,
    pub completeness: TelemetryCompleteness,
    pub provenance: String,
    pub source_updated_at_ms: Option<u64>,
    pub observed_at_ms: u64,
    pub last_successful_parse_at_ms: Option<u64>,
    pub stale: bool,
}

impl TelemetryMetadata {
    pub fn unknown(provenance: &str, observed_at_ms: u64) -> Self {
        Self {
            precision: TelemetryPrecision::Unknown,
            completeness: TelemetryCompleteness::Unknown,
            provenance: provenance.to_string(),
            source_updated_at_ms: None,
            observed_at_ms,
            last_successful_parse_at_ms: None,
            stale: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ContextTelemetryDetails {
    pub tokens: Option<u64>,
    pub provider: Option<String>,
    pub baseline_tokens: Option<u64>,
    pub trailing_tokens: Option<u64>,
    pub active_leaf_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsageTelemetryDetails {
    pub reported_cost: Option<f64>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionTelemetry {
    pub attachment: AttachmentState,
    pub attachment_confidence: AttachmentConfidence,
    pub source_health: SourceHealth,
    pub error: Option<String>,
    pub context: TelemetryMetadata,
    pub usage: TelemetryMetadata,
    pub context_details: ContextTelemetryDetails,
    pub usage_details: UsageTelemetryDetails,
    pub fleet: FleetTelemetry,
}

impl SessionTelemetry {
    pub fn process_only(observed_at_ms: u64) -> Self {
        Self {
            attachment: AttachmentState::ProcessOnly,
            attachment_confidence: AttachmentConfidence::None,
            source_health: SourceHealth::Unavailable,
            error: None,
            context: TelemetryMetadata::unknown("process", observed_at_ms),
            usage: TelemetryMetadata::unknown("process", observed_at_ms),
            context_details: ContextTelemetryDetails {
                tokens: None,
                provider: None,
                baseline_tokens: None,
                trailing_tokens: None,
                active_leaf_id: None,
                reason: Some("no owned Pi session JSONL".to_string()),
            },
            usage_details: UsageTelemetryDetails {
                reported_cost: None,
                reason: Some("no owned Pi session JSONL".to_string()),
            },
            fleet: FleetTelemetry::unavailable(
                observed_at_ms,
                "owned parent session identity unavailable",
            ),
        }
    }
}

/// Return only a process executable name, never command arguments. Pi output
/// uses this label to avoid exposing prompts, task text, or tool arguments
/// embedded in a child command line.
pub fn safe_process_label(command: &str) -> String {
    command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(['\'', '"'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct ChildProcess {
    pub pid: u32,
    pub command: String,
    pub mem_kb: u64,
    pub port: Option<u16>,
}

/// A port left open by a process whose parent session has ended.
#[derive(Debug, Clone, Serialize)]
pub struct OrphanPort {
    pub port: u16,
    pub pid: u32,
    pub command: String,
    pub project_name: String,
}

#[derive(Debug, Clone)]
pub struct AgentSession {
    pub pid: u32,
    pub session_id: String,
    pub cwd: String,
    pub project_name: String,
    pub started_at: u64,
    pub status: SessionStatus,
    pub model: String,
    /// Reasoning effort reported by Pi's `thinkingLevel` field.
    /// Empty when unavailable.
    pub effort: String,
    pub context_percent: f64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_read: u64,
    pub total_cache_create: u64,
    pub turn_count: u32,
    pub current_tasks: Vec<String>,
    pub mem_mb: u64,
    pub version: String,
    pub git_branch: String,
    pub git_added: u32,
    pub git_modified: u32,
    pub token_history: Vec<u64>,
    /// Per-turn context size (input tokens) for context evolution visualization.
    pub context_history: Vec<u64>,
    /// Number of detected compaction events (context dropped > 30% between turns).
    pub compaction_count: u32,
    /// Context window size for this session's model (e.g. 200K, 1M).
    pub context_window: u64,
    pub children: Vec<ChildProcess>,
    /// Structured Pi telemetry distinguishes unavailable values from known zero.
    pub telemetry: Option<SessionTelemetry>,
    /// Platform process-start identity used to detect PID reuse where the host
    /// exposes it. The value is opaque and only compared for equality.
    pub process_start_id: Option<String>,
}

impl AgentSession {
    pub fn total_tokens(&self) -> u64 {
        self.total_input_tokens
            .saturating_add(self.total_output_tokens)
            .saturating_add(self.total_cache_read)
            .saturating_add(self.total_cache_create)
    }

    /// Tokens that represent new work (input + output), excluding cache hits.
    /// Used for rate calculation to avoid inflated numbers from cache_read.
    pub fn active_tokens(&self) -> u64 {
        self.total_input_tokens
            .saturating_add(self.total_output_tokens)
            .saturating_add(self.total_cache_create)
    }

    pub fn context_precision(&self) -> TelemetryPrecision {
        self.telemetry
            .as_ref()
            .map(|telemetry| telemetry.context.precision)
            .unwrap_or(TelemetryPrecision::Exact)
    }

    pub fn usage_precision(&self) -> TelemetryPrecision {
        self.telemetry
            .as_ref()
            .map(|telemetry| telemetry.usage.precision)
            .unwrap_or(TelemetryPrecision::Exact)
    }

    /// Percentage requires both context tokens and a resolved context window.
    pub fn context_value(&self) -> Option<f64> {
        (self.telemetry.is_none()
            || (self.context_precision() != TelemetryPrecision::Unknown && self.context_window > 0))
            .then_some(self.context_percent)
    }

    pub fn context_window_value(&self) -> Option<u64> {
        (self.context_window > 0).then_some(self.context_window)
    }

    /// Context tokens remain meaningful when the model window cannot be resolved.
    pub fn context_tokens_value(&self) -> Option<u64> {
        self.telemetry
            .as_ref()
            .and_then(|telemetry| telemetry.context_details.tokens)
    }

    pub fn context_tokens_without_window(&self) -> Option<u64> {
        (self.context_window_value().is_none())
            .then(|| self.context_tokens_value())
            .flatten()
    }

    /// A partial passive total is useful to display, but must not drive aggregates or rates.
    pub fn total_tokens_value(&self) -> Option<u64> {
        (self.usage_precision() != TelemetryPrecision::Unknown).then(|| self.total_tokens())
    }

    pub fn complete_total_tokens_value(&self) -> Option<u64> {
        let complete = self.telemetry.as_ref().is_none_or(|telemetry| {
            telemetry.usage.completeness == TelemetryCompleteness::Complete
        });
        complete.then(|| self.total_tokens_value()).flatten()
    }

    pub fn usage_is_partial(&self) -> bool {
        self.telemetry
            .as_ref()
            .is_some_and(|telemetry| telemetry.usage.completeness == TelemetryCompleteness::Partial)
    }

    pub fn elapsed(&self) -> Duration {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Duration::from_millis(now.saturating_sub(self.started_at))
    }

    pub fn elapsed_display(&self) -> String {
        let secs = self.elapsed().as_secs();
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else {
            format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session(input: u64, output: u64, cache_read: u64, cache_create: u64) -> AgentSession {
        AgentSession {
            pid: 0,
            session_id: String::new(),
            cwd: String::new(),
            project_name: String::new(),
            started_at: 0,
            status: SessionStatus::Waiting,
            model: String::new(),
            effort: String::new(),
            context_percent: 0.0,
            total_input_tokens: input,
            total_output_tokens: output,
            total_cache_read: cache_read,
            total_cache_create: cache_create,
            turn_count: 0,
            current_tasks: Vec::new(),
            mem_mb: 0,
            version: String::new(),
            git_branch: String::new(),
            git_added: 0,
            git_modified: 0,
            token_history: Vec::new(),
            context_history: Vec::new(),
            compaction_count: 0,
            context_window: 0,
            children: Vec::new(),
            telemetry: None,
            process_start_id: None,
        }
    }

    #[test]
    fn safe_process_label_drops_private_arguments() {
        assert_eq!(
            safe_process_label("/usr/local/bin/node --task 'private prompt'"),
            "node"
        );
        assert_eq!(safe_process_label(r#"C:\tools\bun.exe secret"#), "bun.exe");
    }

    #[test]
    fn test_total_tokens() {
        let session = make_session(100, 50, 200, 30);
        assert_eq!(session.total_tokens(), 380); // 100 + 50 + 200 + 30
        assert_eq!(make_session(u64::MAX, 1, 0, 0).total_tokens(), u64::MAX);
    }

    #[test]
    fn test_active_tokens() {
        let session = make_session(100, 50, 200, 30);
        assert_eq!(session.active_tokens(), 180); // 100 + 50 + 30, excludes cache_read
        assert_eq!(make_session(u64::MAX, 1, 0, 0).active_tokens(), u64::MAX);
    }

    #[test]
    fn process_only_telemetry_distinguishes_unknown_from_known_zero() {
        let mut session = make_session(0, 0, 0, 0);
        assert_eq!(session.context_value(), Some(0.0));
        assert_eq!(session.total_tokens_value(), Some(0));

        session.telemetry = Some(SessionTelemetry::process_only(123));
        assert_eq!(session.context_value(), None);
        assert_eq!(session.context_window_value(), None);
        assert_eq!(session.total_tokens_value(), None);
    }

    #[test]
    fn partial_usage_is_displayable_but_not_aggregateable_and_context_needs_a_window() {
        let mut session = make_session(2, 3, 4, 5);
        let mut telemetry = SessionTelemetry::process_only(1);
        telemetry.usage.precision = TelemetryPrecision::Exact;
        telemetry.usage.completeness = TelemetryCompleteness::Partial;
        telemetry.context.precision = TelemetryPrecision::Estimated;
        telemetry.context_details.tokens = Some(14);
        session.telemetry = Some(telemetry);
        session.context_percent = 7.0;
        assert_eq!(session.total_tokens_value(), Some(14));
        assert_eq!(session.complete_total_tokens_value(), None);
        assert!(session.usage_is_partial());
        assert_eq!(session.context_value(), None);
        assert_eq!(session.context_tokens_without_window(), Some(14));
        session.context_window = 200;
        assert_eq!(session.context_value(), Some(7.0));
    }
}
