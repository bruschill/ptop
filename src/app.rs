use crate::collector::Collector;
use crate::host_info::{AgentAggregate, HostMetrics, HostSampler};
use crate::model::{AgentSession, OrphanPort, SessionStatus};
use crate::theme::Theme;
use std::collections::{HashMap, VecDeque};
#[cfg(test)]
use std::path::PathBuf;
use std::time::Instant;

/// Maximum data points kept for the live token-rate graph.
const GRAPH_HISTORY_LEN: usize = 200;

fn next_packaged_theme_name(theme: &Theme) -> &'static str {
    let names = crate::theme::THEME_NAMES;
    match &theme.source {
        crate::theme::ThemeSource::Packaged { id } => names
            .iter()
            .position(|name| name == id)
            .map(|index| names[(index + 1) % names.len()])
            .unwrap_or(names[0]),
        crate::theme::ThemeSource::File { .. } => names[0],
    }
}

/// Outcome of an Enter-key jump attempt. Distinct from `Option<String>` so
/// callers (notably `--exit-on-jump`) can tell a real terminal jump apart from
/// a no-op (unsupported terminal, or empty session list).
#[derive(Debug, PartialEq, Eq)]
pub enum JumpOutcome {
    /// Actually switched to a terminal pane/tab/window.
    Jumped,
    /// Tried to jump through an applicable backend, but the focus command failed.
    Failed(String),
    /// Unsupported terminal, or nothing selected — nothing happened.
    NoOp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NarrowTab {
    Work,
    Usage,
    System,
}

impl NarrowTab {
    pub const ALL: [Self; 3] = [Self::Work, Self::Usage, Self::System];

    pub fn label(self) -> &'static str {
        match self {
            Self::Work => "Work",
            Self::Usage => "Usage",
            Self::System => "System",
        }
    }

