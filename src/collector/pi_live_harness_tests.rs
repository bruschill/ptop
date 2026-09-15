use super::*;

const EPOCH: &str = "0123456789abcdef0123456789abcdef";
const OTHER_EPOCH: &str = "abcdef0123456789abcdef0123456789";
const SESSION: &str = "attached-session-id";

#[derive(Deserialize)]
struct Fixture {
    wire_keys: Vec<String>,
    phases: Vec<String>,
    limits: Limits,
    valid_frames: Vec<FixtureFrame>,
    invalid_frames: Vec<FixtureInvalid>,
    reducer_cases: Vec<ReducerCase>,
}
#[derive(Deserialize)]
struct Limits {
    max_frame_bytes: usize,
    max_buffer_bytes: usize,
    max_session_id_bytes: usize,
    epoch_hex_bytes: usize,
    max_tool_ids: usize,
    max_tool_call_id_bytes: usize,
    max_nesting: u8,
    heartbeat_ms: u64,
}
#[derive(Deserialize)]
struct FixtureFrame {
    name: String,
    json: String,
}
#[derive(Deserialize)]
struct FixtureInvalid {
    name: String,
    error: String,
    json: String,
}
#[derive(Deserialize)]
struct ReducerCase {
    name: String,
    events: Vec<FixtureEvent>,
    phases: Vec<Option<String>>,
    closed: Option<Vec<bool>>,
}
#[derive(Deserialize)]
struct FixtureEvent {
    hook: String,
    tool_call_id: Option<String>,
    tool_call_id_prefix: Option<String>,
    repeat: Option<usize>,
    is_idle: Option<bool>,
}

fn fixture() -> Fixture {
    serde_json::from_str(include_str!("fixtures/pi_live_harness_v1.json")).unwrap()
}
fn bytes(value: &str) -> Vec<u8> {
    let mut framed = (value.len() as u32).to_be_bytes().to_vec();
    framed.extend_from_slice(value.as_bytes());
    framed
}
fn frame(epoch: &str, sequence: u64, phase: &str, pending: &str) -> String {
    format!("{{\"magic\":\"ptop-live\",\"version\":1,\"epoch\":\"{epoch}\",\"sequence\":{sequence},\"session_id\":\"{SESSION}\",\"phase\":{phase},\"pending_messages\":{pending}}}")
}
fn parsed(json: &str) -> LiveFrame {
    let mut decoder = FrameDecoder::default();
    decoder.push(&bytes(json)).unwrap();
    decoder.next().unwrap().unwrap()
}
fn decode_error(json: &str) -> ProtocolError {
    let mut decoder = FrameDecoder::default();
    decoder.push(&bytes(json)).unwrap();
    decoder.next().unwrap_err()
}
fn assert_phase(reducer: &mut Reducer, event: ReducerEvent<'_>, phase: Option<Phase>) {
    assert_eq!(reducer.reduce(event).map(|state| state.phase), Some(phase));
}

#[test]
fn fixture_guard_drives_decoder_and_reducer_contract() {
    let fixture = fixture();
    assert!(
        !fixture.wire_keys.is_empty()
            && !fixture.phases.is_empty()
            && !fixture.reducer_cases.is_empty()
    );
    assert_eq!(fixture.limits.max_frame_bytes, MAX_FRAME_BYTES);
    assert_eq!(fixture.limits.max_buffer_bytes, MAX_BUFFER_BYTES);
    assert_eq!(fixture.limits.max_session_id_bytes, MAX_SESSION_ID_BYTES);
    assert_eq!(fixture.limits.epoch_hex_bytes, EPOCH_HEX_BYTES);
    assert_eq!(fixture.limits.max_tool_ids, MAX_TOOL_IDS);
    assert_eq!(
        fixture.limits.max_tool_call_id_bytes,
        MAX_TOOL_CALL_ID_BYTES
    );
    assert_eq!(fixture.limits.max_nesting, MAX_NESTING);
    assert_eq!(fixture.limits.heartbeat_ms, HEARTBEAT_MS);
    for valid in &fixture.valid_frames {
        assert!(parsed(&valid.json).sequence <= 1, "{}", valid.name);
    }
    for invalid in &fixture.invalid_frames {
        assert_eq!(
            decode_error(&invalid.json),
            fixture_error(&invalid.error),
            "{}",
            invalid.name
        );
    }
    assert_eq!(
        fixture.phases,
        Phase::ALL
            .map(|phase| phase.wire_name().to_owned())
            .to_vec()
    );
    for key in &fixture.wire_keys {
        assert_eq!(
            decode_error(&remove_key(&fixture.valid_frames[0].json, key)),
            ProtocolError::InvalidSchema,
            "missing {key}"
        );
    }
    assert_eq!(fixture.wire_keys.len(), 7);
}

