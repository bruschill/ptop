use super::{
    pi_subagents::{pi_subagent_runner_run_id, PiSubagentParent, PiSubagentsCollector},
    process, AgentCollector, SharedProcessData,
};
use crate::model::{
    AgentSession, AttachmentConfidence, AttachmentState, ChildProcess, ContextTelemetryDetails,
    FleetTelemetry, SessionStatus, SessionTelemetry, SourceHealth, TelemetryCompleteness,
    TelemetryMetadata, TelemetryPrecision, UsageTelemetryDetails,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::process::Stdio;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::time::{Duration, Instant};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_TAIL_WORK_BYTES: usize = 2 * 1024 * 1024;
const MAX_TAIL_LINE_BYTES: usize = 1024 * 1024;
const MAX_ATTACHMENT_CANDIDATES_PER_COLLECT: usize = 128;
const MAX_PROCESSES_SCANNED_PER_COLLECT: usize = 128;
const MAX_OPEN_FDS_SCANNED_PER_COLLECT: usize = 4096;
#[cfg(any(target_os = "linux", target_vendor = "apple", test))]
const MAX_HERDR_ROOTS: usize = 32;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const MAX_HERDR_JSON_BYTES: usize = 256 * 1024;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const MAX_HERDR_ENV_BYTES: usize = 256 * 1024;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const HERDR_COMMAND_TIMEOUT_MS: u64 = 300;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const HERDR_DISCOVERY_BUDGET_MS: u64 = 1_000;
#[cfg(target_vendor = "apple")]
const MAX_LSOF_RECORD_BYTES: usize = 4096;
const HEADER_READ_CHUNK_BYTES: usize = 4096;
const MAX_TELEMETRY_ERROR_BYTES: usize = 160;
const MAX_SEMANTIC_ENTRIES: usize = 8_192;
const MAX_SEMANTIC_ID_BYTES: usize = 256;
const MAX_SEMANTIC_PARENT_BYTES: usize = 256;
const MAX_SEMANTIC_METADATA_BYTES: usize = 256;
const MAX_MODEL_CATALOG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_MODEL_CATALOG_ENTRIES: usize = 1_024;

/// Passive collector for local Pi coding-agent processes.
///
/// Process rows exist without telemetry. Session JSONL is attached only after
/// ownership is proved with high-confidence evidence.
pub struct PiCollector {
    first_seen_ms: HashMap<u32, u64>,
    cwd_cache: HashMap<u32, String>,
    start_id_cache: HashMap<u32, Option<String>>,
    attachments: HashMap<u32, PiAttachment>,
    herdr_sessions: HashMap<u32, HerdrSession>,
    herdr_ambiguous: HashSet<u32>,
    herdr_sessions_initialized: bool,
    tails: HashMap<PathBuf, PiTail>,
    model_catalogs: HashMap<PathBuf, ModelCatalogCache>,
    subagents: PiSubagentsCollector,
}

#[derive(Debug, Clone)]
struct PiAttachment {
    path: PathBuf,
    session_id: String,
    start_id: Option<String>,
    header_cwd: String,
    identity: FileIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PiHeader {
    session_id: String,
    cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume_serial_number: Option<u32>,
    #[cfg(windows)]
    file_index: Option<u64>,
    #[cfg(not(unix))]
    modified_ms: Option<u64>,
}

#[derive(Debug, Clone)]
struct PiTail {
    identity: FileIdentity,
    header_session_id: String,
    header_cwd: String,
    offset: u64,
    /// A hash of a small suffix immediately before `offset`, never transcript content.
    boundary_fingerprint: Option<u64>,
    discard_oversized_line: bool,
    /// A parse failure occurred since this file identity was last cleanly scanned.
    parse_limited: bool,
    source_updated_at_ms: Option<u64>,
    last_successful_parse_at_ms: Option<u64>,
    complete: bool,
    error: Option<String>,
    semantic: PiSemantic,
}

#[derive(Debug, Clone, Default)]
struct PiSemantic {
    entries: HashMap<String, PiEntry>,
    order: Vec<String>,
    limited: bool,
    invalid: bool,
}

#[derive(Debug, Clone)]
struct PiEntry {
    parent_id: Option<String>,
    kind: PiEntryKind,
    usage: Option<PiUsage>,
    context_chars: u64,
    provider: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    valid_baseline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PiEntryKind {
    Assistant,
    ToolResult,
    Compaction,
    BranchSummary,
    Other,
}

#[derive(Debug, Clone, Default)]
struct PiUsage {
    components: Option<(u64, u64, u64, u64)>,
    total_tokens: Option<u64>,
    cost: Option<f64>,
    invalid_cost: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogRevision {
    length: u64,
    modified: Option<SystemTime>,
    identity: FileIdentity,
}

#[derive(Debug, Clone, Default)]
struct ModelCatalogCache {
    store_revision: Option<CatalogRevision>,
    config_revision: Option<CatalogRevision>,
    /// Normalized provider/model window metadata only. Raw catalog JSON can contain secrets.
    windows: HashMap<(String, String), u64>,
    unavailable: bool,
}

#[derive(Debug)]
struct PiSessionData {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cost: Option<f64>,
    turns: u32,
    token_history: Vec<u64>,
    context_history: Vec<u64>,
    compactions: u32,
    provider: String,
    model: String,
    effort: String,
    context_tokens: Option<u64>,
    baseline_tokens: Option<u64>,
    trailing_tokens: Option<u64>,
    context_window: Option<u64>,
    context_precision: TelemetryPrecision,
    usage_completeness: TelemetryCompleteness,
    context_completeness: TelemetryCompleteness,
    context_reason: Option<String>,
    usage_reason: Option<String>,
    usage_available: bool,
    active_leaf_id: Option<String>,
}

impl Default for PiSessionData {
    fn default() -> Self {
        Self {
            input: 0,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            cost: None,
            turns: 0,
            token_history: Vec::new(),
            context_history: Vec::new(),
            compactions: 0,
            provider: String::new(),
            model: String::new(),
            effort: String::new(),
            context_tokens: None,
            baseline_tokens: None,
            trailing_tokens: None,
            context_window: None,
            context_precision: TelemetryPrecision::Unknown,
            usage_completeness: TelemetryCompleteness::Unknown,
            context_completeness: TelemetryCompleteness::Unknown,
            context_reason: None,
            usage_reason: None,
            usage_available: true,
            active_leaf_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AttachmentCandidate {
    path: PathBuf,
    expected_session_id: Option<String>,
}

struct AttachmentDiscoveryBudget {
    candidate_slots: usize,
    process_slots: usize,
    fd_slots: usize,
}

#[derive(Debug, Clone)]
struct AttachmentResult {
    attachment: Option<PiAttachment>,
    error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct HerdrSession {
    path: PathBuf,
    status: SessionStatus,
}

#[cfg(any(target_os = "linux", target_vendor = "apple", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct HerdrProcessMarker {
    pane_id: String,
    socket_path: String,
}

#[derive(Default)]
struct HerdrDiscovery {
    sessions: HashMap<u32, HerdrSession>,
    ambiguous: HashSet<u32>,
}

#[cfg(any(target_os = "linux", target_vendor = "apple", test))]
#[derive(Debug, Clone, PartialEq)]
struct HerdrPaneSnapshot {
    path: PathBuf,
    revision: u64,
    status: SessionStatus,
}

impl PiCollector {
    pub fn new() -> Self {
        Self {
            first_seen_ms: HashMap::new(),
            cwd_cache: HashMap::new(),
            start_id_cache: HashMap::new(),
            attachments: HashMap::new(),
            herdr_sessions: HashMap::new(),
            herdr_ambiguous: HashSet::new(),
            herdr_sessions_initialized: false,
            tails: HashMap::new(),
            model_catalogs: HashMap::new(),
            subagents: PiSubagentsCollector::new(),
        }
    }

    fn collect_sessions(&mut self, shared: &SharedProcessData) -> Vec<AgentSession> {
        let pi_pids = top_level_pi_pids(&shared.process_info);
        let live: HashSet<u32> = pi_pids.iter().copied().collect();
        self.first_seen_ms.retain(|pid, _| live.contains(pid));
        self.cwd_cache.retain(|pid, _| live.contains(pid));
        self.start_id_cache.retain(|pid, _| live.contains(pid));
        self.attachments.retain(|pid, _| live.contains(pid));
        self.herdr_sessions.retain(|pid, _| live.contains(pid));
        self.herdr_ambiguous.retain(|pid| live.contains(pid));

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
                self.attachments.remove(pid);
                self.herdr_sessions.remove(pid);
                self.herdr_ambiguous.remove(pid);
            }
            self.start_id_cache.insert(*pid, start_id);
            self.first_seen_ms.entry(*pid).or_insert(observed_at_ms);
            if shared.slow_tick || !self.cwd_cache.contains_key(pid) {
                let cwd = process_cwd(*pid).unwrap_or_default();
                self.cwd_cache.insert(*pid, cwd);
            }
        }
        if !self.herdr_sessions_initialized || shared.slow_tick {
            let discovery = discover_herdr_sessions(&live);
            self.herdr_sessions = discovery.sessions;
            self.herdr_ambiguous = discovery.ambiguous;
            self.herdr_sessions_initialized = true;
        }

        let mut read_budget = MAX_TAIL_WORK_BYTES;
        let attachment_results =
            self.resolve_attachments_with_budget(&pi_pids, shared, &mut read_budget);
        let owned_paths: HashSet<PathBuf> = attachment_results
            .values()
            .filter_map(|result| {
                result
                    .attachment
                    .as_ref()
                    .map(|attachment| attachment.path.clone())
            })
            .collect();
        self.tails.retain(|path, _| owned_paths.contains(path));
        let owned_agent_roots: HashSet<PathBuf> = owned_paths
            .iter()
            .filter_map(|path| pi_agent_root(path))
            .collect();
        self.model_catalogs
            .retain(|root, _| owned_agent_roots.contains(root));
        let fleet_parents: Vec<PiSubagentParent> = attachment_results
            .values()
            .filter_map(|result| {
                result
                    .attachment
                    .as_ref()
                    .map(|attachment| PiSubagentParent {
                        session_id: attachment.session_id.clone(),
                        session_file: attachment.path.clone(),
                    })
            })
            .collect();
        let live_pids: HashSet<u32> = shared.process_info.keys().copied().collect();
        let verified_runner_runs: HashMap<u32, String> = shared
            .process_info
            .iter()
            .filter_map(|(pid, process)| {
                pi_subagent_runner_run_id(&process.command).map(|run_id| (*pid, run_id))
            })
            .collect();
        let mut fleet_by_session = self.subagents.collect(
            &fleet_parents,
            &live_pids,
            &verified_runner_runs,
            observed_at_ms,
        );

        pi_pids
            .into_iter()
            .filter_map(|pid| {
                let proc = shared.process_info.get(&pid)?;
                let cwd = self.cwd_cache.get(&pid).cloned().unwrap_or_default();
                let status = self
                    .herdr_sessions
                    .get(&pid)
                    .map(|session| session.status.clone())
                    .unwrap_or(SessionStatus::Unknown);
                let project_name = process::last_path_segment(&cwd)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("pi-{pid}"));

                let process_start_id = self.start_id_cache.get(&pid).cloned().flatten();
                let attachment_result = attachment_results.get(&pid)?;
                let requested_attachment = attachment_result.attachment.as_ref();
                let (attachment, mut telemetry, data) = match requested_attachment {
                    Some(attachment) => match self.telemetry_for_attachment(
                        attachment,
                        observed_at_ms,
                        &mut read_budget,
                    ) {
                        Ok((telemetry, data)) => (Some(attachment), telemetry, data),
                        Err(error) => {
                            self.attachments.remove(&pid);
                            (
                                None,
                                process_only_telemetry(observed_at_ms, Some(error)),
                                PiSessionData::default(),
                            )
                        }
                    },
                    None => (
                        None,
                        process_only_telemetry(observed_at_ms, attachment_result.error.clone()),
                        PiSessionData::default(),
                    ),
                };
                if let Some(attachment) = attachment {
                    if let Some(fleet) = fleet_by_session.remove(&attachment.session_id) {
                        telemetry.fleet = fleet;
                    }
                }
                Some(AgentSession {
                    agent_cli: "pi",
                    pid,
                    session_id: attachment
                        .map(|attachment| attachment.session_id.clone())
                        .unwrap_or_else(|| process_session_id(pid, process_start_id.as_deref())),
                    cwd: if cwd.is_empty() {
                        attachment
                            .map(|attachment| attachment.header_cwd.clone())
                            .unwrap_or(cwd)
                    } else {
                        cwd
                    },
                    project_name,
                    started_at: self
                        .first_seen_ms
                        .get(&pid)
                        .copied()
                        .unwrap_or(observed_at_ms),
                    status,
                    model: data.model.clone(),
                    effort: data.effort.clone(),
                    context_percent: match (data.context_tokens, data.context_window) {
                        (Some(tokens), Some(window)) if window > 0 => {
                            tokens as f64 / window as f64 * 100.0
                        }
                        _ => 0.0,
                    },
                    total_input_tokens: data.input,
                    total_output_tokens: data.output,
                    total_cache_read: data.cache_read,
                    total_cache_create: data.cache_write,
                    turn_count: data.turns,
                    current_tasks: vec![if data.active_leaf_id.is_some() {
                        "persisted Pi session telemetry".to_string()
                    } else {
                        "session telemetry unavailable".to_string()
                    }],
                    mem_mb: proc.rss_kb / 1024,
                    version: String::new(),
                    git_branch: String::new(),
                    git_added: 0,
                    git_modified: 0,
                    token_history: data.token_history,
                    context_history: data.context_history,
                    compaction_count: data.compactions,
                    context_window: data.context_window.unwrap_or(0),
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
                    telemetry: Some(telemetry),
                    process_start_id,
                })
            })
            .collect()
    }

    #[cfg(test)]
    fn resolve_attachments(
        &mut self,
        roots: &[u32],
        shared: &SharedProcessData,
    ) -> HashMap<u32, AttachmentResult> {
        let mut budget = MAX_TAIL_WORK_BYTES;
        self.resolve_attachments_with_budget(roots, shared, &mut budget)
    }

    fn resolve_attachments_with_budget(
        &mut self,
        roots: &[u32],
        shared: &SharedProcessData,
        read_budget: &mut usize,
    ) -> HashMap<u32, AttachmentResult> {
        let mut candidates: HashMap<u32, Vec<AttachmentCandidate>> = HashMap::new();
        let mut candidate_limit_roots = HashSet::new();
        let mut discovery_budget = AttachmentDiscoveryBudget {
            candidate_slots: MAX_ATTACHMENT_CANDIDATES_PER_COLLECT,
            process_slots: MAX_PROCESSES_SCANNED_PER_COLLECT,
            fd_slots: MAX_OPEN_FDS_SCANNED_PER_COLLECT,
        };
        for &root in roots {
            let mut discovered = Vec::new();
            if let Some(session) = self.herdr_sessions.get(&root) {
                if discovery_budget.candidate_slots == 0 {
                    candidate_limit_roots.insert(root);
                } else {
                    discovered.push(AttachmentCandidate {
                        path: session.path.clone(),
                        expected_session_id: None,
                    });
                    discovery_budget.candidate_slots -= 1;
                }
            }
            if let Some(attachment) = self.attachments.get(&root) {
                if attachment.start_id == self.start_id_cache.get(&root).cloned().flatten() {
                    if discovery_budget.candidate_slots == 0 {
                        candidate_limit_roots.insert(root);
                    } else {
                        discovered.push(AttachmentCandidate {
                            path: attachment.path.clone(),
                            expected_session_id: Some(attachment.session_id.clone()),
                        });
                        discovery_budget.candidate_slots -= 1;
                    }
                }
            }
            let (new_candidates, limit_reached) =
                discover_attachment_candidates(root, shared, &mut discovery_budget, read_budget);
            discovered.extend(new_candidates);
            if limit_reached {
                candidate_limit_roots.insert(root);
            }
            candidates.insert(root, discovered);
        }

        let mut claims: HashMap<PathBuf, Vec<u32>> = HashMap::new();
        // Both canonical path and header ID are identities. Either collision is ambiguous.
        let mut id_claims: HashMap<String, Vec<(u32, PathBuf)>> = HashMap::new();
        let mut validated: HashMap<u32, Vec<(PathBuf, PiHeader, FileIdentity)>> = HashMap::new();
        let mut errors: HashMap<u32, String> = candidate_limit_roots
            .into_iter()
            .map(|root| (root, "session candidate limit exceeded".to_string()))
            .collect();
        for root in roots
            .iter()
            .filter(|root| self.herdr_ambiguous.contains(root))
        {
            errors.insert(
                *root,
                "Herdr pane ownership changed during session discovery".to_string(),
            );
        }
        for &root in roots {
            let cwd = self
                .cwd_cache
                .get(&root)
                .map(String::as_str)
                .unwrap_or_default();
            let prior = self.attachments.get(&root).cloned();
            // Canonicalize and merge evidence before parsing each header once. Conflicting
            // marker IDs for one path are ambiguous even if one happens to match the header.
            let mut canonical_candidates: HashMap<PathBuf, Option<String>> = HashMap::new();
            for candidate in candidates.get(&root).into_iter().flatten() {
                let canonical = match canonical_regular_jsonl(&candidate.path) {
                    Ok(path) => path,
                    Err(error) => {
                        errors.entry(root).or_insert(error);
                        continue;
                    }
                };
                match canonical_candidates.entry(canonical) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(candidate.expected_session_id.clone());
                    }
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        match (entry.get(), candidate.expected_session_id.as_ref()) {
                            (Some(existing), Some(candidate_id)) if existing != candidate_id => {
                                errors.insert(
                                    root,
                                    "conflicting session markers name one file".to_string(),
                                );
                            }
                            (None, Some(candidate_id)) => {
                                entry.insert(Some(candidate_id.clone()));
                            }
                            _ => {}
                        }
                    }
                }
            }
            for (path, expected_session_id) in canonical_candidates {
                let candidate = AttachmentCandidate {
                    path,
                    expected_session_id,
                };
                match validate_candidate_with_budget(&candidate, cwd, read_budget) {
                    Ok((path, header, identity)) => {
                        if prior.as_ref().is_some_and(|attachment| {
                            attachment.path == path
                                && (attachment.session_id != header.session_id
                                    || attachment.identity != identity)
                        }) {
                            errors.insert(root, "session file identity changed".to_string());
                        } else {
                            claims.entry(path.clone()).or_default().push(root);
                            id_claims
                                .entry(header.session_id.clone())
                                .or_default()
                                .push((root, path.clone()));
                            validated
                                .entry(root)
                                .or_default()
                                .push((path, header, identity));
                        }
                    }
                    Err(error) => {
                        errors.entry(root).or_insert(error);
                    }
                }
            }
        }

        let mut results = HashMap::new();
        for &root in roots {
            let start_id = self.start_id_cache.get(&root).cloned().flatten();
            let mut matches = validated.remove(&root).unwrap_or_default();
            let path_conflict = matches
                .iter()
                .any(|(path, _, _)| claims.get(path).is_some_and(|owners| owners.len() > 1));
            let id_conflict = matches.iter().any(|(path, header, _)| {
                id_claims.get(&header.session_id).is_some_and(|owners| {
                    owners.len() > 1 && owners.iter().any(|(_, known_path)| known_path != path)
                })
            });
            matches.retain(|(path, header, _)| {
                let path_unique = claims.get(path).is_some_and(|owners| owners.len() == 1);
                let id_unique = id_claims.get(&header.session_id).is_some_and(|owners| {
                    owners.len() == 1 || owners.iter().all(|(_, known_path)| known_path == path)
                });
                path_unique && id_unique
            });
            if errors.contains_key(&root) || path_conflict || id_conflict {
                matches.clear();
            }
            if matches.len() == 1 {
                let (path, header, identity) = matches.pop().expect("one validated attachment");
                let attachment = PiAttachment {
                    path,
                    session_id: header.session_id,
                    start_id,
                    header_cwd: header.cwd,
                    identity,
                };
                self.attachments.insert(root, attachment.clone());
                results.insert(
                    root,
                    AttachmentResult {
                        attachment: Some(attachment),
                        error: None,
                    },
                );
            } else {
                self.attachments.remove(&root);
                let error = if path_conflict {
                    Some("session file is claimed by multiple Pi roots".to_string())
                } else if id_conflict {
                    Some("session ID is claimed by multiple files or Pi roots".to_string())
                } else if matches.len() > 1 {
                    Some("multiple session files are attributed to one Pi root".to_string())
                } else {
                    errors.remove(&root)
                };
                results.insert(
                    root,
                    AttachmentResult {
                        attachment: None,
                        error,
                    },
                );
            }
        }
        results
    }

    fn telemetry_for_attachment(
        &mut self,
        attachment: &PiAttachment,
        observed_at_ms: u64,
        tail_budget: &mut usize,
    ) -> Result<(SessionTelemetry, PiSessionData), String> {
        let expected_header = (&attachment.session_id[..], &attachment.header_cwd[..]);
        let (
            mut data,
            context_is_complete,
            tail_error,
            source_updated_at_ms,
            last_successful_parse_at_ms,
        ) = {
            let tail = self.tail_session_with_expected_header(
                &attachment.path,
                observed_at_ms,
                tail_budget,
                Some(expected_header),
                Some(&attachment.identity),
            )?;
            let complete = tail.complete
                && !tail.parse_limited
                && !tail.semantic.limited
                && !tail.semantic.invalid;
            (
                tail.semantic.session_data(complete),
                complete,
                tail.error.clone(),
                tail.source_updated_at_ms,
                tail.last_successful_parse_at_ms,
            )
        };
        data.context_window = self.resolve_context_window(attachment);
        if data.context_tokens.is_none() {
            data.context_precision = TelemetryPrecision::Unknown;
        }
        if data.context_window.is_none() {
            data.context_reason = Some(match data.context_reason.take() {
                Some(reason) => format!("{reason}; provider/model context window is unavailable"),
                None => "provider/model context window is unavailable".to_string(),
            });
        }
        if !context_is_complete {
            data.usage_completeness = TelemetryCompleteness::Partial;
            data.context_completeness = TelemetryCompleteness::Partial;
            data.context_tokens = None;
            data.baseline_tokens = None;
            data.trailing_tokens = None;
            data.active_leaf_id = None;
            data.context_precision = TelemetryPrecision::Unknown;
            append_reason(
                &mut data.context_reason,
                "context unavailable because session parsing is incomplete",
            );
        }
        let source_health = if tail_error.is_some() {
            SourceHealth::Error
        } else {
            SourceHealth::Healthy
        };
        let metadata = TelemetryMetadata {
            precision: data.context_precision,
            completeness: data.context_completeness,
            provenance: "owned Pi session JSONL".to_string(),
            source_updated_at_ms,
            observed_at_ms,
            last_successful_parse_at_ms,
            stale: false,
        };
        Ok((
            SessionTelemetry {
                attachment: AttachmentState::Attached,
                attachment_confidence: AttachmentConfidence::High,
                source_health,
                error: tail_error.clone(),
                context: metadata.clone(),
                usage: TelemetryMetadata {
                    precision: if !data.usage_available
                        || data.usage_completeness == TelemetryCompleteness::Unknown
                    {
                        TelemetryPrecision::Unknown
                    } else {
                        TelemetryPrecision::Exact
                    },
                    completeness: data.usage_completeness,
                    provenance: "observed Pi session JSONL entries".to_string(),
                    source_updated_at_ms,
                    observed_at_ms,
                    last_successful_parse_at_ms,
                    stale: false,
                },
                context_details: ContextTelemetryDetails {
                    tokens: data.context_tokens,
                    baseline_tokens: data.baseline_tokens,
                    trailing_tokens: data.trailing_tokens,
                    active_leaf_id: data.active_leaf_id.clone(),
                    provider: (!data.provider.is_empty()).then_some(data.provider.clone()),
                    reason: data.context_reason.clone(),
                },
                usage_details: UsageTelemetryDetails {
                    reported_cost: data.cost,
                    reason: data.usage_reason.clone(),
                },
                fleet: FleetTelemetry::unavailable(
                    observed_at_ms,
                    "pi-subagents status root unavailable",
                ),
            },
            data,
        ))
    }

    fn resolve_context_window(&mut self, attachment: &PiAttachment) -> Option<u64> {
        let tail = self.tails.get(&attachment.path)?;
        let data = tail.semantic.session_data(
            tail.complete
                && !tail.parse_limited
                && !tail.semantic.limited
                && !tail.semantic.invalid,
        );
        if data.provider.is_empty() || data.model.is_empty() {
            return None;
        }
        let root = pi_agent_root(&attachment.path)?;
        let catalog = self.model_catalogs.entry(root.clone()).or_default();
        refresh_model_catalog(catalog, &root);
        if catalog.unavailable {
            return None;
        }
        model_window_from_catalog(catalog, &data.provider, &data.model)
    }

    #[cfg(test)]
    fn tail_session(&mut self, path: &Path, observed_at_ms: u64) -> Option<&PiTail> {
        let mut budget = MAX_TAIL_WORK_BYTES;
        self.tail_session_with_expected_header(path, observed_at_ms, &mut budget, None, None)
            .ok()
    }

    #[cfg(test)]
    fn tail_session_with_budget(
        &mut self,
        path: &Path,
        observed_at_ms: u64,
        budget: &mut usize,
    ) -> Option<&PiTail> {
        self.tail_session_with_expected_header(path, observed_at_ms, budget, None, None)
            .ok()
    }

    fn tail_session_with_expected_header(
        &mut self,
        path: &Path,
        observed_at_ms: u64,
        budget: &mut usize,
        expected_header: Option<(&str, &str)>,
        expected_identity: Option<&FileIdentity>,
    ) -> Result<&PiTail, String> {
        // Open once for metadata and reads. The post-read path check below fails closed
        // if replacement races this collection pass.
        let mut file = File::open(path).map_err(|_| "session file is no longer readable")?;
        let metadata = file
            .metadata()
            .map_err(|_| "session file metadata is unavailable")?;
        let path_metadata =
            fs::symlink_metadata(path).map_err(|_| "session file path is no longer readable")?;
        if !metadata.is_file() || path_metadata.file_type().is_symlink() {
            return Err("session file is no longer a regular non-symlink file".to_string());
        }
        let identity = file_identity_from_file(&file, &metadata);
        if expected_identity.is_some_and(|expected| *expected != identity) {
            self.tails.remove(path);
            return Err("session file identity changed after ownership validation".to_string());
        }
        let length = metadata.len();
        // Re-read the header from this open descriptor every pass. File identity and
        // the boundary fingerprint alone cannot prove an in-place rewrite kept the
        // validated session ID and cwd.
        let header = match read_header_from(&mut file, budget) {
            Ok(header) => header,
            Err(error) => {
                self.tails.remove(path);
                return Err(error);
            }
        };
        if expected_header
            .is_some_and(|(session_id, cwd)| header.session_id != session_id || header.cwd != cwd)
        {
            self.tails.remove(path);
            return Err("session file header changed after ownership validation".to_string());
        }
        let reset = self.tails.get(path).is_none_or(|tail| {
            tail.identity != identity
                || length < tail.offset
                || tail.header_session_id != header.session_id
                || tail.header_cwd != header.cwd
                || !fingerprint_matches(&mut file, tail)
        });
        if reset {
            self.tails.insert(
                path.to_path_buf(),
                PiTail {
                    identity: identity.clone(),
                    header_session_id: header.session_id,
                    header_cwd: header.cwd,
                    offset: 0,
                    boundary_fingerprint: None,
                    discard_oversized_line: false,
                    parse_limited: false,
                    source_updated_at_ms: None,
                    last_successful_parse_at_ms: None,
                    complete: false,
                    error: None,
                    semantic: PiSemantic::default(),
                },
            );
        }
        {
            let tail = self
                .tails
                .get_mut(path)
                .ok_or_else(|| "session tail state is unavailable".to_string())?;
            let mut valid_lines = 0;
            if tail.offset < length && *budget > 0 {
                let to_read = (length - tail.offset).min(*budget as u64) as usize;
                file.seek(SeekFrom::Start(tail.offset))
                    .map_err(|_| "session file seek failed")?;
                let mut bytes = vec![0; to_read];
                let count = file
                    .read(&mut bytes)
                    .map_err(|_| "session file read failed")?;
                bytes.truncate(count);
                *budget = budget.saturating_sub(count);
                let advanced = consume_tail_bytes(tail, &bytes, tail.offset);
                tail.offset = tail.offset.saturating_add(advanced.0);
                valid_lines = advanced.1;
                if advanced.0 > 0 {
                    tail.boundary_fingerprint = fingerprint_before(&mut file, tail.offset);
                    tail.source_updated_at_ms = file_modified_ms(path).or(Some(observed_at_ms));
                }
            }
            tail.complete = tail.offset >= length && !tail.discard_oversized_line;
            if valid_lines > 0 {
                tail.last_successful_parse_at_ms = Some(observed_at_ms);
            }
        }
        // Confirm the path still names this exact regular file before publishing state.
        if canonical_regular_jsonl(path).is_err() || file_identity(path) != Some(identity) {
            self.tails.remove(path);
            return Err("session file changed while it was being read".to_string());
        }
        self.tails
            .get(path)
            .ok_or_else(|| "session tail state is unavailable".to_string())
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

fn process_only_telemetry(observed_at_ms: u64, error: Option<String>) -> SessionTelemetry {
    let mut telemetry = SessionTelemetry::process_only(observed_at_ms);
    telemetry.error = error.map(|error| bounded_error(&error));
    telemetry.source_health = if telemetry.error.is_some() {
        SourceHealth::Error
    } else {
        SourceHealth::Unavailable
    };
    telemetry
}

fn discover_attachment_candidates(
    root: u32,
    shared: &SharedProcessData,
    budget: &mut AttachmentDiscoveryBudget,
    read_budget: &mut usize,
) -> (Vec<AttachmentCandidate>, bool) {
    discover_attachment_candidates_with(
        root,
        &shared.children_map,
        budget,
        read_budget,
        process_session_marker,
        process_open_jsonl_paths,
    )
}

fn discover_attachment_candidates_with<M, F>(
    root: u32,
    children_map: &HashMap<u32, Vec<u32>>,
    budget: &mut AttachmentDiscoveryBudget,
    read_budget: &mut usize,
    mut session_marker: M,
    mut open_jsonl_paths: F,
) -> (Vec<AttachmentCandidate>, bool)
where
    M: FnMut(u32, &mut usize) -> Option<(PathBuf, String)>,
    F: FnMut(u32, usize, usize) -> (Vec<PathBuf>, usize, bool),
{
    let mut candidates = Vec::new();
    let mut limited = false;
    if budget.candidate_slots == 0 || budget.process_slots == 0 || budget.fd_slots == 0 {
        return (candidates, true);
    }

    // PI_SESSION_* belongs to shell-tool descendants. A Pi root may have inherited
    // a parent session's marker, so root markers are never ownership evidence.
    // Check bounded descendants first because their marker includes the session ID.
    budget.process_slots -= 1; // root
    let mut visited = HashSet::from([root]);
    let mut stack = Vec::new();
    if let Some(children) = children_map.get(&root) {
        let room = budget.process_slots.min(children.len());
        stack.extend(children.iter().take(room).copied());
        limited |= children.len() > room;
    }
    while let Some(pid) = stack.pop() {
        if !visited.insert(pid) {
            continue;
        }
        if budget.process_slots == 0 || budget.candidate_slots == 0 || *read_budget == 0 {
            limited = true;
            break;
        }
        budget.process_slots -= 1;
        if let Some((path, session_id)) = session_marker(pid, read_budget) {
            candidates.push(AttachmentCandidate {
                path,
                expected_session_id: Some(session_id),
            });
            budget.candidate_slots -= 1;
        }
        if let Some(children) = children_map.get(&pid) {
            let room = budget
                .process_slots
                .saturating_sub(stack.len())
                .min(children.len());
            stack.extend(children.iter().take(room).copied());
            limited |= children.len() > room;
        }
    }

    // A descendant may open unrelated project files. Only the root's direct FDs count.
    if budget.candidate_slots == 0 || budget.fd_slots == 0 {
        limited = true;
    } else {
        let requested = budget.candidate_slots.saturating_add(1);
        let (mut paths, scanned, scan_limited) = open_jsonl_paths(root, requested, budget.fd_slots);
        budget.fd_slots = budget.fd_slots.saturating_sub(scanned);
        limited |= scan_limited;
        if paths.len() > budget.candidate_slots {
            paths.truncate(budget.candidate_slots);
            limited = true;
        }
        budget.candidate_slots -= paths.len();
        candidates.extend(paths.into_iter().map(|path| AttachmentCandidate {
            path,
            expected_session_id: None,
        }));
    }

    (candidates, limited)
}

#[cfg(test)]
fn validate_candidate(
    candidate: &AttachmentCandidate,
    expected_cwd: &str,
) -> Result<(PathBuf, PiHeader, FileIdentity), String> {
    let mut budget = MAX_TAIL_WORK_BYTES;
    validate_candidate_with_budget(candidate, expected_cwd, &mut budget)
}

fn validate_candidate_with_budget(
    candidate: &AttachmentCandidate,
    expected_cwd: &str,
    read_budget: &mut usize,
) -> Result<(PathBuf, PiHeader, FileIdentity), String> {
    let path = canonical_regular_jsonl(&candidate.path)?;
    let mut file = File::open(&path).map_err(|_| "session file is no longer readable")?;
    let metadata = file
        .metadata()
        .map_err(|_| "session file metadata is unavailable")?;
    if !metadata.is_file() {
        return Err("session path is not a regular file".to_string());
    }
    let identity = file_identity_from_file(&file, &metadata);
    let header = read_header_from(&mut file, read_budget)?;
    if !expected_cwd.is_empty() && header.cwd != expected_cwd {
        return Err("session header cwd conflicts with process cwd".to_string());
    }
    if candidate
        .expected_session_id
        .as_deref()
        .is_some_and(|session_id| session_id != header.session_id)
    {
        return Err("session marker ID conflicts with session header".to_string());
    }
    if canonical_regular_jsonl(&path).is_err() || file_identity(&path) != Some(identity.clone()) {
        return Err("session file changed during ownership validation".to_string());
    }
    Ok((path, header, identity))
}

fn canonical_regular_jsonl(path: &Path) -> Result<PathBuf, String> {
    if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
        return Err("session marker does not reference a JSONL file".to_string());
    }
    let metadata =
        fs::symlink_metadata(path).map_err(|_| "session file is not readable".to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("session file must be a regular non-symlink file".to_string());
    }
    fs::canonicalize(path).map_err(|_| "session file cannot be canonicalized".to_string())
}

#[cfg(test)]
fn read_header(path: &Path) -> Result<PiHeader, String> {
    let mut budget = MAX_TAIL_WORK_BYTES;
    read_header_with_budget(path, &mut budget)
}

#[cfg(test)]
fn read_header_with_budget(path: &Path, read_budget: &mut usize) -> Result<PiHeader, String> {
    let mut file = File::open(path).map_err(|_| "session file is not readable".to_string())?;
    read_header_from(&mut file, read_budget)
}

fn read_header_from(file: &mut File, read_budget: &mut usize) -> Result<PiHeader, String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "session header cannot be read".to_string())?;
    let mut line = Vec::new();
    let mut chunk = [0_u8; HEADER_READ_CHUNK_BYTES];
    while line.len() <= MAX_TAIL_LINE_BYTES && *read_budget > 0 {
        let remaining_line = MAX_TAIL_LINE_BYTES + 1 - line.len();
        let read_len = chunk.len().min(remaining_line).min(*read_budget);
        let count = file
            .read(&mut chunk[..read_len])
            .map_err(|_| "session header cannot be read".to_string())?;
        if count == 0 {
            break;
        }
        *read_budget = read_budget.saturating_sub(count);
        if let Some(newline) = chunk[..count].iter().position(|byte| *byte == b'\n') {
            line.extend_from_slice(&chunk[..newline]);
            return parse_header(&line);
        }
        line.extend_from_slice(&chunk[..count]);
    }
    if *read_budget == 0 {
        Err("session header validation budget exhausted".to_string())
    } else {
        Err("session header is incomplete or oversized".to_string())
    }
}