    pub fn shortcut(self) -> char {
        match self {
            Self::Work => 'w',
            Self::Usage => 'u',
            Self::System => 's',
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NarrowSection {
    Sessions,
    Projects,
    Context,
    Tokens,
    Ports,
}

impl NarrowSection {
    pub fn tab(self) -> NarrowTab {
        match self {
            Self::Sessions | Self::Projects => NarrowTab::Work,
            Self::Context | Self::Tokens => NarrowTab::Usage,
            Self::Ports => NarrowTab::System,
        }
    }
}

struct KillConfirmation {
    pid: u32,
    session_id: String,
    process_start_id: Option<String>,
    created_at: Instant,
}

pub struct App {
    pub sessions: Vec<AgentSession>,
    pub selected: usize,
    pub should_quit: bool,
    /// Token rate per tick (delta). Ring buffer for the braille graph.
    pub token_rates: VecDeque<f64>,
    /// Whether the latest token-rate observation is authoritative.
    pub token_rate_known: bool,
    /// Per-session previous token totals, keyed by (agent_cli, session_id).
    prev_tokens: HashMap<(String, String), u64>,
    collector: Collector,
    /// Ports left open by processes whose parent sessions have ended.
    pub orphan_ports: Vec<OrphanPort>,
    /// Transient status message shown in the footer (auto-clears after 3s).
    pub status_msg: Option<(String, Instant)>,
    /// Stable process/session identity awaiting a second kill key press.
    kill_confirm: Option<KillConfirmation>,
    pub theme: Theme,
    pub show_context: bool,
    pub show_tokens: bool,
    pub show_projects: bool,
    pub show_ports: bool,
    pub show_sessions: bool,
    pub narrow_tab: NarrowTab,
    pub active_narrow_section: Option<NarrowSection>,
    pub maximized_narrow_section: Option<NarrowSection>,
    pub config_open: bool,
    pub config_selected: usize,
    pub tree_view: bool,
    pub filter_text: String,
    pub filter_active: bool,
    pub show_timeline: bool,
    pub timeline_scroll: usize,
    pub show_file_audit: bool,
    /// Host vitals sampler (CPU% delta needs prior snapshot).
    host_sampler: HostSampler,
    /// Latest host metrics snapshot (None until first valid sample).
    pub host_metrics: Option<HostMetrics>,
    /// Aggregate metrics across all sessions (recomputed each tick).
    pub agent_aggregate: AgentAggregate,
    /// Help overlay (`?`) visibility.
    pub help_open: bool,
    /// View leader overlay (`v`) visibility.
    pub view_open: bool,
}

impl App {
    pub fn new(theme: Theme, panels: crate::config::PanelVisibility) -> Self {
        Self {
            sessions: Vec::new(),
            selected: 0,
            should_quit: false,
            token_rates: VecDeque::with_capacity(GRAPH_HISTORY_LEN),
            token_rate_known: false,
            prev_tokens: HashMap::new(),
            collector: Collector::new(),
            orphan_ports: Vec::new(),
            status_msg: None,
            kill_confirm: None,
            theme,
            show_context: panels.context,
            show_tokens: panels.tokens,
            show_projects: panels.projects,
            show_ports: panels.ports,
            show_sessions: panels.sessions,
            narrow_tab: NarrowTab::Work,
            active_narrow_section: Some(NarrowSection::Sessions),
            maximized_narrow_section: None,
            config_open: false,
            config_selected: 0,
            tree_view: false,
            filter_text: String::new(),
            filter_active: false,
            show_timeline: false,
            timeline_scroll: 0,
            show_file_audit: false,
            host_sampler: HostSampler::new(),
            host_metrics: None,
            agent_aggregate: AgentAggregate::default(),
            help_open: false,
            view_open: false,
        }
    }

    pub fn is_pi_mode(&self) -> bool {
        true
    }

    pub fn all_usage_known(&self) -> bool {
        self.sessions
            .iter()
            .all(|session| session.complete_total_tokens_value().is_some())
    }

    fn update_token_rate(&mut self) {
        // Unknown samples are omitted instead of entering the graph as false
        // zero activity. If telemetry later attaches, its first known sample
        // establishes the baseline and contributes no fabricated delta.
        self.token_rate_known = !self.sessions.is_empty() && self.all_usage_known();
        if !self.token_rate_known {
            self.token_rates.clear();
            self.prev_tokens.clear();
            return;
        }

        let mut rate: f64 = 0.0;
        for session in &self.sessions {
            let key = (session.agent_cli.to_string(), session.session_id.clone());
            let total = session.active_tokens();
            let previous = self.prev_tokens.get(&key).copied().unwrap_or(total);
            rate += total.saturating_sub(previous) as f64;
            self.prev_tokens.insert(key, total);
        }

        self.token_rates.push_back(rate);
        if self.token_rates.len() > GRAPH_HISTORY_LEN {
            self.token_rates.pop_front();
        }
    }

    pub fn toggle_help(&mut self) {
        self.help_open = !self.help_open;
        if self.help_open {
            self.view_open = false;
        }
    }

    pub fn toggle_view_menu(&mut self) {
        self.view_open = !self.view_open;
        if self.view_open {
            self.help_open = false;
        }
    }

    pub fn toggle_panel(&mut self, panel: u8) {
        match panel {
            1 => self.show_context = !self.show_context,
            2 => self.show_tokens = !self.show_tokens,
            3 => self.show_projects = !self.show_projects,
            4 => self.show_ports = !self.show_ports,
            5 => self.show_sessions = !self.show_sessions,
            _ => return,
        }
        self.persist_panel_visibility();
        self.clamp_narrow_tab();
    }

    fn persist_panel_visibility(&mut self) {
        let panels = crate::config::PanelVisibility {
            context: self.show_context,
            quota: false,
            tokens: self.show_tokens,
            projects: self.show_projects,
            ports: self.show_ports,
            sessions: self.show_sessions,
            mcp: false,
        };
        if let Err(e) = crate::config::save_panel_visibility(&panels) {
            self.set_status(format!("panels save failed: {}", e));
        }
    }

    pub fn toggle_file_audit(&mut self) {
        self.show_file_audit = !self.show_file_audit;
    }

    pub fn toggle_config(&mut self) {
        self.config_open = !self.config_open;
        if self.config_open {
            self.config_selected = 0;
        }
    }

    pub fn config_item_count(&self) -> usize {
        6 // theme + 5 panel toggles
    }

    pub fn config_select_next(&mut self) {
        if self.config_selected + 1 < self.config_item_count() {
            self.config_selected += 1;
        }
    }

    pub fn config_select_prev(&mut self) {
        self.config_selected = self.config_selected.saturating_sub(1);
    }

    pub fn config_toggle_selected(&mut self) {
        match self.config_selected {
            0 => {
                self.cycle_theme();
                return;
            }
            1 => self.show_context = !self.show_context,
            2 => self.show_tokens = !self.show_tokens,
            3 => self.show_projects = !self.show_projects,
            4 => self.show_ports = !self.show_ports,
            5 => self.show_sessions = !self.show_sessions,
            _ => return,
        }
        self.persist_panel_visibility();
        self.clamp_narrow_tab();
    }

    pub fn narrow_tab_visible(&self, tab: NarrowTab) -> bool {
        match tab {
            NarrowTab::Work => self.show_sessions || self.show_projects,
            NarrowTab::Usage => self.show_context || self.show_tokens,
            NarrowTab::System => self.show_ports,
        }
    }

    pub fn visible_narrow_tabs(&self) -> Vec<NarrowTab> {
        NarrowTab::ALL
            .into_iter()
            .filter(|&tab| self.narrow_tab_visible(tab))
            .collect()
    }

    pub fn active_narrow_tab(&self) -> Option<NarrowTab> {
        if self.narrow_tab_visible(self.narrow_tab) {
            Some(self.narrow_tab)
        } else {
            NarrowTab::ALL
                .into_iter()
                .find(|&tab| self.narrow_tab_visible(tab))
        }
    }

    pub fn set_narrow_tab(&mut self, tab: NarrowTab) {
        if self.narrow_tab_visible(tab) {
            self.narrow_tab = tab;
            self.clamp_narrow_section();
        }
    }

    pub fn select_next_narrow_tab(&mut self) {
        let tabs = self.visible_narrow_tabs();
        if tabs.is_empty() {
            return;
        }
        let current = self.active_narrow_tab().unwrap_or(tabs[0]);
        let pos = tabs.iter().position(|&tab| tab == current).unwrap_or(0);
        self.narrow_tab = tabs[(pos + 1) % tabs.len()];
        self.clamp_narrow_section();
    }

    pub fn select_prev_narrow_tab(&mut self) {
        let tabs = self.visible_narrow_tabs();
        if tabs.is_empty() {
            return;
        }
        let current = self.active_narrow_tab().unwrap_or(tabs[0]);
        let pos = tabs.iter().position(|&tab| tab == current).unwrap_or(0);
        self.narrow_tab = tabs[(pos + tabs.len() - 1) % tabs.len()];
        self.clamp_narrow_section();
    }

    fn clamp_narrow_tab(&mut self) {
        if let Some(tab) = self.active_narrow_tab() {
            self.narrow_tab = tab;
        }
        self.clamp_narrow_section();
    }

    pub fn narrow_section_visible(&self, section: NarrowSection) -> bool {
        match section {
            NarrowSection::Sessions => self.show_sessions,
            NarrowSection::Projects => self.show_projects,
            NarrowSection::Context => self.show_context,
            NarrowSection::Tokens => self.show_tokens,
            NarrowSection::Ports => self.show_ports,
        }
    }

    pub fn visible_narrow_sections(&self, tab: NarrowTab) -> Vec<NarrowSection> {
        let sections: &[NarrowSection] = match tab {
            NarrowTab::Work => &[NarrowSection::Sessions, NarrowSection::Projects],
            NarrowTab::Usage => &[NarrowSection::Context, NarrowSection::Tokens],
            NarrowTab::System => &[NarrowSection::Ports],
        };
        sections
            .iter()
            .copied()
            .filter(|&section| self.narrow_section_visible(section))
            .collect()
    }

    pub fn active_narrow_section(&self) -> Option<NarrowSection> {
        let tab = self.active_narrow_tab()?;
        if let Some(section) = self.active_narrow_section {
            if section.tab() == tab && self.narrow_section_visible(section) {
                return Some(section);
            }
        }
        self.visible_narrow_sections(tab).into_iter().next()
    }

    pub fn set_active_narrow_section(&mut self, section: NarrowSection) {
        if self.narrow_section_visible(section) {
            self.narrow_tab = section.tab();
            self.active_narrow_section = Some(section);
            self.clamp_narrow_section();
        }
    }

    pub fn maximized_narrow_section(&self) -> Option<NarrowSection> {
        let section = self.maximized_narrow_section?;
        if self.active_narrow_tab() == Some(section.tab()) && self.narrow_section_visible(section) {
            Some(section)
        } else {
            None
        }
    }

    pub fn toggle_narrow_section_zoom(&mut self, section: NarrowSection) {
        if !self.narrow_section_visible(section) {
            return;
        }
        self.set_active_narrow_section(section);
        self.maximized_narrow_section = if self.maximized_narrow_section() == Some(section) {
            None
        } else {
            Some(section)
        };
    }

    pub fn maximize_active_narrow_section(&mut self) {
        if let Some(section) = self.active_narrow_section() {
            self.maximized_narrow_section = Some(section);
        }
    }

    pub fn restore_narrow_sections(&mut self) {
        self.maximized_narrow_section = None;
    }

    fn clamp_narrow_section(&mut self) {
        self.active_narrow_section = self.active_narrow_section();
        if self.maximized_narrow_section().is_none() {
            self.maximized_narrow_section = None;
        }
    }

    pub fn toggle_timeline(&mut self) {
        self.show_timeline = !self.show_timeline;
        self.timeline_scroll = 0;
    }

    pub fn cycle_theme(&mut self) {
        let catalog = crate::theme::ThemeCatalog::packaged();
        let name = next_packaged_theme_name(&self.theme);
        match catalog.load(&crate::theme::ThemeRequest::BuiltIn(name.to_string())) {
            Ok(theme) => self.theme = theme,
            Err(error) => {
                self.set_status(format!("theme: {name} (load failed: {error})"));
                return;
            }
        }
        if let Err(error) = crate::config::save_theme(name) {
            self.set_status(format!("theme: {name} (save failed: {error})"));
        } else {
            self.set_status(format!("theme: {name}"));
        }
    }

    /// Set a transient status message that auto-clears after 3 seconds.
    pub fn set_status(&mut self, msg: String) {
        self.status_msg = Some((msg, Instant::now()));
    }

    /// Refresh Pi sessions and shared process enrichment.
    pub fn tick(&mut self) {
        let (sessions, orphan_ports) = self.collector.collect();
        self.sessions = sessions;
        self.orphan_ports = orphan_ports;
        self.host_metrics = self.host_sampler.sample();
        self.agent_aggregate = AgentAggregate::from_sessions(&self.sessions);
        if self.selected >= self.sessions.len() && !self.sessions.is_empty() {
            self.selected = self.sessions.len() - 1;
        }
        self.clamp_selection_to_visible();
        self.update_token_rate();
    }

    /// Returns indices of sessions matching the current filter.
    pub fn visible_indices(&self) -> Vec<usize> {
        if self.filter_text.is_empty() {
            return (0..self.sessions.len()).collect();
        }
        let query = self.filter_text.to_lowercase();
        self.sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| Self::session_matches(s, &query))
            .map(|(i, _)| i)
            .collect()
    }

    fn session_matches(s: &AgentSession, query: &str) -> bool {
        s.project_name.to_lowercase().contains(query)
            || s.model.to_lowercase().contains(query)
            || s.session_id.to_lowercase().contains(query)
            || s.initial_prompt.to_lowercase().contains(query)
            || s.cwd.to_lowercase().contains(query)
            || format!("{:?}", s.status).to_lowercase().contains(query)
    }

    /// Ensure `selected` points to a session included in the current filter.
    /// No-op when no sessions match; otherwise snaps to the first visible.
    fn clamp_selection_to_visible(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        if !visible.contains(&self.selected) {
            self.selected = visible[0];
        }
    }

    pub fn filter_push(&mut self, c: char) {
        self.filter_text.push(c);
        self.clamp_selection_to_visible();
    }

    pub fn filter_pop(&mut self) {
        self.filter_text.pop();
        self.clamp_selection_to_visible();
    }

    pub fn clear_filter(&mut self) {
        self.filter_active = false;
        self.filter_text.clear();
    }

    pub fn select_next(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        if let Some(pos) = visible.iter().position(|&i| i == self.selected) {
            if pos + 1 < visible.len() {
                self.selected = visible[pos + 1];
            }
        } else {
            self.selected = visible[0];
        }
    }

    pub fn select_prev(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        if let Some(pos) = visible.iter().position(|&i| i == self.selected) {
            if pos > 0 {
                self.selected = visible[pos - 1];
            }
        } else {
            self.selected = *visible.last().unwrap();
        }
    }

    pub fn select_session(&mut self, index: usize) {
        if index < self.sessions.len() && self.visible_indices().contains(&index) {
            self.selected = index;
        }
    }

    pub fn kill_selected(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let session = &self.sessions[self.selected];
        if matches!(session.status, SessionStatus::Done)
            || session.agent_cli != "pi"
            || session.pid == 0
        {
            return;
        }
        #[cfg(target_os = "windows")]
        {
            self.set_status("Pi process controls are unavailable on Windows".to_string());
            return;
        }

        let selected_identity = (session.pid, session.session_id.clone());

        // Confirm against stable process/session identity, not a mutable table row.
        if let Some(target) = self.kill_confirm.take() {
            let same_target =
                target.pid == selected_identity.0 && target.session_id == selected_identity.1;
            if same_target && target.created_at.elapsed().as_secs() < 2 {
                let process_info = crate::collector::process::get_process_info();
                let verified = process_info.get(&target.pid).is_some_and(|proc| {
                    let same_start = target.process_start_id.as_ref().is_none_or(|expected| {
                        crate::collector::pi::process_start_id(target.pid).as_ref()
                            == Some(expected)
                    });
                    same_start && is_killable_agent_command(&proc.command)
                });
                if !verified {
                    self.set_status(format!(
                        "PID {} no longer matches the selected process",
                        target.pid
                    ));
                    return;
                }
                let _ = std::process::Command::new("kill")
                    .args(["-9", &target.pid.to_string()])
                    .output();
                self.tick();
                return;
            }
        }

        let name = self.session_summary(session);
        self.kill_confirm = Some(KillConfirmation {
            pid: session.pid,
            session_id: session.session_id.clone(),
            process_start_id: session
                .process_start_id
                .clone()
                .or_else(|| crate::collector::pi::process_start_id(session.pid)),
            created_at: Instant::now(),
        });
        self.set_status(format!("Press x again to kill: {name}"));
    }

    /// Kill all orphan port processes (Shift+X).
    /// Does a fresh port scan and validates PID identity + port ownership
    /// immediately before sending any signals to avoid PID reuse / stale cache issues.
    pub fn kill_orphan_ports(&mut self) {
        #[cfg(target_os = "windows")]
        {
            self.set_status("Pi process controls are unavailable on Windows".to_string());
            return;
        }

        use crate::collector::process::get_listening_ports;

        // Fresh port scan right now — don't rely on cached data
        let fresh_ports = get_listening_ports();

        for orphan in &self.orphan_ports {
            // 1. Verify PID still listens on the expected port
            let still_listening = fresh_ports
                .get(&orphan.pid)
                .is_some_and(|ports| ports.contains(&orphan.port));
            if !still_listening {
                continue;
            }
            // 2. Verify PID still runs the expected command (full match, not substring)
            if let Ok(output) = std::process::Command::new("ps")
                .args(["-p", &orphan.pid.to_string(), "-o", "command="])
                .output()
            {
                let current_cmd = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if current_cmd == orphan.command {
                    let _ = std::process::Command::new("kill")
                        .args([&orphan.pid.to_string()])
                        .output();
                }
            }
        }
        // Re-collect to reflect changes
        self.tick();
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    /// Jump to the terminal running the selected session's agent process.
    /// Delegates to the terminal-jumper registry (cmux / tmux / iTerm2);
    /// see [`crate::jump`]. No-op when nothing is selected or no backend
    /// recognizes the process.
    pub fn jump_to_session(&mut self) -> JumpOutcome {
        if self.sessions.is_empty() {
            return JumpOutcome::NoOp;
        }
        let session = &self.sessions[self.selected];
        if session.agent_cli != "pi" || session.pid == 0 {
            return JumpOutcome::NoOp;
        }
        let target_pid = session.pid;
        let process_start_id = session.process_start_id.clone();

        #[cfg(target_os = "windows")]
        {
            self.set_status("Pi process controls are unavailable on Windows".to_string());
            return JumpOutcome::NoOp;
        }

        let process_info = crate::collector::process::get_process_info();
        let verified = process_info.get(&target_pid).is_some_and(|proc| {
            let same_start = process_start_id.as_ref().is_none_or(|expected| {
                crate::collector::pi::process_start_id(target_pid).as_ref() == Some(expected)
            });
            same_start && is_killable_agent_command(&proc.command)
        });
        if !verified {
            self.set_status(format!(
                "PID {target_pid} no longer matches the selected process"
            ));
            return JumpOutcome::NoOp;
        }

        crate::jump::run_jump(target_pid)
    }

    /// Return the Pi telemetry attachment label shown in session tables.
    pub fn session_summary(&self, session: &AgentSession) -> String {
        session
            .telemetry
            .as_ref()
            .map(|telemetry| telemetry.attachment.label().to_string())
            .unwrap_or_else(|| "process only".to_string())
    }
}

fn is_killable_agent_command(cmd: &str) -> bool {
    crate::collector::pi::is_pi_command(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_theme_cycle_starts_at_btop_for_file_themes() {
        let file_theme = Theme {
            source: crate::theme::ThemeSource::File {
                path: PathBuf::from("custom.toml"),
            },
            ..Theme::default()
        };
        assert_eq!(next_packaged_theme_name(&file_theme), "btop");

        let btop = Theme::default();
        assert_eq!(next_packaged_theme_name(&btop), "dracula");
    }

    fn waiting_session(cli: &'static str) -> AgentSession {
        AgentSession {
            agent_cli: cli,
            pid: 1,
            session_id: String::new(),
            cwd: String::new(),
            project_name: String::new(),
            started_at: 0,
            status: SessionStatus::Waiting,
            model: String::new(),
            effort: String::new(),
            context_percent: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_read: 0,
            total_cache_create: 0,
            turn_count: 0,
            compaction_count: 0,
            current_tasks: vec![],
            version: String::new(),
            git_branch: String::new(),
            mem_mb: 0,
            token_history: vec![],
            context_history: vec![],
            context_window: 0,
            subagents: vec![],
            mem_file_count: 0,
            mem_line_count: 0,
            children: vec![],
            initial_prompt: String::new(),
            first_assistant_text: String::new(),
            chat_messages: vec![],
            tool_calls: vec![],
            pending_since_ms: 0,
            thinking_since_ms: 0,
            file_accesses: vec![],
            config_root: String::new(),
            git_added: 0,
            git_modified: 0,
            telemetry: None,
            process_start_id: None,
        }
    }

    #[test]
    fn kill_validation_accepts_only_pi_processes() {
        assert!(is_killable_agent_command("pi --mode rpc"));
        assert!(!is_killable_agent_command("/usr/local/bin/claude"));
        assert!(!is_killable_agent_command("codex --resume abc"));
        assert!(!is_killable_agent_command("/usr/local/bin/opencode"));
        assert!(!is_killable_agent_command("node server.js"));
    }

    #[test]
    fn pi_app_starts_without_sessions() {
        let app = App::new(Theme::default(), crate::config::PanelVisibility::default());
        assert!(app.sessions.is_empty());
    }

    #[test]
    fn pi_summary_reports_attachment_state() {
        let app = App::new(Theme::default(), crate::config::PanelVisibility::default());
        let mut session = waiting_session("pi");
        let mut telemetry = crate::model::SessionTelemetry::process_only(1);
        session.telemetry = Some(telemetry.clone());
        assert_eq!(app.session_summary(&session), "process only");

        telemetry.attachment = crate::model::AttachmentState::Attached;
        session.telemetry = Some(telemetry);
        assert_eq!(app.session_summary(&session), "attached");
    }

    #[test]
    fn unknown_pi_usage_is_not_aggregated_as_known_zero() {
        let mut app = App::new(Theme::default(), crate::config::PanelVisibility::default());
        let mut session = waiting_session("pi");
        session.telemetry = Some(crate::model::SessionTelemetry::process_only(1));
        app.sessions.push(session);
        app.token_rates.push_back(99.0);
        app.prev_tokens
            .insert(("pi".to_string(), "session".to_string()), 99);

        app.update_token_rate();

        assert!(!app.all_usage_known());
        assert!(!app.token_rate_known);
        assert!(app.token_rates.is_empty());
    }

    #[test]
    fn empty_pi_fleet_keeps_token_rate_unknown() {
        let mut app = App::new(Theme::default(), crate::config::PanelVisibility::default());

        app.update_token_rate();

        assert!(!app.token_rate_known);
        assert!(app.token_rates.is_empty());
    }

    #[test]
    fn known_zero_usage_records_a_zero_rate() {
        let mut app = App::new(Theme::default(), crate::config::PanelVisibility::default());
        crate::demo::populate_demo(&mut app);
        app.token_rates.clear();
        app.prev_tokens.clear();

        app.update_token_rate();

        assert!(app.token_rate_known);
        assert_eq!(app.token_rates.back(), Some(&0.0));
    }
}
