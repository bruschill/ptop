use crate::app::App;
use crate::locale::t;
use crate::model::{FleetAvailability, FleetRunState, FleetTelemetry, SourceHealth};
use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{btop_block_active, fmt_tokens, truncate_str};

pub(crate) fn draw_runs_panel(f: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    draw_runs_panel_impl(f, app, area, theme, false);
}

pub(crate) fn draw_runs_panel_active(
    f: &mut Frame,
    app: &App,
    area: Rect,
    theme: &Theme,
    active: bool,
) {
    draw_runs_panel_impl(f, app, area, theme, active);
}

fn draw_runs_panel_impl(f: &mut Frame, app: &App, area: Rect, theme: &Theme, active: bool) {
    let block = btop_block_active(&t("runs.title"), "⁶", theme.proc_box, theme, active);
    f.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let lines = runs_lines(
        app.sessions.get(app.selected),
        inner.width,
        inner.height as usize,
        theme,
        true,
    );
    f.render_widget(Paragraph::new(lines), inner);
}

pub(crate) fn runs_lines(
    session: Option<&crate::model::AgentSession>,
    width: u16,
    max_lines: usize,
    theme: &Theme,
    include_project: bool,
) -> Vec<Line<'static>> {
    if max_lines == 0 {
        return Vec::new();
    }
    let Some(session) = session else {
        return vec![Line::from(Span::styled(
            t("runs.no_session_selected"),
            Style::default().fg(theme.inactive_fg),
        ))];
    };
    let Some(telemetry) = &session.telemetry else {
        return vec![Line::from(Span::styled(
            t("runs.telemetry_unavailable"),
            Style::default().fg(theme.inactive_fg),
        ))];
    };
    let mut lines = Vec::new();
    if include_project {
        lines.push(Line::from(Span::styled(
            format!(" {}", truncate_str(&session.project_name, width as usize)),
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD),
        )));
    }
    if telemetry.fleet.runs.is_empty() {
        if lines.len() < max_lines {
            lines.push(Line::from(Span::styled(
                format!(" {}", fleet_empty_state(&telemetry.fleet)),
                Style::default().fg(theme.inactive_fg),
            )));
        }
    } else {
        lines.extend(fleet_detail_lines(
            &telemetry.fleet,
            width,
            max_lines.saturating_sub(lines.len()),
            theme,
        ));
    }
    lines.truncate(max_lines);
    lines
}

fn fleet_empty_state(fleet: &FleetTelemetry) -> String {
    if fleet.availability == FleetAvailability::NotInstalled {
        return t("runs.not_installed");
    }
    if fleet.source_health != SourceHealth::Healthy {
        return t("runs.telemetry_unavailable");
    }
    match fleet.availability {
        FleetAvailability::Installed if fleet.unsupported_statuses > 0 => {
            t("runs.unsupported_lifecycle")
        }
        FleetAvailability::Installed => t("runs.no_runs"),
        FleetAvailability::NotInstalled | FleetAvailability::Unknown => {
            t("runs.telemetry_unavailable")
        }
    }
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

    let mut lines = Vec::new();
    if fleet.unsupported_statuses > 0 && fleet.source_health == SourceHealth::Healthy {
        lines.push(Line::from(Span::styled(
            format!(" {}", t("runs.unsupported_lifecycle")),
            Style::default().fg(theme.hi_fg),
        )));
    }
    if lines.len() < max_lines {
        lines.push(fleet_summary_line(fleet, theme));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_session_selected_is_distinct_from_telemetry_unavailable() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let app = App::new(Theme::default(), crate::config::PanelVisibility::default());
        let mut terminal = Terminal::new(TestBackend::new(50, 8)).unwrap();
        terminal
            .draw(|frame| draw_runs_panel(frame, &app, Rect::new(0, 0, 50, 8), &app.theme))
            .unwrap();
        assert!(format!("{}", terminal.backend()).contains("no Pi session selected"));
    }

    #[test]
    fn unsupported_warning_precedes_identity_rows_but_errors_outrank_it() {
        let mut fleet = FleetTelemetry::unavailable(1, "test");
        fleet.availability = FleetAvailability::Installed;
        fleet.source_health = SourceHealth::Healthy;
        fleet.unsupported_statuses = 1;
        let lines = fleet_detail_lines(&fleet, 80, 2, &Theme::default());
        assert_eq!(
            lines[0].to_string(),
            " unsupported pi-subagents lifecycle version"
        );

        fleet.source_health = SourceHealth::Error;
        let lines = fleet_detail_lines(&fleet, 80, 2, &Theme::default());
        assert!(!lines[0]
            .to_string()
            .contains("unsupported pi-subagents lifecycle version"));
    }

    #[test]
    fn empty_states_follow_truthful_fleet_fields() {
        let mut fleet = FleetTelemetry::unavailable(1, "test");
        fleet.availability = FleetAvailability::NotInstalled;
        assert_eq!(fleet_empty_state(&fleet), "pi-subagents is not installed");

        fleet.availability = FleetAvailability::Installed;
        fleet.source_health = SourceHealth::Healthy;
        assert_eq!(
            fleet_empty_state(&fleet),
            "pi-subagents is installed; no runs for this session"
        );

        fleet.unsupported_statuses = 1;
        assert_eq!(
            fleet_empty_state(&fleet),
            "unsupported pi-subagents lifecycle version"
        );

        fleet.unsupported_statuses = 0;
        fleet.source_health = SourceHealth::Stale;
        assert_eq!(
            fleet_empty_state(&fleet),
            "pi-subagents telemetry is unavailable or stale"
        );
    }
}
