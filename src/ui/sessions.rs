use crate::app::App;
use crate::locale::t;
use crate::model::AgentSession;
use crate::theme::Theme;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Clear, Paragraph, Row, Table};
use ratatui::Frame;

use super::{btop_block_active, fmt_age, fmt_tokens, grad_at, make_gradient, truncate_str};

#[cfg(test)]
pub(crate) fn draw_sessions_panel(f: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    draw_sessions_panel_impl(f, app, area, theme, false, false);
}

pub(crate) fn draw_sessions_panel_with_promoted_runs(
    f: &mut Frame,
    app: &App,
    area: Rect,
    theme: &Theme,
    runs_promoted: bool,
) {
    draw_sessions_panel_impl(f, app, area, theme, false, runs_promoted);
}

pub(crate) fn draw_sessions_panel_active(
    f: &mut Frame,
    app: &App,
    area: Rect,
    theme: &Theme,
    active: bool,
) {
    draw_sessions_panel_impl(f, app, area, theme, active, false);
}

fn draw_sessions_panel_impl(
    f: &mut Frame,
    app: &App,
    area: Rect,
    theme: &Theme,
    active: bool,
    runs_promoted: bool,
) {
    // Render the outer block
    let block = btop_block_active("sessions", "⁵", theme.proc_box, theme, active);
    f.render_widget(block, area);

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let visible = app.visible_indices();
    let session_rows = visible.len() as u16 * 2;
    let detail_reserve: u16 = if inner.height <= 12 {
        6.min(inner.height.saturating_sub(3))
    } else {
        10.min(inner.height / 2)
    };
    let max_table = inner.height.saturating_sub(detail_reserve);
    let table_h = (1 + session_rows).min(max_table);

    let panel_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(table_h),
            Constraint::Length(1), // separator line
            Constraint::Min(0),
        ])
        .split(inner);

    // Draw separator line between session list and detail
    {
        let sep_area = panel_chunks[1];
        let sep_line = "─".repeat(sep_area.width as usize);
        f.render_widget(
            Paragraph::new(Span::styled(sep_line, Style::default().fg(theme.proc_box))),
            sep_area,
        );
    }

    // ── Session list table ──
    let proc_grad = make_gradient(
        theme.proc_grad.start,
        theme.proc_grad.mid,
        theme.proc_grad.end,
    );
    let mut rows = Vec::new();

    // Responsive columns: keep the identity, current task, status, and context
    // visible first; add lower-value columns back as width allows.
    let w = inner.width;
    let show_pid = w >= 120;
    let show_session_id = w >= 76;
    let show_model = w >= 90;
    let show_tokens = w >= 86;
    let show_memory = w >= 110;
    let show_turn = w >= 110;

    let project_w: u16 = if w >= 120 {
        14
    } else if w >= 80 {
        10
    } else {
        8
    };
    let session_w: u16 = if w >= 110 { 9 } else { 5 };
    let session_label = if w >= 110 {
        t("col.session")
    } else {
        t("col.sess")
    };
    let status_w: u16 = if w >= 100 {
        8
    } else if w >= 72 {
        6
    } else {
        3
    };
    let model_w: u16 = if w >= 110 { 13 } else { 10 };
    let context_w: u16 = if w >= 100 { 7 } else { 4 };
    let context_label = if w >= 100 {
        t("col.context")
    } else {
        t("col.ctx")
    };
    let tokens_w: u16 = if w >= 100 { 7 } else { 5 };

    let visible = app.visible_indices();
    for &i in &visible {
        let session = &app.sessions[i];
        let selected = i == app.selected;
        let marker = if selected { "►" } else { " " };

        let (status_icon_str, status_color) = match &session.status {
            crate::model::SessionStatus::Thinking => (t("sess.think"), theme.proc_misc),
            crate::model::SessionStatus::Executing => (t("sess.exec"), theme.hi_fg),
            crate::model::SessionStatus::Waiting => (t("sess.wait"), grad_at(&proc_grad, 50.0)),
            crate::model::SessionStatus::Unknown => (t("sess.unknown"), theme.inactive_fg),
            crate::model::SessionStatus::Done => (t("sess.done"), theme.inactive_fg),
        };

        let is_1m = session.context_window >= 1_000_000 || session.model.contains("[1m]");
        let model_short = shorten_model(&session.model, is_1m);
        let (ctx_text, ctx_color) = if let Some(percent) = session.context_value() {
            (
                format!("{}{:.0}%", session.context_precision().prefix(), percent),
                grad_at(&proc_grad, percent),
            )
        } else {
            ("—".to_string(), theme.inactive_fg)
        };

        let is_done = matches!(session.status, crate::model::SessionStatus::Done);
        let row_style = if selected {
            Style::default()
                .bg(theme.selected_bg)
                .fg(theme.selected_fg)
                .add_modifier(Modifier::BOLD)
        } else if is_done {
            Style::default().fg(theme.inactive_fg)
        } else {
            Style::default()
        };

        let sid_short = if session.session_id.len() >= 8 {
            &session.session_id[..8]
        } else {
            &session.session_id
        };

        let summary_col = app.session_summary(session);

        let mut cells = vec![
            Cell::from(Span::styled(marker, Style::default().fg(theme.hi_fg))),
            Cell::from(Span::styled("πPI", Style::default().fg(theme.pi_agent))),
        ];
        if show_pid {
            cells.push(Cell::from(Span::styled(
                format!("{}", session.pid),
                Style::default().fg(theme.inactive_fg),
            )));
        }
        cells.push(Cell::from(Span::styled(
            truncate_str(&session.project_name, project_w as usize),
            Style::default().fg(theme.title),
        )));
        if show_session_id {
            cells.push(Cell::from(Span::styled(
                truncate_str(sid_short, session_w as usize),
                Style::default().fg(theme.session_id),
            )));
        }
        cells.extend([
            Cell::from(Span::styled(
                truncate_str(&summary_col, w.saturating_sub(24) as usize),
                Style::default().fg(theme.main_fg),
            )),
            Cell::from(Span::styled(
                truncate_str(&status_icon_str, status_w as usize),
                Style::default().fg(status_color),
            )),
        ]);
        if show_model {
            cells.push(Cell::from(Span::styled(
                truncate_str(&model_short, model_w as usize),
                Style::default().fg(if model_short == "-" {
                    theme.inactive_fg
                } else {
                    theme.graph_text
                }),
            )));
        }
        cells.push(Cell::from(Span::styled(
            ctx_text,
            Style::default().fg(ctx_color),
        )));
        if show_tokens {
            let (tokens, color) = match session.total_tokens_value() {
                Some(total) => (
                    format!(
                        "{}{}{}",
                        session.usage_precision().prefix(),
                        fmt_tokens(total),
                        if session.usage_is_partial() { "+" } else { "" }
                    ),
                    theme.main_fg,
                ),
                None => ("—".to_string(), theme.inactive_fg),
            };
            cells.push(Cell::from(Span::styled(tokens, Style::default().fg(color))));
        }
        if show_memory {
            cells.push(Cell::from(Span::styled(
                if session.mem_mb > 0 {
                    format!("{}M", session.mem_mb)
                } else {
                    "—".into()
                },
                Style::default().fg(theme.graph_text),
            )));
        }
        if show_turn {
            cells.push(Cell::from(Span::styled(
                format!("{}", session.turn_count),
                Style::default().fg(theme.graph_text),
            )));
        }

        rows.push(Row::new(cells).style(row_style).height(1));

        // 2nd line: task text in Summary column
        let summary_idx = 3 + show_pid as usize + show_session_id as usize;
        let total_cols = 6
            + show_pid as usize
            + show_session_id as usize
            + show_model as usize
            + show_tokens as usize
            + show_memory as usize
            + show_turn as usize;
        let task_cells: Vec<Cell> = (0..total_cols)
            .map(|j| {
                if j == summary_idx {
                    let task_text = session
                        .current_tasks
                        .last()
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    Cell::from(Span::styled(
                        task_row_text(task_text, w.saturating_sub(24) as usize),
                        Style::default().fg(theme.graph_text),
                    ))
                } else {
                    Cell::from("")
                }
            })
            .collect();
        rows.push(Row::new(task_cells).height(1));
    }

    let header_style = Style::default()
        .fg(theme.main_fg)
        .add_modifier(Modifier::BOLD);
    let mut header_cells = vec![
        Cell::from(""),
        Cell::from(Span::styled(t("col.ai"), header_style)),
    ];
    if show_pid {
        header_cells.push(Cell::from(Span::styled(t("col.pid"), header_style)));
    }
    header_cells.push(Cell::from(Span::styled(t("col.project"), header_style)));
    if show_session_id {
        header_cells.push(Cell::from(Span::styled(session_label, header_style)));
    }
    header_cells.extend([
        Cell::from(Span::styled(t("col.summary"), header_style)),
        Cell::from(Span::styled(t("col.status"), header_style)),
    ]);
    if show_model {
        header_cells.push(Cell::from(Span::styled(t("col.model"), header_style)));
    }
    header_cells.push(Cell::from(Span::styled(context_label, header_style)));
    if show_tokens {
        header_cells.push(Cell::from(Span::styled(t("col.tokens"), header_style)));
    }
    if show_memory {
        header_cells.push(Cell::from(Span::styled(t("col.memory"), header_style)));
    }
    if show_turn {
        header_cells.push(Cell::from(Span::styled(t("col.turn"), header_style)));
    }
    let header = Row::new(header_cells).height(1);

    let mut widths_vec: Vec<Constraint> = vec![
        Constraint::Length(1), // marker
        Constraint::Length(3), // agent label
    ];
    if show_pid {
        widths_vec.push(Constraint::Length(6)); // pid
    }
    widths_vec.push(Constraint::Length(project_w)); // project
    if show_session_id {
        widths_vec.push(Constraint::Length(session_w)); // session id
    }
    widths_vec.push(Constraint::Fill(1)); // summary (fills remaining)
    widths_vec.push(Constraint::Length(status_w)); // status
    if show_model {
        widths_vec.push(Constraint::Length(model_w)); // model
    }
    widths_vec.push(Constraint::Length(context_w)); // context
    if show_tokens {
        widths_vec.push(Constraint::Length(tokens_w)); // tokens
    }
    if show_memory {
        widths_vec.push(Constraint::Length(8)); // memory
    }
    if show_turn {
        widths_vec.push(Constraint::Length(4)); // turn
    }

    // Scroll using the built row list as the source of truth.
    let visible_sessions = app.visible_indices();
    let total_rows = rows.len();
    let needs_scroll = total_rows > panel_chunks[0].height.saturating_sub(1) as usize;

    // Split table area into [table | scrollbar(1)] when scrollable
    let table_area;
    let scrollbar_area: Option<Rect>;
    if needs_scroll && panel_chunks[0].width > 2 {
        let hsplit = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(panel_chunks[0]);
        table_area = hsplit[0];
        scrollbar_area = Some(hsplit[1]);
    } else {
        table_area = panel_chunks[0];
        scrollbar_area = None;
    }

    // One row is reserved for the header.
    let visible_rows = table_area.height.saturating_sub(1) as usize;
    // Account for filter-hidden sessions above the selected row.
    let selected_pos = visible_sessions
        .iter()
        .position(|&i| i == app.selected)
        .unwrap_or(0);
    let selected_row_start = selected_pos * 2;
    let selected_session_rows = 2;
    let selected_row_end = selected_row_start + selected_session_rows;
    let scroll_offset = selected_row_end.saturating_sub(visible_rows);
    let visible = if scroll_offset < rows.len() {
        rows.into_iter().skip(scroll_offset).collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    f.render_widget(Clear, table_area);
    let table = Table::new(visible, widths_vec).header(header);
    f.render_widget(table, table_area);

    // ── Scrollbar column (dedicated 1-char width, btop-style) ──
    if let Some(sb) = scrollbar_area {
        let bar_h = sb.height as usize;
        if bar_h > 0 {
            let thumb_size = ((visible_rows as f64 / total_rows as f64) * bar_h as f64)
                .ceil()
                .max(1.0) as usize;
            let thumb_size = thumb_size.min(bar_h);
            let thumb_pos = if total_rows > visible_rows {
                ((scroll_offset as f64 / (total_rows - visible_rows) as f64)
                    * (bar_h - thumb_size) as f64)
                    .round() as usize
            } else {
                0
            };

            let buf = f.buffer_mut();
            for i in 0..bar_h {
                let y = sb.y + i as u16;
                let (ch, color) = if i >= thumb_pos && i < thumb_pos + thumb_size {
                    ("┃", theme.main_fg)
                } else {
                    ("│", theme.div_line)
                };
                buf[(sb.x, y)].set_symbol(ch).set_fg(color);
            }

            // ↑/↓ arrows at edges when more content exists
            if scroll_offset > 0 {
                buf[(sb.x, sb.y)].set_symbol("↑").set_fg(theme.proc_box);
            }
            if scroll_offset + visible_rows < total_rows {
                buf[(sb.x, sb.y + sb.height - 1)]
                    .set_symbol("↓")
                    .set_fg(theme.proc_box);
            }
        }
    }

    // ── Selected Pi session detail ──
    if let Some(session) = app.sessions.get(app.selected) {
        let detail_area = panel_chunks[2];
        if detail_area.height < 2 {
            return;
        }
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(detail_area);
        let sid = if parts[0].width <= 80 {
            if session.session_id.len() >= 8 {
                &session.session_id[..8]
            } else {
                &session.session_id
            }
        } else {
            &session.session_id
        };
        let session_ref = if parts[0].width <= 80 {
            format!("►{sid} · {}", session.project_name)
        } else {
            format!("►{sid} · {}", session.cwd)
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate_str(
                    &format!(" {} ({session_ref})", t("detail.session").as_str()),
                    parts[0].width as usize,
                ),
                Style::default()
                    .fg(theme.title)
                    .add_modifier(Modifier::BOLD),
            ))),
            parts[0],
        );
        draw_pi_metadata(f, session, parts[1], theme, !runs_promoted);
    }
}

