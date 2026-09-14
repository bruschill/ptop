use crate::model::{PiHarnessHistoryTelemetry, PiSummaryMarkerKind, TokenComponents};
use crate::theme::Theme;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use super::{fmt_tokens, truncate_str};

const SCALE: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub(crate) fn history_lines(
    history: &PiHarnessHistoryTelemetry,
    width: u16,
    max_lines: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    if width < 5 || max_lines == 0 {
        return Vec::new();
    }
    let full = width >= 90;
    let needed = if full { 7 } else { 4 };
    if max_lines < needed {
        return Vec::new();
    }
    let point_count = history.points.len();
    let available = (width as usize).saturating_sub(4);
    let legend_target = if full { 44 } else { 32 };
    let reserve = legend_target.min(available.saturating_sub(1));
    let columns = point_count
        .max(1)
        .min(available.saturating_sub(reserve))
        .min(64);
    if columns == 0 {
        return Vec::new();
    }
    let heading = truncate_str(
        &format!(
            " History {} · assistants {} shown, {} omitted",
            status(history),
            point_count,
            history.omitted_assistant_points
        ),
        width as usize,
    );
    let mut lines = vec![text_line(heading, theme)];
    if full {
        for (label, component) in [('I', 0), ('O', 1), ('R', 2), ('W', 3)] {
            let values = component_buckets(history, columns, component);
            let max = values
                .iter()
                .filter_map(|value| value.value)
                .max()
                .unwrap_or(0);
            let graph = values
                .into_iter()
                .map(|value| {
                    let glyph = component_glyph(value, max);
                    GraphGlyph::new(glyph, !matches!(glyph, '·' | '◌'))
                })
                .collect();
            lines.push(graph_line(
                label,
                graph,
                Some(format!("max {}", fmt_tokens(max))),
                width,
                theme,
            ));
        }
    } else {
        let buckets = total_buckets(history, columns);
        let max = buckets
            .iter()
            .filter_map(|bucket| bucket.value)
            .max()
            .unwrap_or(0);
        let graph = buckets
            .into_iter()
            .map(|bucket| {
                let glyph = total_glyph(bucket, max);
                GraphGlyph::new(glyph, !matches!(glyph, '·' | '◌'))
            })
            .collect();
        lines.push(graph_line('T', graph, None, width, theme));
    }
    lines.push(graph_line(
        'M',
        model_graph(history, columns),
        Some("│ change  ? uncertain  ! both".to_string()),
        width,
        theme,
    ));
    lines.push(graph_line(
        'E',
        event_graph(history, columns),
        Some(format!(
            "summaries {} shown, {} omitted",
            history.markers.len(),
            history.omitted_summary_events
        )),
        width,
        theme,
    ));
    lines
}

fn status(history: &PiHarnessHistoryTelemetry) -> &'static str {
    match history.status {
        crate::model::ReconciliationStatus::Complete => "complete",
        crate::model::ReconciliationStatus::Partial => "partial",
        crate::model::ReconciliationStatus::Unavailable => "unavailable",
    }
}
fn text_line(text: String, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(text, Style::default().fg(theme.inactive_fg)))
}
#[derive(Clone, Copy)]
struct GraphGlyph {
    glyph: char,
    active: bool,
}

impl GraphGlyph {
    fn new(glyph: char, active: bool) -> Self {
        Self { glyph, active }
    }
}

fn graph_line(
    label: char,
    graph: Vec<GraphGlyph>,
    legend: Option<String>,
    width: u16,
    theme: &Theme,
) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!(" {label}  "),
        Style::default().fg(theme.graph_text),
    )];
    for glyph in &graph {
        let color = if glyph.active {
            theme.pi_agent
        } else {
            theme.inactive_fg
        };
        spans.push(Span::styled(
            glyph.glyph.to_string(),
            Style::default().fg(color),
        ));
    }
    let graph_width: usize = graph
        .iter()
        .map(|glyph| glyph.glyph.width().unwrap_or(0))
        .sum();
    let remaining = (width as usize).saturating_sub(4 + graph_width);
    if let Some(legend) = legend.filter(|_| remaining >= 3) {
        spans.push(Span::styled("  ", Style::default().fg(theme.inactive_fg)));
        spans.push(Span::styled(
            truncate_str(&legend, remaining - 2),
            Style::default().fg(theme.inactive_fg),
        ));
    }
    Line::from(spans)
}

