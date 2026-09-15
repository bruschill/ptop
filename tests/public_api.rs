use ptop::model::{
    PiAssistantUsagePoint, PiCostAttributionBucket, PiCostAttributionTelemetry,
    PiHarnessActivityTelemetry, PiPointAttribution, ReconciliationStatus, TokenComponents,
};

#[test]
fn usage_metric_types_are_available_to_external_rust_consumers() {
    let activity = PiHarnessActivityTelemetry {
        status: ReconciliationStatus::Complete,
        tool_calls: Some(3),
        tool_results: Some(2),
        compactions: Some(1),
        branch_summaries: Some(1),
        reason: None,
    };
    let costs = PiCostAttributionTelemetry {
        status: ReconciliationStatus::Complete,
        total: Some(0.5),
        named: vec![PiCostAttributionBucket {
            provider: "provider".into(),
            model: "model".into(),
            reported_cost: 0.5,
        }],
        unavailable_assistant: Some(0.0),
        overflow: Some(0.0),
        unattributed_tool_or_summary: Some(0.0),
        reason: None,
    };
    let point = PiAssistantUsagePoint {
        components: Some(TokenComponents {
            input_tokens: 1,
            output_tokens: 2,
            cache_read_tokens: 3,
            cache_write_tokens: 4,
        }),
        attribution: Some(PiPointAttribution {
            provider: "provider".into(),
            model: "model".into(),
        }),
        reported_cost: Some(0.5),
        tool_calls: Some(3),
    };

    assert_eq!(activity.tool_calls, Some(3));
    assert_eq!(costs.named[0].reported_cost, 0.5);
    assert_eq!(point.tool_calls, Some(3));
}
