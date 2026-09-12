use crate::app::App;
use crate::locale::t;
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{
    braille_sparkline, btop_block_active, fmt_tokens, grad_at, make_gradient, meter_bar,
    styled_label, truncate_str,
};

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
    let selected = app.sessions.get(app.selected);
    let panel_title = if let Some(session) = selected {
        format!(
            "tokens ({}/{})",
            truncate_str(&session.project_name, 12),
            truncate_str(&session.session_id, 8)
        )
    } else {
        "tokens".to_string()
    };
    let block = btop_block_active(&panel_title, "²", theme.mem_box, theme, active);

    if selected.is_some_and(|session| session.total_tokens_value().is_none()) {
        let lines = vec![
            Line::from(vec![
                styled_label(" Total: ", theme.graph_text),
                Span::styled("—", Style::default().fg(theme.inactive_fg)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                " Session telemetry unavailable",
                Style::default().fg(theme.inactive_fg),
            )),
        ];
        f.render_widget(Paragraph::new(lines).block(block), area);
        return;
    }

    let total_in: u64 = selected.map(|s| s.total_input_tokens).unwrap_or(0);
    let total_out: u64 = selected.map(|s| s.total_output_tokens).unwrap_or(0);
    let cache_read: u64 = selected.map(|s| s.total_cache_read).unwrap_or(0);
    let cache_write: u64 = selected.map(|s| s.total_cache_create).unwrap_or(0);
    let total: u64 = total_in + total_out + cache_read + cache_write;
    let turns: u32 = selected.map(|s| s.turn_count).unwrap_or(0);
    let avg = if turns > 0 { total / turns as u64 } else { 0 };

    // Compute percentages for mini meter bars
    let (in_pct, out_pct, cache_r_pct, cache_w_pct) = if total > 0 {
        (
            total_in as f64 / total as f64 * 100.0,
            total_out as f64 / total as f64 * 100.0,
            cache_read as f64 / total as f64 * 100.0,
            cache_write as f64 / total as f64 * 100.0,
        )
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };

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
    let partial_suffix = if selected.is_some_and(|session| session.usage_is_partial()) {
        "+"
    } else {
        ""
    };
    let total_line = vec![
        styled_label(format!(" {}: ", total_label).as_str(), theme.graph_text),
        Span::styled(
            format!("{}{}", fmt_tokens(total), partial_suffix),
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    let input_label = t("tokens.input");
    let mut input_line = vec![styled_label(
        format!(" {} :", input_label).as_str(),
        theme.graph_text,
    )];
    input_line.extend(meter_bar(in_pct, bar_w, &free_grad, theme.meter_bg));
    input_line.push(Span::styled(
        format!(" {}", fmt_tokens(total_in)),
        Style::default().fg(grad_at(&free_grad, 80.0)),
    ));

    let output_label = t("tokens.output");
    let mut output_line = vec![styled_label(
        format!(" {}:", output_label).as_str(),
        theme.graph_text,
    )];
    output_line.extend(meter_bar(out_pct, bar_w, &used_grad, theme.meter_bg));
    output_line.push(Span::styled(
        format!(" {}", fmt_tokens(total_out)),
        Style::default().fg(grad_at(&used_grad, 80.0)),
    ));

    let cache_r_label = t("tokens.cache_r");
    let mut cache_r_line = vec![styled_label(
        format!(" {}:", cache_r_label).as_str(),
        theme.graph_text,
    )];
    cache_r_line.extend(meter_bar(cache_r_pct, bar_w, &cached_grad, theme.meter_bg));
    cache_r_line.push(Span::styled(
        format!(" {}", fmt_tokens(cache_read)),
        Style::default().fg(grad_at(&cached_grad, 80.0)),
    ));

    let cache_w_label = t("tokens.cache_w");
    let mut cache_w_line = vec![styled_label(
        format!(" {}:", cache_w_label).as_str(),
        theme.graph_text,
    )];
    cache_w_line.extend(meter_bar(cache_w_pct, bar_w, &cached_grad, theme.meter_bg));
    cache_w_line.push(Span::styled(
        format!(" {}", fmt_tokens(cache_write)),
        Style::default().fg(grad_at(&cached_grad, 80.0)),
    ));

    // Per-turn sparkline from selected session's token_history
    let cpu_grad = make_gradient(theme.cpu_grad.start, theme.cpu_grad.mid, theme.cpu_grad.end);
    let all_history: Vec<u64> = app
        .sessions
        .get(app.selected)
        .map(|s| s.token_history.clone())
        .unwrap_or_default();
    let spark_w = (area.width as usize).saturating_sub(16).clamp(5, 20);
    let max_val = all_history.iter().copied().max().unwrap_or(1).max(1);
    let normalized: Vec<f64> = all_history
        .iter()
        .map(|&v| v as f64 / max_val as f64)
        .collect();
    let mut spark_line_spans = vec![styled_label(" ", theme.graph_text)];
    spark_line_spans.extend(braille_sparkline(
        &normalized,
        spark_w,
        &cpu_grad,
        theme.graph_text,
    ));
    let tokens_turn_label = t("tokens.tokens_turn");
    spark_line_spans.push(Span::styled(
        format!(" {}", tokens_turn_label),
        Style::default().fg(theme.graph_text),
    ));

    let turns_label = t("tokens.turns");
    let avg_label = t("tokens.avg");
    let lines = vec![
        Line::from(total_line),
        Line::from(input_line),
        Line::from(output_line),
        Line::from(cache_r_line),
        Line::from(cache_w_line),
        Line::from(spark_line_spans),
        Line::from(vec![
            styled_label(format!(" {}: ", turns_label).as_str(), theme.graph_text),
            Span::styled(format!("{}", turns), Style::default().fg(theme.main_fg)),
            styled_label(format!("  {}: ", avg_label).as_str(), theme.graph_text),
            Span::styled(
                format!("{}/t", fmt_tokens(avg)),
                Style::default().fg(theme.graph_text),
            ),
        ]),
    ];

    f.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PanelVisibility;
    use crate::model::SessionTelemetry;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn process_only_usage_has_no_zero_totals_or_bars() {
        let mut app = App::new(Theme::default(), PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        app.sessions.truncate(1);
        let session = &mut app.sessions[0];
        session.total_input_tokens = 0;
        session.total_output_tokens = 0;
        session.total_cache_read = 0;
        session.total_cache_create = 0;
        session.telemetry = Some(SessionTelemetry::process_only(1));

        let backend = TestBackend::new(50, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_tokens_panel(f, &app, f.area(), &app.theme))
            .unwrap();
        let text = format!("{}", terminal.backend());

        assert!(text.contains("Total: —"), "missing unknown total\n{text}");
        assert!(
            text.contains("Session telemetry unavailable"),
            "missing source state\n{text}"
        );
        assert!(
            !text.contains('■'),
            "unknown usage rendered as a bar\n{text}"
        );
    }
}
