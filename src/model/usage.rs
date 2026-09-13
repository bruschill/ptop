use super::{AgentSession, TelemetryCompleteness, TelemetryPrecision};

/// Parent-session token usage across the currently live Pi sessions.
///
/// Fleet run usage is deliberately not an input: it is a separate accounting
/// stream and may overlap parent transcript usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiveUsageAggregate {
    pub(crate) input_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) cache_read_tokens: Option<u64>,
    pub(crate) cache_write_tokens: Option<u64>,
    pub(crate) total_tokens: Option<u64>,
    pub(crate) turn_count: Option<u64>,
    pub(crate) complete_sessions: usize,
    pub(crate) partial_sessions: usize,
    pub(crate) unavailable_sessions: usize,
    pub(crate) arithmetic_valid: bool,
}

impl LiveUsageAggregate {
    pub(crate) fn live_sessions(self) -> usize {
        self.complete_sessions + self.partial_sessions + self.unavailable_sessions
    }

    pub(crate) fn is_lower_bound(self) -> bool {
        self.partial_sessions > 0 || self.unavailable_sessions > 0
    }

    pub(crate) fn average_tokens_per_turn(self) -> Option<u64> {
        if !self.arithmetic_valid || self.is_lower_bound() {
            return None;
        }
        match (self.total_tokens, self.turn_count) {
            (Some(total), Some(turns)) => total.checked_div(turns).or(Some(0)),
            _ => None,
        }
    }
}