fn parse_header(line: &[u8]) -> Result<PiHeader, String> {
    let value: Value =
        serde_json::from_slice(line).map_err(|_| "session header is invalid".to_string())?;
    if value.get("type").and_then(Value::as_str) != Some("session") {
        return Err("session header has an invalid type".to_string());
    }
    let version = match value.get("version") {
        None => 1,
        Some(value) => value
            .as_u64()
            .ok_or_else(|| "session header has an invalid version".to_string())?,
    };
    if !(1..=3).contains(&version) {
        return Err("session header has an unsupported version".to_string());
    }
    let session_id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .ok_or_else(|| "session header has no valid ID".to_string())?;
    let cwd = value
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 4096)
        .ok_or_else(|| "session header has no valid cwd".to_string())?;
    Ok(PiHeader {
        session_id: session_id.to_string(),
        cwd: cwd.to_string(),
    })
}

impl PiSemantic {
    /// Add one fully framed JSONL entry. The session header is metadata, not a tree node.
    fn add(&mut self, value: &Value) -> bool {
        if value.get("type").and_then(Value::as_str) == Some("session") {
            return true;
        }
        let Some(id) = bounded_string(value.get("id"), MAX_SEMANTIC_ID_BYTES) else {
            // Unknown extension metadata without a tree identity is irrelevant to both
            // lifetime accounting and branch reconstruction. Known tree entries are not.
            if matches!(
                value.get("type").and_then(Value::as_str),
                Some(
                    "message"
                        | "compaction"
                        | "branch_summary"
                        | "model_change"
                        | "thinking_level_change"
                        | "custom_message"
                        | "custom"
                        | "label"
                        | "session_info"
                )
            ) {
                self.invalid = true;
                return false;
            }
            return true;
        };
        if self.entries.contains_key(&id) {
            return true;
        }
        if self.entries.len() >= MAX_SEMANTIC_ENTRIES {
            self.limited = true;
            return false;
        }
        match parse_pi_entry(value) {
            Some(entry) => {
                self.order.push(id.clone());
                self.entries.insert(id, entry);
                true
            }
            None => {
                self.invalid = true;
                false
            }
        }
    }