fn bounds(column: usize, points: usize, columns: usize) -> (usize, usize) {
    (column * points / columns, (column + 1) * points / columns)
}
fn component(component: &TokenComponents, index: usize) -> u64 {
    [
        component.input_tokens,
        component.output_tokens,
        component.cache_read_tokens,
        component.cache_write_tokens,
    ][index]
}
#[derive(Clone, Copy)]
struct ComponentBucket {
    value: Option<u64>,
    null: bool,
}
fn component_buckets(
    history: &PiHarnessHistoryTelemetry,
    columns: usize,
    index: usize,
) -> Vec<ComponentBucket> {
    if history.points.is_empty() {
        return vec![ComponentBucket {
            value: None,
            null: true,
        }];
    }
    (0..columns)
        .map(|column| {
            let (start, end) = bounds(column, history.points.len(), columns);
            let mut out = ComponentBucket {
                value: None,
                null: false,
            };
            for point in &history.points[start..end] {
                match &point.components {
                    Some(value) => {
                        out.value = Some(out.value.unwrap_or(0).max(component(value, index)))
                    }
                    None => out.null = true,
                }
            }
            out
        })
        .collect()
}
fn component_glyph(bucket: ComponentBucket, max: u64) -> char {
    if bucket.null && bucket.value.is_some() {
        '◌'
    } else {
        numeric_glyph(bucket.value, max)
    }
}
#[allow(clippy::manual_div_ceil)]
fn numeric_glyph(value: Option<u64>, max: u64) -> char {
    match value {
        None => '·',
        Some(_) if max == 0 => SCALE[0],
        Some(value) => {
            SCALE[(((value as u128 * 7 + max as u128 - 1) / max as u128) as usize).min(7)]
        }
    }
}
#[derive(Clone, Copy)]
struct TotalBucket {
    value: Option<u64>,
    null: bool,
    overflow: bool,
}
fn total_buckets(history: &PiHarnessHistoryTelemetry, columns: usize) -> Vec<TotalBucket> {
    if history.points.is_empty() {
        return vec![TotalBucket {
            value: None,
            null: true,
            overflow: false,
        }];
    }
    (0..columns)
        .map(|column| {
            let (start, end) = bounds(column, history.points.len(), columns);
            let mut out = TotalBucket {
                value: None,
                null: false,
                overflow: false,
            };
            for point in &history.points[start..end] {
                match &point.components {
                    None => out.null = true,
                    Some(v) => match v
                        .input_tokens
                        .checked_add(v.output_tokens)
                        .and_then(|x| x.checked_add(v.cache_read_tokens))
                        .and_then(|x| x.checked_add(v.cache_write_tokens))
                    {
                        Some(total) => out.value = Some(out.value.unwrap_or(0).max(total)),
                        None => out.overflow = true,
                    },
                }
            }
            out
        })
        .collect()
}
fn total_glyph(bucket: TotalBucket, max: u64) -> char {
    if bucket.overflow {
        if bucket.null {
            '※'
        } else {
            '!'
        }
    } else if bucket.null && bucket.value.is_some() {
        '◌'
    } else {
        numeric_glyph(bucket.value, max)
    }
}
fn model_graph(history: &PiHarnessHistoryTelemetry, columns: usize) -> Vec<GraphGlyph> {
    let mut states = Vec::new();
    for (index, point) in history.points.iter().enumerate() {
        states.push(if index == 0 {
            if point.attribution.is_some() {
                '·'
            } else {
                '?'
            }
        } else {
            match (&history.points[index - 1].attribution, &point.attribution) {
                (Some(previous), Some(current)) if previous == current => '·',
                (Some(_), Some(_)) => '│',
                _ => '?',
            }
        });
    }
    if states.is_empty() {
        return vec![GraphGlyph::new('·', false)];
    }
    (0..columns)
        .map(|column| {
            let (start, end) = bounds(column, states.len(), columns);
            let state = &states[start..end];
            if state.contains(&'│') && state.contains(&'?') {
                GraphGlyph::new('!', true)
            } else if state.contains(&'│') {
                GraphGlyph::new('│', true)
            } else if state.contains(&'?') {
                GraphGlyph::new('?', false)
            } else {
                // A known initial or unchanged attribution is semantic state,
                // not a graph gap, even though both use the same dot glyph.
                GraphGlyph::new('·', true)
            }
        })
        .collect()
}
fn event_graph(history: &PiHarnessHistoryTelemetry, columns: usize) -> Vec<GraphGlyph> {
    let points = history.points.len();
    let mut kinds = vec![(false, false); columns];
    for marker in &history.markers {
        let column = if points == 0 {
            0
        } else {
            let index = (marker.position as usize).min(points - 1);
            (((index + 1) * columns) - 1) / points
        };
        let entry = &mut kinds[column];
        match marker.kind {
            PiSummaryMarkerKind::Compaction => entry.0 = true,
            PiSummaryMarkerKind::BranchSummary => entry.1 = true,
        }
    }
    kinds
        .into_iter()
        .map(|(c, b)| match (c, b) {
            (true, true) => GraphGlyph::new('*', true),
            (true, false) => GraphGlyph::new('C', true),
            (false, true) => GraphGlyph::new('B', true),
            (false, false) => GraphGlyph::new('·', false),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        PiAssistantUsagePoint, PiHarnessHistoryTelemetry, PiPointAttribution, PiSummaryMarker,
        ReconciliationStatus,
    };

    fn history(points: Vec<PiAssistantUsagePoint>) -> PiHarnessHistoryTelemetry {
        PiHarnessHistoryTelemetry {
            status: ReconciliationStatus::Complete,
            assistant_count: points.len() as u32,
            omitted_assistant_points: 0,
            points,
            summary_event_count: 2,
            omitted_summary_events: 0,
            markers: vec![
                PiSummaryMarker {
                    kind: PiSummaryMarkerKind::Compaction,
                    position: 0,
                },
                PiSummaryMarker {
                    kind: PiSummaryMarkerKind::BranchSummary,
                    position: 2,
                },
            ],
            reason: None,
        }
    }
    fn graph_text(graph: &[GraphGlyph]) -> String {
        graph.iter().map(|glyph| glyph.glyph).collect()
    }

    #[test]
    fn graph_preserves_gaps_markers_and_width() {
        let point = |input: Option<u64>| PiAssistantUsagePoint {
            components: input.map(|input_tokens| TokenComponents {
                input_tokens,
                ..TokenComponents::default()
            }),
            attribution: Some(PiPointAttribution {
                provider: "p".into(),
                model: "m".into(),
            }),
        };
        let graph = history_lines(
            &history(vec![point(Some(0)), point(None), point(Some(7))]),
            90,
            7,
            &Theme::default(),
        );
        let text: String = graph
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains('▁') && text.contains('·') && text.contains('C') && text.contains('B'),
            "{text}"
        );
        assert!(graph.iter().all(|line| line.width() <= 90));
    }
    #[test]
    fn zero_point_history_uses_gap_columns_and_inactive_gap_color() {
        let theme = Theme::default();
        let graph = history_lines(&history(Vec::new()), 90, 7, &theme);
        let model = &graph[5];
        assert_eq!(model.spans[1].content, "·");
        assert_eq!(model.spans[1].style.fg, Some(theme.inactive_fg));
        let events = &graph[6];
        assert_eq!(events.spans[1].content, "*");
        assert_eq!(events.spans[1].style.fg, Some(theme.pi_agent));
    }

    #[test]
    fn marker_boundaries_map_to_first_interior_and_final_columns() {
        let mut fixture = history(vec![
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
            };
            3
        ]);
        fixture.markers = vec![
            PiSummaryMarker {
                kind: PiSummaryMarkerKind::Compaction,
                position: 0,
            },
            PiSummaryMarker {
                kind: PiSummaryMarkerKind::BranchSummary,
                position: 1,
            },
            PiSummaryMarker {
                kind: PiSummaryMarkerKind::Compaction,
                position: 3,
            },
        ];
        assert_eq!(graph_text(&event_graph(&fixture, 3)), "CBC");
    }

    #[test]
    fn graph_colors_values_and_uncertainty_individually() {
        let theme = Theme::default();
        let points = vec![
            PiAssistantUsagePoint {
                components: Some(TokenComponents {
                    input_tokens: 1,
                    ..TokenComponents::default()
                }),
                attribution: Some(PiPointAttribution {
                    provider: "p".into(),
                    model: "m".into(),
                }),
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
            },
        ];
        let graph = history_lines(&history(points), 90, 7, &theme);
        let input = &graph[1];
        assert_eq!(input.spans[1].style.fg, Some(theme.pi_agent));
        assert_eq!(input.spans[2].style.fg, Some(theme.inactive_fg));
        let model = &graph[5];
        assert_eq!(model.spans[1].style.fg, Some(theme.pi_agent));
        assert_eq!(model.spans[2].style.fg, Some(theme.inactive_fg));
    }

    #[test]
    fn model_dots_have_semantic_colors_for_known_and_synthetic_points() {
        let theme = Theme::default();
        let known = PiPointAttribution {
            provider: "p".into(),
            model: "m".into(),
        };
        let fixture = history(vec![
            PiAssistantUsagePoint {
                components: None,
                attribution: Some(known.clone()),
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: Some(known),
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
            },
        ]);
        let graph = history_lines(&fixture, 90, 7, &theme);
        let model = &graph[5];
        assert_eq!(model.spans[1].content, "·");
        assert_eq!(model.spans[1].style.fg, Some(theme.pi_agent));
        assert_eq!(model.spans[2].content, "·");
        assert_eq!(model.spans[2].style.fg, Some(theme.pi_agent));
        assert_eq!(model.spans[3].content, "?");
        assert_eq!(model.spans[3].style.fg, Some(theme.inactive_fg));

        let zero = history_lines(&history(Vec::new()), 90, 7, &theme);
        assert_eq!(zero[5].spans[1].content, "·");
        assert_eq!(zero[5].spans[1].style.fg, Some(theme.inactive_fg));

        let changed_then_unknown = history(vec![
            PiAssistantUsagePoint {
                components: None,
                attribution: Some(PiPointAttribution {
                    provider: "p".into(),
                    model: "a".into(),
                }),
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: Some(PiPointAttribution {
                    provider: "p".into(),
                    model: "b".into(),
                }),
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
            },
        ]);
        let combined = model_graph(&changed_then_unknown, 1);
        assert_eq!(graph_text(&combined), "!");
        assert!(combined[0].active);

        let known_null_known = history(vec![
            PiAssistantUsagePoint {
                components: None,
                attribution: Some(PiPointAttribution {
                    provider: "p".into(),
                    model: "a".into(),
                }),
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: Some(PiPointAttribution {
                    provider: "p".into(),
                    model: "a".into(),
                }),
            },
        ]);
        assert_eq!(graph_text(&model_graph(&known_null_known, 1)), "?");
    }

    #[test]
    fn compact_total_preserves_overflow_and_gap() {
        let overflowing = PiAssistantUsagePoint {
            components: Some(TokenComponents {
                input_tokens: u64::MAX,
                output_tokens: 1,
                ..TokenComponents::default()
            }),
            attribution: None,
        };
        let fixture = history(vec![
            overflowing.clone(),
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
            },
        ]);
        assert_eq!(
            total_glyph(total_buckets(&history(vec![overflowing]), 1)[0], 0),
            '!'
        );
        assert_eq!(total_glyph(total_buckets(&fixture, 1)[0], 0), '※');
        let rendered = history_lines(&fixture, 5, 4, &Theme::default());
        assert_eq!(rendered[1].spans[1].content, "※");
        assert_eq!(
            rendered[1].spans[1].style.fg,
            Some(Theme::default().pi_agent)
        );

        let mixed = history(vec![
            PiAssistantUsagePoint {
                components: Some(TokenComponents::default()),
                attribution: None,
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
            },
        ]);
        assert_eq!(total_glyph(total_buckets(&mixed, 1)[0], 0), '◌');
        assert_eq!(component_glyph(component_buckets(&mixed, 1, 0)[0], 0), '◌');
    }

    #[test]
    fn full_rows_use_independent_scales_and_all_zero_is_lowest_glyph() {
        let fixture = history(vec![
            PiAssistantUsagePoint {
                components: Some(TokenComponents {
                    input_tokens: 1,
                    output_tokens: 100,
                    ..TokenComponents::default()
                }),
                attribution: None,
            },
            PiAssistantUsagePoint {
                components: Some(TokenComponents {
                    input_tokens: 2,
                    output_tokens: 1,
                    ..TokenComponents::default()
                }),
                attribution: None,
            },
        ]);
        assert_eq!(
            graph_text(
                &component_buckets(&fixture, 2, 0)
                    .into_iter()
                    .map(|b| GraphGlyph::new(component_glyph(b, 2), true))
                    .collect::<Vec<_>>()
            ),
            "▅█"
        );
        assert_eq!(
            graph_text(
                &component_buckets(&fixture, 2, 1)
                    .into_iter()
                    .map(|b| GraphGlyph::new(component_glyph(b, 100), true))
                    .collect::<Vec<_>>()
            ),
            "█▂"
        );
        let zeros = history(vec![
            PiAssistantUsagePoint {
                components: Some(TokenComponents::default()),
                attribution: None,
            };
            8
        ]);
        assert_eq!(
            graph_text(
                &component_buckets(&zeros, 8, 0)
                    .into_iter()
                    .map(|b| GraphGlyph::new(component_glyph(b, 0), true))
                    .collect::<Vec<_>>()
            ),
            "▁▁▁▁▁▁▁▁"
        );
        assert_eq!(
            (0..=7)
                .map(|value| numeric_glyph(Some(value), 7))
                .collect::<String>(),
            SCALE.iter().collect::<String>()
        );
    }

    #[test]
    fn graph_widths_share_columns_and_truncate_legends() {
        let fixture = history(vec![
            PiAssistantUsagePoint {
                components: None,
                attribution: None
            };
            64
        ]);
        for (width, expected_rows, expected_columns) in
            [(5, 4, 1), (60, 4, 24), (89, 4, 53), (90, 7, 42)]
        {
            let lines = history_lines(&fixture, width, 7, &Theme::default());
            assert_eq!(lines.len(), expected_rows, "{width}");
            assert!(
                lines.iter().all(|line| line.width() <= width as usize),
                "{width}"
            );
            for line in &lines[1..] {
                let columns = line
                    .spans
                    .iter()
                    .skip(1)
                    .take_while(|span| span.content.chars().count() == 1)
                    .count();
                assert_eq!(columns, expected_columns, "{width}: {line:?}");
            }
        }
    }

    #[test]
    fn collapsed_markers_preserve_both_kinds_after_downsampling() {
        let mut fixture = history(vec![
            PiAssistantUsagePoint {
                components: None,
                attribution: None
            };
            64
        ]);
        fixture.markers = vec![
            PiSummaryMarker {
                kind: PiSummaryMarkerKind::Compaction,
                position: 32,
            },
            PiSummaryMarker {
                kind: PiSummaryMarkerKind::BranchSummary,
                position: 32,
            },
        ];
        assert!(graph_text(&event_graph(&fixture, 1)).contains('*'));
        assert!(graph_text(&event_graph(&fixture, 64)).contains('*'));
    }

    #[test]
    fn small_width_returns_no_graph() {
        assert!(history_lines(&history(Vec::new()), 4, 7, &Theme::default()).is_empty());
    }
}
