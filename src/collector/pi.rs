use super::{process, AgentCollector, SharedProcessData};
use crate::model::{AgentSession, ChildProcess, SessionStatus, SessionTelemetry};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

/// Passive collector for local Pi coding-agent processes.
///
/// Phase 1 deliberately stops at process metadata. Session JSONL attachment is
/// added only after ownership can be proved with high-confidence evidence.
pub struct PiCollector {
    first_seen_ms: HashMap<u32, u64>,
    cwd_cache: HashMap<u32, String>,
    start_id_cache: HashMap<u32, Option<String>>,
}

impl PiCollector {
    pub fn new() -> Self {
        Self {
            first_seen_ms: HashMap::new(),
            cwd_cache: HashMap::new(),
            start_id_cache: HashMap::new(),
        }
    }

    fn collect_sessions(&mut self, shared: &SharedProcessData) -> Vec<AgentSession> {
        let pi_pids = top_level_pi_pids(&shared.process_info);
        let live: HashSet<u32> = pi_pids.iter().copied().collect();
        self.first_seen_ms.retain(|pid, _| live.contains(pid));
        self.cwd_cache.retain(|pid, _| live.contains(pid));
        self.start_id_cache.retain(|pid, _| live.contains(pid));

        let observed_at_ms = current_time_ms();
        for pid in &pi_pids {
            let start_id = process_start_id(*pid);
            let pid_reused = self
                .start_id_cache
                .get(pid)
                .is_some_and(|cached| cached != &start_id);
            if pid_reused {
                self.first_seen_ms.remove(pid);
                self.cwd_cache.remove(pid);
            }
            self.start_id_cache.insert(*pid, start_id);
            self.first_seen_ms.entry(*pid).or_insert(observed_at_ms);
            if shared.slow_tick || !self.cwd_cache.contains_key(pid) {
                let cwd = process_cwd(*pid).unwrap_or_default();
                self.cwd_cache.insert(*pid, cwd);
            }
        }

        pi_pids
            .into_iter()
            .filter_map(|pid| {
                let proc = shared.process_info.get(&pid)?;
                let cwd = self.cwd_cache.get(&pid).cloned().unwrap_or_default();
                let project_name = process::last_path_segment(&cwd)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("pi-{pid}"));

                let process_start_id = self.start_id_cache.get(&pid).cloned().flatten();
                Some(AgentSession {
                    agent_cli: "pi",
                    pid,
                    session_id: process_session_id(pid, process_start_id.as_deref()),
                    cwd,
                    project_name,
                    started_at: self
                        .first_seen_ms
                        .get(&pid)
                        .copied()
                        .unwrap_or(observed_at_ms),
                    status: SessionStatus::Unknown,
                    model: String::new(),
                    effort: String::new(),
                    context_percent: 0.0,
                    total_input_tokens: 0,
                    total_output_tokens: 0,
                    total_cache_read: 0,
                    total_cache_create: 0,
                    turn_count: 0,
                    current_tasks: vec!["session telemetry unavailable".to_string()],
                    mem_mb: proc.rss_kb / 1024,
                    version: String::new(),
                    git_branch: String::new(),
                    git_added: 0,
                    git_modified: 0,
                    token_history: Vec::new(),
                    context_history: Vec::new(),
                    compaction_count: 0,
                    context_window: 0,
                    subagents: Vec::new(),
                    mem_file_count: 0,
                    mem_line_count: 0,
                    children: collect_children(pid, shared),
                    initial_prompt: String::new(),
                    first_assistant_text: String::new(),
                    chat_messages: Vec::new(),
                    tool_calls: Vec::new(),
                    pending_since_ms: 0,
                    thinking_since_ms: 0,
                    file_accesses: Vec::new(),
                    config_root: String::new(),
                    telemetry: Some(SessionTelemetry::process_only(observed_at_ms)),
                    process_start_id,
                })
            })
            .collect()
    }
}

impl Default for PiCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentCollector for PiCollector {
    fn collect(&mut self, shared: &SharedProcessData) -> Vec<AgentSession> {
        self.collect_sessions(shared)
    }
}