    fn session_data(&self, context_complete: bool) -> PiSessionData {
        let mut data = PiSessionData::default();
        data.usage_completeness = if self.limited || self.invalid {
            TelemetryCompleteness::Partial
        } else {
            TelemetryCompleteness::Complete
        };
        data.context_completeness = data.usage_completeness;

        // Lifetime accounting intentionally covers all persisted branches.
        for id in &self.order {
            let Some(entry) = self.entries.get(id) else {
                continue;
            };
            if let Some(usage) = &entry.usage {
                if matches!(
                    entry.kind,
                    PiEntryKind::Assistant
                        | PiEntryKind::ToolResult
                        | PiEntryKind::Compaction
                        | PiEntryKind::BranchSummary
                ) {
                    if let Some((input, output, cache_read, cache_write)) = usage.components {
                        let totals = (
                            data.input.checked_add(input),
                            data.output.checked_add(output),
                            data.cache_read.checked_add(cache_read),
                            data.cache_write.checked_add(cache_write),
                        );
                        if let (Some(input), Some(output), Some(cache_read), Some(cache_write)) =
                            totals
                        {
                            data.input = input;
                            data.output = output;
                            data.cache_read = cache_read;
                            data.cache_write = cache_write;
                        } else {
                            data.usage_available = false;
                            data.usage_completeness = TelemetryCompleteness::Partial;
                            append_reason(
                                &mut data.usage_reason,
                                "usage component total overflowed",
                            );
                        }
                        if let Some(cost) = usage.cost {
                            match (data.cost.unwrap_or(0.0) + cost)
                                .is_finite()
                                .then(|| data.cost.unwrap_or(0.0) + cost)
                            {
                                Some(cost) => data.cost = Some(cost),
                                None => {
                                    data.cost = None;
                                    data.usage_completeness = TelemetryCompleteness::Partial;
                                    append_reason(
                                        &mut data.usage_reason,
                                        "reported cost total overflowed",
                                    );
                                }
                            }
                        }
                        if usage.invalid_cost {
                            data.cost = None;
                            data.usage_completeness = TelemetryCompleteness::Partial;
                            append_reason(&mut data.usage_reason, "reported cost is invalid");
                        }
                    } else {
                        data.usage_completeness = TelemetryCompleteness::Partial;
                        append_reason(
                            &mut data.usage_reason,
                            "an observed usage record is incomplete",
                        );
                    }
                }
            }
            if entry.kind == PiEntryKind::Assistant {
                data.turns = data.turns.saturating_add(1);
            }
        }
        if data.usage_available
            && data
                .input
                .checked_add(data.output)
                .and_then(|total| total.checked_add(data.cache_read))
                .and_then(|total| total.checked_add(data.cache_write))
                .is_none()
        {
            data.usage_available = false;
            data.usage_completeness = TelemetryCompleteness::Partial;
            append_reason(&mut data.usage_reason, "combined usage total overflowed");
        }
        if !context_complete {
            data.context_completeness = TelemetryCompleteness::Partial;
            return data;
        }
        let Some(leaf) = self.order.last().cloned() else {
            // A header-only attached session is a complete, known-zero lifetime total.
            data.context_reason = Some("no persisted tree entry".to_string());
            return data;
        };
        data.active_leaf_id = Some(leaf.clone());
        append_reason(
            &mut data.context_reason,
            "active leaf is inferred from the latest persisted entry",
        );
        let mut branch = Vec::new();
        let mut current = leaf.clone();
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(current.clone()) {
                data.context_reason = Some("active branch has a cycle".to_string());
                data.context_completeness = TelemetryCompleteness::Partial;
                return data;
            }
            let Some(entry) = self.entries.get(&current) else {
                data.context_reason = Some("active branch parent is missing".to_string());
                data.context_completeness = TelemetryCompleteness::Partial;
                return data;
            };
            branch.push(current.clone());
            match &entry.parent_id {
                Some(parent) => current = parent.clone(),
                None => break,
            }
        }
        branch.reverse();
        for id in &branch {
            let e = &self.entries[id];
            if let Some(provider) = &e.provider {
                data.provider = provider.clone();
                data.model = e.model.clone().unwrap_or_default();
            }
            if let Some(effort) = &e.effort {
                data.effort = effort.clone();
            }
        }
        data.compactions = branch
            .iter()
            .filter(|id| self.entries[*id].kind == PiEntryKind::Compaction)
            .count() as u32;
        let latest_compaction = branch
            .iter()
            .rposition(|id| self.entries[id].kind == PiEntryKind::Compaction);
        let baseline_index = branch.iter().enumerate().rev().find_map(|(i, id)| {
            let e = &self.entries[id];
            (e.kind == PiEntryKind::Assistant
                && e.valid_baseline
                && valid_baseline(e.usage.as_ref()))
            .then_some(i)
        });
        if latest_compaction.is_some_and(|i| baseline_index.is_none_or(|b| b <= i)) {
            append_reason(
                &mut data.context_reason,
                "context is unknown until a post-compaction assistant baseline",
            );
            return data;
        }
        let (baseline, index) = if let Some(index) = baseline_index {
            (
                context_baseline(self.entries[&branch[index]].usage.as_ref())
                    .expect("validated baseline"),
                index,
            )
        } else {
            // Pi estimates all context-visible messages before the first valid assistant usage.
            let estimate = branch
                .iter()
                .map(|id| self.entries[id].context_chars.div_ceil(4))
                .sum();
            data.context_tokens = Some(estimate);
            data.trailing_tokens = Some(estimate);
            data.context_precision = TelemetryPrecision::Estimated;
            data.context_history.push(estimate);
            return data;
        };
        let trailing: u64 = branch[index + 1..]
            .iter()
            .map(|id| self.entries[id].context_chars.div_ceil(4))
            .sum();
        if trailing > 0 {
            append_reason(&mut data.context_reason, "trailing context is estimated");
        }
        data.baseline_tokens = Some(baseline);
        data.trailing_tokens = Some(trailing);
        let Some(context_tokens) = baseline.checked_add(trailing) else {
            data.context_tokens = None;
            data.context_precision = TelemetryPrecision::Unknown;
            data.context_completeness = TelemetryCompleteness::Partial;
            append_reason(&mut data.context_reason, "context token total overflowed");
            return data;
        };
        data.context_tokens = Some(context_tokens);
        data.context_precision = if trailing == 0 {
            TelemetryPrecision::Inferred
        } else {
            TelemetryPrecision::Estimated
        };
        data.context_history.push(data.context_tokens.unwrap());
        if let Some((input, output, cache_read, cache_write)) = self.entries[&branch[index]]
            .usage
            .as_ref()
            .and_then(|u| u.components)
        {
            data.token_history.push(
                input
                    .saturating_add(output)
                    .saturating_add(cache_read)
                    .saturating_add(cache_write),
            );
        }
        data
    }
}