fn remove_key(json: &str, key: &str) -> String {
    let mut object =
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(json).unwrap();
    assert!(object.remove(key).is_some());
    serde_json::to_string(&object).unwrap()
}

#[test]
fn decoder_requires_each_key_and_rejects_each_duplicate() {
    let valid = frame(EPOCH, 0, "null", "null");
    let object = serde_json::from_str::<serde_json::Value>(&valid).unwrap();
    for key in fixture().wire_keys {
        assert_eq!(
            decode_error(&remove_key(&valid, &key)),
            ProtocolError::InvalidSchema,
            "missing {key}"
        );
        let duplicate = format!(
            "{},\"{}\":{}}}",
            &valid[..valid.len() - 1],
            key,
            object[&key].clone()
        );
        assert_eq!(
            decode_error(&duplicate),
            ProtocolError::InvalidSchema,
            "duplicate {key}"
        );
    }
}

#[test]
fn decoder_has_exact_identity_utf8_and_schema_errors() {
    assert_eq!(
        decode_error("{\"magic\":\"ptop-live\"}"),
        ProtocolError::InvalidSchema
    );
    assert_eq!(
        decode_error(&frame(EPOCH, 0, "\"future\"", "null")),
        ProtocolError::InvalidSchema
    );
    assert_eq!(
        decode_error(&frame(EPOCH, 0, "null", "0")),
        ProtocolError::InvalidSchema
    );
    assert_eq!(
        decode_error(&frame(EPOCH, 0, "null", "null").replace("\"version\":1", "\"version\":2")),
        ProtocolError::InvalidSchema
    );
    assert_eq!(
        decode_error(&frame(EPOCH, 0, "null", "null").replace(SESSION, "")),
        ProtocolError::InvalidIdentity
    );
    assert_eq!(
        decode_error(&frame(EPOCH, 0, "null", "null").replace(SESSION, "x\\u0001")),
        ProtocolError::InvalidIdentity
    );
    assert_eq!(
        decode_error(&frame(EPOCH, 0, "null", "null").replace(SESSION, "x\\u202e")),
        ProtocolError::InvalidIdentity
    );
    assert_eq!(
        decode_error(
            &frame(EPOCH, 0, "null", "null").replace(EPOCH, "ABCDEF0123456789abcdef0123456789")
        ),
        ProtocolError::InvalidIdentity
    );
    assert_eq!(
        decode_error(
            &frame(EPOCH, 0, "null", "null").replace(EPOCH, "0123456789abcdef0123456789abcde")
        ),
        ProtocolError::InvalidIdentity
    );
    assert_eq!(
        decode_error(&frame(EPOCH, 0, "null", "null").replace("ptop-live", "wrong")),
        ProtocolError::InvalidSchema
    );
    let exact = "x".repeat(MAX_SESSION_ID_BYTES);
    assert_eq!(
        parsed(&frame(EPOCH, 0, "null", "null").replace(SESSION, &exact)).session_id,
        exact
    );
    let over = "x".repeat(MAX_SESSION_ID_BYTES + 1);
    assert_eq!(
        decode_error(&frame(EPOCH, 0, "null", "null").replace(SESSION, &over)),
        ProtocolError::InvalidIdentity
    );
    let mut decoder = FrameDecoder::default();
    decoder.push(&[0, 0, 0, 1, 0xff]).unwrap();
    assert_eq!(decoder.next(), Err(ProtocolError::InvalidUtf8));
}

#[test]
fn decoder_frames_partial_extra_and_limits() {
    let first = bytes(&frame(EPOCH, 0, "null", "null"));
    let next = bytes(&frame(EPOCH, 1, "\"idle\"", "false"));
    let mut decoder = FrameDecoder::default();
    decoder.push(&first[..5]).unwrap();
    assert_eq!(decoder.next().unwrap(), None);
    decoder.push(&first[5..]).unwrap();
    decoder.push(&next).unwrap();
    assert_eq!(decoder.next().unwrap().unwrap().sequence, 0);
    assert_eq!(decoder.next().unwrap().unwrap().sequence, 1);
    let mut decoder = FrameDecoder::default();
    decoder.push(&[0, 0, 0, 0]).unwrap();
    assert_eq!(decoder.next(), Err(ProtocolError::InvalidLength));
    let mut decoder = FrameDecoder::default();
    assert_eq!(
        decoder.push(&vec![0; MAX_BUFFER_BYTES + 1]),
        Err(ProtocolError::BufferLimit)
    );
}

