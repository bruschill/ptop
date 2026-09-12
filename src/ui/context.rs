use crate::app::App;
use crate::locale::t;
use crate::theme::Theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};
use ratatui::Frame;

use super::{
    braille_graph_multirow, btop_block_active, fmt_tokens, grad_at, make_gradient, meter_bar,
    truncate_str,
};

pub(crate) fn draw_context_panel(f: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    draw_context_panel_active(f, app, area, theme, false);
}

pub(crate) fn draw_context_panel_active(
    f: &mut Frame,
    app: &App,
    area: Rect,
    theme: &Theme,
    active: bool,
) {
    let cpu_grad = make_gradient(theme.cpu_grad.start, theme.cpu_grad.mid, theme.cpu_grad.end);

    let block = btop_block_active("context", "¹", theme.cpu_box, theme, active);
    f.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    // Compact mode: single-line text summary when too short for graph
    if inner.height <= 1 {
        let ticks_per_min = 30usize;
        let rates: Vec<f64> = app.token_rates.iter().copied().collect();
        let tokens_per_min: f64 = rates.iter().rev().take(ticks_per_min).sum();
        let total = aggregate_tokens_label(app);
        let active = if app.is_pi_mode() {
            "—".to_string()
        } else {
            app.sessions
                .iter()
                .filter(|s| s.status.is_active())
                .count()
                .to_string()
        };

        let rate_label = t("context.rate");
        let total_label = t("context.total");
        let active_label = t("context.active");
        let line = Line::from(vec![
            Span::styled(
                format!(" {} ", rate_label),
                Style::default().fg(theme.graph_text),
            ),
            Span::styled(
                if app.token_rate_known {
                    format!("{}/min", fmt_tokens(tokens_per_min as u64))
                } else {
                    "—/min".to_string()
                },
                Style::default().fg(grad_at(&cpu_grad, 50.0)),
            ),
            Span::styled(
                format!("  {} ", total_label),
                Style::default().fg(theme.graph_text),
            ),
            Span::styled(total, Style::default().fg(theme.main_fg)),
            Span::styled(
                format!("  {} {}", active, active_label),
                Style::default().fg(theme.proc_misc),
            ),
        ]);
        f.render_widget(Paragraph::new(line), inner);
        return;
    }

    // Full mode: sparkline graph + context bars
    let halves = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(inner);

    draw_context_sparkline(f, app, halves[0], &cpu_grad, theme);
    draw_context_bars(f, app, halves[1], &cpu_grad, theme);
}

fn draw_context_sparkline(
    f: &mut Frame,
    app: &App,
    area: Rect,
    cpu_grad: &[Color; 101],
    theme: &Theme,
) {
    let avail_h = area.height as usize;
    let avail_w = area.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    let spark_w = avail_w.saturating_sub(2).max(4);
    let rates: Vec<f64> = if app.token_rate_known {
        app.token_rates.iter().copied().collect()
    } else {
        Vec::new()
    };
    let max_rate = rates.iter().cloned().fold(1.0_f64, f64::max);
    let normalized: Vec<f64> = rates.iter().map(|&v| v / max_rate).collect();

    // Graph title with current rate (btop-style)
    let ticks_per_min = 30usize;
    let tokens_per_min: f64 = rates.iter().rev().take(ticks_per_min).sum();
    let current_pct = normalized.last().copied().unwrap_or(0.0) * 100.0;
    let pct_color = grad_at(cpu_grad, current_pct);
    let token_rate_label = t("context.token_rate");
    lines.push(Line::from(vec![
        Span::styled(token_rate_label, Style::default().fg(theme.graph_text)),
        Span::styled(
            if app.token_rate_known {
                format!("  {}/min", fmt_tokens(tokens_per_min as u64))
            } else {
                "  —/min".to_string()
            },
            Style::default().fg(pct_color),
        ),
    ]));

    // Multi-row braille area graph (fills available height minus title + summary)
    let graph_h = avail_h.saturating_sub(2).max(1);
    let graph_rows =
        braille_graph_multirow(&normalized, spark_w, graph_h, cpu_grad, theme.graph_text);
    for row_spans in graph_rows {
        let mut line_spans = vec![Span::styled(" ", Style::default())];
        line_spans.extend(row_spans);
        lines.push(Line::from(line_spans));
    }

    // Summary line: total tokens
    let total_tokens = aggregate_tokens_label(app);
    let total_label = t("context.total");
    lines.push(Line::from(vec![
        Span::styled(
            format!(" {total_tokens}"),
            Style::default().fg(theme.main_fg),
        ),
        Span::styled(
            format!(" {}", total_label),
            Style::default().fg(theme.graph_text),
        ),
    ]));

    f.render_widget(Paragraph::new(lines), area);
}

