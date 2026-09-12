//! ptop — AI agent monitor.
//!
//! This crate is both a binary (the TUI, entered via [`run`]) and a library.
//! The library surface exists so a separate local tool (e.g. a web UI) can
//! reuse the data-collection layer in-process and serialize it via
//! [`snapshot::Snapshot`] / [`app::App::to_snapshot`], without reimplementing
//! session discovery and without depending on the terminal frontend.
//!
//! # Public API for library consumers
//!
//! The stable surface for in-process consumers is [`app`] (notably
//! [`App::to_snapshot`](app::App::to_snapshot) and [`App::tick`](app::App::tick)),
//! [`snapshot`], [`config`], [`demo`], [`host_info`], and the data types in
//! [`model`]. The [`collector`], [`locale`], [`theme`], and [`ui`] modules are
//! published mainly to support the bundled TUI binary and may change without a
//! semver-major bump — depend on them at your own risk.
//!
//! Enum wire formats are part of the snapshot contract: variants such as
//! [`model::SessionStatus`] serialize as their CamelCase names (`"Thinking"`,
//! `"Executing"`, …) and chat roles serialize as `"user"` / `"assistant"`.
//! These strings are stable and won't be renamed without a major version bump.
//!
//! # Threading model
//!
//! [`App`] is **not** `Send`: it owns boxed collector trait objects
//! and must stay on the thread that created it. Don't move it between threads
//! or share it with request handlers — instead, run the collector loop on one
//! thread, serialize each [`snapshot::Snapshot`] to JSON, and hand the *string*
//! to other threads.
//!
//! # Typical usage
//!
//! ```no_run
//! use ptop::app::App;
//! use ptop::{config, theme::Theme};
//!
//! let cfg = config::load_config();
//! let mut app = App::new(Theme::default(), cfg.panels);
//! loop {
//!     app.tick();                             // refresh Pi process and telemetry data
//!     let snap = app.to_snapshot(2_000);      // pure read → JSON-friendly DTO
//!     let json = serde_json::to_string(&snap).unwrap();
//!     // ... serve `json`, sleep for the interval, repeat ...
//!     # break;
//! }
//! ```

pub mod app;
pub mod collector;
pub mod config;
pub mod demo;
pub mod host_info;
pub mod jump;
pub mod locale;
pub mod model;
pub mod snapshot;
pub mod theme;
pub mod ui;

use app::{App, JumpOutcome};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use std::ffi::{OsStr, OsString};
use std::io::{self, stdout};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// Construct a headless `App` from loaded config + theme. Shared by the
/// `--json` and `--once` entry points.
fn build_app(theme: theme::Theme, cfg: &config::AppConfig) -> App {
    App::new(theme, cfg.panels)
}

fn has_flag(args: &[OsString], flag: &str) -> bool {
    args.iter().any(|argument| argument == OsStr::new(flag))
}

fn theme_request_from_args(args: &[OsString]) -> Result<Option<theme::ThemeRequest>, String> {
    let mut built_in = None;
    let mut file = None;
    let mut index = 1;

    while index < args.len() {
        let argument = &args[index];
        if argument == OsStr::new("--theme") {
            if built_in.is_some() {
                return Err("--theme may be specified only once".to_string());
            }
            let value = args
                .get(index + 1)
                .filter(|value| !value.to_string_lossy().starts_with('-'))
                .ok_or("--theme requires a built-in theme name")?;
            let name = value.to_str().ok_or("--theme names must be valid UTF-8")?;
            built_in = Some(name.to_string());
            index += 2;
            continue;
        }
        if argument == OsStr::new("--theme-file") {
            if file.is_some() {
                return Err("--theme-file may be specified only once".to_string());
            }
            let value = args
                .get(index + 1)
                .filter(|value| !value.to_string_lossy().starts_with('-'))
                .ok_or("--theme-file requires a path")?;
            file = Some(PathBuf::from(value));
            index += 2;
            continue;
        }
        index += 1;
    }

    match (built_in, file) {
        (Some(_), Some(_)) => Err("--theme and --theme-file are mutually exclusive".to_string()),
        (Some(name), None) => Ok(Some(theme::ThemeRequest::BuiltIn(name))),
        (None, Some(path)) => Ok(Some(theme::ThemeRequest::File(path))),
        (None, None) => Ok(None),
    }
}