fn append_reason(reason: &mut Option<String>, addition: &str) {
    match reason {
        Some(reason) if !reason.contains(addition) => {
            reason.push_str("; ");
            reason.push_str(addition);
        }
        None => *reason = Some(addition.to_string()),
        _ => {}
    }
}

fn bounded_string(value: Option<&Value>, max: usize) -> Option<String> {
    let value = value?.as_str()?;
    (!value.is_empty() && value.len() <= max).then(|| value.to_string())
}

fn valid_baseline(usage: Option<&PiUsage>) -> bool {
    context_baseline(usage).is_some_and(|n| n > 0)
}

fn context_baseline(usage: Option<&PiUsage>) -> Option<u64> {
    let usage = usage?;
    match usage.total_tokens {
        Some(total) if total > 0 => Some(total),
        _ => usage.components.and_then(|(input, output, read, write)| {
            input
                .checked_add(output)?
                .checked_add(read)?
                .checked_add(write)
        }),
    }
}

fn parse_pi_entry(value: &Value) -> Option<PiEntry> {
    let kind = match value.get("type").and_then(Value::as_str) {
        Some("compaction") => PiEntryKind::Compaction,
        Some("branch_summary") => PiEntryKind::BranchSummary,
        Some("message") => match value.pointer("/message/role").and_then(Value::as_str) {
            Some("assistant") => PiEntryKind::Assistant,
            Some("toolResult") => PiEntryKind::ToolResult,
            _ => PiEntryKind::Other,
        },
        _ => PiEntryKind::Other,
    };
    let parent_id = match value.get("parentId") {
        Some(Value::Null) => None,
        Some(v) => Some(bounded_string(Some(v), MAX_SEMANTIC_PARENT_BYTES)?),
        None => return None,
    };
    let message = value.get("message");
    let usage_value = if matches!(kind, PiEntryKind::Assistant | PiEntryKind::ToolResult) {
        value.pointer("/message/usage")
    } else {
        value.get("usage")
    };
    let usage = match usage_value {
        Some(value) => Some(parse_usage(value)?),
        None => None,
    };
    let provider_value = message
        .and_then(|m| m.get("provider"))
        .or_else(|| value.get("provider"));
    let model_value = message
        .and_then(|m| m.get("model"))
        .or_else(|| value.get("modelId"));
    let effort_value = value.get("thinkingLevel");
    let provider = match provider_value {
        Some(value) => Some(bounded_string(Some(value), MAX_SEMANTIC_METADATA_BYTES)?),
        None => None,
    };
    let model = match model_value {
        Some(value) => Some(bounded_string(Some(value), MAX_SEMANTIC_METADATA_BYTES)?),
        None => None,
    };
    let effort = match effort_value {
        Some(value) => Some(bounded_string(Some(value), MAX_SEMANTIC_METADATA_BYTES)?),
        None => None,
    };
    let context_chars = context_entry_chars(value, message);
    Some(PiEntry {
        parent_id,
        kind,
        usage,
        context_chars,
        provider,
        model,
        effort,
        valid_baseline: !matches!(
            message
                .and_then(|m| m.get("stopReason"))
                .and_then(Value::as_str),
            Some("aborted") | Some("error")
        ),
    })
}

fn parse_usage(v: &Value) -> Option<PiUsage> {
    if !v.is_object() {
        return None;
    }
    let number = |name| v.get(name).and_then(Value::as_u64);
    let components = match (
        number("input"),
        number("output"),
        number("cacheRead"),
        number("cacheWrite"),
    ) {
        (Some(input), Some(output), Some(read), Some(write)) => Some((input, output, read, write)),
        _ => None,
    };
    let total_tokens = v.get("totalTokens").and_then(Value::as_u64);
    let cost_value = v.pointer("/cost/total");
    let cost = cost_value
        .and_then(Value::as_f64)
        .filter(|cost| cost.is_finite() && *cost >= 0.0);
    Some(PiUsage {
        components,
        total_tokens,
        cost,
        invalid_cost: cost_value.is_some() && cost.is_none(),
    })
}

fn context_entry_chars(value: &Value, message: Option<&Value>) -> u64 {
    if value.get("type").and_then(Value::as_str) == Some("custom_message") {
        return content_chars(value.get("content"));
    }
    let Some(message) = message else {
        return match value.get("type").and_then(Value::as_str) {
            Some("compaction") | Some("branch_summary") => utf16_len(
                value
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            _ => 0,
        };
    };
    match message.get("role").and_then(Value::as_str) {
        Some("user") | Some("toolResult") | Some("custom") => content_chars(message.get("content")),
        Some("assistant") => message
            .get("content")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|item| match item.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            utf16_len(item.get("text").and_then(Value::as_str).unwrap_or_default())
                        }
                        Some("thinking") => utf16_len(
                            item.get("thinking")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                        ),
                        Some("toolCall") => {
                            utf16_len(item.get("name").and_then(Value::as_str).unwrap_or_default())
                                .saturating_add(utf16_len(
                                    &item
                                        .get("arguments")
                                        .map(Value::to_string)
                                        .unwrap_or_default(),
                                ))
                        }
                        _ => 0,
                    })
                    .sum()
            })
            .unwrap_or(0),
        Some("bashExecution")
            if message.get("excludeFromContext").and_then(Value::as_bool) == Some(true) =>
        {
            0
        }
        Some("bashExecution") => utf16_len(
            message
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .saturating_add(utf16_len(
            message
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )),
        _ => 0,
    }
}

fn utf16_len(s: &str) -> u64 {
    s.encode_utf16().count() as u64
}
fn content_chars(content: Option<&Value>) -> u64 {
    match content {
        Some(Value::String(s)) => utf16_len(s),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item.get("type").and_then(Value::as_str) {
                Some("text") => {
                    utf16_len(item.get("text").and_then(Value::as_str).unwrap_or_default())
                }
                Some("image") => 4800,
                _ => 0,
            })
            .sum(),
        _ => 0,
    }
}

fn pi_agent_root(session_path: &Path) -> Option<PathBuf> {
    let sessions = session_path
        .ancestors()
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some("sessions"))?;
    sessions.parent().map(Path::to_path_buf)
}

fn catalog_revision(path: &Path) -> Option<CatalogRevision> {
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    Some(CatalogRevision {
        length: metadata.len(),
        modified: metadata.modified().ok(),
        identity: file_identity_from_file(&file, &metadata),
    })
}

/// Read no more than the accepted limit, even if the path grows after metadata inspection.
fn load_catalog(path: &Path) -> Result<Option<Value>, ()> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let mut bytes = Vec::with_capacity((MAX_MODEL_CATALOG_BYTES as usize).min(64 * 1024));
    file.take(MAX_MODEL_CATALOG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() as u64 > MAX_MODEL_CATALOG_BYTES {
        return Err(());
    }
    serde_json::from_slice(&bytes).map(Some).map_err(|_| ())
}

fn normalize_model_id(value: &Value) -> Option<String> {
    bounded_string(value.get("id"), MAX_SEMANTIC_METADATA_BYTES)
}

fn insert_window(
    windows: &mut HashMap<(String, String), u64>,
    provider: &str,
    model: String,
    window: u64,
) {
    if window == 0 || provider.len() > MAX_SEMANTIC_METADATA_BYTES {
        return;
    }
    let key = (provider.to_string(), model);
    if windows.contains_key(&key) || windows.len() < MAX_MODEL_CATALOG_ENTRIES {
        windows.insert(key, window);
    }
}

fn refresh_model_catalog(cache: &mut ModelCatalogCache, root: &Path) {
    let store_path = root.join("models-store.json");
    let config_path = root.join("models.json");
    let store_revision = catalog_revision(&store_path);
    let config_revision = catalog_revision(&config_path);
    if cache.store_revision == store_revision && cache.config_revision == config_revision {
        return;
    }
    cache.store_revision = store_revision;
    cache.config_revision = config_revision;
    cache.windows.clear();
    cache.unavailable = false;
    let store = match load_catalog(&store_path) {
        Ok(value) => value,
        Err(()) => {
            cache.unavailable = true;
            return;
        }
    };
    let config = match load_catalog(&config_path) {
        Ok(value) => value,
        Err(()) => {
            cache.unavailable = true;
            return;
        }
    };

    // Store values are the only base models. Do not retain the raw catalog.
    if let Some(store) = store.as_ref().and_then(Value::as_object) {
        for (provider, entry) in store {
            if provider.len() > MAX_SEMANTIC_METADATA_BYTES {
                continue;
            }
            for model in entry
                .get("models")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let (Some(id), Some(window)) = (
                    normalize_model_id(model),
                    model.get("contextWindow").and_then(Value::as_u64),
                ) {
                    insert_window(&mut cache.windows, provider, id, window);
                }
            }
        }
    }

    // Config is applied transiently: overrides require a base; custom models replace/create.
    if let Some(providers) = config
        .as_ref()
        .and_then(|config| config.get("providers"))
        .and_then(Value::as_object)
    {
        for (provider, entry) in providers {
            if provider.len() > MAX_SEMANTIC_METADATA_BYTES {
                continue;
            }
            if let Some(overrides) = entry.get("modelOverrides").and_then(Value::as_object) {
                for (model, override_) in overrides {
                    let key = (provider.clone(), model.clone());
                    if let (Some(existing), Some(window)) = (
                        cache.windows.get_mut(&key),
                        override_.get("contextWindow").and_then(Value::as_u64),
                    ) {
                        if window > 0 {
                            *existing = window;
                        }
                    }
                }
            }
            for custom in entry
                .get("models")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(id) = normalize_model_id(custom) {
                    let window = custom
                        .get("contextWindow")
                        .and_then(Value::as_u64)
                        .filter(|window| *window > 0)
                        .unwrap_or(128_000);
                    insert_window(&mut cache.windows, provider, id, window);
                }
            }
        }
    }
}

fn model_window_from_catalog(
    cache: &ModelCatalogCache,
    provider: &str,
    model: &str,
) -> Option<u64> {
    cache
        .windows
        .get(&(provider.to_string(), model.to_string()))
        .copied()
}

fn consume_tail_bytes(tail: &mut PiTail, bytes: &[u8], offset: u64) -> (u64, usize) {
    // `bytes` is invocation-local. PiTail retains only framing offsets and hashes.
    let mut committed = 0usize;
    let mut valid = 0;
    let mut cursor = 0;
    while cursor < bytes.len() {
        if tail.discard_oversized_line {
            if let Some(pos) = bytes[cursor..].iter().position(|b| *b == b'\n') {
                cursor += pos + 1;
                committed = cursor;
                tail.discard_oversized_line = false;
            } else {
                committed = bytes.len();
                break;
            }
            continue;
        }
        let Some(pos) = bytes[cursor..].iter().position(|b| *b == b'\n') else {
            if bytes.len() - cursor >= MAX_TAIL_LINE_BYTES {
                tail.parse_limited = true;
                tail.error = Some("JSONL line exceeds 1 MiB limit".to_string());
                tail.discard_oversized_line = true;
                committed = bytes.len();
            }
            break;
        };
        let end = cursor + pos;
        if end - cursor > MAX_TAIL_LINE_BYTES {
            tail.parse_limited = true;
            tail.error = Some("JSONL line exceeds 1 MiB limit".to_string());
        } else if !bytes[cursor..end].is_empty() {
            match serde_json::from_slice::<Value>(&bytes[cursor..end]) {
                Ok(value) => {
                    valid += 1;
                    if !tail.semantic.add(&value) {
                        // Semantic loss makes telemetry partial, but the JSONL framing itself
                        // remains healthy and must not poison later append recovery.
                        tail.parse_limited = true;
                    } else {
                        tail.error = None;
                    }
                }
                Err(_) => {
                    tail.parse_limited = true;
                    tail.error = Some("malformed JSONL line ignored".to_string());
                }
            }
        }
        cursor = end + 1;
        committed = cursor;
    }
    let _ = offset;
    (committed as u64, valid)
}

fn fingerprint_before(file: &mut File, offset: u64) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let size = offset.min(64) as usize;
    if size == 0 {
        return Some(0);
    }
    let mut bytes = vec![0; size];
    file.seek(SeekFrom::Start(offset - size as u64)).ok()?;
    file.read_exact(&mut bytes).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some(hasher.finish())
}

fn fingerprint_matches(file: &mut File, tail: &PiTail) -> bool {
    tail.boundary_fingerprint == fingerprint_before(file, tail.offset)
}

fn bounded_error(error: &str) -> String {
    error.chars().take(MAX_TELEMETRY_ERROR_BYTES).collect()
}

