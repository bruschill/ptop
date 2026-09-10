use crate::app::App;
use crate::model::{FleetRunState, FleetTelemetry};
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{btop_block_active, fmt_tokens, truncate_str};

pub(crate) fn draw_runs_panel(f: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    let block = btop_block_active("runs", "⁷", theme.proc_box, theme, false);
    f.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let Some(session) = app.sessions.get(app.selected) else {
        return;
    };
    let Some(telemetry) = &session.telemetry else {
        return;
    };

    let mut lines = vec![Line::from(Span::styled(
        format!(
            " {}",
            truncate_str(&session.project_name, inner.width as usize)
        ),
        Style::default()
            .fg(theme.title)
            .add_modifier(Modifier::BOLD),
    ))];
    lines.extend(fleet_detail_lines(
        &telemetry.fleet,
        inner.width,
        (inner.height as usize).saturating_sub(lines.len()),
        theme,
    ));
    f.render_widget(Paragraph::new(lines), inner);
}

pub(crate) fn fleet_detail_lines(
    fleet: &FleetTelemetry,
    width: u16,
    max_lines: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    if max_lines == 0 {
        return Vec::new();
    }

    let mut lines = vec![fleet_summary_line(fleet, theme)];
    for run in &fleet.runs {
        if lines.len() >= max_lines {
            break;
        }
        let (icon, color) = fleet_state_style(run.state, theme);
        let token_label = run
            .usage
            .total_tokens
            .map(|tokens| format!(" · {} tok", fmt_tokens(tokens)))
            .unwrap_or_default();
        let terminal_label = run
            .process_terminal
            .as_ref()
            .map(|proof| format!(" · exit {}", proof.state.label()))
            .unwrap_or_default();
        let stale_label = if run.stale { " stale" } else { "" };
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {icon}{stale_label} "),
                Style::default().fg(color),
            ),
            Span::styled(
                truncate_str(&run.run_id, 12),
                Style::default().fg(theme.main_fg),
            ),
            Span::styled(
                format!(
                    " · {} · {} · {}{}{}",
                    run.state.label(),
                    run.execution.label(),
                    run.mode.label(),
                    token_label,
                    terminal_label
                ),
                Style::default().fg(theme.inactive_fg),
            ),
        ]));

        for child in &run.children {
            if lines.len() >= max_lines {
                break;
            }
            let (child_icon, child_color) = fleet_state_style(child.state, theme);
            let child_tokens = child
                .usage
                .total_tokens
                .map(|tokens| format!(" · {} tok", fmt_tokens(tokens)))
                .unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(
                    format!("    {child_icon} "),
                    Style::default().fg(child_color),
                ),
                Span::styled(
                    truncate_str(&child.name, (width as usize).saturating_sub(28).max(8)),
                    Style::default().fg(theme.graph_text),
                ),
                Span::styled(
                    format!(" · {}{}", child.state.label(), child_tokens),
                    Style::default().fg(theme.inactive_fg),
                ),
            ]));
        }
    }

    lines
}

pub(crate) fn fleet_summary_line(fleet: &FleetTelemetry, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            " Fleet ",
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{} · bg {} · fg {} · {} run{} · {}d retention",
                fleet.source_health.label(),
                fleet.background_visibility.label(),
                fleet.foreground_visibility.label(),
                fleet.runs.len(),
                if fleet.runs.len() == 1 { "" } else { "s" },
                fleet.retention_days
            ),
            Style::default().fg(theme.inactive_fg),
        ),
    ])
}

fn fleet_state_style(state: FleetRunState, theme: &Theme) -> (&'static str, Color) {
    match state {
        FleetRunState::Running => ("●", theme.main_fg),
        FleetRunState::Queued | FleetRunState::Paused => ("◌", theme.graph_text),
        FleetRunState::Complete => ("✓", theme.proc_misc),
        FleetRunState::Failed | FleetRunState::Partial | FleetRunState::Rejected => {
            ("✗", theme.hi_fg)
        }
        FleetRunState::Stopped => ("■", theme.graph_text),
        FleetRunState::Unknown => ("?", theme.inactive_fg),
    }
}

pub(crate) fn selected_has_runs(app: &App) -> bool {
    app.sessions
        .get(app.selected)
        .and_then(|session| session.telemetry.as_ref())
        .is_some_and(|telemetry| !telemetry.fleet.runs.is_empty())
}