fn draw_context_bars(f: &mut Frame, app: &App, area: Rect, cpu_grad: &[Color; 101], theme: &Theme) {
    let header_style = Style::default()
        .fg(theme.main_fg)
        .add_modifier(Modifier::BOLD);

    // bar width = remaining space after Project(10) + pct(5) + info(10) + padding
    let bar_width = (area.width as usize).saturating_sub(30).clamp(4, 20);

    let mut rows = Vec::new();

    let project_label = t("context.project");
    let context_label = t("context.context");
    let window_label = t("context.window");

    for session in &app.sessions {
        let context_cell = if let Some(raw_pct) = session.context_value() {
            let bar_pct = raw_pct.min(100.0);
            let warn = if raw_pct >= 90.0 {
                "⚠"
            } else if raw_pct >= 75.0 {
                "!"
            } else {
                ""
            };
            let pct_color = grad_at(cpu_grad, bar_pct);
            let mut spans = meter_bar(bar_pct, bar_width, cpu_grad, theme.meter_bg);
            spans.push(Span::styled(
                format!(
                    " {}{:>3.0}%{}",
                    session.context_precision().prefix(),
                    raw_pct,
                    warn
                ),
                Style::default().fg(pct_color),
            ));
            Line::from(spans)
        } else if let Some(tokens) = session.context_tokens_without_window() {
            Line::from(Span::styled(
                format!(
                    " {}{} tokens",
                    session.context_precision().prefix(),
                    fmt_tokens(tokens)
                ),
                Style::default().fg(theme.inactive_fg),
            ))
        } else {
            Line::from(Span::styled("—", Style::default().fg(theme.inactive_fg)))
        };

        let ctx_info = match (session.context_window_value(), session.compaction_count) {
            (Some(window), 0) => fmt_tokens(window),
            (Some(window), n) => format!("{} C{}", fmt_tokens(window), n),
            (None, 0) if session.context_tokens_without_window().is_some() => {
                "window —".to_string()
            }
            (None, 0) => "—".to_string(),
            (None, n) if session.context_tokens_without_window().is_some() => {
                format!("window — C{n}")
            }
            (None, n) => format!("— C{n}"),
        };

        rows.push(Row::new(vec![
            Cell::from(Span::styled(
                truncate_str(&session.project_name, 10),
                Style::default().fg(theme.title),
            )),
            Cell::from(context_cell),
            Cell::from(Span::styled(
                ctx_info,
                Style::default().fg(theme.graph_text),
            )),
        ]));
    }

    if app.sessions.is_empty() {
        let no_active = t("context.no_active_sessions");
        rows.push(Row::new(vec![
            Cell::from(Span::styled(
                no_active,
                Style::default().fg(theme.inactive_fg),
            )),
            Cell::from(""),
            Cell::from(""),
        ]));
    }

    let header = Row::new(vec![
        Cell::from(Span::styled(project_label, header_style)),
        Cell::from(Span::styled(context_label, header_style)),
        Cell::from(Span::styled(window_label, header_style)),
    ]);

    let widths = [
        Constraint::Length(10),
        Constraint::Min(10),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths).header(header);
    f.render_widget(table, area);
}

fn aggregate_tokens_label(app: &App) -> String {
    if app.all_usage_known() {
        fmt_tokens(
            app.sessions
                .iter()
                .map(|session| session.total_tokens())
                .sum(),
        )
    } else {
        "—".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PanelVisibility;
    use crate::model::SessionTelemetry;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn process_only_context_has_no_zero_bar() {
        let mut app = App::new_pi(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        app.sessions.truncate(1);
        let session = &mut app.sessions[0];
        session.context_percent = 0.0;
        session.context_window = 0;
        session.telemetry = Some(SessionTelemetry::process_only(1));
        app.token_rate_known = false;

        let backend = TestBackend::new(80, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_context_panel(f, &app, f.area(), &app.theme))
            .unwrap();
        let text = format!("{}", terminal.backend());

        assert!(text.contains('—'), "missing unknown marker\n{text}");
        assert!(text.contains("—/min"), "missing unknown rate\n{text}");
        assert!(
            !text.contains("0%"),
            "unknown context rendered as zero\n{text}"
        );
    }

    #[test]
    fn inferred_context_tokens_without_window_show_no_meter() {
        let mut app = App::new_pi(Theme::default(), &[], PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        app.sessions.truncate(1);
        let session = &mut app.sessions[0];
        session.context_window = 0;
        let mut telemetry = SessionTelemetry::process_only(1);
        telemetry.context.precision = crate::model::TelemetryPrecision::Estimated;
        telemetry.context_details.tokens = Some(12_000);
        session.telemetry = Some(telemetry);

        let backend = TestBackend::new(140, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_context_panel(f, &app, f.area(), &app.theme))
            .unwrap();
        let text = format!("{}", terminal.backend());

        assert!(
            text.contains("≈12k tokens"),
            "missing token estimate\n{text}"
        );
        assert!(
            text.contains("window —"),
            "missing unavailable window\n{text}"
        );
        assert!(
            !text.contains("■"),
            "unknown window rendered a meter\n{text}"
        );
    }
}
