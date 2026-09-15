use crate::model::{PiHarnessHistoryTelemetry, PiSummaryMarkerKind, TokenComponents};
use crate::theme::Theme;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use super::{fmt_tokens, grad_at, make_gradient, truncate_str};

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
    let needed = if full { 9 } else { 6 };
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
            let family = match component {
                0 => &theme.used_grad,
                1 => &theme.proc_grad,
                2 => &theme.cached_grad,
                _ => &theme.free_grad,
            };
            let gradient = make_gradient(family.start, family.mid, family.end);
            let graph = values
                .into_iter()
                .map(|value| {
                    let glyph = component_glyph(value, max);
                    if glyph == '◌' {
                        GraphGlyph::colored(glyph, theme.warning_fg)
                    } else if let Some(value) = value.value {
                        GraphGlyph::colored(
                            glyph,
                            grad_at(&gradient, value as f64 / max.max(1) as f64 * 100.0),
                        )
                    } else {
                        GraphGlyph::new(glyph, false)
                    }
                })
                .collect();
            lines.push(graph_line(
                label,
                graph,
                Some(format!("peak {} / turn", fmt_tokens(max))),
                width,
                theme,
            ));
        }
        lines.push(cost_line(history, columns, width, theme));
        lines.push(tool_line(history, columns, width, theme));
    } else {
        let buckets = total_buckets(history, columns);
        let max = buckets
            .iter()
            .filter_map(|bucket| bucket.value)
            .max()
            .unwrap_or(0);
        let gradient = make_gradient(theme.cpu_grad.start, theme.cpu_grad.mid, theme.cpu_grad.end);
        let graph = buckets
            .into_iter()
            .map(|bucket| {
                let glyph = total_glyph(bucket, max);
                if matches!(glyph, '!' | '※' | '◌') {
                    GraphGlyph::colored(glyph, theme.warning_fg)
                } else if let Some(value) = bucket.value {
                    GraphGlyph::colored(
                        glyph,
                        grad_at(&gradient, value as f64 / max.max(1) as f64 * 100.0),
                    )
                } else {
                    GraphGlyph::new(glyph, false)
                }
            })
            .collect();
        lines.push(graph_line(
            'T',
            graph,
            Some(format!("peak {} / turn", fmt_tokens(max))),
            width,
            theme,
        ));
        lines.push(cost_line(history, columns, width, theme));
        lines.push(tool_line(history, columns, width, theme));
    }
    lines.push(graph_line(
        'M',
        model_graph(history, columns, theme),
        Some("│ change  ? uncertain  ! both".to_string()),
        width,
        theme,
    ));
    lines.push(graph_line(
        'E',
        event_graph(history, columns, theme),
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
    color: Option<Color>,
}

impl GraphGlyph {
    fn new(glyph: char, active: bool) -> Self {
        Self {
            glyph,
            active,
            color: None,
        }
    }

    fn colored(glyph: char, color: Color) -> Self {
        Self {
            glyph,
            active: true,
            color: Some(color),
        }
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
        let color = glyph.color.unwrap_or(if glyph.active {
            theme.pi_agent
        } else {
            theme.inactive_fg
        });
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
fn cost_line(
    history: &PiHarnessHistoryTelemetry,
    columns: usize,
    width: u16,
    theme: &Theme,
) -> Line<'static> {
    let values = cost_buckets(history, columns);
    let max = values
        .iter()
        .filter_map(|value| value.value)
        .fold(0.0_f64, f64::max);
    let gradient = make_gradient(
        theme.free_grad.start,
        theme.free_grad.mid,
        theme.free_grad.end,
    );
    let graph = values
        .into_iter()
        .map(|value| {
            if value.null && value.value.is_some() {
                return GraphGlyph::colored('◌', theme.warning_fg);
            }
            match value.value {
                Some(value) => GraphGlyph::colored(
                    cost_glyph(value, max),
                    grad_at(&gradient, percentage(value, max)),
                ),
                None => GraphGlyph::new('·', false),
            }
        })
        .collect();
    graph_line(
        '$',
        graph,
        Some(format!("peak ${} / turn", fmt_cost_peak(max))),
        width,
        theme,
    )
}

fn fmt_cost_peak(value: f64) -> String {
    if value == 0.0 {
        "0.0000".to_string()
    } else if value < 0.0001 || value >= 1_000_000.0 {
        format!("{value:.4e}")
    } else {
        format!("{value:.4}")
    }
}

fn percentage(value: f64, max: f64) -> f64 {
    if max > 0.0 {
        (value / max * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    }
}

fn cost_glyph(value: f64, max: f64) -> char {
    if max == 0.0 {
        SCALE[0]
    } else {
        SCALE[((value / max * 7.0).ceil() as usize).min(7)]
    }
}

struct CostBucket {
    value: Option<f64>,
    null: bool,
}

fn cost_buckets(history: &PiHarnessHistoryTelemetry, columns: usize) -> Vec<CostBucket> {
    if history.points.is_empty() {
        return vec![CostBucket {
            value: None,
            null: true,
        }];
    }
    (0..columns)
        .map(|column| {
            let (start, end) = bounds(column, history.points.len(), columns);
            let mut out = CostBucket {
                value: None,
                null: false,
            };
            for point in &history.points[start..end] {
                match point
                    .reported_cost
                    .filter(|value| value.is_finite() && *value >= 0.0)
                {
                    Some(value) => {
                        out.value = Some(out.value.unwrap_or(0.0).max(value));
                    }
                    None => out.null = true,
                }
            }
            out
        })
        .collect()
}

fn tool_line(
    history: &PiHarnessHistoryTelemetry,
    columns: usize,
    width: u16,
    theme: &Theme,
) -> Line<'static> {
    let values = optional_buckets(history, columns, |point| point.tool_calls.map(u64::from));
    let max = values
        .iter()
        .filter_map(|value| value.value)
        .max()
        .unwrap_or(0);
    let gradient = make_gradient(
        theme.proc_grad.start,
        theme.proc_grad.mid,
        theme.proc_grad.end,
    );
    let graph = values
        .into_iter()
        .map(|bucket| {
            if bucket.null && bucket.value.is_some() {
                return GraphGlyph::colored('◌', theme.warning_fg);
            }
            match bucket.value {
                Some(value) => GraphGlyph::colored(
                    numeric_glyph(Some(value), max),
                    grad_at(&gradient, value as f64 / max.max(1) as f64 * 100.0),
                ),
                None => GraphGlyph::new('·', false),
            }
        })
        .collect();
    graph_line(
        '#',
        graph,
        Some(format!("peak {max} calls / turn")),
        width,
        theme,
    )
}

fn optional_buckets<F>(
    history: &PiHarnessHistoryTelemetry,
    columns: usize,
    value: F,
) -> Vec<ComponentBucket>
where
    F: Fn(&crate::model::PiAssistantUsagePoint) -> Option<u64>,
{
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
                match value(point) {
                    Some(value) => out.value = Some(out.value.unwrap_or(0).max(value)),
                    None => out.null = true,
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
fn model_graph(
    history: &PiHarnessHistoryTelemetry,
    columns: usize,
    theme: &Theme,
) -> Vec<GraphGlyph> {
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
                GraphGlyph::colored('!', theme.warning_fg)
            } else if state.contains(&'│') {
                GraphGlyph::colored('│', theme.status_fg)
            } else if state.contains(&'?') {
                GraphGlyph::new('?', false)
            } else {
                // A known initial or unchanged attribution is semantic state,
                // not a graph gap, even though both use the same dot glyph.
                GraphGlyph::colored('·', theme.pi_agent)
            }
        })
        .collect()
}
fn event_graph(
    history: &PiHarnessHistoryTelemetry,
    columns: usize,
    theme: &Theme,
) -> Vec<GraphGlyph> {
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
            (true, true) => GraphGlyph::colored('*', theme.hi_fg),
            (true, false) => GraphGlyph::colored('C', theme.warning_fg),
            (false, true) => GraphGlyph::colored('B', theme.status_fg),
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
            reported_cost: None,
            tool_calls: None,
        };
        let graph = history_lines(
            &history(vec![point(Some(0)), point(None), point(Some(7))]),
            90,
            9,
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
        let graph = history_lines(&history(Vec::new()), 90, 9, &theme);
        let model = &graph[7];
        assert_eq!(model.spans[1].content, "·");
        assert_eq!(model.spans[1].style.fg, Some(theme.inactive_fg));
        let events = &graph[8];
        assert_eq!(events.spans[1].content, "*");
        assert_eq!(events.spans[1].style.fg, Some(theme.hi_fg));
    }

    #[test]
    fn marker_boundaries_map_to_first_interior_and_final_columns() {
        let mut fixture = history(vec![
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
                reported_cost: None,
                tool_calls: None,
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
        assert_eq!(
            graph_text(&event_graph(&fixture, 3, &Theme::default())),
            "CBC"
        );
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
                reported_cost: None,
                tool_calls: None,
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
                reported_cost: None,
                tool_calls: None,
            },
        ];
        let graph = history_lines(&history(points), 90, 9, &theme);
        let input = &graph[1];
        assert_eq!(
            input.spans[1].style.fg,
            Some(grad_at(
                &make_gradient(
                    theme.used_grad.start,
                    theme.used_grad.mid,
                    theme.used_grad.end
                ),
                100.0
            ))
        );
        assert_eq!(input.spans[2].style.fg, Some(theme.inactive_fg));
        let model = &graph[7];
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
                reported_cost: None,
                tool_calls: None,
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: Some(known),
                reported_cost: None,
                tool_calls: None,
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
                reported_cost: None,
                tool_calls: None,
            },
        ]);
        let graph = history_lines(&fixture, 90, 9, &theme);
        let model = &graph[7];
        assert_eq!(model.spans[1].content, "·");
        assert_eq!(model.spans[1].style.fg, Some(theme.pi_agent));
        assert_eq!(model.spans[2].content, "·");
        assert_eq!(model.spans[2].style.fg, Some(theme.pi_agent));
        assert_eq!(model.spans[3].content, "?");
        assert_eq!(model.spans[3].style.fg, Some(theme.inactive_fg));

        let zero = history_lines(&history(Vec::new()), 90, 9, &theme);
        assert_eq!(zero[5].spans[1].content, "·");
        assert_eq!(zero[5].spans[1].style.fg, Some(theme.inactive_fg));

        let changed_then_unknown = history(vec![
            PiAssistantUsagePoint {
                components: None,
                attribution: Some(PiPointAttribution {
                    provider: "p".into(),
                    model: "a".into(),
                }),
                reported_cost: None,
                tool_calls: None,
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: Some(PiPointAttribution {
                    provider: "p".into(),
                    model: "b".into(),
                }),
                reported_cost: None,
                tool_calls: None,
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
                reported_cost: None,
                tool_calls: None,
            },
        ]);
        let combined = model_graph(&changed_then_unknown, 1, &Theme::default());
        assert_eq!(graph_text(&combined), "!");
        assert!(combined[0].active);

        let known_null_known = history(vec![
            PiAssistantUsagePoint {
                components: None,
                attribution: Some(PiPointAttribution {
                    provider: "p".into(),
                    model: "a".into(),
                }),
                reported_cost: None,
                tool_calls: None,
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
                reported_cost: None,
                tool_calls: None,
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: Some(PiPointAttribution {
                    provider: "p".into(),
                    model: "a".into(),
                }),
                reported_cost: None,
                tool_calls: None,
            },
        ]);
        assert_eq!(
            graph_text(&model_graph(&known_null_known, 1, &Theme::default())),
            "?"
        );
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
            reported_cost: None,
            tool_calls: None,
        };
        let fixture = history(vec![
            overflowing.clone(),
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
                reported_cost: None,
                tool_calls: None,
            },
        ]);
        assert_eq!(
            total_glyph(total_buckets(&history(vec![overflowing]), 1)[0], 0),
            '!'
        );
        assert_eq!(total_glyph(total_buckets(&fixture, 1)[0], 0), '※');
        let rendered = history_lines(&fixture, 5, 6, &Theme::default());
        assert_eq!(rendered[1].spans[1].content, "※");
        assert_eq!(
            rendered[1].spans[1].style.fg,
            Some(Theme::default().warning_fg)
        );

        let mixed = history(vec![
            PiAssistantUsagePoint {
                components: Some(TokenComponents::default()),
                attribution: None,
                reported_cost: None,
                tool_calls: None,
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
                reported_cost: None,
                tool_calls: None,
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
                reported_cost: None,
                tool_calls: None,
            },
            PiAssistantUsagePoint {
                components: Some(TokenComponents {
                    input_tokens: 2,
                    output_tokens: 1,
                    ..TokenComponents::default()
                }),
                attribution: None,
                reported_cost: None,
                tool_calls: None,
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
                reported_cost: None,
                tool_calls: None,
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
                attribution: None,
                reported_cost: None,
                tool_calls: None,
            };
            64
        ]);
        for (width, expected_rows, expected_columns) in
            [(5, 6, 1), (60, 6, 24), (89, 6, 53), (90, 9, 42)]
        {
            let lines = history_lines(
                &fixture,
                width,
                if width >= 90 { 9 } else { 6 },
                &Theme::default(),
            );
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
                attribution: None,
                reported_cost: None,
                tool_calls: None,
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
        assert!(graph_text(&event_graph(&fixture, 1, &Theme::default())).contains('*'));
        assert!(graph_text(&event_graph(&fixture, 64, &Theme::default())).contains('*'));
    }

    #[test]
    fn enhanced_history_is_atomic_at_full_and_compact_thresholds() {
        let fixture = history(vec![PiAssistantUsagePoint {
            components: Some(TokenComponents::default()),
            attribution: None,
            reported_cost: Some(0.0),
            tool_calls: Some(0),
        }]);
        assert!(history_lines(&fixture, 90, 8, &Theme::default()).is_empty());
        assert_eq!(history_lines(&fixture, 90, 9, &Theme::default()).len(), 9);
        assert!(history_lines(&fixture, 89, 5, &Theme::default()).is_empty());
        assert_eq!(history_lines(&fixture, 89, 6, &Theme::default()).len(), 6);
    }

    #[test]
    fn cost_scaling_preserves_fractional_and_huge_finite_values() {
        assert_eq!(cost_glyph(0.0, 0.000_000_5), '▁');
        assert_eq!(cost_glyph(0.000_000_25, 0.000_000_5), '▅');
        assert_eq!(cost_glyph(0.000_000_5, 0.000_000_5), '█');
        assert_eq!(fmt_cost_peak(0.000_000_5), "5.0000e-7");
        assert_eq!(fmt_cost_peak(1.0e300), "1.0000e300");

        let fixture = history(vec![
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
                reported_cost: Some(0.000_000_5),
                tool_calls: Some(1),
            },
            PiAssistantUsagePoint {
                components: None,
                attribution: None,
                reported_cost: Some(1.0e300),
                tool_calls: None,
            },
        ]);
        let buckets = cost_buckets(&fixture, 2);
        assert_eq!(buckets[0].value, Some(0.000_000_5));
        assert_eq!(buckets[1].value, Some(1.0e300));
        let mixed = cost_buckets(&fixture, 1);
        assert_eq!(mixed[0].value, Some(1.0e300));
        assert!(!mixed[0].null);

        let mut with_gap = fixture;
        with_gap.points[1].reported_cost = None;
        let mixed = cost_buckets(&with_gap, 1);
        assert_eq!(mixed[0].value, Some(0.000_000_5));
        assert!(mixed[0].null);
        let line = cost_line(&with_gap, 1, 40, &Theme::default());
        assert_eq!(line.spans[1].content, "◌");
        assert_eq!(line.spans[1].style.fg, Some(Theme::default().warning_fg));
    }

    #[test]
    fn metric_families_use_distinct_theme_colors_and_preserve_tool_uncertainty() {
        let theme = Theme::default();
        let mut fixture = history(vec![
            PiAssistantUsagePoint {
                components: Some(TokenComponents {
                    input_tokens: 1,
                    output_tokens: 2,
                    cache_read_tokens: 3,
                    cache_write_tokens: 4,
                }),
                attribution: Some(PiPointAttribution {
                    provider: "p".into(),
                    model: "a".into(),
                }),
                reported_cost: Some(0.25),
                tool_calls: Some(1),
            },
            PiAssistantUsagePoint {
                components: Some(TokenComponents {
                    input_tokens: 2,
                    output_tokens: 3,
                    cache_read_tokens: 4,
                    cache_write_tokens: 5,
                }),
                attribution: Some(PiPointAttribution {
                    provider: "p".into(),
                    model: "b".into(),
                }),
                reported_cost: Some(0.5),
                tool_calls: None,
            },
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
        ];
        fixture.summary_event_count = 2;

        let lines = history_lines(&fixture, 120, 9, &theme);
        assert_eq!(lines.len(), 9);
        let gradients = [
            make_gradient(
                theme.used_grad.start,
                theme.used_grad.mid,
                theme.used_grad.end,
            ),
            make_gradient(
                theme.proc_grad.start,
                theme.proc_grad.mid,
                theme.proc_grad.end,
            ),
            make_gradient(
                theme.cached_grad.start,
                theme.cached_grad.mid,
                theme.cached_grad.end,
            ),
            make_gradient(
                theme.free_grad.start,
                theme.free_grad.mid,
                theme.free_grad.end,
            ),
        ];
        for (row, (value, max)) in [(1, (1, 2)), (2, (2, 3)), (3, (3, 4)), (4, (4, 5))] {
            assert_eq!(
                lines[row].spans[1].style.fg,
                Some(grad_at(
                    &gradients[row - 1],
                    value as f64 / max as f64 * 100.0
                ))
            );
        }
        assert_eq!(lines[7].spans[1].style.fg, Some(theme.pi_agent));
        assert_eq!(lines[7].spans[2].style.fg, Some(theme.status_fg));
        assert_eq!(lines[8].spans[1].style.fg, Some(theme.warning_fg));
        assert_eq!(lines[8].spans[2].style.fg, Some(theme.status_fg));

        let tool_bucket = optional_buckets(&fixture, 1, |point| point.tool_calls.map(u64::from));
        assert_eq!(tool_bucket[0].value, Some(1));
        assert!(tool_bucket[0].null);
        let tool = tool_line(&fixture, 1, 40, &theme);
        assert_eq!(tool.spans[1].content, "◌");
        assert_eq!(tool.spans[1].style.fg, Some(theme.warning_fg));
    }

    #[test]
    fn small_width_returns_no_graph() {
        assert!(history_lines(&history(Vec::new()), 4, 7, &Theme::default()).is_empty());
    }
}
