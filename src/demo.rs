use crate::app::App;
use crate::model::{
    AgentSession, AttachmentConfidence, AttachmentState, ChatMessage, ChatRole, ChildProcess,
    ContextTelemetryDetails, FileAccess, FileOp, FleetChild, FleetExecution, FleetIdentitySource,
    FleetProcessTerminal, FleetProcessTerminalState, FleetRun, FleetRunMode, FleetRunState,
    FleetTelemetry, FleetUsage, FleetVisibility, OrphanPort, RateLimitInfo, SessionStatus,
    SessionTelemetry, SourceHealth, SubAgent, TelemetryCompleteness, TelemetryMetadata,
    TelemetryPrecision, ToolCall, UsageTelemetryDetails,
};
use std::time::{SystemTime, UNIX_EPOCH};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn now_secs() -> u64 {
    now_ms() / 1000
}

/// Populate deterministic, collector-free fixtures for the active monitor mode.
pub fn populate_demo(app: &mut App) {
    if app.is_pi_mode() {
        populate_pi_demo(app);
    } else {
        populate_legacy_demo(app);
    }
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
            agent_cli: "pi",
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
            subagents: vec![],
            mem_file_count: 0,
            mem_line_count: 0,
            children: vec![ChildProcess {
                pid: 7310,
                command: "node worker.js --checkout".into(),
                mem_kb: 82_000,
                port: Some(4173),
            }],
            initial_prompt: String::new(),
            first_assistant_text: String::new(),
            chat_messages: vec![],
            tool_calls: vec![],
            pending_since_ms: 0,
            thinking_since_ms: 0,
            file_accesses: vec![],
            config_root: String::new(),
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
            agent_cli: "pi",
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
            subagents: vec![],
            mem_file_count: 0,
            mem_line_count: 0,
            children: vec![ChildProcess {
                pid: 7414,
                command: "python metrics_server.py --debug".into(),
                mem_kb: 44_000,
                port: Some(9090),
            }],
            initial_prompt: String::new(),
            first_assistant_text: String::new(),
            chat_messages: vec![],
            tool_calls: vec![],
            pending_since_ms: 0,
            thinking_since_ms: 0,
            file_accesses: vec![],
            config_root: String::new(),
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
            agent_cli: "pi",
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
            subagents: vec![],
            mem_file_count: 0,
            mem_line_count: 0,
            children: vec![],
            initial_prompt: String::new(),
            first_assistant_text: String::new(),
            chat_messages: vec![],
            tool_calls: vec![],
            pending_since_ms: 0,
            thinking_since_ms: 0,
            file_accesses: vec![],
            config_root: String::new(),
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
    app.summaries.clear();
    app.rate_limits.clear();
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

fn populate_legacy_demo(app: &mut App) {
    let now = now_ms();

    // --- Sessions ---
    app.sessions = vec![
        AgentSession {
            agent_cli: "claude",
            pid: 7336,
            session_id: "a1b2c3d4-5678-9abc-def0-111111111111".into(),
            cwd: "/Users/demo/webshop".into(),
            project_name: "webshop".into(),
            started_at: now - 2 * 3600 * 1000, // 2h ago
            status: SessionStatus::Executing,
            model: "claude-opus-4-6".into(),
            effort: String::new(),
            context_percent: 72.0,
            total_input_tokens: 48_200,
            total_output_tokens: 12_800,
            total_cache_read: 1_420_000,
            total_cache_create: 185_000,
            turn_count: 34,
            current_tasks: vec!["Edit src/checkout/payment.rs".into()],
            mem_mb: 342,
            version: "2.1.87".into(),
            git_branch: "main".into(),
            git_added: 2,
            git_modified: 8,
            token_history: vec![
                18000, 22000, 45000, 38000, 52000, 41000, 35000, 28000, 61000, 55000, 48000, 39000,
                44000, 50000, 32000, 27000, 58000, 46000, 42000, 36000, 53000, 47000, 41000, 38000,
                62000, 55000, 49000, 43000, 51000, 44000, 38000, 33000, 56000, 48000,
            ],
            context_history: vec![
                20000, 35000, 52000, 68000, 85000, 102000, 118000, 135000, 148000, 162000, 175000,
                185000, 192000, // compaction event: 192k -> 65k (66% drop)
                65000, 78000, 92000, 108000, 125000, 138000, 145000, 155000, 168000, 178000,
                185000, 190000, // second compaction: 190k -> 58k
                58000, 72000, 88000, 105000, 120000, 135000, 142000, 148000,
            ],
            compaction_count: 2,
            context_window: 200_000,
            subagents: vec![
                SubAgent {
                    name: "Explore test coverage".into(),

                    status: "done".into(),
                    tokens: 12_400,
                },
                SubAgent {
                    name: "Run integration tests".into(),

                    status: "working".into(),
                    tokens: 8_200,
                },
            ],
            mem_file_count: 4,
            mem_line_count: 12,
            children: vec![
                ChildProcess {
                    pid: 7401,
                    command: "cargo build --release".into(),
                    mem_kb: 342_000,
                    port: None,
                },
                ChildProcess {
                    pid: 7455,
                    command: "cargo test".into(),
                    mem_kb: 28_000,
                    port: None,
                },
            ],

            first_assistant_text: String::new(),
            chat_messages: vec![
                ChatMessage {
                    role: ChatRole::User,
                    text: "Implement Stripe payment integration for checkout flow".into(),
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    text: "I'll inspect checkout and config paths first, then wire the smallest payment boundary.".into(),
                },
                ChatMessage {
                    role: ChatRole::User,
                    text: "Keep webhook handling minimal and make tests cover declined cards.".into(),
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    text: "Payment code is in place; current pass is tightening test failures around webhook signatures.".into(),
                },
            ],
            initial_prompt: "Implement Stripe payment integration for checkout flow".into(),
            tool_calls: vec![
                ToolCall {
                    name: "Read".into(),
                    arg: "src/checkout/mod.rs".into(),
                    duration_ms: 85,
                },
                ToolCall {
                    name: "Read".into(),
                    arg: "Cargo.toml".into(),
                    duration_ms: 42,
                },
                ToolCall {
                    name: "Bash".into(),
                    arg: "cargo add stripe-rust".into(),
                    duration_ms: 3200,
                },
                ToolCall {
                    name: "Edit".into(),
                    arg: "src/checkout/payment.rs".into(),
                    duration_ms: 120,
                },
                ToolCall {
                    name: "Edit".into(),
                    arg: "src/checkout/mod.rs".into(),
                    duration_ms: 95,
                },
                ToolCall {
                    name: "Write".into(),
                    arg: "src/checkout/stripe.rs".into(),
                    duration_ms: 180,
                },
                ToolCall {
                    name: "Bash".into(),
                    arg: "cargo test".into(),
                    duration_ms: 8400,
                },
                ToolCall {
                    name: "Edit".into(),
                    arg: "src/checkout/stripe.rs".into(),
                    duration_ms: 110,
                },
                ToolCall {
                    name: "Bash".into(),
                    arg: "cargo test checkout".into(),
                    duration_ms: 4200,
                },
                ToolCall {
                    name: "Read".into(),
                    arg: "src/config.rs".into(),
                    duration_ms: 55,
                },
                ToolCall {
                    name: "Edit".into(),
                    arg: "src/config.rs".into(),
                    duration_ms: 90,
                },
                ToolCall {
                    name: "Grep".into(),
                    arg: "STRIPE_SECRET".into(),
                    duration_ms: 320,
                },
                ToolCall {
                    name: "Agent".into(),
                    arg: "security review".into(),
                    duration_ms: 12400,
                },
                ToolCall {
                    name: "Bash".into(),
                    arg: "cargo clippy".into(),
                    duration_ms: 5100,
                },
                ToolCall {
                    name: "Edit".into(),
                    arg: "src/checkout/payment.rs".into(),
                    duration_ms: 145,
                },
                ToolCall {
                    name: "Bash".into(),
                    arg: "cargo test".into(),
                    duration_ms: 7800,
                },
                // Currently running: timeline bar grows in real time.
                ToolCall {
                    name: "WebSearch".into(),
                    arg: "stripe webhook best practice".into(),
                    duration_ms: 0,
                },
            ],
            pending_since_ms: now - 6_000, // 6s ago => bar animates
            thinking_since_ms: 0,
            config_root: "~/.claude".into(),
            telemetry: None,
            process_start_id: None,
            file_accesses: vec![
                FileAccess {
                    path: "src/checkout/payment.rs".into(),
                    operation: FileOp::Read,
                    turn_index: 2,
                },
                FileAccess {
                    path: "src/checkout/mod.rs".into(),
                    operation: FileOp::Read,
                    turn_index: 3,
                },
                FileAccess {
                    path: "src/checkout/payment.rs".into(),
                    operation: FileOp::Edit,
                    turn_index: 5,
                },
                FileAccess {
                    path: "src/config/stripe.rs".into(),
                    operation: FileOp::Write,
                    turn_index: 7,
                },
                FileAccess {
                    path: "src/checkout/payment.rs".into(),
                    operation: FileOp::Edit,
                    turn_index: 10,
                },
                FileAccess {
                    path: "tests/checkout_test.rs".into(),
                    operation: FileOp::Write,
                    turn_index: 12,
                },
                FileAccess {
                    path: "src/models/order.rs".into(),
                    operation: FileOp::Read,
                    turn_index: 15,
                },
                FileAccess {
                    path: "src/checkout/payment.rs".into(),
                    operation: FileOp::Edit,
                    turn_index: 18,
                },
                FileAccess {
                    path: "Cargo.toml".into(),
                    operation: FileOp::Read,
                    turn_index: 20,
                },
                FileAccess {
                    path: "tests/checkout_test.rs".into(),
                    operation: FileOp::Edit,
                    turn_index: 22,
                },
            ],
        },
        AgentSession {
            agent_cli: "claude",
            pid: 8840,
            session_id: "b2c3d4e5-6789-abcd-ef01-222222222222".into(),
            cwd: "/Users/demo/ml-pipeline".into(),
            project_name: "ml-pipeline".into(),
            started_at: now - 47 * 60 * 1000, // 47m ago
            status: SessionStatus::Waiting,
            model: "claude-sonnet-4-6".into(),
            effort: String::new(),
            context_percent: 91.0,
            total_input_tokens: 82_000,
            total_output_tokens: 38_000,
            total_cache_read: 2_100_000,
            total_cache_create: 280_000,
            turn_count: 71,
            current_tasks: vec!["waiting for user input".into()],
            mem_mb: 128,
            version: "2.1.87".into(),
            git_branch: "feat/batch-inference".into(),
            git_added: 1,
            git_modified: 4,
            token_history: vec![
                32000, 28000, 41000, 55000, 62000, 48000, 35000, 29000, 44000, 58000, 51000, 39000,
                46000, 53000, 42000, 37000, 60000, 52000, 45000, 38000, 56000, 49000, 43000, 36000,
                63000, 57000, 50000, 44000, 54000, 47000,
            ],
            context_history: vec![
                15000, 28000, 45000, 62000, 80000, 95000, 112000, 128000, 142000, 158000, 172000,
                182000, 190000, 195000, // compaction: 195k -> 70k
                70000, 85000, 98000, 115000, 130000, 145000, 158000, 170000, 182000,
            ],
            compaction_count: 1,
            context_window: 200_000,
            subagents: vec![],
            mem_file_count: 2,
            mem_line_count: 8,
            children: vec![],

            first_assistant_text: String::new(),
            chat_messages: vec![
                ChatMessage {
                    role: ChatRole::User,
                    text: "Add batch inference endpoint with GPU scheduling".into(),
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    text: "Endpoint is implemented; waiting on your choice for queue priority behavior.".into(),
                },
            ],
            initial_prompt: "Add batch inference endpoint with GPU scheduling".into(),
            tool_calls: vec![],
            pending_since_ms: 0,
            thinking_since_ms: 0,
            config_root: "~/.claude-work".into(),
            telemetry: None,
            process_start_id: None,
            file_accesses: vec![],
        },
        AgentSession {
            agent_cli: "claude",
            pid: 9102,
            session_id: "c3d4e5f6-789a-bcde-f012-333333333333".into(),
            cwd: "/Users/demo/api-server".into(),
            project_name: "api-server".into(),
            started_at: now - 15 * 60 * 1000, // 15m ago
            status: SessionStatus::Executing,
            model: "claude-haiku-4-5".into(),
            effort: String::new(),
            context_percent: 42.0,
            total_input_tokens: 5_200,
            total_output_tokens: 2_800,
            total_cache_read: 320_000,
            total_cache_create: 45_000,
            turn_count: 12,
            current_tasks: vec!["Bash npm run dev".into()],
            mem_mb: 86,
            version: "2.1.87".into(),
            git_branch: "main".into(),
            git_added: 0,
            git_modified: 2,
            token_history: vec![
                8000, 12000, 15000, 22000, 18000, 25000, 20000, 16000, 28000, 24000, 19000, 14000,
            ],
            context_history: vec![],
            compaction_count: 0,
            context_window: 200_000,
            subagents: vec![],
            mem_file_count: 1,
            mem_line_count: 3,
            children: vec![
                ChildProcess {
                    pid: 9150,
                    command: "node server.js".into(),
                    mem_kb: 98_000,
                    port: Some(3000),
                },
                ChildProcess {
                    pid: 9178,
                    command: "node worker.js".into(),
                    mem_kb: 52_000,
                    port: Some(3001),
                },
                ChildProcess {
                    pid: 9203,
                    command: "postgres -D /usr/local/var/postgres".into(),
                    mem_kb: 24_000,
                    port: Some(5432),
                },
            ],

            first_assistant_text: String::new(),
            chat_messages: vec![
                ChatMessage {
                    role: ChatRole::User,
                    text: "Fix CORS headers and add rate limiting middleware".into(),
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    text: "I'll patch middleware order, then run the API smoke tests.".into(),
                },
            ],
            initial_prompt: "Fix CORS headers and add rate limiting middleware".into(),
            tool_calls: vec![
                ToolCall {
                    name: "Read".into(),
                    arg: "src/server.ts".into(),
                    duration_ms: 60,
                },
                ToolCall {
                    name: "Edit".into(),
                    arg: "src/middleware/cors.ts".into(),
                    duration_ms: 150,
                },
                ToolCall {
                    name: "Write".into(),
                    arg: "src/middleware/rate-limit.ts".into(),
                    duration_ms: 200,
                },
                ToolCall {
                    name: "Bash".into(),
                    arg: "npm test".into(),
                    duration_ms: 2800,
                },
                ToolCall {
                    name: "Edit".into(),
                    arg: "src/server.ts".into(),
                    duration_ms: 110,
                },
                ToolCall {
                    name: "Bash".into(),
                    arg: "npm run dev".into(),
                    duration_ms: 1500,
                },
            ],
            pending_since_ms: 0,
            // Model is generating its next reply - virtual "Thinking" row
            // animates. 1s offset keeps the bar visibly growing against the
            // session's 2.8s max tool duration before it caps at 100%.
            thinking_since_ms: now - 1_000,
            config_root: "~/.claude".into(),
            telemetry: None,
            process_start_id: None,
            file_accesses: vec![],
        },
        AgentSession {
            agent_cli: "codex",
            pid: 8901,
            session_id: "d4e5f6a7-89ab-cdef-0123-444444444444".into(),
            cwd: "/Users/demo/data-viz".into(),
            project_name: "data-viz".into(),
            started_at: now - 5 * 60 * 1000, // 5m ago
            status: SessionStatus::Thinking,
            model: "gpt-5.4".into(),
            effort: "medium".into(),
            context_percent: 18.0,
            total_input_tokens: 3_100,
            total_output_tokens: 1_400,
            total_cache_read: 85_000,
            total_cache_create: 12_000,
            turn_count: 6,
            current_tasks: vec!["Write src/charts/heatmap.py".into()],
            mem_mb: 64,
            version: "0.116.0".into(),
            git_branch: "feat/heatmap".into(),
            git_added: 3,
            git_modified: 1,
            token_history: vec![5000, 8000, 12000, 18000, 15000, 22000],
            context_history: vec![],
            compaction_count: 0,
            context_window: 200_000,
            subagents: vec![],
            mem_file_count: 0,
            mem_line_count: 0,
            children: vec![ChildProcess {
                pid: 8950,
                command: "python -m http.server 8080".into(),
                mem_kb: 32_000,
                port: Some(8080),
            }],

            first_assistant_text: String::new(),
            chat_messages: vec![
                ChatMessage {
                    role: ChatRole::User,
                    text: "Create interactive heatmap component with D3.js".into(),
                },
                ChatMessage {
                    role: ChatRole::Assistant,
                    text: "Building the component now; next check is responsive canvas sizing.".into(),
                },
            ],
            initial_prompt: "Create interactive heatmap component with D3.js".into(),
            tool_calls: vec![],
            pending_since_ms: 0,
            thinking_since_ms: 0,
            config_root: "~/.codex".into(),
            telemetry: None,
            process_start_id: None,
            file_accesses: vec![],
        },
        AgentSession {
            agent_cli: "opencode",
            pid: 9500,
            session_id: "ses_e5f6a7b8-9abc-def0-1234-555555555555".into(),
            cwd: "/Users/demo/infra".into(),
            project_name: "infra".into(),
            started_at: now - 8 * 60 * 1000, // 8m ago
            status: SessionStatus::Thinking,
            model: "ollama/qwen3:14b".into(),
            effort: String::new(),
            context_percent: 35.0,
            total_input_tokens: 14_200,
            total_output_tokens: 3_800,
            total_cache_read: 0,
            total_cache_create: 0,
            turn_count: 9,
            current_tasks: vec!["thinking...".into()],
            mem_mb: 156,
            version: "1.4.0".into(),
            git_branch: "main".into(),
            git_added: 1,
            git_modified: 3,
            token_history: vec![
                3000, 5000, 8000, 6000, 9000, 7000, 11000, 8000, 10000,
            ],
            context_history: vec![],
            compaction_count: 0,
            context_window: 200_000,
            subagents: vec![],
            mem_file_count: 0,
            mem_line_count: 0,
            children: vec![],

            first_assistant_text: String::new(),
            chat_messages: vec![],
            initial_prompt: "Refactor Terraform modules for multi-region".into(),
            tool_calls: vec![],
            pending_since_ms: 0,
            thinking_since_ms: 0,
            config_root: "~/.local/share/opencode".into(),
            telemetry: None,
            process_start_id: None,
            file_accesses: vec![],
        },
    ];

    // --- Summaries (pre-populated, no LLM calls) ---
    app.summaries.insert(
        "a1b2c3d4-5678-9abc-def0-111111111111".into(),
        "Stripe payment integration".into(),
    );
    app.summaries.insert(
        "b2c3d4e5-6789-abcd-ef01-222222222222".into(),
        "Batch inference endpoint".into(),
    );
    app.summaries.insert(
        "c3d4e5f6-789a-bcde-f012-333333333333".into(),
        "CORS fix + rate limiting".into(),
    );
    app.summaries.insert(
        "d4e5f6a7-89ab-cdef-0123-444444444444".into(),
        "D3 heatmap component".into(),
    );
    app.summaries.insert(
        "ses_e5f6a7b8-9abc-def0-1234-555555555555".into(),
        "Terraform multi-region refactor".into(),
    );

    // --- Rate limits ---
    app.rate_limits = vec![
        RateLimitInfo {
            source: "claude".into(),
            five_hour_pct: Some(35.0),
            five_hour_resets_at: Some(now_secs() + 3 * 3600),
            five_hour_window_minutes: Some(300),
            seven_day_pct: Some(12.0),
            seven_day_resets_at: Some(now_secs() + 5 * 24 * 3600),
            seven_day_window_minutes: Some(10_080),
            updated_at: Some(now_secs() - 10),
        },
        RateLimitInfo {
            source: "codex".into(),
            five_hour_pct: Some(9.0),
            five_hour_resets_at: Some(now_secs() + 4 * 3600),
            five_hour_window_minutes: Some(300),
            seven_day_pct: Some(14.0),
            seven_day_resets_at: Some(now_secs() + 6 * 24 * 3600),
            seven_day_window_minutes: Some(10_080),
            updated_at: Some(now_secs() - 5),
        },
    ];

    // --- Token rates (synthetic sparkline) ---
    let rates = [
        0.0, 0.0, 120.0, 340.0, 580.0, 420.0, 0.0, 0.0, 890.0, 1200.0, 950.0, 680.0, 0.0, 0.0,
        450.0, 780.0, 1100.0, 1350.0, 920.0, 610.0, 0.0, 340.0, 670.0, 890.0, 1050.0, 780.0, 520.0,
        0.0, 0.0, 1400.0, 1180.0, 850.0, 620.0, 410.0, 0.0, 560.0, 820.0, 1060.0, 1280.0, 940.0,
        700.0, 480.0, 0.0, 0.0, 380.0, 720.0, 1150.0, 1320.0, 980.0, 650.0,
    ];
    app.token_rates.clear();
    for &r in &rates {
        app.token_rates.push_back(r);
    }

    // --- Orphan ports ---
    app.orphan_ports = vec![OrphanPort {
        port: 4000,
        pid: 6543,
        command: "node dist/server.js".into(),
        project_name: "old-project".into(),
    }];

    // --- Host metrics + agent aggregate (demo values) ---
    app.host_metrics = Some(crate::host_info::HostMetrics {
        cpu_pct: 23.0,
        mem_pct: 41.0,
        load1: 1.8,
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
        let mut app = App::new_pi(Theme::default(), &[], PanelVisibility::default());
        populate_demo(&mut app);

        assert!(app.is_pi_mode());
        assert!(app.token_rate_known);
        assert!(app.sessions.len() >= 3);
        assert!(app.sessions.iter().all(|session| session.agent_cli == "pi"));
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
        let mut app = App::new_pi(Theme::default(), &[], PanelVisibility::default());
        populate_demo(&mut app);
        let json = serde_json::to_string(&app.to_snapshot(2_000)).unwrap();

        assert!(json.contains("\"monitor_mode\":\"pi\""));
        assert!(json.contains("\"token_rate_value\":"));
        assert!(json.contains("\"command\":\"node\""));
        assert!(!json.contains("--checkout"));
        assert!(!json.contains("private"));
        assert!(!json.contains("initial_prompt"));
        assert!(!json.contains("security review"));
        assert!(!json.contains("payment boundary"));
    }

    #[test]
    fn legacy_demo_remains_legacy_fixture_set() {
        let mut app = App::new_with_config(Theme::default(), &[], PanelVisibility::default());
        populate_demo(&mut app);

        assert!(!app.is_pi_mode());
        assert_eq!(app.sessions.len(), 5);
        assert!(app.sessions.iter().all(|session| session.agent_cli != "pi"));
        assert!(app.token_rate_known);
    }
}