#[test]
fn decoder_accepts_every_later_phase_and_pending_combination() {
    let mut connection = ProtocolConnection::new(SESSION, None);
    connection
        .accept(parsed(&frame(EPOCH, 0, "null", "null")))
        .unwrap();
    let phases = [
        "null",
        "\"idle\"",
        "\"generating\"",
        "\"tool_running\"",
        "\"compacting\"",
        "\"waiting_for_user\"",
    ];
    let mut sequence = 1;
    for phase in phases {
        for pending in ["null", "true", "false"] {
            connection
                .accept(parsed(&frame(EPOCH, sequence, phase, pending)))
                .unwrap();
            sequence += 1;
        }
    }
}

#[test]
fn connection_has_exact_initial_sequence_epoch_and_reconnect_rules() {
    for (phase, pending) in [("\"idle\"", "null"), ("null", "true")] {
        let mut connection = ProtocolConnection::new(SESSION, None);
        assert_eq!(
            connection.accept(parsed(&frame(EPOCH, 0, phase, pending))),
            Err(ProtocolError::InvalidInitialFrame)
        );
    }
    let mut connection = ProtocolConnection::new(SESSION, None);
    assert_eq!(
        connection.accept(parsed(
            &frame(EPOCH, 0, "null", "null").replace(SESSION, "wrong")
        )),
        Err(ProtocolError::InvalidIdentity)
    );
    let mut connection = ProtocolConnection::new(SESSION, None);
    connection
        .accept(parsed(&frame(EPOCH, 0, "null", "null")))
        .unwrap();
    assert_eq!(
        connection.accept(parsed(&frame(OTHER_EPOCH, 1, "null", "null"))),
        Err(ProtocolError::InvalidSequence)
    );
    let mut connection = ProtocolConnection::new(SESSION, None);
    connection
        .accept(parsed(&frame(EPOCH, 0, "null", "null")))
        .unwrap();
    assert_eq!(
        connection.accept(parsed(&frame(EPOCH, 2, "null", "null"))),
        Err(ProtocolError::InvalidSequence)
    );
    let mut connection = ProtocolConnection::new(SESSION, None);
    connection
        .accept(parsed(&frame(EPOCH, 0, "null", "null")))
        .unwrap();
    connection
        .accept(parsed(&frame(EPOCH, 1, "null", "null")))
        .unwrap();
    assert_eq!(
        connection.accept(parsed(&frame(EPOCH, 1, "null", "null"))),
        Err(ProtocolError::InvalidSequence)
    );
    let mut connection = ProtocolConnection::new(SESSION, None);
    connection
        .accept(parsed(&frame(EPOCH, 0, "null", "null")))
        .unwrap();
    connection.next_sequence = u64::MAX;
    assert_eq!(
        connection.accept(parsed(&frame(EPOCH, u64::MAX, "null", "null"))),
        Err(ProtocolError::InvalidSequence)
    );
    let mut first = ProtocolConnection::new(SESSION, None);
    first
        .accept(parsed(&frame(EPOCH, 0, "null", "null")))
        .unwrap();
    let old_epoch = first.epoch().unwrap().to_owned();
    let mut reused = ProtocolConnection::new(SESSION, Some(&old_epoch));
    assert_eq!(
        reused.accept(parsed(&frame(EPOCH, 0, "null", "null"))),
        Err(ProtocolError::ReusedEpoch)
    );
    let mut reconnect = ProtocolConnection::new(SESSION, Some(&old_epoch));
    reconnect
        .accept(parsed(&frame(OTHER_EPOCH, 0, "null", "null")))
        .unwrap();
}

#[test]
fn heartbeat_anchors_initial_cadence_coalescing_and_backpressure() {
    let latest = ReducedState {
        phase: Some(Phase::ToolRunning),
        pending_messages: Some(true),
    };
    let mut schedule = HeartbeatSchedule::new();
    schedule.update(latest);
    assert_eq!(
        schedule.poll(0),
        HeartbeatAction::Start(ReducedState::unknown())
    );
    assert_eq!(schedule.poll(0), HeartbeatAction::Wait);
    assert_eq!(schedule.complete(0), HeartbeatAction::Wait);
    assert_eq!(schedule.poll(999), HeartbeatAction::Wait);
    assert_eq!(schedule.poll(1_000), HeartbeatAction::Start(latest));
    assert_eq!(schedule.poll(1_500), HeartbeatAction::Wait);
    assert_eq!(schedule.complete(1_500), HeartbeatAction::Wait);
    assert_eq!(schedule.poll(1_999), HeartbeatAction::Wait);
    assert_eq!(schedule.poll(2_000), HeartbeatAction::Start(latest));
    assert_eq!(schedule.poll(2_999), HeartbeatAction::Wait);
    assert_eq!(schedule.poll(3_000), HeartbeatAction::Close);

    let mut late = HeartbeatSchedule::new();
    assert!(matches!(late.poll(0), HeartbeatAction::Start(_)));
    late.complete(0);
    late.update(latest);
    assert_eq!(late.poll(2_500), HeartbeatAction::Start(latest));
    assert_eq!(late.poll(2_500), HeartbeatAction::Wait);
    assert_eq!(late.complete(2_600), HeartbeatAction::Wait);
    assert_eq!(late.poll(3_499), HeartbeatAction::Wait);
    assert_eq!(late.poll(3_500), HeartbeatAction::Start(latest));
}