fn resolve_initial_theme(
    explicit: Option<theme::ThemeRequest>,
    config: &config::AppConfig,
    startup_directory: &Path,
    config_file: Option<&Path>,
    home_directory: Option<&Path>,
) -> Result<theme::Theme, String> {
    let is_cli_request = explicit.is_some();
    let mut request = explicit.unwrap_or_else(|| {
        config
            .theme_file
            .clone()
            .map(theme::ThemeRequest::File)
            .unwrap_or_else(|| theme::ThemeRequest::BuiltIn(config.theme.clone()))
    });

    if let theme::ThemeRequest::File(path) = &mut request {
        let base = if is_cli_request {
            startup_directory
        } else {
            config_file.and_then(Path::parent).ok_or(
                "cannot resolve a relative configured theme path without a config directory",
            )?
        };
        *path = resolve_theme_path(path, base, home_directory)?;
    }

    theme::ThemeCatalog::packaged()
        .load(&request)
        .map_err(|error| error.to_string())
}

fn resolve_theme_path(path: &Path, base: &Path, home: Option<&Path>) -> Result<PathBuf, String> {
    let mut components = path.components();
    let expanded = if matches!(
        components.next(),
        Some(Component::Normal(first)) if first == OsStr::new("~")
    ) {
        let home = home.ok_or("cannot expand '~' because no home directory is available")?;
        home.join(components.collect::<PathBuf>())
    } else {
        path.to_path_buf()
    };

    if expanded.is_absolute() {
        Ok(expanded)
    } else {
        Ok(base.join(expanded))
    }
}

fn exit_with_message(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1)
}