/// Match only executable-position Pi commands. This accepts the `pi` wrapper
/// and Node/Bun execution of the installed package entry point. It does not
/// match later arguments, unrelated `pi` substrings, or arbitrary `cli.js`
/// files.
pub fn is_pi_command(command: &str) -> bool {
    let tokens = command_tokens(command);
    is_pi_argv(&tokens)
}

fn is_pi_argv(tokens: &[String]) -> bool {
    let Some(executable) = tokens.first() else {
        return false;
    };
    let executable_name = binary_name(executable);

    if binary_equals(executable_name, "pi") {
        return true;
    }

    if binary_equals(executable_name, "env") {
        let nested: Vec<String> = tokens
            .iter()
            .skip(1)
            .skip_while(|token| token.starts_with('-'))
            .cloned()
            .collect();
        return is_pi_argv(&nested);
    }

    let is_bun = binary_equals(executable_name, "bun");
    if !is_bun
        && !["node", "nodejs"]
            .iter()
            .any(|name| binary_equals(executable_name, name))
    {
        return false;
    }

    runtime_entry_arg(tokens, is_bun).is_some_and(is_installed_pi_entry)
}

fn runtime_entry_arg(tokens: &[String], is_bun: bool) -> Option<&str> {
    let mut index = 1;
    let mut saw_bun_run = false;

    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return tokens.get(index + 1).map(String::as_str);
        }
        if is_bun && !saw_bun_run && token == "run" {
            saw_bun_run = true;
            index += 1;
            continue;
        }
        if token.starts_with('-') {
            if runtime_option_takes_value(token, is_bun) {
                index += 2;
                continue;
            }
            if runtime_flag_is_supported(token, is_bun) || token.contains('=') {
                index += 1;
                continue;
            }
            return None;
        }
        return Some(token);
    }

    None
}

fn runtime_option_takes_value(option: &str, is_bun: bool) -> bool {
    matches!(
        option,
        "-r" | "--require"
            | "--import"
            | "--loader"
            | "--experimental-loader"
            | "--conditions"
            | "--env-file"
            | "--icu-data-dir"
            | "--openssl-config"
    ) || (is_bun && matches!(option, "--cwd" | "--preload"))
}

fn runtime_flag_is_supported(option: &str, is_bun: bool) -> bool {
    matches!(
        option,
        "--enable-source-maps"
            | "--no-warnings"
            | "--preserve-symlinks"
            | "--preserve-symlinks-main"
            | "--experimental-strip-types"
            | "--trace-warnings"
            | "--watch"
    ) || option.starts_with("--inspect")
        || (is_bun && matches!(option, "--bun" | "--hot" | "--silent"))
}

fn is_installed_pi_entry(token: &str) -> bool {
    let normalized = token
        .trim_matches(['\'', '"'])
        .replace('\\', "/")
        .to_ascii_lowercase();
    let package_marker = "/@earendil-works/pi-coding-agent/";
    let Some((_, suffix)) = normalized.split_once(package_marker) else {
        return false;
    };
    matches!(
        suffix,
        "dist/bundle/cli.js" | "dist/bun/cli.js" | "dist/cli.js"
    )
}

#[cfg(windows)]
fn binary_equals(actual: &str, expected: &str) -> bool {
    let stem = if actual.to_ascii_lowercase().ends_with(".exe") {
        &actual[..actual.len() - 4]
    } else {
        actual
    };
    stem.eq_ignore_ascii_case(expected)
}

#[cfg(not(windows))]
fn binary_equals(actual: &str, expected: &str) -> bool {
    actual == expected || actual.strip_suffix(".exe") == Some(expected)
}

