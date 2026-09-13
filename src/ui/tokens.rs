use crate::app::App;
use crate::locale::t;
use crate::model::aggregate_live_usage;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{btop_block_active, fmt_tokens, grad_at, make_gradient, meter_bar, styled_label};

pub(crate) fn draw_tokens_panel(f: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    draw_tokens_panel_active(f, app, area, theme, false);
}

pub(crate) fn draw_tokens_panel_active(
    f: &mut Frame,
    app: &App,
    area: Rect,
    theme: &Theme,
    active: bool,
) {
    let aggregate = aggregate_live_usage(&app.sessions);
    let panel_title = if area.width < 38 {
        t("tokens.title_short")
    } else {
        t("tokens.title")
    };
    let block = btop_block_active(&panel_title, "²", theme.mem_box, theme, active);
    let total = aggregate.total_tokens;
    let has_displayable_usage = total.is_some();

    let free_grad = make_gradient(
        theme.free_grad.start,
        theme.free_grad.mid,
        theme.free_grad.end,
    );
    let used_grad = make_gradient(
        theme.used_grad.start,
        theme.used_grad.mid,
        theme.used_grad.end,
    );
    let cached_grad = make_gradient(
        theme.cached_grad.start,
        theme.cached_grad.mid,
        theme.cached_grad.end,
    );
    let bar_w = (area.width as usize).saturating_sub(20).clamp(5, 15);

    let total_label = t("tokens.total");
    let total_value = total
        .map(|value| {
            format!(
                "{}{}",
                fmt_tokens(value),
                if aggregate.is_lower_bound() { "+" } else { "" }
            )
        })
        .unwrap_or_else(|| "—".to_string());
    let total_line = vec![
        styled_label(format!(" {}: ", total_label).as_str(), theme.graph_text),
        Span::styled(
            total_value,
            Style::default()
                .fg(if has_displayable_usage {
                    theme.title
                } else {
                    theme.inactive_fg
                })
                .add_modifier(Modifier::BOLD),
        ),
    ];

    let usage_line =
        |label: String, value: Option<u64>, gradient: &[ratatui::style::Color; 101]| {
            let mut line = vec![styled_label(
                format!(" {}:", label).as_str(),
                theme.graph_text,
            )];
            if let (Some(value), Some(total)) = (value, total) {
                let percentage = if total > 0 {
                    value as f64 / total as f64 * 100.0
                } else {
                    0.0
                };
                line.extend(meter_bar(percentage, bar_w, gradient, theme.meter_bg));
                line.push(Span::styled(
                    format!(" {}", fmt_tokens(value)),
                    Style::default().fg(grad_at(gradient, 80.0)),
                ));
            } else {
                line.push(Span::styled(" —", Style::default().fg(theme.inactive_fg)));
            }
            line
        };

    let coverage_line = if area.width < 52 {
        vec![
            styled_label(" L:", theme.graph_text),
            Span::styled(
                aggregate.live_sessions().to_string(),
                Style::default().fg(theme.main_fg),
            ),
            styled_label(" C:", theme.graph_text),
            Span::styled(
                aggregate.complete_sessions.to_string(),
                Style::default().fg(theme.main_fg),
            ),
            styled_label(" P:", theme.graph_text),
            Span::styled(
                aggregate.partial_sessions.to_string(),
                Style::default().fg(theme.main_fg),
            ),
            styled_label(" U:", theme.graph_text),
            Span::styled(
                aggregate.unavailable_sessions.to_string(),
                Style::default().fg(theme.inactive_fg),
            ),
        ]
    } else {
        vec![
            styled_label(
                format!(" {}: ", t("tokens.live")).as_str(),
                theme.graph_text,
            ),
            Span::styled(
                aggregate.live_sessions().to_string(),
                Style::default().fg(theme.main_fg),
            ),
            styled_label(
                format!("  {}: ", t("tokens.complete")).as_str(),
                theme.graph_text,
            ),
            Span::styled(
                aggregate.complete_sessions.to_string(),
                Style::default().fg(theme.main_fg),
            ),
            styled_label(
                format!("  {}: ", t("tokens.partial")).as_str(),
                theme.graph_text,
            ),
            Span::styled(
                aggregate.partial_sessions.to_string(),
                Style::default().fg(theme.main_fg),
            ),
            styled_label(
                format!("  {}: ", t("tokens.unavailable")).as_str(),
                theme.graph_text,
            ),
            Span::styled(
                aggregate.unavailable_sessions.to_string(),
                Style::default().fg(theme.inactive_fg),
            ),
        ]
    };

    let average = aggregate
        .average_tokens_per_turn()
        .map(|value| format!("{}/t", fmt_tokens(value)))
        .unwrap_or_else(|| "—".to_string());
    let turns = aggregate.turn_count.map_or_else(
        || "—".to_string(),
        |value| {
            format!(
                "{value}{}",
                if aggregate.is_lower_bound() { "+" } else { "" }
            )
        },
    );
    let lines = vec![
        Line::from(total_line),
        Line::from(usage_line(
            t("tokens.input"),
            aggregate.input_tokens,
            &free_grad,
        )),
        Line::from(usage_line(
            t("tokens.output"),
            aggregate.output_tokens,
            &used_grad,
        )),
        Line::from(usage_line(
            t("tokens.cache_r"),
            aggregate.cache_read_tokens,
            &cached_grad,
        )),
        Line::from(usage_line(
            t("tokens.cache_w"),
            aggregate.cache_write_tokens,
            &cached_grad,
        )),
        Line::from(coverage_line),
        Line::from(vec![
            styled_label(
                format!(" {}: ", t("tokens.turns")).as_str(),
                theme.graph_text,
            ),
            Span::styled(turns, Style::default().fg(theme.main_fg)),
            styled_label(
                format!("  {}: ", t("tokens.avg")).as_str(),
                theme.graph_text,
            ),
            Span::styled(average, Style::default().fg(theme.graph_text)),
        ]),
    ];

    f.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PanelVisibility;
    use crate::model::{SessionTelemetry, TelemetryCompleteness};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn rendered_tokens(app: &App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_tokens_panel(f, app, f.area(), &app.theme))
            .unwrap();
        format!("{}", terminal.backend())
    }

    #[test]
    fn aggregate_is_selection_independent_and_shows_coverage() {
        let mut app = App::new(Theme::default(), PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        app.selected = 2;
        let text = rendered_tokens(&app, 100, 18);
        assert!(
            text.contains("Total Tokens / all live sessions"),
            "missing aggregate title\n{text}"
        );
        assert!(
            text.contains("Total: 729.6k"),
            "token total should not follow the selected session\n{text}"
        );
        assert!(text.contains("Live: 3"), "missing live coverage\n{text}");
        assert!(
            text.contains("Complete: 3"),
            "missing complete coverage\n{text}"
        );
        assert!(
            !text.contains("tokens/turn"),
            "selected sparkline remains\n{text}"
        );
    }

    fn mixed_coverage_app() -> App {
        let mut app = App::new(Theme::default(), PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        app.sessions.truncate(3);
        app.sessions[1]
            .telemetry
            .as_mut()
            .unwrap()
            .usage
            .completeness = TelemetryCompleteness::Partial;
        app.sessions[2].telemetry = Some(SessionTelemetry::process_only(1));
        app
    }

    #[test]
    fn compact_coverage_and_lower_bounds_fit_a_narrow_panel() {
        let app = mixed_coverage_app();
        let text = rendered_tokens(&app, 33, 10);
        assert!(
            text.contains("L:3 C:1 P:1 U:1"),
            "missing compact coverage\n{text}"
        );
        assert!(
            text.contains("Total:") && text.contains('+'),
            "missing lower-bound total\n{text}"
        );
        assert!(
            text.contains("Turns: 43+"),
            "missing lower-bound turns\n{text}"
        );
    }

    #[test]
    fn empty_known_zero_and_partial_usage_render_distinctly() {
        let empty = App::new(Theme::default(), PanelVisibility::default());
        let text = rendered_tokens(&empty, 100, 18);
        assert!(text.contains("Total: 0"), "empty total is not zero\n{text}");
        assert!(
            text.contains("Turns: 0"),
            "empty turns are not zero\n{text}"
        );
        assert!(
            text.contains("Live: 0"),
            "empty coverage is missing\n{text}"
        );

        let mut known_zero = App::new(Theme::default(), PanelVisibility::default());
        crate::demo::populate_demo(&mut known_zero);
        known_zero.sessions.truncate(1);
        let session = &mut known_zero.sessions[0];
        session.total_input_tokens = 0;
        session.total_output_tokens = 0;
        session.total_cache_read = 0;
        session.total_cache_create = 0;
        let text = rendered_tokens(&known_zero, 100, 18);
        assert!(
            text.contains("Total: 0"),
            "known zero became unavailable\n{text}"
        );
        assert!(
            text.contains("Turns: 27"),
            "known turns are missing\n{text}"
        );
        assert!(
            text.contains("Complete: 1"),
            "known coverage is missing\n{text}"
        );

        known_zero.sessions[0]
            .telemetry
            .as_mut()
            .unwrap()
            .usage
            .completeness = TelemetryCompleteness::Partial;
        let text = rendered_tokens(&known_zero, 100, 18);
        assert!(
            text.contains("Total: 0+"),
            "partial total lacks marker\n{text}"
        );
        assert!(
            text.contains("Turns: 27+"),
            "partial turns lack marker\n{text}"
        );
        assert!(
            text.contains("Partial: 1"),
            "partial coverage is missing\n{text}"
        );
    }

    #[test]
    fn process_only_usage_has_no_zero_totals_or_bars() {
        let mut app = App::new(Theme::default(), PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        for session in &mut app.sessions {
            session.total_input_tokens = 0;
            session.total_output_tokens = 0;
            session.total_cache_read = 0;
            session.total_cache_create = 0;
            session.telemetry = Some(SessionTelemetry::process_only(1));
        }
        let text = rendered_tokens(&app, 100, 18);
        assert!(text.contains("Total: —"), "missing unknown total\n{text}");
        assert!(
            text.contains("Unavailable: 3"),
            "missing source state\n{text}"
        );
        assert!(
            !text.contains('■'),
            "unknown usage rendered as a bar\n{text}"
        );
    }
}