fn fixture_error(name: &str) -> ProtocolError {
    match name {
        "invalid_schema" => ProtocolError::InvalidSchema,
        "invalid_identity" => ProtocolError::InvalidIdentity,
        _ => panic!("unknown fixture error"),
    }
}

fn fixture_event<'a>(
    event: &'a FixtureEvent,
    generated_tool_call_id: Option<&'a str>,
) -> ReducerEvent<'a> {
    let tool_call_id = generated_tool_call_id
        .or(event.tool_call_id.as_deref())
        .unwrap_or("fixture-tool");
    match event.hook.as_str() {
        "session_start" => ReducerEvent::SessionStart,
        "session_shutdown" => ReducerEvent::SessionShutdown,
        "agent_start" => ReducerEvent::AgentStart,
        "agent_end" => ReducerEvent::AgentEnd,
        "agent_settled" => ReducerEvent::AgentSettled {
            is_idle: event.is_idle.unwrap(),
        },
        "tool_execution_start" => ReducerEvent::ToolStart { tool_call_id },
        "tool_execution_end" => ReducerEvent::ToolEnd { tool_call_id },
        "session_before_compact" => ReducerEvent::CompactStart,
        "session_compact" | "session_compact_failed" => ReducerEvent::CompactEnd,
        "ui_prompt_start" => ReducerEvent::PromptStart,
        "ui_prompt_end" => ReducerEvent::PromptEnd,
        _ => panic!("unknown fixture hook"),
    }
}
fn fixture_phase(value: Option<&str>) -> Option<Phase> {
    match value {
        None => None,
        Some("idle") => Some(Phase::Idle),
        Some("generating") => Some(Phase::Generating),
        Some("tool_running") => Some(Phase::ToolRunning),
        Some("compacting") => Some(Phase::Compacting),
        Some("waiting_for_user") => Some(Phase::WaitingForUser),
        Some(_) => panic!("unknown fixture phase"),
    }
}

#[test]
fn fixture_reducer_cases_drive_real_reducer() {
    for case in fixture().reducer_cases {
        assert_eq!(case.events.len(), case.phases.len(), "{}", case.name);
        let closed = case
            .closed
            .unwrap_or_else(|| vec![false; case.events.len()]);
        assert_eq!(closed.len(), case.events.len(), "{}", case.name);
        let mut reducer = Reducer::default();
        for ((event, phase), closed) in case.events.iter().zip(case.phases.iter()).zip(closed) {
            let repeat = event.repeat.unwrap_or(1);
            let mut result = None;
            for index in 0..repeat {
                let generated = event
                    .tool_call_id_prefix
                    .as_ref()
                    .map(|prefix| format!("{prefix}{index}"));
                result = reducer.reduce(fixture_event(event, generated.as_deref()));
            }
            if closed {
                assert_eq!(result, None, "{}", case.name);
            } else {
                assert_eq!(
                    result.map(|state| state.phase),
                    Some(fixture_phase(phase.as_deref())),
                    "{}",
                    case.name
                );
            }
        }
    }
}

#[test]
fn reducer_rejects_exact_contradictions_and_recovers_only_at_boundaries() {
    let mut reducer = Reducer::default();
    assert_phase(
        &mut reducer,
        ReducerEvent::ToolStart { tool_call_id: "a" },
        Some(Phase::ToolRunning),
    );
    assert_phase(
        &mut reducer,
        ReducerEvent::ToolStart { tool_call_id: "a" },
        None,
    );
    assert_phase(
        &mut reducer,
        ReducerEvent::ToolEnd { tool_call_id: "a" },
        None,
    );
    assert_phase(&mut reducer, ReducerEvent::CompactEnd, None);
    assert_phase(&mut reducer, ReducerEvent::PromptEnd, None);
    assert_phase(
        &mut reducer,
        ReducerEvent::AgentSettled { is_idle: false },
        None,
    );
    assert_phase(
        &mut reducer,
        ReducerEvent::AgentSettled { is_idle: true },
        Some(Phase::Idle),
    );
    assert_phase(
        &mut reducer,
        ReducerEvent::AgentStart,
        Some(Phase::Generating),
    );
    assert_phase(&mut reducer, ReducerEvent::AgentStart, None);
    assert_phase(
        &mut reducer,
        ReducerEvent::PromptStart,
        Some(Phase::WaitingForUser),
    );
}

