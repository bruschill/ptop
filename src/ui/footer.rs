use crate::app::App;
use crate::locale::t;
use crate::theme::Theme;
use chrono::Timelike;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::truncate_str;

pub(crate) fn draw_footer(f: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    // Filter input mode: show filter bar instead of normal keybindings
    if app.filter_active {
        let visible_count = app.visible_indices().len();
        let count = format!(
            "{}/{} {}",
            visible_count,
            app.sessions.len(),
            t("footer.sessions")
        );
        let suffix = if area.width <= 80 {
            format!("_  {}", count)
        } else {
            format!(
                "_  {}  (Esc {}, Enter {})",
                count,
                t("footer.esc_clear")
                    .split(',')
                    .next()
                    .unwrap_or(&t("footer.esc_clear")),
                t("footer.esc_clear")
                    .split(',')
                    .nth(1)
                    .unwrap_or("keep")
                    .trim()
            )
        };
        let filter_w = (area.width as usize).saturating_sub(2 + suffix.chars().count());
        let spans = vec![
            Span::styled(" /", Style::default().fg(theme.hi_fg)),
            Span::styled(
                truncate_str(&app.filter_text, filter_w),
                Style::default().fg(theme.title),
            ),
            Span::styled(suffix, Style::default().fg(theme.inactive_fg)),
        ];
        f.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }

    let compact = area.width <= 80;
    let ultra_compact = area.width <= 70;

    let mut spans = vec![
        Span::styled(" ↑↓", Style::default().fg(theme.hi_fg)),
        Span::styled(
            format!(" {} ", t("footer.select")),
            Style::default().fg(theme.main_fg),
        ),
    ];
    if !ultra_compact {
        spans.push(Span::styled("↵", Style::default().fg(theme.hi_fg)));
        spans.push(Span::styled(
            format!(" {} ", t("footer.jump")),
            Style::default().fg(theme.main_fg),
        ));
    }
    if compact {
        spans.push(Span::styled("←→", Style::default().fg(theme.hi_fg)));
        spans.push(Span::styled(" tabs ", Style::default().fg(theme.main_fg)));
    }
    if !ultra_compact {
        spans.push(Span::styled("x", Style::default().fg(theme.hi_fg)));
        spans.push(Span::styled(
            format!(" {} ", t("footer.kill")),
            Style::default().fg(theme.main_fg),
        ));
    }
    spans.push(Span::styled("/", Style::default().fg(theme.hi_fg)));
    spans.push(Span::styled(
        format!(" {} ", t("footer.filter")),
        Style::default().fg(theme.main_fg),
    ));
    if !ultra_compact {
        spans.push(Span::styled("v", Style::default().fg(theme.hi_fg)));
        spans.push(Span::styled(
            format!(" {} ", t("footer.view")),
            Style::default().fg(theme.main_fg),
        ));
        if !compact {
            spans.push(Span::styled("c", Style::default().fg(theme.hi_fg)));
            spans.push(Span::styled(
                format!(" {} ", t("footer.config")),
                Style::default().fg(theme.main_fg),
            ));
        }
        spans.push(Span::styled("?", Style::default().fg(theme.hi_fg)));
        spans.push(Span::styled(
            format!(" {} ", t("footer.help")),
            Style::default().fg(theme.main_fg),
        ));
    }
    spans.push(Span::styled("q", Style::default().fg(theme.hi_fg)));
    spans.push(Span::styled(
        format!(" {} ", t("footer.quit")),
        Style::default().fg(theme.main_fg),
    ));

    // Show active filter or transient status
    if !compact && !app.filter_text.is_empty() {
        spans.push(Span::styled(
            format!(" /{} ", app.filter_text),
            Style::default().fg(theme.status_fg),
        ));
    } else if !compact {
        let status_text = app
            .status_msg
            .as_ref()
            .filter(|(_, when)| when.elapsed().as_secs() < 3)
            .map(|(msg, _)| msg.as_str());
        if let Some(msg) = status_text {
            spans.push(Span::styled(
                format!(" {msg} "),
                Style::default().fg(theme.status_fg),
            ));
        } else {
            spans.push(Span::styled(
                t("footer.auto"),
                Style::default().fg(theme.inactive_fg),
            ));
        }
    }

    let now = chrono::Utc::now();
    let peak_info = peak_hours_warning(app.is_pi_mode(), now.hour(), now.minute());
    if let Some(ref peak) = peak_info.filter(|_| !compact) {
        spans.push(Span::styled(
            format!(" {peak} "),
            Style::default().fg(theme.warning_fg),
        ));
    }

    let visible_count = app.visible_indices().len();
    let sessions_label = t("footer.sessions");
    let count_label = if visible_count < app.sessions.len() {
        format!(
            "{}/{} {}",
            visible_count,
            app.sessions.len(),
            sessions_label
        )
    } else {
        format!("{} {}", app.sessions.len(), sessions_label)
    };
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let count_w = count_label.chars().count();
    if used + count_w < area.width as usize {
        let remaining = (area.width as usize).saturating_sub(used + count_w);
        spans.push(Span::styled(
            format!("{:>width$}", count_label, width = remaining),
            Style::default().fg(theme.graph_text),
        ));
    }

    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

// US business hours = PT 5am–11am = UTC 12:00–18:00.
fn peak_hours_warning(pi_mode: bool, hour: u32, minute: u32) -> Option<String> {
    if pi_mode || !(12..18).contains(&hour) {
        return None;
    }
    let mins_left = (18 - hour) * 60 - minute;
    let h = mins_left / 60;
    let m = mins_left % 60;
    let peak_label = t("footer.peak_hours");
    let resets_in = t("footer.resets_in");
    Some(format!("⚡{} ({} {}h{:02}m)", peak_label, resets_in, h, m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PanelVisibility;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn pi_mode_suppresses_provider_peak_hours_warning() {
        assert!(peak_hours_warning(true, 13, 0).is_none());
        assert!(peak_hours_warning(false, 13, 0).is_some());
    }

    #[test]
    fn footer_renders_concise_cmux_socket_failure() {
        let mut app = App::new(Theme::default(), PanelVisibility::default());
        app.set_status("cmux: socket broken; restart cmux".to_string());

        let backend = TestBackend::new(120, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                draw_footer(
                    f,
                    &app,
                    Rect {
                        x: 0,
                        y: 0,
                        width: 120,
                        height: 1,
                    },
                    &app.theme,
                )
            })
            .unwrap();
        let text = format!("{}", terminal.backend());

        assert!(text.contains("cmux: socket broken; restart cmux"));
        assert!(!text.contains("Broken pipe"));
        assert!(!text.contains("select-workspace"));
    }
}