pub fn run() -> io::Result<()> {
    let args: Vec<OsString> = std::env::args_os().collect();
    let options = match runtime_options_from_args(&args) {
        Ok(options) => options,
        Err(message) => exit_with_message(&message),
    };

    // Keep theme-independent commands ahead of config and theme resolution.
    if has_flag(&args, "--version") || has_flag(&args, "-V") {
        println!("ptop {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if has_flag(&args, "--update") {
        return run_update();
    }

    let explicit_theme = match theme_request_from_args(&args) {
        Ok(request) => request,
        Err(message) => exit_with_message(&message),
    };
    let cfg = match config::load_config_for_startup(explicit_theme.is_some()) {
        Ok(config) => config,
        Err(message) => exit_with_message(&message),
    };
    let startup_directory = match std::env::current_dir() {
        Ok(directory) => directory,
        Err(error) => exit_with_message(&format!("cannot determine startup directory: {error}")),
    };
    let initial_theme = match resolve_initial_theme(
        explicit_theme,
        &cfg,
        &startup_directory,
        config::config_path().as_deref(),
        dirs::home_dir().as_deref(),
    ) {
        Ok(theme) => theme,
        Err(message) => exit_with_message(&message),
    };

    let demo_mode = options.demo_mode;
    let exit_on_jump = options.exit_on_jump;
    let mouse_capture = options.mouse_capture;

    // --json flag: print a machine-readable JSON snapshot and exit.
    // Useful for scripting and as a manual check of the web snapshot API; the
    // web tool uses the library `App::to_snapshot` directly.
    if has_flag(&args, "--json") {
        let mut app = build_app(initial_theme, &cfg);
        if demo_mode {
            demo::populate_demo(&mut app);
        } else {
            app.tick();
        }
        match serde_json::to_string_pretty(&app.to_snapshot(2000)) {
            Ok(json) => {
                println!("{}", json);
                return Ok(());
            }
            Err(e) => {
                eprintln!("failed to serialize snapshot: {}", e);
                std::process::exit(1);
            }
        }
    }

    // --once flag: print snapshot and exit
    if has_flag(&args, "--once") {
        let mut app = build_app(initial_theme, &cfg);
        if demo_mode {
            demo::populate_demo(&mut app);
        } else {
            app.tick();
        }
        print_snapshot(&app);
        return Ok(());
    }

    // Resolve the theme before terminal setup so failures cannot corrupt the terminal.
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    if mouse_capture {
        stdout().execute(EnableMouseCapture)?;
    }
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let app_result = run_app(&mut terminal, demo_mode, initial_theme, exit_on_jump, &cfg);

    // Always attempt both cleanup steps regardless of app result
    let r1 = if mouse_capture {
        stdout().execute(DisableMouseCapture).map(|_| ())
    } else {
        Ok(())
    };
    let r2 = disable_raw_mode();
    let r3 = stdout().execute(LeaveAlternateScreen).map(|_| ());

    // Return app error first, then cleanup errors
    app_result.and(r1).and(r2).and(r3)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RuntimeOptions {
    demo_mode: bool,
    exit_on_jump: bool,
    mouse_capture: bool,
}

fn runtime_options_from_args(args: &[OsString]) -> Result<RuntimeOptions, String> {
    let mut options = RuntimeOptions {
        demo_mode: false,
        exit_on_jump: false,
        mouse_capture: false,
    };
    let mut index = 1;

    while index < args.len() {
        let argument = &args[index];
        if argument == OsStr::new("--theme") || argument == OsStr::new("--theme-file") {
            let option = argument.to_string_lossy();
            args.get(index + 1)
                .filter(|value| !value.to_string_lossy().starts_with('-'))
                .ok_or_else(|| format!("{option} requires a value"))?;
            index += 2;
            continue;
        }

        if argument == OsStr::new("--demo") {
            options.demo_mode = true;
        } else if argument == OsStr::new("--exit-on-jump") {
            options.exit_on_jump = true;
        } else if argument == OsStr::new("--mouse") {
            options.mouse_capture = true;
        } else if !matches!(
            argument.to_str(),
            Some("--json" | "--once" | "--update" | "--version" | "-V")
        ) {
            return Err(format!(
                "unknown option or argument: {}",
                argument.to_string_lossy()
            ));
        }
        index += 1;
    }

    Ok(options)
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    demo_mode: bool,
    theme: theme::Theme,
    exit_on_jump: bool,
    config: &config::AppConfig,
) -> io::Result<()> {
    let mut app = App::new(theme, config.panels);
    if demo_mode {
        demo::populate_demo(&mut app);
    } else {
        app.tick();
    }

    let mut last_tick = std::time::Instant::now();
    let tick_interval = Duration::from_secs(2);
    let render_interval = Duration::from_millis(500);

    loop {
        terminal.draw(|f| ui::draw(f, &app))?;

        // Poll at 500ms for smooth animations; data tick every 2s
        let had_input = if event::poll(render_interval)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key_press(&mut app, key, demo_mode, exit_on_jump, |app| {
                        app.jump_to_session()
                    })
                }
                Event::Mouse(mouse) => {
                    let size = terminal.size()?;
                    let area = Rect::new(0, 0, size.width, size.height);
                    handle_mouse_event(&mut app, mouse, area, demo_mode);
                }
                _ => {}
            }
            true
        } else {
            false
        };

        if demo_mode {
            // Rotate token rates to animate the sparkline
            if let Some(front) = app.token_rates.pop_front() {
                app.token_rates.push_back(front);
            }
        } else if !had_input && last_tick.elapsed() >= tick_interval {
            // Data tick every 2s — skip when handling input to avoid lag
            app.tick();
            last_tick = std::time::Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn handle_key_press(
    app: &mut App,
    key: KeyEvent,
    demo_mode: bool,
    exit_on_jump: bool,
    jump_to_session: impl FnOnce(&mut App) -> JumpOutcome,
) {
    if app.help_open {
        // Any key dismisses help.
        app.help_open = false;
    } else if app.view_open {
        match key.code {
            KeyCode::Esc | KeyCode::Char('v') => app.view_open = false,
            KeyCode::Char(c @ '1'..='5') => app.toggle_panel(c as u8 - b'0'),
            KeyCode::Char('t') => app.cycle_theme(),
            _ => {}
        }
    } else if app.config_open {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('c') => app.toggle_config(),
            KeyCode::Down | KeyCode::Char('j') => app.config_select_next(),
            KeyCode::Up | KeyCode::Char('k') => app.config_select_prev(),
            KeyCode::Enter | KeyCode::Char(' ') => app.config_toggle_selected(),
            _ => {}
        }
    } else if app.filter_active {
        match key.code {
            KeyCode::Esc => app.clear_filter(),
            KeyCode::Enter => app.filter_active = false,
            KeyCode::Backspace => app.filter_pop(),
            KeyCode::Down => app.select_next(),
            KeyCode::Up => app.select_prev(),
            KeyCode::Char(c) => app.filter_push(c),
            _ => {}
        }
    } else {
        match key.code {
            KeyCode::Char('q') => app.quit(),
            KeyCode::Char('r') if !demo_mode => app.tick(),
            KeyCode::Down | KeyCode::Char('j') => app.select_next(),
            KeyCode::Up | KeyCode::Char('k') => app.select_prev(),
            KeyCode::Right | KeyCode::Tab => app.select_next_narrow_tab(),
            KeyCode::Left | KeyCode::BackTab => app.select_prev_narrow_tab(),
            KeyCode::Char('w') => app.set_narrow_tab(app::NarrowTab::Work),
            KeyCode::Char('u') => app.set_narrow_tab(app::NarrowTab::Usage),
            KeyCode::Char('s') => app.set_narrow_tab(app::NarrowTab::System),
            KeyCode::Char('+') | KeyCode::Char('=') => app.maximize_active_narrow_section(),
            KeyCode::Char('-') => app.restore_narrow_sections(),
            KeyCode::Char('x') if !demo_mode => app.kill_selected(),
            KeyCode::Char('X') if !demo_mode => app.kill_orphan_ports(),
            KeyCode::Char('t') => app.cycle_theme(),
            KeyCode::Char(c @ '1'..='5') => app.toggle_panel(c as u8 - b'0'),
            KeyCode::Char('c') => app.toggle_config(),
            KeyCode::Char('v') => app.toggle_view_menu(),
            KeyCode::Char('?') => app.toggle_help(),
            KeyCode::Char('/') => app.filter_active = true,
            KeyCode::Esc if !app.filter_text.is_empty() => app.clear_filter(),
            KeyCode::Enter if !demo_mode => match jump_to_session(app) {
                JumpOutcome::Jumped if exit_on_jump => app.quit(),
                JumpOutcome::Failed(msg) => app.set_status(msg),
                JumpOutcome::Jumped | JumpOutcome::NoOp => {}
            },
            _ => {}
        }
    }
}

fn handle_mouse_event(app: &mut App, mouse: MouseEvent, area: Rect, demo_mode: bool) {
    if app.help_open || app.view_open || app.config_open || app.filter_active {
        return;
    }

    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(target) = ui::click_target(app, area, mouse.column, mouse.row) {
                match target {
                    ui::ClickTarget::NarrowTab(tab) => app.set_narrow_tab(tab),
                    ui::ClickTarget::NarrowSection(section) => {
                        app.set_active_narrow_section(section);
                    }
                    ui::ClickTarget::NarrowZoom(section) => {
                        app.toggle_narrow_section_zoom(section);
                    }
                    ui::ClickTarget::Session(index) => {
                        app.select_session(index);
                        app.set_active_narrow_section(app::NarrowSection::Sessions);
                    }
                    ui::ClickTarget::KillOrphanPorts => {
                        app.set_active_narrow_section(app::NarrowSection::Ports);
                        if !demo_mode {
                            app.kill_orphan_ports();
                        }
                    }
                }
            }
        }
        MouseEventKind::ScrollDown => app.select_next(),
        MouseEventKind::ScrollUp => app.select_prev(),
        MouseEventKind::ScrollRight => app.select_next_narrow_tab(),
        MouseEventKind::ScrollLeft => app.select_prev_narrow_tab(),
        _ => {}
    }
}