fn file_modified_ms(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

fn file_identity(path: &Path) -> Option<FileIdentity> {
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    Some(file_identity_from_file(&file, &metadata))
}

fn file_identity_from_file(file: &File, metadata: &fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = file;
        FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
    #[cfg(windows)]
    {
        let (volume_serial_number, file_index, modified_ms) =
            if let Some((volume, index)) = windows_file_identity(file) {
                (Some(volume), Some(index), None)
            } else {
                (
                    None,
                    None,
                    metadata
                        .modified()
                        .ok()
                        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                        .map(|duration| duration.as_millis() as u64),
                )
            };
        FileIdentity {
            volume_serial_number,
            file_index,
            modified_ms,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        FileIdentity {
            modified_ms: metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as u64),
        }
    }
}

#[cfg(windows)]
fn windows_file_identity(file: &File) -> Option<(u32, u64)> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    // SAFETY: `file` owns a valid handle for this call, and `info` points to writable storage.
    let result =
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, info.as_mut_ptr()) };
    if result == 0 {
        return None;
    }
    // SAFETY: a successful call initialized the full structure.
    let info = unsafe { info.assume_init() };
    let file_index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Some((info.dwVolumeSerialNumber, file_index))
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

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn discover_herdr_sessions(live: &HashSet<u32>) -> HerdrDiscovery {
    let binary = std::env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    let deadline = Instant::now() + Duration::from_millis(HERDR_DISCOVERY_BUDGET_MS);
    discover_herdr_sessions_with(live, process_herdr_marker, |marker, args| {
        let remaining = deadline.checked_duration_since(Instant::now())?;
        run_bounded_herdr_json(
            &binary,
            &marker.socket_path,
            args,
            remaining.min(Duration::from_millis(HERDR_COMMAND_TIMEOUT_MS)),
        )
    })
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn discover_herdr_sessions(_live: &HashSet<u32>) -> HerdrDiscovery {
    HerdrDiscovery::default()
}

#[cfg(any(target_os = "linux", target_vendor = "apple", test))]
fn discover_herdr_sessions_with<P, F>(
    live: &HashSet<u32>,
    mut marker_for_pid: P,
    mut run: F,
) -> HerdrDiscovery
where
    P: FnMut(u32) -> Option<HerdrProcessMarker>,
    F: FnMut(&HerdrProcessMarker, &[String]) -> Option<Value>,
{
    let mut discovery = HerdrDiscovery::default();
    let mut roots: Vec<u32> = live.iter().copied().collect();
    roots.sort_unstable();
    for pid in roots.into_iter().take(MAX_HERDR_ROOTS) {
        let Some(marker) = marker_for_pid(pid) else {
            continue;
        };
        let pane_id = &marker.pane_id;
        let pane_args = vec![
            "pane".to_string(),
            "current".to_string(),
            "--pane".to_string(),
            pane_id.clone(),
        ];
        let Some(first) = run(&marker, &pane_args)
            .as_ref()
            .and_then(|value| parse_herdr_pane_snapshot(value, pane_id))
        else {
            continue;
        };
        let process_args = vec![
            "pane".to_string(),
            "process-info".to_string(),
            "--pane".to_string(),
            pane_id.clone(),
        ];
        let process_matches = run(&marker, &process_args)
            .as_ref()
            .is_some_and(|value| herdr_process_info_contains(value, pane_id, pid));
        let second = run(&marker, &pane_args)
            .as_ref()
            .and_then(|value| parse_herdr_pane_snapshot(value, pane_id));
        if !process_matches || second.as_ref().is_none_or(|second| second != &first) {
            discovery.ambiguous.insert(pid);
            continue;
        }
        discovery.sessions.insert(
            pid,
            HerdrSession {
                path: first.path,
                status: first.status,
            },
        );
    }
    discovery
}

#[cfg(any(target_os = "linux", target_vendor = "apple", test))]
fn parse_herdr_pane_snapshot(value: &Value, pane_id: &str) -> Option<HerdrPaneSnapshot> {
    let pane = value.pointer("/result/pane")?;
    if pane.get("pane_id").and_then(Value::as_str) != Some(pane_id)
        || pane.get("agent").and_then(Value::as_str) != Some("pi")
    {
        return None;
    }
    let session = pane.get("agent_session")?;
    if session.get("agent").and_then(Value::as_str) != Some("pi")
        || session.get("source").and_then(Value::as_str) != Some("herdr:pi")
        || session.get("kind").and_then(Value::as_str) != Some("path")
    {
        return None;
    }
    let path = session.get("value").and_then(Value::as_str)?;
    if path.len() > 4096 || !Path::new(path).is_absolute() {
        return None;
    }
    let status = match pane.get("agent_status").and_then(Value::as_str) {
        Some("working") => SessionStatus::Executing,
        Some("idle" | "done" | "blocked") => SessionStatus::Waiting,
        _ => SessionStatus::Unknown,
    };
    Some(HerdrPaneSnapshot {
        path: PathBuf::from(path),
        revision: pane.get("revision").and_then(Value::as_u64)?,
        status,
    })
}

#[cfg(any(target_os = "linux", target_vendor = "apple", test))]
fn herdr_process_info_contains(value: &Value, pane_id: &str, pid: u32) -> bool {
    value.pointer("/result/process_info").is_some_and(|info| {
        info.get("pane_id").and_then(Value::as_str) == Some(pane_id)
            && info
                .get("foreground_processes")
                .and_then(Value::as_array)
                .is_some_and(|processes| {
                    processes.iter().any(|process| {
                        process.get("pid").and_then(Value::as_u64) == Some(u64::from(pid))
                    })
                })
    })
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn run_bounded_herdr_json(
    binary: &std::ffi::OsStr,
    socket_path: &str,
    args: &[String],
    timeout: Duration,
) -> Option<Value> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    let mut command = std::process::Command::new(binary);
    command
        .args(args)
        .env("HERDR_SOCKET_PATH", socket_path)
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let fd = stdout.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        let _ = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        let _ = child.wait();
        return None;
    }

    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let mut status = None;
    let mut eof = false;
    let mut failed = false;
    while !eof || status.is_none() {
        let mut chunk = [0_u8; 4096];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(count) => {
                    if bytes.len().saturating_add(count) > MAX_HERDR_JSON_BYTES {
                        failed = true;
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..count]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
        if failed {
            break;
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(child_status) => status = child_status,
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
        if (!eof || status.is_none()) && Instant::now() >= deadline {
            failed = true;
            break;
        }
        if !eof || status.is_none() {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    if failed {
        let _ = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    }
    if status.is_none() {
        status = child.wait().ok();
    }
    if failed || status.is_none_or(|status| !status.success()) {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

#[cfg(any(target_os = "linux", target_vendor = "apple", test))]
fn parse_env_value(env: &[u8], name: &[u8], max_len: usize) -> Option<String> {
    env.split(|byte| *byte == 0).find_map(|entry| {
        let value = entry.strip_prefix(name)?;
        if value.is_empty() || value.len() > max_len {
            return None;
        }
        std::str::from_utf8(value).ok().map(str::to_string)
    })
}

#[cfg(any(target_os = "linux", target_vendor = "apple", test))]
fn parse_herdr_process_marker(env: &[u8]) -> Option<HerdrProcessMarker> {
    if parse_env_value(env, b"HERDR_ENV=", 8)?.as_str() != "1" {
        return None;
    }
    let pane_id = parse_env_value(env, b"HERDR_PANE_ID=", 128)?;
    let socket_path = parse_env_value(env, b"HERDR_SOCKET_PATH=", 4096)?;
    if !Path::new(&socket_path).is_absolute() {
        return None;
    }
    Some(HerdrProcessMarker {
        pane_id,
        socket_path,
    })
}

#[cfg(target_os = "linux")]
fn process_herdr_marker(pid: u32) -> Option<HerdrProcessMarker> {
    let file = File::open(format!("/proc/{pid}/environ")).ok()?;
    let mut bytes = Vec::new();
    file.take((MAX_HERDR_ENV_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_HERDR_ENV_BYTES {
        return None;
    }
    parse_herdr_process_marker(&bytes)
}

#[cfg(target_vendor = "apple")]
fn process_herdr_marker(pid: u32) -> Option<HerdrProcessMarker> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut size = 0_usize;
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
        || size == 0
        || size > MAX_HERDR_ENV_BYTES
    {
        return None;
    }
    let mut bytes = vec![0_u8; size];
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    bytes.truncate(size);
    let env = macos_process_environment(&bytes)?;
    parse_herdr_process_marker(env)
}

#[cfg(target_vendor = "apple")]
fn macos_process_environment(procargs: &[u8]) -> Option<&[u8]> {
    let argc = i32::from_ne_bytes(procargs.get(..4)?.try_into().ok()?);
    if !(0..=4096).contains(&argc) {
        return None;
    }
    let mut offset = 4;
    offset += procargs.get(offset..)?.iter().position(|byte| *byte == 0)? + 1;
    while procargs.get(offset) == Some(&0) {
        offset += 1;
    }
    for _ in 0..argc {
        offset += procargs.get(offset..)?.iter().position(|byte| *byte == 0)? + 1;
    }
    while procargs.get(offset) == Some(&0) {
        offset += 1;
    }
    procargs.get(offset..)
}

#[cfg(any(target_os = "linux", test))]
fn parse_session_marker(env: &[u8]) -> Option<(PathBuf, String)> {
    let mut path = None;
    let mut session_id = None;
    for entry in env.split(|byte| *byte == 0) {
        if let Some(value) = entry.strip_prefix(b"PI_SESSION_FILE=") {
            if value.len() <= 4096 && !value.is_empty() {
                path = std::str::from_utf8(value).ok().map(PathBuf::from);
            }
        } else if let Some(value) = entry.strip_prefix(b"PI_SESSION_ID=") {
            if value.len() <= 256 && !value.is_empty() {
                session_id = std::str::from_utf8(value).ok().map(str::to_string);
            }
        }
    }
    path.zip(session_id)
}

#[cfg(target_os = "linux")]
fn process_session_marker(pid: u32, read_budget: &mut usize) -> Option<(PathBuf, String)> {
    if *read_budget == 0 {
        return None;
    }
    let mut file = File::open(format!("/proc/{pid}/environ")).ok()?;
    let mut bytes = vec![0; *read_budget];
    let count = file.read(&mut bytes).ok()?;
    bytes.truncate(count);
    *read_budget = read_budget.saturating_sub(count);
    if bytes.last() != Some(&0) {
        bytes.truncate(
            bytes
                .iter()
                .rposition(|byte| *byte == 0)
                .map_or(0, |index| index + 1),
        );
    }
    parse_session_marker(&bytes)
}

#[cfg(target_vendor = "apple")]
fn process_session_marker(_pid: u32, _read_budget: &mut usize) -> Option<(PathBuf, String)> {
    // macOS `ps eww` joins argv and environment with no reliable boundary.
    // Parsing it could mistake an argument for a marker, so macOS attaches only
    // through a Pi root's directly open JSONL descriptor.
    None
}

#[cfg(target_os = "windows")]
fn process_session_marker(_pid: u32, _read_budget: &mut usize) -> Option<(PathBuf, String)> {
    None
}

#[cfg(all(
    not(target_os = "linux"),
    not(target_vendor = "apple"),
    not(target_os = "windows")
))]
fn process_session_marker(_pid: u32, _read_budget: &mut usize) -> Option<(PathBuf, String)> {
    None
}

#[cfg(target_os = "linux")]
fn process_open_jsonl_paths(
    pid: u32,
    result_limit: usize,
    scan_limit: usize,
) -> (Vec<PathBuf>, usize, bool) {
    let Ok(mut entries) = fs::read_dir(format!("/proc/{pid}/fd")) else {
        return (Vec::new(), 0, false);
    };
    let mut paths = Vec::new();
    let mut scanned = 0;
    while scanned < scan_limit {
        let Some(entry) = entries.next() else {
            return (paths, scanned, false);
        };
        scanned += 1;
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(path) = fs::read_link(entry.path()) else {
            continue;
        };
        if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
            paths.push(path);
            if paths.len() >= result_limit {
                return (paths, scanned, false);
            }
        }
    }
    (paths, scanned, true)
}

#[cfg(target_vendor = "apple")]
fn process_open_jsonl_paths(
    pid: u32,
    result_limit: usize,
    scan_limit: usize,
) -> (Vec<PathBuf>, usize, bool) {
    let Ok(mut child) = std::process::Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-Fn"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return (Vec::new(), 0, false);
    };
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return (Vec::new(), 0, false);
    };

    let byte_limit = scan_limit.saturating_mul(MAX_LSOF_RECORD_BYTES);
    let mut paths = Vec::new();
    let mut scanned = 0;
    let mut bytes_read = 0;
    let mut record = Vec::with_capacity(256);
    let mut record_oversized = false;
    let mut chunk = [0_u8; 4096];
    let mut eof = false;
    let mut read_failed = false;
    let mut result_limit_reached = false;
    let mut limited = false;

    'output: while scanned < scan_limit && bytes_read < byte_limit {
        let remaining = byte_limit - bytes_read;
        let read_len = remaining.min(chunk.len());
        let count = match stdout.read(&mut chunk[..read_len]) {
            Ok(0) => {
                eof = true;
                break;
            }
            Ok(count) => count,
            Err(_) => {
                read_failed = true;
                break;
            }
        };
        bytes_read += count;
        for byte in &chunk[..count] {
            if *byte == b'\n' {
                if record_oversized {
                    limited = true;
                    break 'output;
                }
                if let Some(name) = record.strip_prefix(b"n") {
                    scanned += 1;
                    if let Ok(name) = std::str::from_utf8(name) {
                        let path = PathBuf::from(name);
                        if path.extension().and_then(|extension| extension.to_str())
                            == Some("jsonl")
                        {
                            paths.push(path);
                            if paths.len() >= result_limit {
                                result_limit_reached = true;
                                break 'output;
                            }
                        }
                    }
                    if scanned >= scan_limit {
                        limited = true;
                        break 'output;
                    }
                }
                record.clear();
                record_oversized = false;
            } else if record.len() < MAX_LSOF_RECORD_BYTES {
                record.push(*byte);
            } else {
                record_oversized = true;
            }
        }
    }
    if !eof && !result_limit_reached {
        limited = true;
    }

    drop(stdout);
    if !eof {
        let _ = child.kill();
    }
    let status = child.wait().ok();
    if read_failed || (eof && status.is_none_or(|status| !status.success())) {
        return (Vec::new(), scanned, false);
    }
    (paths, scanned, limited)
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn process_open_jsonl_paths(
    _pid: u32,
    _result_limit: usize,
    _scan_limit: usize,
) -> (Vec<PathBuf>, usize, bool) {
    (Vec::new(), 0, false)
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
    use std::io::Write;
    use std::time::{Duration, Instant};

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

    fn herdr_pane(path: &Path, revision: u64, status: &str) -> Value {
        serde_json::json!({
            "result": {
                "pane": {
                    "agent": "pi",
                    "agent_session": {
                        "agent": "pi",
                        "kind": "path",
                        "source": "herdr:pi",
                        "value": path
                    },
                    "agent_status": status,
                    "pane_id": "w1X:p1",
                    "revision": revision
                }
            }
        })
    }

    fn herdr_process_info(pid: u32) -> Value {
        serde_json::json!({
            "result": {
                "process_info": {
                    "foreground_processes": [{"pid": pid}, {"pid": 26131}],
                    "pane_id": "w1X:p1"
                }
            }
        })
    }

    fn herdr_test_socket() -> &'static str {
        if cfg!(windows) {
            r"C:\herdr.sock"
        } else {
            "/tmp/herdr.sock"
        }
    }

    fn herdr_marker() -> HerdrProcessMarker {
        HerdrProcessMarker {
            pane_id: "w1X:p1".to_string(),
            socket_path: herdr_test_socket().to_string(),
        }
    }

    #[test]
    fn herdr_process_marker_is_autodetected_from_the_pi_process() {
        let env = format!(
            "HERDR_ENV=1\0HERDR_PANE_ID=w1X:p1\0HERDR_SOCKET_PATH={}\0",
            herdr_test_socket()
        );
        let incomplete = format!(
            "HERDR_PANE_ID=w1X:p1\0HERDR_SOCKET_PATH={}\0",
            herdr_test_socket()
        );

        assert_eq!(
            parse_herdr_process_marker(env.as_bytes()),
            Some(herdr_marker())
        );
        assert!(parse_herdr_process_marker(incomplete.as_bytes()).is_none());
    }

    #[test]
    fn herdr_discovery_maps_stable_pi_session_and_status_to_owning_process() {
        let dir = tempfile::tempdir().unwrap();
        let session_path = dir.path().join("pi-session.jsonl");
        let live = HashSet::from([25835]);
        let discovered = discover_herdr_sessions_with(
            &live,
            |_pid| Some(herdr_marker()),
            |_marker, args| match args {
                [scope, action, flag, pane]
                    if scope == "pane"
                        && action == "current"
                        && flag == "--pane"
                        && pane == "w1X:p1" =>
                {
                    Some(herdr_pane(&session_path, 7, "working"))
                }
                [scope, action, flag, pane]
                    if scope == "pane"
                        && action == "process-info"
                        && flag == "--pane"
                        && pane == "w1X:p1" =>
                {
                    Some(herdr_process_info(25835))
                }
                _ => None,
            },
        );

        assert!(discovered.ambiguous.is_empty());
        assert_eq!(
            discovered.sessions.get(&25835),
            Some(&HerdrSession {
                path: session_path,
                status: SessionStatus::Executing,
            })
        );
    }

    #[test]
    fn herdr_discovery_routes_each_process_through_its_own_server_socket() {
        use std::cell::RefCell;

        let dir = tempfile::tempdir().unwrap();
        let calls = RefCell::new(Vec::new());
        let live = HashSet::from([10, 20]);
        let discovered = discover_herdr_sessions_with(
            &live,
            |pid| {
                Some(HerdrProcessMarker {
                    pane_id: format!("pane-{pid}"),
                    socket_path: dir
                        .path()
                        .join(format!("herdr-{pid}.sock"))
                        .to_string_lossy()
                        .into_owned(),
                })
            },
            |marker, args| {
                let pid: u32 = marker.pane_id.strip_prefix("pane-")?.parse().ok()?;
                calls
                    .borrow_mut()
                    .push((pid, marker.socket_path.clone(), args[1].clone()));
                if args.get(1).map(String::as_str) == Some("process-info") {
                    return Some(serde_json::json!({
                        "result": {
                            "process_info": {
                                "foreground_processes": [{"pid": pid}],
                                "pane_id": marker.pane_id
                            }
                        }
                    }));
                }
                Some(serde_json::json!({
                    "result": {
                        "pane": {
                            "agent": "pi",
                            "agent_session": {
                                "agent": "pi",
                                "kind": "path",
                                "source": "herdr:pi",
                                "value": dir.path().join(format!("session-{pid}.jsonl"))
                            },
                            "agent_status": "working",
                            "pane_id": marker.pane_id,
                            "revision": 1
                        }
                    }
                }))
            },
        );

        assert_eq!(discovered.sessions.len(), 2);
        let calls = calls.into_inner();
        assert_eq!(calls.len(), 6);
        for (pid, socket_path, _action) in calls {
            assert_eq!(
                socket_path,
                dir.path()
                    .join(format!("herdr-{pid}.sock"))
                    .to_string_lossy()
            );
        }
    }

    #[test]
    fn herdr_pane_restart_is_ambiguous() {
        use std::cell::Cell;

        let dir = tempfile::tempdir().unwrap();
        let first_path = dir.path().join("first.jsonl");
        let second_path = dir.path().join("second.jsonl");
        let pane_reads = Cell::new(0);
        let discovered = discover_herdr_sessions_with(
            &HashSet::from([10]),
            |_pid| Some(herdr_marker()),
            |_marker, args| {
                if args.get(1).map(String::as_str) == Some("process-info") {
                    return Some(herdr_process_info(10));
                }
                let count = pane_reads.get();
                pane_reads.set(count + 1);
                Some(if count == 0 {
                    herdr_pane(&first_path, 7, "working")
                } else {
                    herdr_pane(&second_path, 8, "idle")
                })
            },
        );

        assert!(discovered.sessions.is_empty());
        assert_eq!(discovered.ambiguous, HashSet::from([10]));
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn herdr_command_runner_bounds_time_and_output() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-herdr");
        fs::write(
            &script,
            "#!/bin/sh\nif [ \"$1\" = sleep ]; then sleep 2; printf '{}\\n'; exit; fi\nprintf '{\"padding\":\"'\ndd if=/dev/zero bs=1024 count=300 2>/dev/null | tr '\\000' x\nprintf '\"}\\n'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();

        let started = Instant::now();
        assert!(run_bounded_herdr_json(
            script.as_os_str(),
            "/tmp/herdr.sock",
            &["sleep".to_string()],
            Duration::from_millis(50),
        )
        .is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(run_bounded_herdr_json(
            script.as_os_str(),
            "/tmp/herdr.sock",
            &["oversized".to_string()],
            Duration::from_secs(1),
        )
        .is_none());
    }

    #[test]
    fn herdr_reported_session_attaches_without_an_open_jsonl_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let path = session_file(&dir, "herdr-session", &cwd);
        let shared = shared(vec![proc(10, 1, "pi")]);
        let mut collector = PiCollector::new();
        collector.cwd_cache.insert(10, cwd);
        collector.start_id_cache.insert(10, None);
        collector.herdr_sessions.insert(
            10,
            HerdrSession {
                path: path.clone(),
                status: SessionStatus::Executing,
            },
        );

        let result = collector.resolve_attachments(&[10], &shared);

        assert_eq!(
            result
                .get(&10)
                .and_then(|result| result.attachment.as_ref())
                .map(|attachment| attachment.path.clone()),
            Some(fs::canonicalize(path).unwrap())
        );
    }

    #[test]
    fn ambiguous_herdr_claim_blocks_a_cached_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let path = session_file(&dir, "herdr-session", &cwd);
        let shared = shared(vec![proc(10, 1, "pi")]);
        let mut collector = PiCollector::new();
        collector.cwd_cache.insert(10, cwd);
        collector.start_id_cache.insert(10, None);
        collector.herdr_sessions.insert(
            10,
            HerdrSession {
                path,
                status: SessionStatus::Executing,
            },
        );
        assert!(collector
            .resolve_attachments(&[10], &shared)
            .get(&10)
            .is_some_and(|result| result.attachment.is_some()));

        collector.herdr_sessions.clear();
        collector.herdr_ambiguous.insert(10);
        let result = collector.resolve_attachments(&[10], &shared);

        assert!(result
            .get(&10)
            .is_some_and(|result| result.attachment.is_none()));
    }

    #[test]
    fn pid_reuse_drops_cached_herdr_ownership() {
        let pid = u32::MAX;
        let mut collector = PiCollector::new();
        collector
            .start_id_cache
            .insert(pid, Some("stale-start".to_string()));
        collector.cwd_cache.insert(pid, "/tmp".to_string());
        collector.herdr_sessions.insert(
            pid,
            HerdrSession {
                path: PathBuf::from("/tmp/stale.jsonl"),
                status: SessionStatus::Executing,
            },
        );
        collector.herdr_sessions_initialized = true;

        collector.collect_sessions(&shared(vec![proc(pid, 1, "pi")]));

        assert!(!collector.herdr_sessions.contains_key(&pid));
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

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_pi_telemetry_boundary_is_process_only() {
        let mut budget = 4_096;
        assert!(process_session_marker(42, &mut budget).is_none());
        assert_eq!(process_open_jsonl_paths(42, 8, 128), (Vec::new(), 0, false));
        assert!(process_cwd(42).is_none());
        assert!(process_start_id(42).is_none());
        assert!(discover_herdr_sessions(&HashSet::from([42]))
            .sessions
            .is_empty());
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

    fn session_header(id: &str, cwd: &str) -> String {
        format!(
            "{}\n",
            serde_json::json!({"type": "session", "version": 3, "id": id, "cwd": cwd})
        )
    }

    fn session_file(dir: &tempfile::TempDir, id: &str, cwd: &str) -> PathBuf {
        let path = dir.path().join("session.jsonl");
        fs::write(&path, session_header(id, cwd)).unwrap();
        path
    }

    #[test]
    fn session_fixture_escapes_windows_paths() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = r"C:\Users\runneradmin\project";
        let path = session_file(&dir, "windows-path", cwd);

        assert_eq!(read_header(&path).unwrap().cwd, cwd);
    }

    #[test]
    fn marker_attachment_requires_matching_header_and_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy();
        let path = session_file(&dir, "session-a", &cwd);
        let candidate = AttachmentCandidate {
            path: path.clone(),
            expected_session_id: Some("session-a".to_string()),
        };
        assert_eq!(
            validate_candidate(&candidate, &cwd).unwrap().0,
            fs::canonicalize(&path).unwrap()
        );
        assert!(validate_candidate(&candidate, "/another/project").is_err());
        let conflicting = AttachmentCandidate {
            path,
            expected_session_id: Some("session-b".to_string()),
        };
        assert!(validate_candidate(&conflicting, &cwd).is_err());
    }

    #[test]
    fn cwd_without_marker_does_not_create_an_attachment_candidate() {
        let marker = parse_session_marker(b"PATH=/bin\0PWD=/tmp/project\0");
        assert!(marker.is_none());
    }

    #[test]
    fn marker_parser_requires_both_session_values() {
        assert!(parse_session_marker(b"PI_SESSION_FILE=/tmp/a.jsonl\0").is_none());
        let marker =
            parse_session_marker(b"PI_SESSION_ID=id\0PI_SESSION_FILE=/tmp/a.jsonl\0").unwrap();
        assert_eq!(marker.0, PathBuf::from("/tmp/a.jsonl"));
        assert_eq!(marker.1, "id");
    }

    #[test]
    fn discovery_ignores_root_markers_and_descendant_open_files() {
        let children = HashMap::from([(10, vec![11])]);
        let mut budget = AttachmentDiscoveryBudget {
            candidate_slots: 4,
            process_slots: 4,
            fd_slots: 4,
        };
        let mut read_budget = MAX_TAIL_WORK_BYTES;
        let (candidates, limited) = discover_attachment_candidates_with(
            10,
            &children,
            &mut budget,
            &mut read_budget,
            |pid, _budget| {
                Some((
                    PathBuf::from(format!("/tmp/marker-{pid}.jsonl")),
                    format!("marker-{pid}"),
                ))
            },
            |pid, limit, scan_limit| {
                let paths: Vec<_> = vec![PathBuf::from(format!("/tmp/open-{pid}.jsonl"))]
                    .into_iter()
                    .take(limit)
                    .collect();
                (paths, scan_limit.min(1), false)
            },
        );
        assert!(!limited);
        assert_eq!(
            candidates,
            vec![
                AttachmentCandidate {
                    path: PathBuf::from("/tmp/marker-11.jsonl"),
                    expected_session_id: Some("marker-11".to_string()),
                },
                AttachmentCandidate {
                    path: PathBuf::from("/tmp/open-10.jsonl"),
                    expected_session_id: None,
                },
            ]
        );
    }

    #[test]
    fn discovery_stops_at_global_candidate_and_process_limits() {
        let children = HashMap::from([(10, (11..1000).collect())]);
        let mut budget = AttachmentDiscoveryBudget {
            candidate_slots: 2,
            process_slots: 3,
            fd_slots: 4,
        };
        let mut read_budget = MAX_TAIL_WORK_BYTES;
        let (candidates, limited) = discover_attachment_candidates_with(
            10,
            &children,
            &mut budget,
            &mut read_budget,
            |pid, _budget| Some((PathBuf::from(format!("/tmp/{pid}.jsonl")), pid.to_string())),
            |_pid, _result_limit, _scan_limit| (Vec::new(), 0, false),
        );
        assert!(limited);
        assert!(candidates.len() <= 2);
        assert_eq!(budget.process_slots, 0);
    }

    #[cfg(unix)]
    #[test]
    fn session_file_beneath_symlinked_parent_is_canonicalized() {
        let dir = tempfile::tempdir().unwrap();
        let real_dir = dir.path().join("real");
        let linked_dir = dir.path().join("linked");
        fs::create_dir(&real_dir).unwrap();
        std::os::unix::fs::symlink(&real_dir, &linked_dir).unwrap();
        let real_path = real_dir.join("session.jsonl");
        let linked_path = linked_dir.join("session.jsonl");
        fs::write(&real_path, "{}\n").unwrap();

        assert_eq!(
            canonical_regular_jsonl(&linked_path).unwrap(),
            fs::canonicalize(real_path).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_session_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy();
        let target = session_file(&dir, "id", &cwd);
        let link = dir.path().join("link.jsonl");
        std::os::unix::fs::symlink(target, &link).unwrap();
        assert!(canonical_regular_jsonl(&link).is_err());
    }

    #[test]
    fn tailer_reads_appends_and_recovers_from_partial_and_malformed_lines() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy();
        let path = session_file(&dir, "tail", &cwd);
        let mut collector = PiCollector::new();
        let first = collector.tail_session(&path, 10).unwrap().offset;
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"type\":")
            .unwrap();
        let partial = collector.tail_session(&path, 11).unwrap();
        assert_eq!(partial.offset, first);
        assert!(partial.boundary_fingerprint.is_some());
        assert!(!partial.complete);
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\"message\"}\nnot-json\n{}\n")
            .unwrap();
        let recovered = collector.tail_session(&path, 12).unwrap();
        assert!(recovered.complete);
        assert!(recovered.error.is_none());
        let source_updated_at_ms = recovered.source_updated_at_ms;
        let unchanged = collector.tail_session(&path, 13).unwrap();
        assert_eq!(unchanged.source_updated_at_ms, source_updated_at_ms);
    }

    #[test]
    fn tailer_resets_after_truncation_replacement_and_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy();
        let path = session_file(&dir, "first", &cwd);
        let mut collector = PiCollector::new();
        collector.tail_session(&path, 1).unwrap();
        fs::write(&path, session_header("second", &cwd)).unwrap();
        assert_eq!(read_header(&path).unwrap().session_id, "second");
        assert_eq!(
            collector.tail_session(&path, 2).unwrap().offset,
            fs::metadata(&path).unwrap().len()
        );
        let replacement = dir.path().join("replacement.jsonl");
        fs::write(&replacement, session_header("third", &cwd)).unwrap();
        fs::rename(replacement, &path).unwrap();
        assert_eq!(read_header(&path).unwrap().session_id, "third");
        assert_eq!(
            collector.tail_session(&path, 3).unwrap().offset,
            fs::metadata(&path).unwrap().len()
        );
        fs::remove_file(&path).unwrap();
        assert!(collector.tail_session(&path, 4).is_none());
    }

    #[test]
    fn reopened_header_must_match_the_resolved_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let path = session_file(&dir, "owned-a", &cwd);
        let suffix = format!(
            "{{\"type\":\"message\",\"padding\":\"{}\"}}\n",
            "x".repeat(128)
        );
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(suffix.as_bytes())
            .unwrap();
        let attachment = PiAttachment {
            path: path.clone(),
            session_id: "owned-a".to_string(),
            start_id: None,
            header_cwd: cwd.clone(),
            identity: file_identity(&path).unwrap(),
        };
        let mut collector = PiCollector::new();
        let mut budget = MAX_TAIL_WORK_BYTES;
        assert!(collector
            .telemetry_for_attachment(&attachment, 1, &mut budget)
            .is_ok());
        let original_len = fs::metadata(&path).unwrap().len();

        // Keep inode, length, and the tail fingerprint unchanged while replacing
        // only the ownership header.
        fs::write(
            &path,
            format!("{}{suffix}", session_header("owned-b", &cwd)),
        )
        .unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), original_len);
        let mut budget = MAX_TAIL_WORK_BYTES;
        assert!(collector
            .telemetry_for_attachment(&attachment, 2, &mut budget)
            .is_err());
        assert!(!collector.tails.contains_key(&path));
    }

    #[test]
    fn reopened_file_identity_must_match_the_resolved_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let path = session_file(&dir, "owned", &cwd);
        let attachment = PiAttachment {
            path: path.clone(),
            session_id: "owned".to_string(),
            start_id: None,
            header_cwd: cwd.clone(),
            identity: file_identity(&path).unwrap(),
        };
        let replacement = dir.path().join("replacement.jsonl");
        fs::write(&replacement, session_header("owned", &cwd)).unwrap();
        fs::rename(replacement, &path).unwrap();

        let mut collector = PiCollector::new();
        let mut budget = MAX_TAIL_WORK_BYTES;
        assert!(collector
            .telemetry_for_attachment(&attachment, 1, &mut budget)
            .is_err());
        assert!(!collector.tails.contains_key(&path));
    }

    #[test]
    fn attachment_cache_rejects_a_file_claimed_by_two_roots_and_evicts_dead_tails() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let path = session_file(&dir, "shared", &cwd);
        let attachment = PiAttachment {
            path: path.clone(),
            session_id: "shared".to_string(),
            start_id: None,
            header_cwd: cwd.clone(),
            identity: file_identity(&path).unwrap(),
        };
        let mut collector = PiCollector::new();
        collector.cwd_cache.insert(10, cwd.clone());
        collector.cwd_cache.insert(20, cwd);
        collector.attachments.insert(10, attachment.clone());
        collector.attachments.insert(20, attachment);
        collector.tails.insert(
            path,
            PiTail {
                identity: file_identity(&dir.path().join("session.jsonl")).unwrap(),
                header_session_id: "shared".to_string(),
                header_cwd: dir.path().to_string_lossy().into_owned(),
                offset: 0,
                boundary_fingerprint: None,
                discard_oversized_line: false,
                parse_limited: false,
                source_updated_at_ms: None,
                last_successful_parse_at_ms: None,
                complete: false,
                error: None,
                semantic: PiSemantic::default(),
            },
        );
        let process_state = shared(vec![proc(10, 1, "pi"), proc(20, 1, "pi")]);
        let results = collector.resolve_attachments(&[10, 20], &process_state);
        assert!(results.values().all(|result| result.attachment.is_none()));
        assert!(results.values().all(|result| result.error.is_some()));
        collector.collect_sessions(&shared(Vec::new()));
        assert!(collector.tails.is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn one_shared_path_conflict_rejects_a_roots_other_unique_match() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let unique = session_file(&dir, "unique", &cwd);
        let shared_path = dir.path().join("shared.jsonl");
        fs::write(&shared_path, session_header("shared", &cwd)).unwrap();
        let _open_shared = File::open(&shared_path).unwrap();
        let unique_identity = file_identity(&unique).unwrap();
        let shared_identity = file_identity(&shared_path).unwrap();
        let root = std::process::id();
        let other_root = root.saturating_add(1);
        let mut collector = PiCollector::new();
        collector.cwd_cache.insert(root, cwd.clone());
        collector.cwd_cache.insert(other_root, cwd.clone());
        collector.attachments.insert(
            root,
            PiAttachment {
                path: unique,
                session_id: "unique".to_string(),
                start_id: None,
                header_cwd: cwd.clone(),
                identity: unique_identity,
            },
        );
        collector.attachments.insert(
            other_root,
            PiAttachment {
                path: shared_path,
                session_id: "shared".to_string(),
                start_id: None,
                header_cwd: cwd,
                identity: shared_identity,
            },
        );
        let state = shared(vec![proc(root, 1, "pi"), proc(other_root, 1, "pi")]);
        let results = collector.resolve_attachments(&[root, other_root], &state);
        assert!(results.values().all(|result| result.attachment.is_none()));
        assert!(results.values().all(|result| result.error.is_some()));
    }

    #[test]
    fn pid_reuse_drops_cached_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let path = session_file(&dir, "old", &cwd);
        let mut collector = PiCollector::new();
        collector
            .start_id_cache
            .insert(10, Some("old-start".to_string()));
        let identity = file_identity(&path).unwrap();
        collector.attachments.insert(
            10,
            PiAttachment {
                path,
                session_id: "old".to_string(),
                start_id: Some("old-start".to_string()),
                header_cwd: cwd,
                identity,
            },
        );
        collector.collect_sessions(&shared(vec![proc(10, 1, "pi")]));
        assert!(!collector.attachments.contains_key(&10));
    }

    #[test]
    fn oversized_line_stays_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy();
        let path = session_file(&dir, "large", &cwd);
        let mut collector = PiCollector::new();
        collector.tail_session(&path, 1).unwrap();
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&vec![b'x'; MAX_TAIL_LINE_BYTES + 1])
            .unwrap();
        file.write_all(b"\n{}\n").unwrap();
        collector.tail_session(&path, 2).unwrap();
        let tail = collector.tail_session(&path, 3).unwrap();
        assert!(tail.boundary_fingerprint.is_some());
        assert!(tail.parse_limited);
        assert!(
            tail.error.is_none() || tail.error.as_deref() == Some("JSONL line exceeds 1 MiB limit")
        );
    }

    #[test]
    fn partial_content_is_not_retained_and_offset_stays_at_newline() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy();
        let path = session_file(&dir, "privacy", &cwd);
        let mut collector = PiCollector::new();
        let offset = collector.tail_session(&path, 1).unwrap().offset;
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"prompt\":\"secret\"")
            .unwrap();
        let tail = collector.tail_session(&path, 2).unwrap();
        assert_eq!(tail.offset, offset);
        assert!(tail.boundary_fingerprint.is_some());
        assert!(!format!("{tail:?}").contains("secret"));
    }

    #[test]
    fn marker_parser_requires_nul_boundaries() {
        assert!(parse_session_marker(b"PI_SESSION_FILE=/a.jsonl PI_SESSION_ID=id").is_none());
        assert!(parse_session_marker(b"argv PI_SESSION_FILE=/a.jsonl\0PI_SESSION_ID=id").is_none());
    }

    #[test]
    fn header_defaults_missing_version_to_v1_and_rejects_unsupported_versions() {
        assert!(parse_header(br#"{"type":"session","id":"a","cwd":"/tmp"}"#).is_ok());
        assert!(
            parse_header(br#"{"type":"session","version":null,"id":"a","cwd":"/tmp"}"#).is_err()
        );
        assert!(parse_header(br#"{"type":"session","version":4,"id":"a","cwd":"/tmp"}"#).is_err());
        assert!(
            parse_header(br#"{"type":"session","version":"2","id":"a","cwd":"/tmp"}"#).is_err()
        );
        assert!(parse_header(br#"{"type":"session","version":2,"id":"a","cwd":"/tmp"}"#).is_ok());
    }

    #[test]
    fn header_read_is_buffered_and_charged_to_the_shared_budget() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy();
        let path = session_file(&dir, "header-budget", &cwd);
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&vec![b'x'; MAX_TAIL_LINE_BYTES])
            .unwrap();
        let mut file = File::open(path).unwrap();
        let mut budget = MAX_TAIL_WORK_BYTES;
        assert_eq!(
            read_header_from(&mut file, &mut budget).unwrap().session_id,
            "header-budget"
        );
        assert!(MAX_TAIL_WORK_BYTES - budget <= HEADER_READ_CHUNK_BYTES);
    }

    #[test]
    fn parse_limit_persists_after_valid_recovery_until_reset() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy();
        let path = session_file(&dir, "loss", &cwd);
        let mut collector = PiCollector::new();
        collector.tail_session(&path, 1).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"bad\n{}\n")
            .unwrap();
        let tail = collector.tail_session(&path, 2).unwrap();
        assert!(tail.error.is_none());
        assert!(tail.parse_limited);
        assert!(!tail.complete || tail.parse_limited);
    }

    #[test]
    fn shared_budget_limits_all_attached_tail_reads() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy();
        let a = session_file(&dir, "budget-a", &cwd);
        let b = dir.path().join("b.jsonl");
        fs::write(&b, session_header("budget-b", &cwd)).unwrap();
        let mut collector = PiCollector::new();
        let a_offset = collector.tail_session(&a, 1).unwrap().offset;
        let b_offset = collector.tail_session(&b, 1).unwrap().offset;
        for path in [&a, &b] {
            fs::OpenOptions::new()
                .append(true)
                .open(path)
                .unwrap()
                .write_all(b"{}\n")
                .unwrap();
        }

        let a_header_work = fs::metadata(&a).unwrap().len() as usize;
        let b_header_work = fs::metadata(&b).unwrap().len() as usize;
        let mut budget = a_header_work + 3 + b_header_work;
        assert_eq!(
            collector
                .tail_session_with_budget(&a, 2, &mut budget)
                .unwrap()
                .offset,
            a_offset + 3
        );
        assert_eq!(budget, b_header_work);
        assert_eq!(
            collector
                .tail_session_with_budget(&b, 2, &mut budget)
                .unwrap()
                .offset,
            b_offset
        );
        assert_eq!(budget, 0);
    }

    #[test]
    fn truncate_regrow_with_changed_boundary_resets_tail() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy();
        let path = session_file(&dir, "regrow", &cwd);
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"bad\n{}\n")
            .unwrap();
        let mut collector = PiCollector::new();
        let old = collector.tail_session(&path, 1).unwrap();
        assert!(old.parse_limited);
        let old_offset = old.offset;

        let replacement = format!(
            "{}\n{{}}\n{{}}\n{{}}\n",
            serde_json::json!({
                "type": "session",
                "version": 3,
                "id": "regrow",
                "cwd": cwd,
                "replaced": true
            })
        );
        fs::write(&path, replacement).unwrap();
        assert!(fs::metadata(&path).unwrap().len() >= old_offset);
        let tail = collector.tail_session(&path, 2).unwrap();
        assert!(tail.offset > old_offset);
        assert!(
            !tail.parse_limited,
            "reset must discard prior parse-loss state"
        );
    }

    #[test]
    fn cloned_files_with_same_header_id_are_rejected_by_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let first = session_file(&dir, "clone", &cwd);
        let second = dir.path().join("second.jsonl");
        fs::write(&second, fs::read(&first).unwrap()).unwrap();
        let mut collector = PiCollector::new();
        for (pid, path) in [(10, first), (20, second)] {
            collector.cwd_cache.insert(pid, cwd.clone());
            collector.attachments.insert(
                pid,
                PiAttachment {
                    identity: file_identity(&path).unwrap(),
                    path,
                    session_id: "clone".into(),
                    start_id: None,
                    header_cwd: cwd.clone(),
                },
            );
        }
        let state = shared(vec![proc(10, 1, "pi"), proc(20, 1, "pi")]);
        let result = collector.resolve_attachments(&[10, 20], &state);
        assert!(result.values().all(|item| item.attachment.is_none()));
    }

    #[test]
    fn partial_append_does_not_advance_parse_freshness() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy();
        let path = session_file(&dir, "fresh", &cwd);
        let mut collector = PiCollector::new();
        let first = collector
            .tail_session(&path, 10)
            .unwrap()
            .last_successful_parse_at_ms;
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"message\":")
            .unwrap();
        assert_eq!(
            collector
                .tail_session(&path, 11)
                .unwrap()
                .last_successful_parse_at_ms,
            first
        );
    }

    #[test]
    fn semantic_usage_deduplicates_branches_and_context_uses_active_leaf() {
        let mut semantic = PiSemantic::default();
        for line in [
            r#"{"type":"message","id":"u","parentId":null,"message":{"role":"user","content":"hello"}}"#,
            r#"{"type":"message","id":"a","parentId":"u","message":{"role":"assistant","provider":"openai","model":"gpt","usage":{"input":10,"output":5,"cacheRead":2,"cacheWrite":1,"totalTokens":18},"content":[]}}"#,
            r#"{"type":"message","id":"b","parentId":"a","message":{"role":"assistant","provider":"openai","model":"gpt","usage":{"input":3,"output":4,"cacheRead":0,"cacheWrite":0,"totalTokens":7},"content":[]}}"#,
            r#"{"type":"message","id":"other","parentId":"a","message":{"role":"toolResult","usage":{"input":2,"output":0,"cacheRead":0,"cacheWrite":0},"content":"nested"}}"#,
            r#"{"type":"message","id":"b","parentId":"a","message":{"role":"assistant","usage":{"input":999},"content":[]}}"#,
        ] {
            assert!(semantic.add(&serde_json::from_str(line).unwrap()));
        }
        let data = semantic.session_data(true);
        assert_eq!(
            (data.input, data.output, data.cache_read, data.cache_write),
            (15, 9, 2, 1)
        );
        assert_eq!(data.context_tokens, Some(20));
        assert_eq!(data.active_leaf_id.as_deref(), Some("other"));
        assert_eq!(data.context_precision, TelemetryPrecision::Estimated);
    }

    #[test]
    fn semantic_context_rejects_missing_parent_and_requires_post_compaction_baseline() {
        let mut semantic = PiSemantic::default();
        semantic.add(&serde_json::from_str(r#"{"type":"message","id":"a","parentId":"missing","message":{"role":"assistant","usage":{"totalTokens":10},"content":[]}}"#).unwrap());
        assert!(semantic.session_data(true).context_tokens.is_none());
        let mut semantic = PiSemantic::default();
        for line in [
            r#"{"type":"message","id":"a","parentId":null,"message":{"role":"assistant","usage":{"totalTokens":10},"content":[]}}"#,
            r#"{"type":"compaction","id":"c","parentId":"a","usage":{"input":1},"summary":"summary"}"#,
        ] {
            semantic.add(&serde_json::from_str(line).unwrap());
        }
        assert!(semantic.session_data(true).context_tokens.is_none());
    }

    #[test]
    fn semantic_uses_component_fallback_and_excludes_failed_baselines() {
        let mut semantic = PiSemantic::default();
        for line in [
            r#"{"type":"message","id":"a","parentId":null,"message":{"role":"assistant","stopReason":"error","usage":{"totalTokens":99},"content":[]}}"#,
            r#"{"type":"message","id":"b","parentId":"a","message":{"role":"assistant","usage":{"input":2,"output":3,"cacheRead":4,"cacheWrite":5},"content":[]}}"#,
        ] {
            semantic.add(&serde_json::from_str(line).unwrap());
        }
        let data = semantic.session_data(true);
        assert_eq!(data.baseline_tokens, Some(14));
    }

    #[test]
    fn header_only_semantics_are_complete_known_zero_without_a_leaf() {
        let semantic = PiSemantic::default();
        let data = semantic.session_data(true);
        assert_eq!(data.usage_completeness, TelemetryCompleteness::Complete);
        assert_eq!(
            (data.input, data.output, data.cache_read, data.cache_write),
            (0, 0, 0, 0)
        );
        assert!(data.active_leaf_id.is_none());
        assert!(data.context_tokens.is_none());
    }

    #[test]
    fn incomplete_or_invalid_semantics_hide_context_but_keep_partial_usage() {
        let mut semantic = PiSemantic::default();
        semantic.add(&serde_json::from_str(r#"{"type":"message","id":"a","parentId":null,"message":{"role":"assistant","usage":{"input":2,"output":3,"cacheRead":4,"cacheWrite":5,"totalTokens":14},"content":[]}}"#).unwrap());
        semantic.invalid = true;
        let data = semantic.session_data(false);
        assert_eq!(
            data.input + data.output + data.cache_read + data.cache_write,
            14
        );
        assert_eq!(data.usage_completeness, TelemetryCompleteness::Partial);
        assert!(data.context_tokens.is_none());
        assert!(data.active_leaf_id.is_none());
    }

    #[test]
    fn context_estimation_handles_custom_messages_utf16_and_excluded_bash() {
        let mut semantic = PiSemantic::default();
        for line in [
            r#"{"type":"message","id":"u","parentId":null,"message":{"role":"user","content":"😀"}}"#,
            r#"{"type":"custom_message","id":"c","parentId":"u","content":"😀"}"#,
            r#"{"type":"message","id":"b","parentId":"c","message":{"role":"bashExecution","command":"four","output":"four","excludeFromContext":true}}"#,
        ] {
            assert!(semantic.add(&serde_json::from_str(line).unwrap()));
        }
        let data = semantic.session_data(true);
        // Pi rounds each context-visible message independently: ceil(2/4) + ceil(2/4).
        assert_eq!(data.context_tokens, Some(2));
        assert_eq!(data.context_precision, TelemetryPrecision::Estimated);
    }

    #[test]
    fn malformed_usage_never_becomes_a_baseline_or_complete_total() {
        let mut semantic = PiSemantic::default();
        semantic.add(&serde_json::from_str(r#"{"type":"message","id":"a","parentId":null,"message":{"role":"assistant","usage":{"input":2,"output":3,"cacheRead":"bad","cacheWrite":5},"content":[]}}"#).unwrap());
        let data = semantic.session_data(true);
        assert_eq!(
            data.input + data.output + data.cache_read + data.cache_write,
            0
        );
        assert_eq!(data.usage_completeness, TelemetryCompleteness::Partial);
        assert!(data.baseline_tokens.is_none());
    }

    #[test]
    fn model_catalog_is_provider_scoped_and_applies_overrides_custom_defaults_and_agent_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("alternate-agent");
        fs::create_dir_all(root.join("sessions/project")).unwrap();
        fs::write(root.join("models-store.json"), r#"{"one":{"models":[{"id":"same","contextWindow":10}]},"two":{"models":[{"id":"same","contextWindow":20}]}}"#).unwrap();
        fs::write(root.join("models.json"), r#"{"providers":{"one":{"modelOverrides":{"same":{"contextWindow":15}},"models":[{"id":"custom"}]}}}"#).unwrap();
        let mut cache = ModelCatalogCache::default();
        refresh_model_catalog(&mut cache, &root);
        assert_eq!(model_window_from_catalog(&cache, "one", "same"), Some(15));
        assert_eq!(model_window_from_catalog(&cache, "two", "same"), Some(20));
        assert_eq!(
            model_window_from_catalog(&cache, "one", "custom"),
            Some(128_000)
        );
        assert_eq!(model_window_from_catalog(&cache, "two", "custom"), None);
        assert_eq!(
            pi_agent_root(&root.join("sessions/project/session.jsonl")),
            Some(root)
        );
    }

    #[test]
    fn missing_parent_id_is_malformed_not_a_root() {
        let mut semantic = PiSemantic::default();
        assert!(!semantic.add(&serde_json::from_str(
            r#"{"type":"message","id":"a","message":{"role":"assistant","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0},"content":[]}}"#,
        ).unwrap()));
        assert!(semantic.invalid);
        assert_eq!(
            semantic.session_data(false).context_completeness,
            TelemetryCompleteness::Partial
        );
    }

    #[test]
    fn catalog_override_cannot_create_a_model_and_cache_retains_only_windows() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("agent");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("models-store.json"),
            r#"{"one":{"models":[{"id":"base","contextWindow":10}]}}"#,
        )
        .unwrap();
        fs::write(root.join("models.json"), r#"{"providers":{"one":{"modelOverrides":{"base":{"contextWindow":15},"missing":{"contextWindow":999}},"models":[{"id":"custom"}]}}}"#).unwrap();
        let mut cache = ModelCatalogCache::default();
        refresh_model_catalog(&mut cache, &root);
        assert_eq!(model_window_from_catalog(&cache, "one", "base"), Some(15));
        assert_eq!(model_window_from_catalog(&cache, "one", "missing"), None);
        assert_eq!(
            model_window_from_catalog(&cache, "one", "custom"),
            Some(128_000)
        );
        assert_eq!(cache.windows.len(), 2);
        // ModelCatalogCache has no serde_json::Value fields: secrets in source files are dropped.
        assert!(!format!("{cache:?}").contains("apiKey"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_file_identity_survives_in_place_updates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.jsonl");
        fs::write(&path, "first\n").unwrap();
        let identity = file_identity(&path).unwrap();
        assert!(identity.volume_serial_number.is_some());
        assert!(identity.file_index.is_some());
        assert!(identity.modified_ms.is_none());

        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"second\n")
            .unwrap();
        assert_eq!(file_identity(&path), Some(identity));
    }

    #[test]
    fn catalog_same_length_replacement_refreshes_by_identity() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("agent");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("models-store.json");
        let first = r#"{"one":{"models":[{"id":"same","contextWindow":10}]}}"#;
        let second = r#"{"one":{"models":[{"id":"same","contextWindow":20}]}}"#;
        assert_eq!(first.len(), second.len());
        fs::write(&path, first).unwrap();
        fs::write(root.join("models.json"), "{}").unwrap();
        let mut cache = ModelCatalogCache::default();
        refresh_model_catalog(&mut cache, &root);
        assert_eq!(model_window_from_catalog(&cache, "one", "same"), Some(10));
        let replacement = root.join("replacement.json");
        fs::write(&replacement, second).unwrap();
        fs::rename(replacement, &path).unwrap();
        refresh_model_catalog(&mut cache, &root);
        assert_eq!(model_window_from_catalog(&cache, "one", "same"), Some(20));
    }

    #[test]
    fn overflow_or_invalid_cost_makes_usage_partial() {
        let mut semantic = PiSemantic::default();
        for line in [
            r#"{"type":"message","id":"a","parentId":null,"message":{"role":"assistant","usage":{"input":18446744073709551615,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":1,"cost":{"total":1}},"content":[]}}"#,
            r#"{"type":"message","id":"b","parentId":"a","message":{"role":"assistant","usage":{"input":1,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":1,"cost":{"total":-1}},"content":[]}}"#,
        ] {
            assert!(semantic.add(&serde_json::from_str(line).unwrap()));
        }
        let data = semantic.session_data(true);
        assert!(!data.usage_available);
        assert_eq!(data.usage_completeness, TelemetryCompleteness::Partial);
        assert!(data.cost.is_none());

        let mut cross_component = PiSemantic::default();
        assert!(cross_component.add(&serde_json::from_str(
            r#"{"type":"message","id":"a","parentId":null,"message":{"role":"assistant","usage":{"input":18446744073709551615,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":1},"content":[]}}"#,
        ).unwrap()));
        let data = cross_component.session_data(true);
        assert!(!data.usage_available);
        assert_eq!(data.usage_completeness, TelemetryCompleteness::Partial);
        assert!(data
            .usage_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("combined usage total overflowed")));
    }

    #[test]
    fn oversized_catalog_is_unavailable_without_retaining_raw_data() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("agent");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("models-store.json"),
            vec![b'x'; MAX_MODEL_CATALOG_BYTES as usize + 1],
        )
        .unwrap();
        let mut cache = ModelCatalogCache::default();
        refresh_model_catalog(&mut cache, &root);
        assert!(cache.unavailable);
        assert!(cache.windows.is_empty());
    }

    #[test]
    fn collector_evicts_catalogs_without_owned_attachments() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("agent");
        let mut collector = PiCollector::new();
        collector
            .model_catalogs
            .insert(root, ModelCatalogCache::default());
        collector.collect_sessions(&shared(Vec::new()));
        assert!(collector.model_catalogs.is_empty());
    }

    #[test]
    fn documented_parser_limits_match_release_constants() {
        let docs = include_str!("../../docs/pi-support.md");
        let expected = [
            format!(
                "{} MiB of Pi session JSONL per collection tick",
                MAX_TAIL_WORK_BYTES / (1024 * 1024)
            ),
            format!(
                "{} MiB per Pi session JSONL line",
                MAX_TAIL_LINE_BYTES / (1024 * 1024)
            ),
            format!(
                "{} semantic tree entries per attached session",
                MAX_SEMANTIC_ENTRIES
            ),
            format!(
                "{} attachment candidates per collection tick",
                MAX_ATTACHMENT_CANDIDATES_PER_COLLECT
            ),
            format!(
                "{} Pi processes considered for attachment per collection tick",
                MAX_PROCESSES_SCANNED_PER_COLLECT
            ),
            format!(
                "{} open file descriptors per collection tick",
                MAX_OPEN_FDS_SCANNED_PER_COLLECT
            ),
            format!(
                "Model catalog files are capped at {} MiB and {} retained model entries",
                MAX_MODEL_CATALOG_BYTES / (1024 * 1024) as u64,
                MAX_MODEL_CATALOG_ENTRIES
            ),
        ];
        for expected in expected {
            assert!(
                docs.contains(&expected),
                "missing documented limit: {expected}"
            );
        }
    }

    #[test]
    #[ignore = "release benchmark; run with cargo test --release pi_parser_release_benchmark -- --ignored --nocapture"]
    fn pi_parser_release_benchmark() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let mut fixture =
            b"{\"type\":\"session\",\"version\":3,\"id\":\"benchmark\",\"cwd\":\"/tmp\"}\n"
                .to_vec();
        let padding = "x".repeat(96);
        for index in 0..MAX_SEMANTIC_ENTRIES {
            let parent = if index == 0 {
                "null".to_string()
            } else {
                format!("\"entry-{}\"", index - 1)
            };
            let line = format!(
                "{{\"type\":\"message\",\"id\":\"entry-{index}\",\"parentId\":{parent},\"message\":{{\"role\":\"assistant\",\"usage\":{{\"input\":1,\"output\":1,\"cacheRead\":1,\"cacheWrite\":1,\"totalTokens\":4}},\"content\":[{{\"type\":\"text\",\"text\":\"{padding}\"}}]}}}}\n"
            );
            fixture.extend_from_slice(line.as_bytes());
            if fixture.len() > MAX_TAIL_WORK_BYTES + MAX_TAIL_LINE_BYTES.min(line.len()) {
                break;
            }
        }
        assert!(fixture.len() > MAX_TAIL_WORK_BYTES);
        fs::write(&path, &fixture).unwrap();

        let started = Instant::now();
        let mut collector = PiCollector::new();
        let mut budget = MAX_TAIL_WORK_BYTES;
        let tail = collector
            .tail_session_with_budget(&path, 1, &mut budget)
            .unwrap();
        let parsed = tail.semantic.session_data(tail.complete);
        let offset = tail.offset;
        let complete = tail.complete;
        let entry_count = tail.semantic.entries.len();
        let elapsed = started.elapsed();

        assert_eq!(
            budget, 0,
            "production tailer did not consume its tick budget"
        );
        assert!(!complete, "oversized fixture should require another tick");
        assert!(offset > (MAX_TAIL_WORK_BYTES / 2) as u64);
        assert!(offset <= MAX_TAIL_WORK_BYTES as u64);
        assert_eq!(parsed.turns as usize, entry_count);
        eprintln!("tailed {offset} JSONL bytes across {entry_count} entries in {elapsed:?}");
        assert!(
            elapsed <= Duration::from_secs(1),
            "2 MiB parser gate exceeded one second: {elapsed:?}"
        );
    }
}
