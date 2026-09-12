use crate::app::App;
use crate::model::{
    AgentSession, AttachmentConfidence, AttachmentState, ChildProcess, ContextTelemetryDetails,
    FleetChild, FleetExecution, FleetIdentitySource, FleetProcessTerminal,
    FleetProcessTerminalState, FleetRun, FleetRunMode, FleetRunState, FleetTelemetry, FleetUsage,
    FleetVisibility, OrphanPort, SessionStatus, SessionTelemetry, SourceHealth,
    TelemetryCompleteness, TelemetryMetadata, TelemetryPrecision, UsageTelemetryDetails,
};
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Populate deterministic, collector-free Pi fixtures.
pub fn populate_demo(app: &mut App) {
    populate_pi_demo(app);
}

fn pi_telemetry(
    now: u64,
    provider: &str,
    baseline_tokens: u64,
    trailing_tokens: u64,
    fleet: FleetTelemetry,
) -> SessionTelemetry {
    let context_tokens = baseline_tokens + trailing_tokens;
    let context_precision = if trailing_tokens == 0 {
        TelemetryPrecision::Inferred
    } else {
        TelemetryPrecision::Estimated
    };
    let metadata = |precision: TelemetryPrecision, provenance: &str| TelemetryMetadata {
        precision,
        completeness: TelemetryCompleteness::Complete,
        provenance: provenance.to_string(),
        source_updated_at_ms: Some(now - 1_000),
        observed_at_ms: now,
        last_successful_parse_at_ms: Some(now - 1_000),
        stale: false,
    };
    SessionTelemetry {
        attachment: AttachmentState::Attached,
        attachment_confidence: AttachmentConfidence::High,
        source_health: SourceHealth::Healthy,
        error: None,
        context: metadata(context_precision, "owned Pi session JSONL"),
        usage: metadata(
            TelemetryPrecision::Exact,
            "observed Pi session JSONL entries",
        ),
        context_details: ContextTelemetryDetails {
            tokens: Some(context_tokens),
            provider: Some(provider.into()),
            baseline_tokens: Some(baseline_tokens),
            trailing_tokens: Some(trailing_tokens),
            active_leaf_id: Some("leaf-demo".into()),
            reason: None,
        },
        usage_details: UsageTelemetryDetails {
            reported_cost: Some(0.42),
            reason: None,
        },
        fleet,
    }
}

fn healthy_fleet(now: u64) -> FleetTelemetry {
    FleetTelemetry {
        source_health: SourceHealth::Healthy,
        provenance: "pi-subagents status.json".into(),
        foreground_visibility: FleetVisibility::Unavailable,
        background_visibility: FleetVisibility::Supported,
        observed_at_ms: now,
        source_updated_at_ms: Some(now - 2_000),
        stale: false,
        retention_days: 30,
        scanned_statuses: 1,
        malformed_statuses: 0,
        unsupported_statuses: 0,
        omitted_statuses: 0,
        runs: vec![FleetRun {
            lifecycle_version: Some(3),
            run_id: "checkout-review".into(),
            parent_run_id: None,
            nested: false,
            mode: FleetRunMode::Workflow,
            state: FleetRunState::Running,
            execution: FleetExecution::Background,
            runner_pid: Some(7310),
            started_at_ms: Some(now - 12 * 60 * 1_000),
            updated_at_ms: Some(now - 2_000),
            ended_at_ms: None,
            source_updated_at_ms: now - 2_000,
            stale: false,
            process_terminal: Some(FleetProcessTerminal {
                state: FleetProcessTerminalState::Pending,
                observed_at_ms: Some(now - 2_000),
            }),
            usage: FleetUsage {
                input_tokens: Some(18_400),
                output_tokens: Some(5_900),
                total_tokens: Some(24_300),
                reported_cost: Some(0.18),
                accounting: crate::model::FleetUsageAccounting::SeparateRunAggregate,
            },
            children: vec![FleetChild {
                id: "security-pass".into(),
                run_id: None,
                identity_source: FleetIdentitySource::ChildId,
                name: "reviewer".into(),
                state: FleetRunState::Running,
                execution: FleetExecution::Background,
                model: Some("claude-sonnet-4-6".into()),
                current_tool: Some("Read".into()),
                activity: Some("active".into()),
                started_at_ms: Some(now - 10 * 60 * 1_000),
                updated_at_ms: Some(now - 2_000),
                ended_at_ms: None,
                usage: FleetUsage {
                    input_tokens: Some(7_600),
                    output_tokens: Some(2_100),
                    total_tokens: Some(9_700),
                    reported_cost: Some(0.07),
                    accounting: crate::model::FleetUsageAccounting::SeparateRunAggregate,
                },
                children: vec![],
            }],
            omitted_children: 0,
            reason: None,
        }],
        reason: None,
    }
}