#[test]
fn reducer_tracks_nested_precedence_and_tool_identifier_bounds_privately() {
    let mut reducer = Reducer::default();
    let exact = "x".repeat(MAX_TOOL_CALL_ID_BYTES);
    assert_phase(
        &mut reducer,
        ReducerEvent::ToolStart {
            tool_call_id: &exact,
        },
        Some(Phase::ToolRunning),
    );
    assert_phase(
        &mut reducer,
        ReducerEvent::ToolEnd {
            tool_call_id: &exact,
        },
        None,
    );
    let over = "x".repeat(MAX_TOOL_CALL_ID_BYTES + 1);
    assert_phase(
        &mut reducer,
        ReducerEvent::ToolStart {
            tool_call_id: &over,
        },
        None,
    );
    assert_phase(
        &mut reducer,
        ReducerEvent::CompactStart,
        Some(Phase::Compacting),
    );
    assert_phase(
        &mut reducer,
        ReducerEvent::CompactStart,
        Some(Phase::Compacting),
    );
    assert_phase(
        &mut reducer,
        ReducerEvent::PromptStart,
        Some(Phase::WaitingForUser),
    );
    assert_phase(
        &mut reducer,
        ReducerEvent::PromptStart,
        Some(Phase::WaitingForUser),
    );
    assert_phase(
        &mut reducer,
        ReducerEvent::PromptEnd,
        Some(Phase::WaitingForUser),
    );
    assert_phase(
        &mut reducer,
        ReducerEvent::PromptEnd,
        Some(Phase::Compacting),
    );
    assert_phase(
        &mut reducer,
        ReducerEvent::CompactEnd,
        Some(Phase::Compacting),
    );
    assert_phase(&mut reducer, ReducerEvent::CompactEnd, None);
    let sentinel = "PROMPT_TOOL_ARGUMENT_RESULT_SESSION_EPOCH_SENTINEL";
    let parsed_frame = parsed(&frame(EPOCH, 0, "null", "null").replace(SESSION, sentinel));
    let mut connection = ProtocolConnection::new(sentinel, None);
    connection.accept(parsed_frame.clone()).unwrap();
    let mut privacy = Reducer::default();
    privacy.reduce(ReducerEvent::ToolStart {
        tool_call_id: sentinel,
    });
    let epoch_sentinel = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let epoch_frame = parsed(&frame(epoch_sentinel, 0, "null", "null"));
    for value in [
        format!("{parsed_frame:?}"),
        format!("{connection:?}"),
        format!("{privacy:?}"),
        format!("{epoch_frame:?}"),
        format!(
            "{:?}",
            ReducerEvent::ToolStart {
                tool_call_id: sentinel
            }
        ),
        format!(
            "{:?}",
            decode_error(
                &frame(EPOCH, 0, "null", "null")
                    .replace(SESSION, sentinel)
                    .replace("\"version\":1", "\"version\":2")
            )
        ),
    ] {
        assert!(!value.contains(sentinel));
        assert!(!value.contains(epoch_sentinel));
    }
}

#[test]
fn reducer_rejects_tool_count_and_nesting_overflow() {
    let mut reducer = Reducer::default();
    for index in 0..MAX_TOOL_IDS {
        assert_phase(
            &mut reducer,
            ReducerEvent::ToolStart {
                tool_call_id: &format!("tool-{index}"),
            },
            Some(Phase::ToolRunning),
        );
    }
    assert_phase(
        &mut reducer,
        ReducerEvent::ToolStart {
            tool_call_id: "tool-overflow",
        },
        None,
    );
    for _ in 0..MAX_NESTING {
        assert_phase(
            &mut reducer,
            ReducerEvent::CompactStart,
            Some(Phase::Compacting),
        );
    }
    assert_phase(&mut reducer, ReducerEvent::CompactStart, None);
    for _ in 0..MAX_NESTING {
        assert_phase(
            &mut reducer,
            ReducerEvent::PromptStart,
            Some(Phase::WaitingForUser),
        );
    }
    assert_phase(&mut reducer, ReducerEvent::PromptStart, None);
}