/// Strip control characters (including ANSI escapes) and Unicode bidi
/// overrides from a string for safe terminal output. Defeats CVE-2021-42574
/// (Trojan Source) style attacks via RTLO/LRO/PDF/isolate characters.
fn sanitize_output(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(*c,
                '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{200E}'
                | '\u{200F}')
        })
        .collect()
}

fn format_context_value(session: &model::AgentSession) -> String {
    session
        .context_value()
        .map(|percent| format!("{}{:>3.0}%", session.context_precision().prefix(), percent))
        .or_else(|| {
            session.context_tokens_without_window().map(|tokens| {
                format!(
                    "{}{} (window —)",
                    session.context_precision().prefix(),
                    fmt_tok(tokens)
                )
            })
        })
        .unwrap_or_else(|| "—".to_string())
}

fn format_token_value(session: &model::AgentSession) -> String {
    session
        .total_tokens_value()
        .map(|total| {
            format!(
                "{}{}{}",
                session.usage_precision().prefix(),
                fmt_tok(total),
                if session.usage_is_partial() { "+" } else { "" }
            )
        })
        .unwrap_or_else(|| "—".to_string())
}

fn print_snapshot(app: &App) {
    println!("ptop — {} Pi processes\n", app.sessions.len());
    for session in &app.sessions {
        let status = match &session.status {
            model::SessionStatus::Thinking => "◉ Think",
            model::SessionStatus::Executing => "● Exec",
            model::SessionStatus::Waiting => "◌ Wait",
            model::SessionStatus::Unknown => "? Unknown",
            model::SessionStatus::Done => "✓ Done",
        };
        let sid_short = if session.session_id.len() >= 7 {
            &session.session_id[..7]
        } else {
            &session.session_id
        };
        let project_label = format!("{}({})", session.project_name, sid_short);
        let summary = sanitize_output(&app.session_summary(session));
        let model = if session.model.is_empty() {
            "—".to_string()
        } else {
            session.model.clone()
        };
        let context = format_context_value(session);
        let tokens = format_token_value(session);
        let age = format!("seen:{}", session.elapsed_display());
        println!(
            "  {} {:<20} {} {} {:<10} CTX:{} Tok:{} Mem:{}M {}",
            session.pid,
            sanitize_output(&project_label),
            summary,
            status,
            model,
            context,
            tokens,
            session.mem_mb,
            age,
        );
        if let Some(task) = session.current_tasks.last() {
            println!("       └─ {}", sanitize_output(task));
        }
        if let Some(telemetry) = &session.telemetry {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let observed_age =
                ui::fmt_age(now_ms.saturating_sub(telemetry.context.observed_at_ms) / 1_000);
            println!(
                "       telemetry: {} ({}) · source {} · observed {}",
                telemetry.attachment.label(),
                telemetry.attachment_confidence.label(),
                telemetry.source_health.label(),
                observed_age
            );
            if let Some(provider) = &telemetry.context_details.provider {
                println!(
                    "       provider/model: {}/{}",
                    sanitize_output(provider),
                    sanitize_output(&session.model)
                );
            }
            println!(
                "       context: {} · {}/{} · {}",
                context.trim(),
                telemetry.context.precision.label(),
                telemetry.context.completeness.label(),
                sanitize_output(&telemetry.context.provenance)
            );
            println!(
                "       tokens: {} · {}/{} · {}",
                tokens,
                telemetry.usage.precision.label(),
                telemetry.usage.completeness.label(),
                sanitize_output(&telemetry.usage.provenance)
            );
            if let Some(reason) = &telemetry.context_details.reason {
                println!("       context note: {}", sanitize_output(reason));
            }
            if let Some(cost) = telemetry.usage_details.reported_cost {
                println!("       reported cost: {cost:.4}");
            }
            let fleet = &telemetry.fleet;
            println!(
                "       fleet: {} · background {} · foreground {} · {} run{} · status.json · {}d retention",
                fleet.source_health.label(),
                fleet.background_visibility.label(),
                fleet.foreground_visibility.label(),
                fleet.runs.len(),
                if fleet.runs.len() == 1 { "" } else { "s" },
                fleet.retention_days
            );
            if let Some(reason) = &fleet.reason {
                println!("       fleet note: {}", sanitize_output(reason));
            }
            for run in &fleet.runs {
                let run_tokens = run
                    .usage
                    .total_tokens
                    .map(|tokens| format!(" · {} tok", fmt_tok(tokens)))
                    .unwrap_or_default();
                let terminal = run
                    .process_terminal
                    .as_ref()
                    .map(|proof| format!(" · exit {}", proof.state.label()))
                    .unwrap_or_default();
                let stale = if run.stale { " · stale" } else { "" };
                println!(
                    "       run {} · {} · {} · {}{}{}{}",
                    sanitize_output(&run.run_id),
                    run.execution.label(),
                    run.mode.label(),
                    run.state.label(),
                    run_tokens,
                    terminal,
                    stale
                );
                for child in &run.children {
                    let child_tokens = child
                        .usage
                        .total_tokens
                        .map(|tokens| format!(" · {} tok", fmt_tok(tokens)))
                        .unwrap_or_default();
                    println!(
                        "         child {} · {} · {}{}",
                        sanitize_output(&child.name),
                        child.execution.label(),
                        child.state.label(),
                        child_tokens
                    );
                }
            }
        }
        if session.process_start_id.is_none() {
            println!("       identity: PID only (reuse not guarded)");
        }
        for child in &session.children {
            let port = child.port.map(|p| format!(":{}", p)).unwrap_or_default();
            let command = model::safe_process_label(&child.command);
            println!(
                "       {} {} {}K {}",
                child.pid,
                sanitize_output(&command),
                child.mem_kb / 1024,
                port,
            );
        }
    }
}