fn unavailable_fleet(now: u64) -> FleetTelemetry {
    FleetTelemetry::unavailable(now, "no Pi subagent runs discovered")
}

fn populate_pi_demo(app: &mut App) {
    let now = now_ms();
    app.sessions = vec![
        AgentSession {
            pid: 7301,
            session_id: "pi-demo-checkout".into(),
            cwd: "/Users/demo/storefront".into(),
            project_name: "storefront".into(),
            started_at: now - 42 * 60 * 1_000,
            status: SessionStatus::Executing,
            model: "claude-sonnet-4-6".into(),
            effort: String::new(),
            context_percent: 64.0,
            total_input_tokens: 52_400,
            total_output_tokens: 14_800,
            total_cache_read: 320_000,
            total_cache_create: 18_600,
            turn_count: 27,
            current_tasks: vec![],
            mem_mb: 286,
            version: "pi 0.45.0".into(),
            git_branch: "feat/checkout".into(),
            git_added: 3,
            git_modified: 7,
            token_history: vec![12_000, 18_400, 22_000, 19_600, 28_000],
            context_history: vec![48_000, 72_000, 98_000, 128_000],
            compaction_count: 0,
            context_window: 200_000,
            children: vec![ChildProcess {
                pid: 7310,
                command: "node worker.js --checkout".into(),
                mem_kb: 82_000,
                port: Some(4173),
            }],
            telemetry: Some(pi_telemetry(
                now,
                "anthropic",
                123_200,
                4_800,
                healthy_fleet(now),
            )),
            process_start_id: Some("pi-demo-7301".into()),
        },
        AgentSession {
            pid: 7402,
            session_id: "pi-demo-metrics".into(),
            cwd: "/Users/demo/observability".into(),
            project_name: "observability".into(),
            started_at: now - 18 * 60 * 1_000,
            status: SessionStatus::Waiting,
            model: "gpt-5.4".into(),
            effort: String::new(),
            context_percent: 87.0,
            total_input_tokens: 31_200,
            total_output_tokens: 8_900,
            total_cache_read: 145_000,
            total_cache_create: 9_400,
            turn_count: 16,
            current_tasks: vec![],
            mem_mb: 154,
            version: "pi 0.45.0".into(),
            git_branch: "main".into(),
            git_added: 0,
            git_modified: 2,
            token_history: vec![9_000, 15_000, 21_000, 17_000],
            context_history: vec![94_000, 132_000, 174_000],
            compaction_count: 0,
            context_window: 200_000,
            children: vec![ChildProcess {
                pid: 7414,
                command: "python metrics_server.py --debug".into(),
                mem_kb: 44_000,
                port: Some(9090),
            }],
            telemetry: Some(pi_telemetry(
                now,
                "openai",
                174_000,
                0,
                unavailable_fleet(now),
            )),
            process_start_id: Some("pi-demo-7402".into()),
        },
        AgentSession {
            pid: 7520,
            session_id: "pi-demo-release".into(),
            cwd: "/Users/demo/release-tools".into(),
            project_name: "release-tools".into(),
            started_at: now - 7 * 60 * 1_000,
            status: SessionStatus::Thinking,
            model: "claude-opus-4-6".into(),
            effort: String::new(),
            context_percent: 29.0,
            total_input_tokens: 18_900,
            total_output_tokens: 6_300,
            total_cache_read: 98_000,
            total_cache_create: 6_100,
            turn_count: 9,
            current_tasks: vec![],
            mem_mb: 198,
            version: "pi 0.45.0".into(),
            git_branch: "release/validation".into(),
            git_added: 1,
            git_modified: 4,
            token_history: vec![8_000, 13_000, 16_000],
            context_history: vec![22_000, 41_000, 58_000],
            compaction_count: 0,
            context_window: 200_000,
            children: vec![],
            telemetry: Some(pi_telemetry(
                now,
                "anthropic",
                53_000,
                5_000,
                unavailable_fleet(now),
            )),
            process_start_id: Some("pi-demo-7520".into()),
        },
    ];
    app.orphan_ports = vec![OrphanPort {
        port: 8088,
        pid: 7298,
        command: "node abandoned-preview.js --token private".into(),
        project_name: "old-preview".into(),
    }];
    // Thirty two-second intervals give the rate label a complete minute.
    app.token_rates = [
        420.0, 480.0, 510.0, 540.0, 570.0, 600.0, 630.0, 660.0, 690.0, 720.0, 750.0, 780.0, 810.0,
        780.0, 750.0, 720.0, 690.0, 660.0, 630.0, 600.0, 570.0, 540.0, 510.0, 480.0, 450.0, 420.0,
        450.0, 480.0, 510.0, 600.0,
    ]
    .into_iter()
    .collect();
    app.token_rate_known = true;
    app.host_metrics = Some(crate::host_info::HostMetrics {
        cpu_pct: 31.0,
        mem_pct: 46.0,
        load1: 2.1,
    });
    app.agent_aggregate = crate::host_info::AgentAggregate::from_sessions(&app.sessions);
}

