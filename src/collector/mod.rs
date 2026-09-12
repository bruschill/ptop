pub mod pi;
mod pi_subagents;
pub mod process;

use crate::model::{AgentSession, OrphanPort, SessionStatus};
use std::collections::{HashMap, HashSet};

/// Strip control characters and Unicode bidi override/isolate marks before
/// bounded lifecycle labels are rendered in the terminal.
pub(crate) fn sanitize_terminal_text(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    *c,
                    '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}'
                )
        })
        .collect()
}

/// Process data fetched once per tick for Pi collection and tests.
pub(crate) struct SharedProcessData {
    pub(crate) process_info: HashMap<u32, process::ProcInfo>,
    pub(crate) children_map: HashMap<u32, Vec<u32>>,
    pub(crate) ports: HashMap<u32, Vec<u16>>,
    /// True on slow poll ticks (every 5 ticks, about 10 seconds).
    pub(crate) slow_tick: bool,
}

impl SharedProcessData {
    /// Fetch process info every tick, but reuse cached ports when provided.
    fn fetch(cached_ports: Option<&HashMap<u32, Vec<u16>>>, slow_tick: bool) -> Self {
        let process_info = process::get_process_info();
        let children_map = process::get_children_map(&process_info);
        let ports = match cached_ports {
            Some(ports) => ports.clone(),
            None => process::get_listening_ports(),
        };
        Self {
            process_info,
            children_map,
            ports,
            slow_tick,
        }
    }
}

/// Info about a child process that owns an open port, tracked for orphan detection.
#[derive(Clone)]
struct TrackedPortChild {
    port: u16,
    command: String,
    project_name: String,
}

/// Collects Pi sessions and shared process enrichment.
pub struct Collector {
    pi: pi::PiCollector,
    tick_count: u32,
    cached_ports: HashMap<u32, Vec<u16>>,
    /// PID set snapshot from the last port scan. A change invalidates the cache.
    cached_port_pids: Vec<u32>,
    cached_git: HashMap<String, (u32, u32)>,
    /// Port-owning children from previous ticks, keyed by child PID.
    tracked_port_children: HashMap<u32, TrackedPortChild>,
}

/// How often to refresh expensive I/O (in ticks). 5 ticks × 2s = 10s.
const SLOW_POLL_INTERVAL: u32 = 5;

impl Collector {
    pub fn new() -> Self {
        Self {
            pi: pi::PiCollector::new(),
            tick_count: SLOW_POLL_INTERVAL,
            cached_ports: HashMap::new(),
            cached_port_pids: Vec::new(),
            cached_git: HashMap::new(),
            tracked_port_children: HashMap::new(),
        }
    }

    /// Collect live Pi sessions and any still-listening orphan child ports.
    pub fn collect(&mut self) -> (Vec<AgentSession>, Vec<OrphanPort>) {
        let slow_tick = self.tick_count >= SLOW_POLL_INTERVAL;
        if slow_tick {
            self.tick_count = 0;
        }
        self.tick_count += 1;

        // Refresh ports on the slow tick or when the PID set changes.
        let fresh_process = SharedProcessData::fetch(Some(&self.cached_ports), slow_tick);
        let mut current_pids: Vec<u32> = fresh_process.process_info.keys().copied().collect();
        current_pids.sort_unstable();
        let pids_changed = current_pids != self.cached_port_pids;

        let shared = if slow_tick || pids_changed {
            let shared = SharedProcessData::fetch(None, slow_tick);
            self.cached_ports = shared.ports.clone();
            self.cached_port_pids = current_pids;
            shared
        } else {
            fresh_process
        };

        let mut sessions = self.pi.collect(&shared);

        // Refresh Git stats only on slow ticks. Compute new directories on demand.
        if slow_tick {
            self.cached_git.clear();
            for session in &mut sessions {
                let stats = process::collect_git_stats(&session.cwd);
                self.cached_git.insert(session.cwd.clone(), stats);
                session.git_added = stats.0;
                session.git_modified = stats.1;
            }
        } else {
            for session in &mut sessions {
                let stats = self
                    .cached_git
                    .get(&session.cwd)
                    .copied()
                    .unwrap_or_else(|| {
                        let stats = process::collect_git_stats(&session.cwd);
                        self.cached_git.insert(session.cwd.clone(), stats);
                        stats
                    });
                session.git_added = stats.0;
                session.git_modified = stats.1;
            }
        }

        sessions.retain(|session| !matches!(session.status, SessionStatus::Done));
        sessions.sort_by_key(|session| std::cmp::Reverse(session.started_at));

        // Track child-owned ports while their Pi session is live.
        let mut live_child_pids = HashSet::new();
        for session in &sessions {
            for child in &session.children {
                live_child_pids.insert(child.pid);
                if let Some(port) = child.port {
                    self.tracked_port_children.insert(
                        child.pid,
                        TrackedPortChild {
                            port,
                            command: child.command.clone(),
                            project_name: session.project_name.clone(),
                        },
                    );
                }
            }
        }

        // A tracked child becomes an orphan only while it remains alive and listening.
        let mut orphan_ports = Vec::new();
        let mut stale_pids = Vec::new();
        for (pid, tracked) in &self.tracked_port_children {
            if live_child_pids.contains(pid) {
                continue;
            }
            let still_listening = shared
                .ports
                .get(pid)
                .is_some_and(|ports| ports.contains(&tracked.port));
            if shared.process_info.contains_key(pid) && still_listening {
                orphan_ports.push(OrphanPort {
                    port: tracked.port,
                    pid: *pid,
                    command: tracked.command.clone(),
                    project_name: tracked.project_name.clone(),
                });
            } else {
                stale_pids.push(*pid);
            }
        }
        for pid in stale_pids {
            self.tracked_port_children.remove(&pid);
        }
        orphan_ports.sort_by_key(|orphan| orphan.port);

        (sessions, orphan_ports)
    }
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}