fn run_update() -> io::Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    println!("ptop v{current} — checking for updates...\n");

    // Download to a private temp file (O_EXCL + random suffix) so a local
    // attacker can't pre-place a symlink or swap the file mid-run.
    let tmp = tempfile::Builder::new()
        .prefix("ptop-installer-")
        .suffix(".sh")
        .tempfile()?;
    let installer_path = tmp.path().to_path_buf();

    let dl_status = std::process::Command::new("curl")
        .args([
            "--proto",
            "=https",
            "--tlsv1.2",
            "-LsSf",
            "https://github.com/bruschill/ptop/releases/latest/download/ptop-installer.sh",
            "-o",
        ])
        .arg(&installer_path)
        .status()?;

    if !dl_status.success() {
        eprintln!("\nDownload failed. You can also update manually:");
        eprintln!("  cargo install ptop --force");
        std::process::exit(1);
    }

    // Show checksum so the user can verify if desired.
    // macOS ships `shasum` (Perl) by default, Linux ships `sha256sum` (coreutils).
    let checksum_shown = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(&installer_path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !checksum_shown {
        let _ = std::process::Command::new("sha256sum")
            .arg(&installer_path)
            .status();
    }

    let status = std::process::Command::new("sh")
        .arg(&installer_path)
        .status()?;

    // NamedTempFile::drop removes the file; explicit drop to sequence it
    // after sh exits.
    drop(tmp);

    if !status.success() {
        eprintln!("\nUpdate failed. You can also update manually:");
        eprintln!("  cargo install ptop --force");
        std::process::exit(1);
    }

    Ok(())
}