fn telemetry_observed_age(observed_at_ms: u64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    fmt_age(now_ms.saturating_sub(observed_at_ms) / 1_000)
}

fn telemetry_metadata_line(
    label: &str,
    value: String,
    metadata: &crate::model::TelemetryMetadata,
    theme: &Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {label} "), Style::default().fg(theme.graph_text)),
        Span::styled(value, Style::default().fg(theme.inactive_fg)),
        Span::styled(
            format!(
                " · {}/{} · {}",
                metadata.precision.label(),
                metadata.completeness.label(),
                metadata.provenance
            ),
            Style::default().fg(theme.inactive_fg),
        ),
    ])
}

fn draw_pi_metadata(
    f: &mut Frame,
    session: &AgentSession,
    area: Rect,
    theme: &Theme,
    show_runs: bool,
) {
    let Some(telemetry) = &session.telemetry else {
        return;
    };
    let identity = if session.process_start_id.is_some() {
        "PID + process start"
    } else {
        "PID only · reuse not guarded"
    };
    let context = session
        .context_value()
        .map(|percent| format!("{}{percent:.0}%", session.context_precision().prefix()))
        .or_else(|| {
            session.context_tokens_without_window().map(|tokens| {
                format!(
                    "{}{} tokens (window —)",
                    session.context_precision().prefix(),
                    fmt_tokens(tokens)
                )
            })
        })
        .unwrap_or_else(|| "—".to_string());
    let compact_for_runs = show_runs && !telemetry.fleet.runs.is_empty() && area.height <= 6;
    let tokens = session
        .total_tokens_value()
        .map(|total| {
            format!(
                "{}{}",
                session.usage_precision().prefix(),
                fmt_tokens(total)
            )
        })
        .unwrap_or_else(|| "—".to_string());
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" Attachment ", Style::default().fg(theme.graph_text)),
            Span::styled(
                telemetry.attachment.label(),
                Style::default().fg(theme.main_fg),
            ),
            Span::styled(" · confidence ", Style::default().fg(theme.graph_text)),
            Span::styled(
                telemetry.attachment_confidence.label(),
                Style::default().fg(theme.inactive_fg),
            ),
        ]),
        Line::from(vec![
            Span::styled(" Source ", Style::default().fg(theme.graph_text)),
            Span::styled(
                telemetry.source_health.label(),
                Style::default().fg(theme.inactive_fg),
            ),
            Span::styled(
                format!(
                    " · observed {}",
                    telemetry_observed_age(telemetry.context.observed_at_ms)
                ),
                Style::default().fg(theme.inactive_fg),
            ),
        ]),
        telemetry_metadata_line("Context", context.clone(), &telemetry.context, theme),
        telemetry_metadata_line(
            "Tokens",
            format!(
                "{}{}",
                tokens,
                if session.usage_is_partial() { "+" } else { "" }
            ),
            &telemetry.usage,
            theme,
        ),
        Line::from(Span::styled(
            format!(" Identity {identity}"),
            Style::default().fg(theme.inactive_fg),
        )),
    ];

    if compact_for_runs {
        lines = vec![
            Line::from(vec![
                Span::styled(" Attachment ", Style::default().fg(theme.graph_text)),
                Span::styled(
                    telemetry.attachment.label(),
                    Style::default().fg(theme.main_fg),
                ),
                Span::styled(" · source ", Style::default().fg(theme.graph_text)),
                Span::styled(
                    telemetry.source_health.label(),
                    Style::default().fg(theme.inactive_fg),
                ),
            ]),
            Line::from(vec![
                Span::styled(" Context ", Style::default().fg(theme.graph_text)),
                Span::styled(context, Style::default().fg(theme.inactive_fg)),
                Span::styled(" · Tokens ", Style::default().fg(theme.graph_text)),
                Span::styled(
                    format!(
                        "{}{}",
                        tokens,
                        if session.usage_is_partial() { "+" } else { "" }
                    ),
                    Style::default().fg(theme.inactive_fg),
                ),
            ]),
            Line::from(Span::styled(
                format!(" Identity {identity}"),
                Style::default().fg(theme.inactive_fg),
            )),
        ];
    }

    if !compact_for_runs {
        if let Some(provider) = &telemetry.context_details.provider {
            lines.push(Line::from(Span::styled(
                format!(" Provider/model: {}/{}", provider, session.model),
                Style::default().fg(theme.inactive_fg),
            )));
        }
        if let Some(reason) = &telemetry.context_details.reason {
            lines.push(Line::from(Span::styled(
                format!(
                    " Context note: {}",
                    truncate_str(reason, area.width as usize)
                ),
                Style::default().fg(theme.inactive_fg),
            )));
        }
        if let Some(cost) = telemetry.usage_details.reported_cost {
            lines.push(Line::from(Span::styled(
                format!(" Reported cost: {cost:.4}"),
                Style::default().fg(theme.inactive_fg),
            )));
        }
    }

    if lines.len() < area.height as usize {
        let fleet = &telemetry.fleet;
        if show_runs {
            lines.extend(super::runs::fleet_detail_lines(
                fleet,
                area.width,
                (area.height as usize).saturating_sub(lines.len()),
                theme,
            ));
        } else {
            lines.push(super::runs::fleet_summary_line(fleet, theme));
        }
    }

    if !session.children.is_empty() && lines.len() < area.height as usize {
        lines.push(Line::from(Span::styled(
            " Children",
            Style::default()
                .fg(theme.title)
                .add_modifier(Modifier::BOLD),
        )));
        for child in session
            .children
            .iter()
            .take((area.height as usize).saturating_sub(lines.len()))
        {
            let command = crate::model::safe_process_label(&child.command);
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {:<6}", child.pid),
                    Style::default().fg(theme.main_fg),
                ),
                Span::styled(
                    truncate_str(&command, (area.width as usize).saturating_sub(16)),
                    Style::default().fg(theme.graph_text),
                ),
            ]));
        }
    }

    f.render_widget(Paragraph::new(lines), area);
}

