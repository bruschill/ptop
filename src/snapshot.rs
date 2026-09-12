//! Serializable snapshot of live monitor state for the JSON / Web API.
//!
//! Builds an owned, JSON-friendly view from an [`App`] so headless consumers
//! (e.g. a web server) can serialize the same data the TUI renders without
//! depending on ratatui. The list fields stay lean; a bounded tail of the
//! richer per-session telemetry and token history is also included for the
//! detail view. Prompt text, assistant text, tool data, and child transcripts
//! are never included.
//!
//! This is a pure read: [`App::to_snapshot`] never ticks or spawns anything.
//! Call it after [`App::tick`] on a background thread.

use crate::app::App;
use crate::host_info::{AgentAggregate, HostMetrics};
use crate::model::{
    AttachmentConfidence, AttachmentState, ChildProcess, ContextTelemetryDetails, FleetTelemetry,
    OrphanPort, SessionStatus, SourceHealth, TelemetryMetadata, UsageTelemetryDetails,
};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Top-level snapshot returned by [`App::to_snapshot`].
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// Unix-epoch milliseconds when this snapshot was built.
    pub generated_at_ms: u64,
    /// Host vitals (CPU / mem / load1). `None` on unsupported platforms or
    /// before the first valid sample.
    pub host: Option<HostMetrics>,
    /// Legacy aggregate metrics. In Pi process-only mode, token, context, and
    /// active-count members are compatibility placeholders rather than zeros.
    pub aggregate: AgentAggregate,
    /// Most recent per-tick token rate: the delta of *active* tokens, where
    /// active = input + output + cache_create (cache_read is excluded to avoid
    /// inflated rates). It therefore will NOT equal successive `total_tokens`
    /// diffs (which include cache_read). `0.0` on the first tick of a fresh
    /// process (no prior totals to diff against). This field is a compatibility
    /// placeholder when `token_rate_value` is `null`.
    pub token_rate: f64,
    /// Authoritative per-tick token rate, or `null` when usage is unavailable.
    pub token_rate_value: Option<f64>,
    /// Collector tick interval in milliseconds. Divide `token_rate` by
    /// `interval_ms / 1000` for a per-second rate.
    pub interval_ms: u64,
    /// Live Pi sessions, newest first (same order as the TUI).
    pub sessions: Vec<SessionView>,
    /// Ports left open by processes whose parent session has ended. Empty on a
    /// one-shot snapshot — orphan detection needs cross-tick history, so it
    /// only populates for a long-running monitor.
    pub orphan_ports: Vec<OrphanPort>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextTelemetryView {
    pub percent: Option<f64>,
    #[serde(flatten)]
    pub details: ContextTelemetryDetails,
    pub window_tokens: Option<u64>,
    #[serde(flatten)]
    pub metadata: TelemetryMetadata,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageTelemetryView {
    pub total_tokens: Option<u64>,
    #[serde(flatten)]
    pub details: UsageTelemetryDetails,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_create_tokens: Option<u64>,
    #[serde(flatten)]
    pub metadata: TelemetryMetadata,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionTelemetryView {
    pub attachment: AttachmentState,
    pub attachment_confidence: AttachmentConfidence,
    pub source_health: SourceHealth,
    pub error: Option<String>,
    pub context: ContextTelemetryView,
    pub usage: UsageTelemetryView,
    /// Local pi-subagents lifecycle metadata. Run aggregates remain separate
    /// from parent transcript totals to avoid adapter double counting.
    pub fleet: FleetTelemetry,
}

/// A single session, flattened and curated for JSON consumers.
#[derive(Debug, Clone, Serialize)]
pub struct SessionView {
    /// OS process id of the Pi process for this session.
    pub pid: u32,
    /// Opaque process-start identity, or `null` when PID reuse cannot be
    /// guarded with a platform start identity.
    pub process_start_id: Option<String>,
    /// Agent-assigned session identifier (stable for the life of the session).
    pub session_id: String,
    /// Project / workspace name (usually the basename of `cwd`).
    pub project_name: String,
    /// Absolute working directory of the session.
    pub cwd: String,
    /// Coarse activity state; serializes as its variant name (e.g. `"Thinking"`).
    pub status: SessionStatus,
    /// Model identifier reported by the session (e.g. `"claude-opus-4-6"`).
    pub model: String,
    /// Reasoning effort reported by Pi; empty when unavailable.
    pub effort: String,
    /// Agent CLI version string, if known.
    pub version: String,
    /// Legacy context-window fill. For Pi records this is a compatibility
    /// placeholder; use `telemetry.context.percent` as the authoritative value.
    pub context_percent: f64,
    /// Legacy context-window size. Pi-aware consumers must use
    /// `telemetry.context.window_tokens`.
    pub context_window: u64,
    /// Legacy token total. Pi-aware consumers must use
    /// `telemetry.usage.total_tokens`.
    pub total_tokens: u64,
    /// Cumulative input (prompt) tokens for the session.
    pub input_tokens: u64,
    /// Cumulative output (completion) tokens for the session.
    pub output_tokens: u64,
    /// Cumulative cache-read tokens (excluded from the active-token rate).
    pub cache_read_tokens: u64,
    /// Cumulative cache-write (cache-creation) tokens.
    pub cache_create_tokens: u64,
    /// Number of user/assistant turns observed.
    pub turn_count: u32,
    /// Resident memory of the session process tree, in MiB.
    pub mem_mb: u64,
    /// Current git branch of `cwd`, or empty when not a repo.
    pub git_branch: String,
    /// Files added in the working tree (git status), not session-scoped.
    pub git_added: u32,
    /// Files modified in the working tree (git status), not session-scoped.
    pub git_modified: u32,
    /// First collector observation, in Unix-epoch milliseconds.
    pub started_at_ms: u64,
    /// Wall-clock seconds since `started_at_ms`.
    pub elapsed_secs: u64,
    /// Metadata-only Pi attachment state label.
    pub summary: String,
    /// Most recent current-task line, if any.
    pub current_task: Option<String>,
    /// Child processes, each with any owned listening port.
    pub children: Vec<ChildProcess>,
    // --- richer fields for the per-session detail view ---
    /// Number of detected context-compaction events.
    pub compaction_count: u32,
    /// Per-turn token totals for a relative per-session sparkline.
    pub token_history: Vec<u64>,
    /// Authoritative telemetry state for collectors that must distinguish
    /// unknown values from numeric compatibility placeholders.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<SessionTelemetryView>,
}

/// Keep at most the last `n` items of a slice.
fn tail<T: Clone>(v: &[T], n: usize) -> Vec<T> {
    if v.len() > n {
        v[v.len() - n..].to_vec()
    } else {
        v.to_vec()
    }
}

fn epoch_ms(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

impl App {
    /// Build an owned, JSON-serializable snapshot of the current monitor state.
    ///
    /// Pure read. Intended flow for a web server: lock the `App`, call `tick()`,
    /// call `to_snapshot()`, then release the lock.
    pub fn to_snapshot(&self, interval_ms: u64) -> Snapshot {
        let now = SystemTime::now();

        let sessions = self
            .sessions
            .iter()
            .map(|s| SessionView {
                pid: s.pid,
                process_start_id: s.process_start_id.clone(),
                session_id: s.session_id.clone(),
                project_name: s.project_name.clone(),
                cwd: s.cwd.clone(),
                status: s.status.clone(),
                model: s.model.clone(),
                effort: s.effort.clone(),
                version: s.version.clone(),
                context_percent: s.context_percent,
                context_window: s.context_window,
                total_tokens: s.total_tokens(),
                input_tokens: s.total_input_tokens,
                output_tokens: s.total_output_tokens,
                cache_read_tokens: s.total_cache_read,
                cache_create_tokens: s.total_cache_create,
                turn_count: s.turn_count,
                mem_mb: s.mem_mb,
                git_branch: s.git_branch.clone(),
                git_added: s.git_added,
                git_modified: s.git_modified,
                started_at_ms: s.started_at,
                elapsed_secs: s.elapsed().as_secs(),
                summary: self.session_summary(s),
                current_task: s.current_tasks.last().cloned(),
                children: s
                    .children
                    .iter()
                    .map(|child| ChildProcess {
                        pid: child.pid,
                        command: crate::model::safe_process_label(&child.command),
                        mem_kb: child.mem_kb,
                        port: child.port,
                    })
                    .collect(),
                compaction_count: s.compaction_count,
                token_history: tail(&s.token_history, 64),
                telemetry: s.telemetry.as_ref().map(|telemetry| {
                    let usage_known = s.total_tokens_value().is_some();
                    SessionTelemetryView {
                        attachment: telemetry.attachment,
                        attachment_confidence: telemetry.attachment_confidence,
                        source_health: telemetry.source_health,
                        error: telemetry.error.clone(),
                        context: ContextTelemetryView {
                            percent: s.context_value(),
                            window_tokens: s.context_window_value(),
                            details: telemetry.context_details.clone(),
                            metadata: telemetry.context.clone(),
                        },
                        usage: UsageTelemetryView {
                            total_tokens: s.total_tokens_value(),
                            details: telemetry.usage_details.clone(),
                            input_tokens: usage_known.then_some(s.total_input_tokens),
                            output_tokens: usage_known.then_some(s.total_output_tokens),
                            cache_read_tokens: usage_known.then_some(s.total_cache_read),
                            cache_create_tokens: usage_known.then_some(s.total_cache_create),
                            metadata: telemetry.usage.clone(),
                        },
                        fleet: telemetry.fleet.clone(),
                    }
                }),
            })
            .collect();

        Snapshot {
            generated_at_ms: epoch_ms(now).unwrap_or(0),
            host: self.host_metrics,
            aggregate: self.agent_aggregate,
            token_rate: self.token_rates.back().copied().unwrap_or(0.0),
            token_rate_value: self
                .token_rate_known
                .then(|| self.token_rates.back().copied().unwrap_or(0.0)),
            interval_ms,
            sessions,
            orphan_ports: self
                .orphan_ports
                .iter()
                .map(|orphan| OrphanPort {
                    port: orphan.port,
                    pid: orphan.pid,
                    command: crate::model::safe_process_label(&orphan.command),
                    project_name: orphan.project_name.clone(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::config::PanelVisibility;
    use crate::demo::populate_demo;
    use crate::model::{
        FleetExecution, FleetRun, FleetRunMode, FleetRunState, FleetTelemetry, FleetUsage,
        SessionStatus, SourceHealth, TelemetryCompleteness, TelemetryPrecision,
    };
    use crate::theme::Theme;
    use std::time::{Duration, UNIX_EPOCH};

    fn demo_app() -> App {
        let mut app = App::new(Theme::default(), PanelVisibility::default());
        populate_demo(&mut app);
        app
    }

    #[test]
    fn tail_keeps_last_n_and_handles_short_inputs() {
        let v = vec![1, 2, 3, 4, 5];
        assert_eq!(tail(&v, 2), vec![4, 5]); // last n
        assert_eq!(tail(&v, 5), vec![1, 2, 3, 4, 5]); // exact fit
        assert_eq!(tail(&v, 9), vec![1, 2, 3, 4, 5]); // n > len → full clone
        assert_eq!(tail(&v, 0), Vec::<i32>::new()); // n = 0 → empty
        assert_eq!(tail(&Vec::<i32>::new(), 3), Vec::<i32>::new()); // empty input
    }

    #[test]
    fn epoch_ms_is_monotonic_and_zero_at_unix_epoch() {
        assert_eq!(epoch_ms(UNIX_EPOCH), Some(0));
        let later = UNIX_EPOCH + Duration::from_millis(1_500);
        assert_eq!(epoch_ms(later), Some(1_500));
    }

    #[test]
    fn session_status_serializes_as_variant_name() {
        // The web UI matches on these exact strings — they are part of the
        // stable JSON contract and must not be renamed without a major bump.
        for (status, wire) in [
            (SessionStatus::Thinking, "\"Thinking\""),
            (SessionStatus::Executing, "\"Executing\""),
            (SessionStatus::Waiting, "\"Waiting\""),
            (SessionStatus::Unknown, "\"Unknown\""),
            (SessionStatus::Done, "\"Done\""),
        ] {
            assert_eq!(serde_json::to_string(&status).unwrap(), wire);
        }
    }

    #[test]
    fn to_snapshot_is_a_pure_read() {
        let app = demo_app();
        let before = app.sessions.len();
        let a = app.to_snapshot(2_000);
        let b = app.to_snapshot(2_000);
        // No mutation of the App, and repeated calls agree on shape.
        assert_eq!(app.sessions.len(), before);
        assert_eq!(a.sessions.len(), b.sessions.len());
        assert_eq!(a.sessions.len(), before);
    }

    #[test]
    fn to_snapshot_maps_fields_and_passes_interval_through() {
        let app = demo_app();
        let snap = app.to_snapshot(1_234);

        assert_eq!(snap.interval_ms, 1_234);
        assert!(snap.generated_at_ms > 0);
        assert!(!snap.sessions.is_empty());
        assert!(snap.host.is_some(), "demo populates host metrics");

        for session in &snap.sessions {
            assert!(session.token_history.len() <= 64);
        }
    }

    #[test]
    fn pi_snapshot_uses_structured_unknowns_instead_of_numeric_placeholders() {
        let mut app = demo_app();
        app.token_rate_known = false;
        let session = app.sessions.first_mut().unwrap();
        session.context_percent = 0.0;
        session.context_window = 0;
        session.total_input_tokens = 0;
        session.total_output_tokens = 0;
        session.total_cache_read = 0;
        session.total_cache_create = 0;
        session.telemetry = Some(crate::model::SessionTelemetry::process_only(123));
        let telemetry = session.telemetry.as_mut().unwrap();
        telemetry.context_details.provider = Some("test-provider".to_string());
        let mut fleet = FleetTelemetry::unavailable(123, "test");
        fleet.source_health = SourceHealth::Healthy;
        fleet.reason = None;
        let mut run_usage = FleetUsage::separate_run_aggregate();
        run_usage.total_tokens = Some(42);
        fleet.runs.push(FleetRun {
            lifecycle_version: Some(3),
            run_id: "run-1".to_string(),
            parent_run_id: None,
            nested: false,
            mode: FleetRunMode::Workflow,
            state: FleetRunState::Running,
            execution: FleetExecution::Background,
            runner_pid: None,
            started_at_ms: Some(100),
            updated_at_ms: Some(120),
            ended_at_ms: None,
            source_updated_at_ms: 120,
            stale: false,
            process_terminal: None,
            usage: run_usage,
            children: Vec::new(),
            omitted_children: 0,
            reason: None,
        });
        telemetry.fleet = fleet;
        session.process_start_id = Some("test:1".to_string());
        session.children = vec![ChildProcess {
            pid: 99,
            command: "node --task private-prompt".to_string(),
            mem_kb: 1,
            port: None,
        }];
        app.orphan_ports = vec![OrphanPort {
            port: 3000,
            pid: 100,
            command: "bun --prompt private-orphan".to_string(),
            project_name: "project".to_string(),
        }];

        let snap = app.to_snapshot(2_000);
        let pi = &snap.sessions[0];
        assert_eq!(snap.token_rate_value, None);
        let telemetry = pi.telemetry.as_ref().unwrap();
        assert_eq!(telemetry.context.percent, None);
        assert_eq!(telemetry.context.window_tokens, None);
        assert_eq!(telemetry.usage.total_tokens, None);
        assert_eq!(telemetry.fleet.runs[0].usage.total_tokens, Some(42));
        assert_eq!(
            telemetry.context.details.provider.as_deref(),
            Some("test-provider")
        );
        assert_eq!(pi.process_start_id.as_deref(), Some("test:1"));
        assert_eq!(pi.children[0].command, "node");
        assert_eq!(snap.orphan_ports[0].command, "bun");
        assert_eq!(telemetry.fleet.runs[0].run_id, "run-1");
        assert_eq!(telemetry.fleet.runs[0].state, FleetRunState::Running);

        let json = serde_json::to_value(&snap).unwrap();
        let session_json = json["sessions"][0].as_object().unwrap();
        for private_or_legacy_field in [
            "agent_cli",
            "config_root",
            "subagents",
            "chat_messages",
            "tool_calls",
            "initial_prompt",
            "first_assistant_text",
            "file_accesses",
        ] {
            assert!(
                !session_json.contains_key(private_or_legacy_field),
                "snapshot exposed {private_or_legacy_field}"
            );
        }
        assert!(json["sessions"][0]["telemetry"]["context"]["percent"].is_null());
        assert_eq!(
            json["sessions"][0]["telemetry"]["fleet"]["runs"][0]["usage"]["accounting"],
            "separate_run_aggregate"
        );
        assert!(!json.to_string().contains("private-"));
    }

    #[test]
    fn pi_json_distinguishes_unknown_known_zero_inferred_estimated_and_partial() {
        let mut app = demo_app();
        app.token_rate_known = false;
        let base = app.sessions[0].clone();
        app.sessions.clear();

        let make_session = |id: &str,
                            precision: TelemetryPrecision,
                            completeness: TelemetryCompleteness,
                            percent: f64,
                            window: u64,
                            tokens: u64| {
            let mut session = base.clone();
            session.session_id = id.to_string();
            session.context_percent = percent;
            session.context_window = window;
            session.total_input_tokens = tokens;
            session.total_output_tokens = 0;
            session.total_cache_read = 0;
            session.total_cache_create = 0;
            let mut telemetry = crate::model::SessionTelemetry::process_only(123);
            telemetry.context.precision = precision;
            telemetry.context.completeness = completeness;
            telemetry.context_details.tokens =
                (precision != TelemetryPrecision::Unknown).then_some(tokens);
            telemetry.usage.precision = precision;
            telemetry.usage.completeness = completeness;
            session.telemetry = Some(telemetry);
            session
        };

        app.sessions.push(make_session(
            "unknown",
            TelemetryPrecision::Unknown,
            TelemetryCompleteness::Unknown,
            0.0,
            0,
            0,
        ));
        app.sessions.push(make_session(
            "known-zero",
            TelemetryPrecision::Exact,
            TelemetryCompleteness::Complete,
            0.0,
            200_000,
            0,
        ));
        app.sessions.push(make_session(
            "inferred",
            TelemetryPrecision::Inferred,
            TelemetryCompleteness::Complete,
            42.0,
            200_000,
            14,
        ));
        app.sessions.push(make_session(
            "estimated-partial",
            TelemetryPrecision::Estimated,
            TelemetryCompleteness::Partial,
            43.0,
            200_000,
            15,
        ));

        let json = serde_json::to_value(app.to_snapshot(2_000)).unwrap();
        let sessions = json["sessions"].as_array().unwrap();
        assert!(sessions[0]["telemetry"]["context"]["percent"].is_null());
        assert!(sessions[0]["telemetry"]["usage"]["total_tokens"].is_null());
        assert_eq!(sessions[1]["telemetry"]["context"]["percent"], 0.0);
        assert_eq!(sessions[1]["telemetry"]["usage"]["total_tokens"], 0);
        assert_eq!(sessions[1]["telemetry"]["context"]["precision"], "exact");
        assert_eq!(sessions[2]["telemetry"]["context"]["precision"], "inferred");
        assert_eq!(sessions[2]["telemetry"]["context"]["percent"], 42.0);
        assert_eq!(
            sessions[3]["telemetry"]["context"]["precision"],
            "estimated"
        );
        assert_eq!(sessions[3]["telemetry"]["usage"]["completeness"], "partial");
        assert_eq!(sessions[3]["telemetry"]["usage"]["total_tokens"], 15);
    }

    #[test]
    fn snapshot_round_trips_through_serde_json() {
        let snap = demo_app().to_snapshot(2_000);
        let json = serde_json::to_string(&snap).expect("snapshot serializes");
        assert!(json.contains("\"sessions\""));
        assert!(json.contains("\"interval_ms\":2000"));
        // Re-parse as generic JSON to confirm it is well-formed.
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert!(parsed["sessions"].is_array());
    }

    #[test]
    fn readme_documents_json_snapshot_privacy_surface() {
        let readme = include_str!("../README.md");
        assert!(readme.contains("--json"));
        assert!(readme.contains("JSON snapshot includes"));
        assert!(readme.contains("chat_messages"));
        assert!(readme.contains("summary"));
    }
}