fn binary_name(token: &str) -> &str {
    token
        .trim_matches(['\'', '"'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(token)
}

fn command_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut chars = command.chars().peekable();

    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some(open), '\\') if chars.peek() == Some(&open) => {
                current.push(open);
                chars.next();
            }
            (None, '\'' | '"') => quote = Some(ch),
            (Some(open), close) if open == close => quote = None,
            (None, ch) if ch.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn top_level_pi_pids(process_info: &HashMap<u32, process::ProcInfo>) -> Vec<u32> {
    let candidates: HashSet<u32> = process_info
        .values()
        .filter(|proc| is_pi_command(&proc.command))
        .map(|proc| proc.pid)
        .collect();

    let mut roots: Vec<u32> = candidates
        .iter()
        .copied()
        .filter(|pid| !has_pi_ancestor(*pid, &candidates, process_info))
        .collect();
    roots.sort_unstable();
    roots
}

fn has_pi_ancestor(
    pid: u32,
    candidates: &HashSet<u32>,
    process_info: &HashMap<u32, process::ProcInfo>,
) -> bool {
    let mut current = pid;
    let mut visited = HashSet::new();
    while visited.insert(current) {
        let Some(proc) = process_info.get(&current) else {
            return false;
        };
        if candidates.contains(&proc.ppid) {
            return true;
        }
        if proc.ppid <= 1 {
            return false;
        }
        current = proc.ppid;
    }
    false
}

fn collect_children(pid: u32, shared: &SharedProcessData) -> Vec<ChildProcess> {
    let mut children = Vec::new();
    let mut stack = shared.children_map.get(&pid).cloned().unwrap_or_default();
    let mut visited = HashSet::new();

    while let Some(child_pid) = stack.pop() {
        if !visited.insert(child_pid) {
            continue;
        }
        if let Some(proc) = shared.process_info.get(&child_pid) {
            children.push(ChildProcess {
                pid: child_pid,
                command: proc.command.clone(),
                mem_kb: proc.rss_kb,
                port: shared
                    .ports
                    .get(&child_pid)
                    .and_then(|ports| ports.first().copied()),
            });
            if let Some(descendants) = shared.children_map.get(&child_pid) {
                stack.extend(descendants);
            }
        }
    }

    children.sort_by_key(|child| child.pid);
    children
}

#[cfg(target_os = "linux")]
fn process_cwd(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(target_os = "windows")]
fn process_cwd(_pid: u32) -> Option<String> {
    // ProcInfo does not yet expose a trustworthy Windows cwd. Keep the row
    // process-only instead of guessing ownership from a session directory.
    None
}

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
fn process_cwd(pid: u32) -> Option<String> {
    let output = std::process::Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix('n').map(str::to_string))
}

#[cfg(target_os = "linux")]
pub(crate) fn process_start_id(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = &stat[stat.rfind(')')? + 2..];
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    fields.get(19).map(|value| format!("linux:{value}"))
}

#[cfg(target_os = "windows")]
pub(crate) fn process_start_id(_pid: u32) -> Option<String> {
    // Windows v1 stays process-only and does not claim PID-reuse protection
    // until ProcInfo exposes a start identity from its existing sysinfo scan.
    None
}

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
pub(crate) fn process_start_id(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .env("LC_ALL", "C")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then(|| format!("unix:{value}"))
}