fn task_row_text(task_text: &str, max_width: usize) -> String {
    truncate_str(&format!("└─ {task_text}"), max_width)
}

pub(crate) fn shorten_model(model: &str, is_1m: bool) -> String {
    // Pi may report model identifiers such as "claude-opus-4-6"; display them compactly.
    let s = model.strip_prefix("claude-").unwrap_or(model);
    let s = s.trim_end_matches("[1m]");
    // Extract name and version: "opus-4-6" → ("opus", "4.6")
    let base = if let Some(pos) = s.find(|c: char| c.is_ascii_digit()) {
        let name = s[..pos].trim_end_matches('-');
        let ver = s[pos..].replace('-', ".");
        format!("{}{}", name, ver)
    } else {
        s.to_string()
    };
    if is_1m {
        format!("{}[1m]", base)
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PanelVisibility;
    use crate::model::{
        FleetExecution, FleetRun, FleetRunMode, FleetRunState, FleetTelemetry, FleetUsage,
        SessionStatus, SourceHealth, TelemetryCompleteness, TelemetryPrecision,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn non_1m_context_window_does_not_show_1m_suffix() {
        let mut app = App::new(Theme::default(), PanelVisibility::default());
        app.sessions.push(AgentSession {
            pid: 42,
            session_id: "pi-session".into(),
            cwd: "/tmp/project".into(),
            project_name: "project".into(),
            started_at: 0,
            status: SessionStatus::Waiting,
            model: "gpt-5".into(),
            effort: String::new(),
            context_percent: 58.7,
            total_input_tokens: 1_000,
            total_output_tokens: 500,
            total_cache_read: 0,
            total_cache_create: 0,
            turn_count: 1,
            current_tasks: vec!["waiting for input".into()],
            mem_mb: 0,
            version: String::new(),
            git_branch: String::new(),
            git_added: 0,
            git_modified: 0,
            token_history: Vec::new(),
            context_history: Vec::new(),
            compaction_count: 0,
            context_window: 258_400,
            children: Vec::new(),
            telemetry: None,
            process_start_id: None,
        });

        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                draw_sessions_panel(
                    f,
                    &app,
                    Rect {
                        x: 0,
                        y: 0,
                        width: 120,
                        height: 20,
                    },
                    &app.theme,
                )
            })
            .unwrap();
        let text = format!("{}", terminal.backend());

        assert!(
            text.contains("gpt5"),
            "model should render in session row\n{text}"
        );
        assert!(
            !text.contains("[1m]"),
            "non-1M context windows must not be labeled as 1M\n{text}"
        );
    }

    #[test]
    fn task_row_text_respects_terminal_display_width() {
        assert_eq!(task_row_text("ＡＢＣＤ", 6), "└─ Ａ…");
    }

    #[test]
    fn session_table_clears_rows_when_selection_scrolls() {
        let mut app = App::new(Theme::default(), PanelVisibility::default());
        app.sessions = vec![
            test_session("first111", "first"),
            test_session("second22", "second"),
            test_session("third333", "third"),
            test_session("fourth44", "fourth"),
        ];
        app.selected = 3;

        let backend = TestBackend::new(120, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let area = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 14,
        };

        terminal
            .draw(|f| {
                let lines = (0..area.height)
                    .map(|_| Line::from("STALE".repeat(24)))
                    .collect::<Vec<_>>();
                f.render_widget(Paragraph::new(lines), area);
            })
            .unwrap();

        terminal
            .draw(|f| draw_sessions_panel(f, &app, area, &app.theme))
            .unwrap();

        app.selected = 2;
        terminal
            .draw(|f| draw_sessions_panel(f, &app, area, &app.theme))
            .unwrap();

        let text = format!("{}", terminal.backend());
        assert!(
            !text.contains("STALE") && !text.contains("fourth44"),
            "offscreen session row should be cleared after selection scroll\n{text}"
        );
    }

    #[test]
    fn session_table_repaints_selection_marker_when_moving() {
        let mut app = App::new(Theme::default(), PanelVisibility::default());
        app.sessions = vec![
            test_session("first111", "first"),
            test_session("second22", "second"),
        ];

        let backend = TestBackend::new(120, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let area = Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 14,
        };

        app.selected = 0;
        terminal
            .draw(|f| draw_sessions_panel(f, &app, area, &app.theme))
            .unwrap();

        app.selected = 1;
        terminal
            .draw(|f| draw_sessions_panel(f, &app, area, &app.theme))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(1, 2)].symbol(), " ");
        assert_eq!(buffer[(1, 4)].symbol(), "►");
    }

    #[test]
    fn process_only_pi_session_renders_unknown_context_and_tokens() {
        let mut app = App::new(Theme::default(), PanelVisibility::default());
        let mut session = test_session("process-42", "project");
        session.pid = 42;
        session.context_percent = 0.0;
        session.context_window = 0;
        session.total_input_tokens = 0;
        session.total_output_tokens = 0;
        session.total_cache_read = 0;
        session.total_cache_create = 0;
        session.telemetry = Some(crate::model::SessionTelemetry::process_only(123));
        session.children = vec![crate::model::ChildProcess {
            pid: 99,
            command: "node --task private-prompt".to_string(),
            mem_kb: 1,
            port: None,
        }];
        app.sessions.push(session);

        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_sessions_panel(f, &app, f.area(), &app.theme))
            .unwrap();
        let text = format!("{}", terminal.backend());

        assert!(
            !text.contains("0%"),
            "unknown context rendered as zero\n{text}"
        );
        assert!(
            text.contains("process only"),
            "missing attachment state\n{text}"
        );
        assert!(
            text.contains("Context —"),
            "missing unknown context\n{text}"
        );
        assert!(text.contains("Tokens —"), "missing unknown usage\n{text}");
        assert!(
            text.contains("unknown/unknown · process"),
            "missing precision, completeness, or provenance\n{text}"
        );
        assert!(text.contains("node"), "missing safe child label\n{text}");
        assert!(
            !text.contains("private-prompt"),
            "child arguments leaked\n{text}"
        );
    }

    #[test]
    fn pi_session_table_distinguishes_telemetry_precision_and_known_zero() {
        let mut app = App::new(Theme::default(), PanelVisibility::default());
        for (id, precision, completeness, percent, tokens, window) in [
            (
                "unknown",
                TelemetryPrecision::Unknown,
                TelemetryCompleteness::Unknown,
                0.0,
                0,
                0,
            ),
            (
                "known-zero",
                TelemetryPrecision::Exact,
                TelemetryCompleteness::Complete,
                0.0,
                0,
                200_000,
            ),
            (
                "inferred",
                TelemetryPrecision::Inferred,
                TelemetryCompleteness::Complete,
                42.0,
                14,
                200_000,
            ),
            (
                "estimated",
                TelemetryPrecision::Estimated,
                TelemetryCompleteness::Partial,
                43.0,
                15,
                200_000,
            ),
        ] {
            let mut session = test_session(id, id);
            session.context_percent = percent;
            session.context_window = window;
            session.total_input_tokens = tokens;
            session.total_output_tokens = 0;
            session.total_cache_read = 0;
            session.total_cache_create = 0;
            let mut telemetry = crate::model::SessionTelemetry::process_only(123);
            telemetry.context.precision = precision;
            telemetry.context.completeness = completeness;
            telemetry.context_details.tokens =
                (precision != TelemetryPrecision::Unknown).then_some(tokens);
            telemetry.usage.precision = precision;
            telemetry.usage.completeness = completeness;
            session.telemetry = Some(telemetry);
            app.sessions.push(session);
        }

        let backend = TestBackend::new(160, 32);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_sessions_panel(f, &app, f.area(), &app.theme))
            .unwrap();
        let text = format!("{}", terminal.backend());

        assert!(text.contains("0%"), "known zero context is missing\n{text}");
        assert!(text.contains("~42%"), "inferred context is missing\n{text}");
        assert!(
            text.contains("≈43%"),
            "estimated context is missing\n{text}"
        );
        assert!(
            text.contains("≈15+"),
            "partial usage marker is missing\n{text}"
        );
        assert!(
            text.contains("Context —"),
            "unknown context is missing\n{text}"
        );
    }

    #[test]
    fn pi_fleet_runs_render_in_selected_session_detail_at_narrow_width() {
        let mut app = App::new(Theme::default(), PanelVisibility::default());
        let mut session = test_session("parent-session", "project");
        session.telemetry = Some(crate::model::SessionTelemetry::process_only(123));
        let telemetry = session.telemetry.as_mut().unwrap();
        let mut fleet = FleetTelemetry::unavailable(123, "test");
        fleet.source_health = SourceHealth::Healthy;
        fleet.reason = None;
        let mut usage = FleetUsage::separate_run_aggregate();
        usage.total_tokens = Some(42);
        fleet.runs.push(FleetRun {
            lifecycle_version: Some(3),
            run_id: "run-1".to_string(),
            parent_run_id: None,
            nested: false,
            mode: FleetRunMode::Workflow,
            state: FleetRunState::Running,
            execution: FleetExecution::Background,
            runner_pid: None,
            started_at_ms: Some(100),
            updated_at_ms: Some(120),
            ended_at_ms: None,
            source_updated_at_ms: 120,
            stale: false,
            process_terminal: None,
            usage,
            children: Vec::new(),
            omitted_children: 0,
            reason: None,
        });
        telemetry.fleet = fleet;
        app.sessions.push(session);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| draw_sessions_panel(f, &app, f.area(), &app.theme))
            .unwrap();
        let text = format!("{}", terminal.backend());

        assert!(
            text.contains("Fleet healthy · bg supported · fg unavailable · 1 run"),
            "{text}"
        );
        assert!(text.contains("run-1"), "{text}");
        assert!(text.contains("running · background · workflow"), "{text}");
        assert_eq!(
            app.sessions.len(),
            1,
            "fleet metadata must not add process rows"
        );
    }

    fn test_session(session_id: &str, project_name: &str) -> AgentSession {
        AgentSession {
            pid: 42,
            session_id: session_id.into(),
            cwd: format!("/tmp/{project_name}"),
            project_name: project_name.into(),
            started_at: 0,
            status: SessionStatus::Waiting,
            model: "claude-opus-4-6".into(),
            effort: String::new(),
            context_percent: 10.0,
            total_input_tokens: 1_000,
            total_output_tokens: 500,
            total_cache_read: 0,
            total_cache_create: 0,
            turn_count: 1,
            current_tasks: vec!["waiting for input".into()],
            mem_mb: 0,
            version: String::new(),
            git_branch: String::new(),
            git_added: 0,
            git_modified: 0,
            token_history: Vec::new(),
            context_history: Vec::new(),
            compaction_count: 0,
            context_window: 200_000,
            children: Vec::new(),
            telemetry: None,
            process_start_id: None,
        }
    }
}