/// Aggregates parent-session usage only. Unknown sessions do not become zero;
/// known partial sessions contribute a lower-bound subtotal.
pub(crate) fn aggregate_live_usage(sessions: &[AgentSession]) -> LiveUsageAggregate {
    let mut aggregate = LiveUsageAggregate {
        input_tokens: Some(0),
        output_tokens: Some(0),
        cache_read_tokens: Some(0),
        cache_write_tokens: Some(0),
        total_tokens: Some(0),
        turn_count: Some(0),
        complete_sessions: 0,
        partial_sessions: 0,
        unavailable_sessions: 0,
        arithmetic_valid: true,
    };

    for session in sessions {
        if session.usage_precision() == TelemetryPrecision::Unknown {
            aggregate.unavailable_sessions += 1;
            continue;
        }

        aggregate.turn_count = aggregate
            .turn_count
            .and_then(|turns| turns.checked_add(u64::from(session.turn_count)));

        if session.telemetry.as_ref().is_some_and(|telemetry| {
            telemetry.usage.completeness != TelemetryCompleteness::Complete
        }) {
            aggregate.partial_sessions += 1;
        } else {
            aggregate.complete_sessions += 1;
        }

        let session_total = session
            .total_input_tokens
            .checked_add(session.total_output_tokens)
            .and_then(|total| total.checked_add(session.total_cache_read))
            .and_then(|total| total.checked_add(session.total_cache_create));
        let components = [
            (&mut aggregate.input_tokens, session.total_input_tokens),
            (&mut aggregate.output_tokens, session.total_output_tokens),
            (&mut aggregate.cache_read_tokens, session.total_cache_read),
            (
                &mut aggregate.cache_write_tokens,
                session.total_cache_create,
            ),
        ];
        for (total, value) in components {
            *total = total.and_then(|current| current.checked_add(value));
        }
        aggregate.total_tokens = aggregate
            .total_tokens
            .and_then(|total| session_total.and_then(|value| total.checked_add(value)));
    }

    if !sessions.is_empty() && aggregate.complete_sessions + aggregate.partial_sessions == 0 {
        aggregate.input_tokens = None;
        aggregate.output_tokens = None;
        aggregate.cache_read_tokens = None;
        aggregate.cache_write_tokens = None;
        aggregate.total_tokens = None;
        aggregate.turn_count = None;
    }

    aggregate.arithmetic_valid = aggregate.input_tokens.is_some()
        && aggregate.output_tokens.is_some()
        && aggregate.cache_read_tokens.is_some()
        && aggregate.cache_write_tokens.is_some()
        && aggregate.total_tokens.is_some()
        && aggregate.turn_count.is_some();
    if !aggregate.arithmetic_valid {
        aggregate.input_tokens = None;
        aggregate.output_tokens = None;
        aggregate.cache_read_tokens = None;
        aggregate.cache_write_tokens = None;
        aggregate.total_tokens = None;
        aggregate.turn_count = None;
    }
    aggregate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FleetUsage, SessionStatus, SessionTelemetry};

    fn session(input: u64, output: u64, cache_read: u64, cache_write: u64) -> AgentSession {
        AgentSession {
            pid: 1,
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
            total_cache_create: cache_write,
            turn_count: 2,
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

    fn partial_session() -> AgentSession {
        let mut session = session(1, 2, 3, 4);
        let mut telemetry = SessionTelemetry::process_only(1);
        telemetry.usage.precision = TelemetryPrecision::Exact;
        telemetry.usage.completeness = TelemetryCompleteness::Partial;
        session.telemetry = Some(telemetry);
        session
    }

    fn unknown_session() -> AgentSession {
        let mut session = session(0, 0, 0, 0);
        session.telemetry = Some(SessionTelemetry::process_only(1));
        session
    }

    #[test]
    fn aggregates_multiple_sessions_independent_of_selection() {
        let sessions = vec![session(1, 2, 3, 4), session(10, 20, 30, 40)];
        let aggregate = aggregate_live_usage(&sessions);
        assert_eq!(aggregate.input_tokens, Some(11));
        assert_eq!(aggregate.output_tokens, Some(22));
        assert_eq!(aggregate.cache_read_tokens, Some(33));
        assert_eq!(aggregate.cache_write_tokens, Some(44));
        assert_eq!(aggregate.total_tokens, Some(110));
        assert_eq!(aggregate.turn_count, Some(4));
        assert_eq!(aggregate.average_tokens_per_turn(), Some(27));
    }

    #[test]
    fn zero_live_sessions_are_an_exact_zero() {
        let aggregate = aggregate_live_usage(&[]);
        assert_eq!(aggregate.total_tokens, Some(0));
        assert_eq!(aggregate.turn_count, Some(0));
        assert_eq!(aggregate.average_tokens_per_turn(), Some(0));
    }

    #[test]
    fn known_zero_is_not_unavailable() {
        let aggregate = aggregate_live_usage(&[session(0, 0, 0, 0)]);
        assert_eq!(aggregate.total_tokens, Some(0));
        assert_eq!(aggregate.turn_count, Some(2));
        assert!(!aggregate.is_lower_bound());
    }

    #[test]
    fn partial_usage_is_a_known_lower_bound() {
        let aggregate = aggregate_live_usage(&[partial_session()]);
        assert_eq!(aggregate.total_tokens, Some(10));
        assert_eq!(aggregate.partial_sessions, 1);
        assert_eq!(aggregate.turn_count, Some(2));
        assert!(aggregate.is_lower_bound());
        assert_eq!(aggregate.average_tokens_per_turn(), None);
    }

    #[test]
    fn unknown_only_usage_has_no_total() {
        let aggregate = aggregate_live_usage(&[unknown_session()]);
        assert_eq!(aggregate.total_tokens, None);
        assert_eq!(aggregate.turn_count, None);
        assert_eq!(aggregate.unavailable_sessions, 1);
        assert!(aggregate.is_lower_bound());
    }

    #[test]
    fn mixed_known_and_unknown_usage_is_a_lower_bound() {
        let aggregate = aggregate_live_usage(&[session(1, 2, 3, 4), unknown_session()]);
        assert_eq!(aggregate.total_tokens, Some(10));
        assert_eq!(aggregate.turn_count, Some(2));
        assert!(aggregate.is_lower_bound());
    }

    #[test]
    fn overflow_makes_the_aggregate_unavailable() {
        let aggregate = aggregate_live_usage(&[session(u64::MAX, 1, 0, 0)]);
        assert!(!aggregate.arithmetic_valid);
        assert_eq!(aggregate.total_tokens, None);
        assert_eq!(aggregate.average_tokens_per_turn(), None);
    }

    #[test]
    fn fleet_usage_is_excluded() {
        let mut session = session(1, 2, 3, 4);
        let mut telemetry = SessionTelemetry::process_only(1);
        telemetry.usage.precision = TelemetryPrecision::Exact;
        telemetry.usage.completeness = TelemetryCompleteness::Complete;
        telemetry.fleet.runs.push(crate::model::FleetRun {
            lifecycle_version: Some(3),
            run_id: "run".into(),
            parent_run_id: None,
            nested: false,
            mode: crate::model::FleetRunMode::Single,
            state: crate::model::FleetRunState::Complete,
            execution: crate::model::FleetExecution::InProcess,
            runner_pid: None,
            started_at_ms: None,
            updated_at_ms: None,
            ended_at_ms: None,
            source_updated_at_ms: 0,
            stale: false,
            process_terminal: None,
            usage: FleetUsage {
                input_tokens: Some(1_000),
                output_tokens: Some(1_000),
                total_tokens: Some(2_000),
                reported_cost: None,
                accounting: crate::model::FleetUsageAccounting::SeparateRunAggregate,
            },
            children: Vec::new(),
            omitted_children: 0,
            reason: None,
        });
        session.telemetry = Some(telemetry);
        assert_eq!(aggregate_live_usage(&[session]).total_tokens, Some(10));
    }
}