fn process_session_id(pid: u32, start_id: Option<&str>) -> String {
    use std::hash::{Hash, Hasher};

    match start_id {
        Some(start_id) => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            start_id.hash(&mut hasher);
            format!("process-{pid}-{:x}", hasher.finish())
        }
        None => format!("process-{pid}"),
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::process::ProcInfo;
    use crate::model::TelemetryPrecision;

    fn proc(pid: u32, ppid: u32, command: &str) -> ProcInfo {
        ProcInfo {
            pid,
            ppid,
            rss_kb: 1024,
            cpu_pct: 0.0,
            command: command.to_string(),
        }
    }

    fn shared(procs: Vec<ProcInfo>) -> SharedProcessData {
        let process_info: HashMap<u32, ProcInfo> =
            procs.into_iter().map(|proc| (proc.pid, proc)).collect();
        let children_map = process::get_children_map(&process_info);
        SharedProcessData {
            process_info,
            children_map,
            ports: HashMap::new(),
            slow_tick: false,
            mcp_server_pids: HashSet::new(),
            mcp_owned_rollouts: HashSet::new(),
            mcp_suppress: true,
            desktop_rollout_fd_map: HashMap::new(),
        }
    }

    #[test]
    fn matches_supported_pi_launches() {
        assert!(is_pi_command("pi"));
        assert!(is_pi_command("/usr/local/bin/pi --mode rpc --no-session"));
        assert!(is_pi_command("/usr/bin/env pi -p hello"));
        assert!(is_pi_command(
            "node /opt/lib/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js"
        ));
        assert!(is_pi_command(
            "bun run C:\\Users\\me\\node_modules\\@earendil-works\\pi-coding-agent\\dist\\bun\\cli.js --mode rpc"
        ));
        assert!(is_pi_command(
            r#""C:\Program Files\nodejs\node.exe" "C:\Users\me\node_modules\@earendil-works\pi-coding-agent\dist\cli.js""#
        ));
        assert!(is_pi_command(
            "node --require preload.js /opt/node_modules/@earendil-works/pi-coding-agent/dist/cli.js"
        ));
    }

    #[test]
    fn rejects_bare_strings_and_unrelated_cli_scripts() {
        assert!(!is_pi_command("pip install pi"));
        #[cfg(not(windows))]
        assert!(!is_pi_command("Pi"));
        assert!(!is_pi_command("bash -c pi"));
        assert!(!is_pi_command("node ./pi"));
        assert!(!is_pi_command("node /tmp/cli.js --name pi"));
        assert!(!is_pi_command(
            "node app.js /opt/node_modules/@earendil-works/pi-coding-agent/dist/cli.js"
        ));
        assert!(!is_pi_command(
            "node --unknown-option /opt/node_modules/@earendil-works/pi-coding-agent/dist/cli.js app.js"
        ));
        assert!(!is_pi_command(
            "node /opt/pi-coding-agent/dist/bundle/cli.js"
        ));
    }

    #[test]
    fn nested_pi_process_is_not_a_top_level_row() {
        let mut processes = HashMap::new();
        for proc in [
            proc(10, 1, "pi"),
            proc(11, 10, "sh"),
            proc(12, 11, "pi --mode rpc"),
            proc(20, 1, "pi"),
        ] {
            processes.insert(proc.pid, proc);
        }
        assert_eq!(top_level_pi_pids(&processes), vec![10, 20]);
    }

    #[test]
    fn process_session_id_changes_with_process_start_identity() {
        assert_eq!(process_session_id(42, None), "process-42");
        assert_ne!(
            process_session_id(42, Some("start-a")),
            process_session_id(42, Some("start-b"))
        );
    }

    #[test]
    fn identical_command_pid_reuse_replaces_cached_identity() {
        let shared = shared(vec![proc(10, 1, "pi")]);
        let mut collector = PiCollector::new();
        collector
            .start_id_cache
            .insert(10, Some("stale-start".to_string()));
        collector.first_seen_ms.insert(10, 1);
        collector.cwd_cache.insert(10, "/stale/cwd".to_string());

        let sessions = collector.collect_sessions(&shared);

        assert_eq!(sessions.len(), 1);
        assert_ne!(
            sessions[0].session_id,
            process_session_id(10, Some("stale-start"))
        );
        assert_ne!(sessions[0].started_at, 1);
    }

    #[test]
    fn process_only_rows_remain_distinct_for_same_cwd() {
        let shared = shared(vec![proc(10, 1, "pi"), proc(20, 1, "pi")]);
        let mut collector = PiCollector::new();
        collector.cwd_cache.insert(10, "/tmp/project".to_string());
        collector.cwd_cache.insert(20, "/tmp/project".to_string());

        let sessions = collector.collect_sessions(&shared);

        assert_eq!(sessions.len(), 2);
        assert_ne!(sessions[0].session_id, sessions[1].session_id);
        assert!(sessions.iter().all(|session| {
            session.agent_cli == "pi"
                && session.status == SessionStatus::Unknown
                && session.context_value().is_none()
                && session.usage_precision() == TelemetryPrecision::Unknown
        }));
    }
}