fn fmt_tok(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        format!("{}", n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{TelemetryCompleteness, TelemetryPrecision};
    use crossterm::event::{KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;

    #[test]
    fn mouse_capture_is_opt_in() {
        assert!(
            !runtime_options_from_args(&[OsString::from("ptop")])
                .unwrap()
                .mouse_capture
        );
        assert!(
            runtime_options_from_args(&[OsString::from("ptop"), OsString::from("--mouse")])
                .unwrap()
                .mouse_capture
        );
    }

    #[test]
    fn runtime_options_accept_pi_demo_with_mouse() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();

        assert_eq!(
            runtime_options_from_args(&args(&["ptop"])),
            Ok(RuntimeOptions {
                demo_mode: false,
                exit_on_jump: false,
                mouse_capture: false,
            })
        );
        assert_eq!(
            runtime_options_from_args(&args(&["ptop", "--demo", "--mouse"])),
            Ok(RuntimeOptions {
                demo_mode: true,
                exit_on_jump: false,
                mouse_capture: true,
            })
        );
    }

    #[test]
    fn runtime_options_reject_removed_and_unknown_options() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();

        for option in ["--legacy", "--setup", "--unknown"] {
            let error = runtime_options_from_args(&args(&["ptop", option])).unwrap_err();
            assert!(error.contains(option), "{error}");
        }
    }

    #[test]
    fn demo_mouse_orphan_click_does_not_tick_collectors() {
        let mut app = App::new(theme::Theme::default(), config::PanelVisibility::default());
        demo::populate_demo(&mut app);
        let area = Rect::new(0, 0, 120, 40);
        let (column, row) = (0..area.width)
            .flat_map(|column| (0..area.height).map(move |row| (column, row)))
            .find(|&(column, row)| {
                ui::click_target(&app, area, column, row) == Some(ui::ClickTarget::KillOrphanPorts)
            })
            .expect("Pi demo exposes an orphan-port kill target");
        let session_ids = app
            .sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect::<Vec<_>>();
        let token_rates = app.token_rates.clone();

        handle_mouse_event(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
            area,
            true,
        );

        assert_eq!(
            app.sessions
                .iter()
                .map(|session| session.session_id.clone())
                .collect::<Vec<_>>(),
            session_ids,
            "demo orphan click must not invoke the collector-refreshing kill action"
        );
        assert_eq!(app.token_rates, token_rates);
        assert_eq!(app.orphan_ports.len(), 1);
    }

    #[test]
    fn pi_demo_renders_attached_telemetry() {
        let mut app = App::new(theme::Theme::default(), config::PanelVisibility::default());
        demo::populate_demo(&mut app);
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| ui::draw(frame, &app)).unwrap();
        let text = format!("{}", terminal.backend());

        assert!(
            text.contains("storefront"),
            "missing Pi demo session\n{text}"
        );
        assert!(
            text.contains("context"),
            "missing Pi telemetry panel\n{text}"
        );
    }

    #[test]
    fn theme_flags_are_parsed_once_and_are_mutually_exclusive() {
        assert_eq!(
            theme_request_from_args(&[
                OsString::from("ptop"),
                OsString::from("--theme"),
                OsString::from("nord"),
            ])
            .unwrap(),
            Some(theme::ThemeRequest::BuiltIn("nord".to_string()))
        );
        assert_eq!(
            theme_request_from_args(&[
                OsString::from("ptop"),
                OsString::from("--theme-file"),
                OsString::from("custom.toml"),
            ])
            .unwrap(),
            Some(theme::ThemeRequest::File(PathBuf::from("custom.toml")))
        );
        assert!(theme_request_from_args(&[
            OsString::from("ptop"),
            OsString::from("--theme"),
            OsString::from("nord"),
            OsString::from("--theme-file"),
            OsString::from("custom.toml"),
        ])
        .is_err());
        assert!(theme_request_from_args(
            &[OsString::from("ptop"), OsString::from("--theme-file"),]
        )
        .is_err());
    }

    #[test]
    fn cli_theme_overrides_a_broken_configured_theme_file() {
        let config = config::AppConfig {
            theme_file: Some(PathBuf::from("missing.toml")),
            ..config::AppConfig::default()
        };
        let theme = resolve_initial_theme(
            Some(theme::ThemeRequest::BuiltIn("nord".to_string())),
            &config,
            Path::new("/startup"),
            Some(Path::new("/config/ptop/config.toml")),
            Some(Path::new("/home/user")),
        )
        .unwrap();
        assert_eq!(theme.source, theme::ThemeSource::Packaged { id: "nord" });
    }

    #[test]
    fn configured_theme_files_resolve_relative_to_the_config_directory() {
        let directory = tempfile::tempdir().unwrap();
        let config_directory = directory.path().join("config/ptop");
        std::fs::create_dir_all(&config_directory).unwrap();
        let theme_path = config_directory.join("custom.toml");
        std::fs::write(
            &theme_path,
            include_str!("theme/builtins/btop.toml")
                .replace("name = \"btop\"", "name = \"Custom\""),
        )
        .unwrap();

        let config = config::AppConfig {
            theme_file: Some(PathBuf::from("custom.toml")),
            ..config::AppConfig::default()
        };
        let loaded = resolve_initial_theme(
            None,
            &config,
            Path::new("/different/startup"),
            Some(&config_directory.join("config.toml")),
            None,
        )
        .unwrap();

        assert_eq!(loaded.name, "Custom");
        assert_eq!(loaded.source, theme::ThemeSource::File { path: theme_path });
    }

    #[test]
    fn tilde_theme_paths_require_an_available_home_directory() {
        let error = resolve_theme_path(
            Path::new("~/themes/custom.toml"),
            Path::new("/startup"),
            None,
        )
        .unwrap_err();
        assert!(error.contains("no home directory"));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_cli_theme_file_names_do_not_panic() {
        use std::os::unix::ffi::OsStringExt;

        let directory = tempfile::tempdir().unwrap();
        let filename = OsString::from_vec(vec![b't', 0xff, b'.', b't']);
        let path = directory.path().join(&filename);
        let write_result = std::fs::write(&path, include_str!("theme/builtins/btop.toml"));
        let request = theme_request_from_args(&[
            OsString::from("ptop"),
            OsString::from("--theme-file"),
            filename,
        ])
        .unwrap();
        let load_result = resolve_initial_theme(
            request,
            &config::AppConfig::default(),
            directory.path(),
            None,
            None,
        );

        match write_result {
            Ok(()) => assert_eq!(
                load_result.unwrap().source,
                theme::ThemeSource::File { path }
            ),
            Err(error) => {
                #[cfg(target_os = "macos")]
                {
                    assert_eq!(error.raw_os_error(), Some(libc::EILSEQ));
                    let load_error = load_result.unwrap_err();
                    assert!(load_error.contains("theme file"), "{load_error}");
                }
                #[cfg(not(target_os = "macos"))]
                panic!("failed to create non-UTF-8 theme filename: {error}");
            }
        }
    }

    #[test]
    fn product_entry_constructs_the_app() {
        let cfg = config::AppConfig::default();
        let app = build_app(theme::Theme::default(), &cfg);
        assert!(app.sessions.is_empty());
    }

    #[test]
    fn text_values_distinguish_unknown_exact_inferred_estimated_and_partial() {
        let mut app = App::new(theme::Theme::default(), config::PanelVisibility::default());
        demo::populate_demo(&mut app);
        let session = app.sessions.first_mut().unwrap();
        session.context_percent = 0.0;
        session.context_window = 0;
        session.total_input_tokens = 0;
        session.total_output_tokens = 0;
        session.total_cache_read = 0;
        session.total_cache_create = 0;
        session.telemetry = Some(model::SessionTelemetry::process_only(1));

        assert_eq!(format_context_value(session), "—");
        assert_eq!(format_token_value(session), "—");

        let telemetry = session.telemetry.as_mut().unwrap();
        telemetry.context.precision = TelemetryPrecision::Exact;
        telemetry.context.completeness = TelemetryCompleteness::Complete;
        telemetry.context_details.tokens = Some(0);
        telemetry.usage.precision = TelemetryPrecision::Exact;
        telemetry.usage.completeness = TelemetryCompleteness::Complete;
        session.context_window = 200_000;
        assert_eq!(format_context_value(session).trim(), "0%");
        assert_eq!(format_token_value(session), "0");

        session.context_percent = 42.0;
        session.total_input_tokens = 14;
        let telemetry = session.telemetry.as_mut().unwrap();
        telemetry.context.precision = TelemetryPrecision::Inferred;
        telemetry.usage.precision = TelemetryPrecision::Inferred;
        assert_eq!(format_context_value(session), "~ 42%");
        assert_eq!(format_token_value(session), "~14");

        let telemetry = session.telemetry.as_mut().unwrap();
        telemetry.context.precision = TelemetryPrecision::Estimated;
        telemetry.usage.precision = TelemetryPrecision::Estimated;
        telemetry.usage.completeness = TelemetryCompleteness::Partial;
        assert_eq!(format_context_value(session), "≈ 42%");
        assert_eq!(format_token_value(session), "≈14+");
    }

    #[test]
    fn enter_jump_failure_renders_footer_status() {
        let mut app = App::new(theme::Theme::default(), config::PanelVisibility::default());
        demo::populate_demo(&mut app);

        handle_key_press(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            false,
            false,
            |_| JumpOutcome::Failed("cmux: socket broken; restart cmux".to_string()),
        );

        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui::draw(f, &app)).unwrap();
        let text = format!("{}", terminal.backend());

        assert!(text.contains("cmux: socket broken; restart cmux"));
        assert!(!text.contains("Broken pipe"));
        assert!(!text.contains("select-workspace"));
    }
}
