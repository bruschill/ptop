use ptop::model::{
    PiLiveHarnessProvenance, PiLiveHarnessTelemetry, PiLivePhase, SessionTelemetry, SourceHealth,
};
use ptop::snapshot::SessionTelemetryView;

fn external_live_harness_accessor(
    telemetry: &SessionTelemetryView,
) -> Option<&PiLiveHarnessTelemetry> {
    telemetry.live_harness.as_ref()
}

#[test]
fn live_harness_types_are_available_to_external_rust_consumers() {
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.7.0");
    let live = PiLiveHarnessTelemetry {
        phase: Some(PiLivePhase::ToolRunning),
        pending_messages: Some(false),
        source_health: SourceHealth::Healthy,
        provenance: PiLiveHarnessProvenance::ExtensionAfUnixV1,
        observed_at_ms: Some(123),
        stale: false,
        reason: None,
    };
    let mut telemetry = SessionTelemetry::process_only(123);
    telemetry.live_harness = Some(live);

    let _view_accessor: fn(&SessionTelemetryView) -> Option<&PiLiveHarnessTelemetry> =
        external_live_harness_accessor;
    let json = serde_json::to_value(telemetry).unwrap();
    assert_eq!(json["live_harness"]["phase"], "tool_running");
    assert_eq!(json["live_harness"]["provenance"], "extension_af_unix_v1");
}