#[cfg(test)]
mod tests {
    use super::populate_demo;
    use crate::app::App;
    use crate::config::PanelVisibility;
    use crate::model::TelemetryPrecision;
    use crate::theme::Theme;

    #[test]
    fn pi_demo_has_attached_sessions_with_authoritative_token_rate_and_runs() {
        let mut app = App::new(Theme::default(), PanelVisibility::default());
        populate_demo(&mut app);

        assert!(app.token_rate_known);
        assert!(app.sessions.len() >= 3);
        assert!(app.sessions.iter().all(|session| {
            session.telemetry.as_ref().is_some_and(|telemetry| {
                telemetry.attachment == crate::model::AttachmentState::Attached
                    && telemetry.usage.completeness == crate::model::TelemetryCompleteness::Complete
                    && telemetry.usage.precision == TelemetryPrecision::Exact
                    && telemetry.context_details.tokens
                        == Some(
                            telemetry.context_details.baseline_tokens.unwrap()
                                + telemetry.context_details.trailing_tokens.unwrap(),
                        )
                    && telemetry.context.precision
                        == if telemetry.context_details.trailing_tokens == Some(0) {
                            TelemetryPrecision::Inferred
                        } else {
                            TelemetryPrecision::Estimated
                        }
            })
        }));
        assert_eq!(
            app.sessions[0]
                .telemetry
                .as_ref()
                .unwrap()
                .context_details
                .provider
                .as_deref(),
            Some("anthropic")
        );
        assert_eq!(
            app.sessions[1]
                .telemetry
                .as_ref()
                .unwrap()
                .context_details
                .provider
                .as_deref(),
            Some("openai")
        );
        let fleet = &app.sessions[0].telemetry.as_ref().unwrap().fleet;
        assert_eq!(fleet.scanned_statuses, 1);
        assert_eq!(
            fleet.foreground_visibility,
            crate::model::FleetVisibility::Unavailable
        );
        assert_eq!(
            fleet.background_visibility,
            crate::model::FleetVisibility::Supported
        );
        let run = fleet.runs.first().unwrap();
        assert_eq!(run.lifecycle_version, Some(3));
        let child = run.children.first().unwrap();
        assert_ne!(child.run_id.as_deref(), Some(run.run_id.as_str()));
        assert_eq!(child.name, "reviewer");
        assert_eq!(app.token_rates.len(), 30);
        assert_eq!(app.token_rates.iter().sum::<f64>(), 18_000.0);
        assert!(app.host_metrics.is_some());
        assert!(app.agent_aggregate.active_count >= 2);
    }

    #[test]
    fn pi_demo_snapshot_uses_pi_privacy_suppression() {
        let mut app = App::new(Theme::default(), PanelVisibility::default());
        populate_demo(&mut app);
        let json = serde_json::to_string(&app.to_snapshot(2_000)).unwrap();

        assert!(!json.contains("monitor_mode"));
        assert!(json.contains("\"token_rate_value\":"));
        assert!(json.contains("\"command\":\"node\""));
        assert!(!json.contains("--checkout"));
        assert!(!json.contains("private"));
        assert!(!json.contains("initial_prompt"));
        assert!(!json.contains("security review"));
        assert!(!json.contains("payment boundary"));
    }
}
