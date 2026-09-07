use super::{process, AgentCollector, SharedProcessData};
use crate::model::{
    AgentSession, AttachmentConfidence, AttachmentState, ChildProcess, SessionStatus,
    SessionTelemetry, SourceHealth, TelemetryCompleteness, TelemetryMetadata,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
#[cfg(target_vendor = "apple")]
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_TAIL_WORK_BYTES: usize = 2 * 1024 * 1024;
const MAX_TAIL_LINE_BYTES: usize = 1024 * 1024;
const MAX_ATTACHMENT_CANDIDATES_PER_COLLECT: usize = 128;
const MAX_PROCESSES_SCANNED_PER_COLLECT: usize = 128;
const MAX_OPEN_FDS_SCANNED_PER_COLLECT: usize = 4096;
#[cfg(target_vendor = "apple")]
const MAX_LSOF_RECORD_BYTES: usize = 4096;
const HEADER_READ_CHUNK_BYTES: usize = 4096;
const MAX_TELEMETRY_ERROR_BYTES: usize = 160;

/// Passive collector for local Pi coding-agent processes.
///
/// Process rows exist without telemetry. Session JSONL is attached only after
/// ownership is proved with high-confidence evidence.
pub struct PiCollector {
    first_seen_ms: HashMap<u32, u64>,
    cwd_cache: HashMap<u32, String>,
    start_id_cache: HashMap<u32, Option<String>>,
    attachments: HashMap<u32, PiAttachment>,
    tails: HashMap<PathBuf, PiTail>,
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

impl PiCollector {
    pub fn new() -> Self {
        Self {
            first_seen_ms: HashMap::new(),
            cwd_cache: HashMap::new(),
            start_id_cache: HashMap::new(),
            attachments: HashMap::new(),
            tails: HashMap::new(),
        }
    }

    fn collect_sessions(&mut self, shared: &SharedProcessData) -> Vec<AgentSession> {
        let pi_pids = top_level_pi_pids(&shared.process_info);
        let live: HashSet<u32> = pi_pids.iter().copied().collect();
        self.first_seen_ms.retain(|pid, _| live.contains(pid));
        self.cwd_cache.retain(|pid, _| live.contains(pid));
        self.start_id_cache.retain(|pid, _| live.contains(pid));
        self.attachments.retain(|pid, _| live.contains(pid));

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
            }
            self.start_id_cache.insert(*pid, start_id);
            self.first_seen_ms.entry(*pid).or_insert(observed_at_ms);
            if shared.slow_tick || !self.cwd_cache.contains_key(pid) {
                let cwd = process_cwd(*pid).unwrap_or_default();
                self.cwd_cache.insert(*pid, cwd);
            }
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
                let attachment_result = attachment_results.get(&pid)?;
                let requested_attachment = attachment_result.attachment.as_ref();
                let (attachment, telemetry) = match requested_attachment {
                    Some(attachment) => match self.telemetry_for_attachment(
                        attachment,
                        observed_at_ms,
                        &mut read_budget,
                    ) {
                        Ok(telemetry) => (Some(attachment), telemetry),
                        Err(error) => {
                            self.attachments.remove(&pid);
                            (None, process_only_telemetry(observed_at_ms, Some(error)))
                        }
                    },
                    None => (
                        None,
                        process_only_telemetry(observed_at_ms, attachment_result.error.clone()),
                    ),
                };
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
    ) -> Result<SessionTelemetry, String> {
        let expected_header = (&attachment.session_id[..], &attachment.header_cwd[..]);
        let tail = self.tail_session_with_expected_header(
            &attachment.path,
            observed_at_ms,
            tail_budget,
            Some(expected_header),
            Some(&attachment.identity),
        )?;
        let source_health = if tail.error.is_some() {
            SourceHealth::Error
        } else {
            SourceHealth::Healthy
        };
        let completeness = if tail.complete && !tail.parse_limited {
            TelemetryCompleteness::Complete
        } else {
            TelemetryCompleteness::Partial
        };
        let metadata = TelemetryMetadata {
            precision: crate::model::TelemetryPrecision::Unknown,
            completeness,
            provenance: "owned Pi session JSONL".to_string(),
            source_updated_at_ms: tail.source_updated_at_ms,
            observed_at_ms,
            last_successful_parse_at_ms: tail.last_successful_parse_at_ms,
            stale: false,
        };
        Ok(SessionTelemetry {
            attachment: AttachmentState::Attached,
            attachment_confidence: AttachmentConfidence::High,
            source_health,
            error: tail.error.clone(),
            context: metadata.clone(),
            usage: metadata,
        })
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
        let identity = file_identity_from_metadata(&metadata);
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
    let identity = file_identity_from_metadata(&metadata);
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
    let canonical =
        fs::canonicalize(path).map_err(|_| "session file cannot be canonicalized".to_string())?;
    if canonical != path {
        return Err("session file path must not traverse a symlink".to_string());
    }
    Ok(canonical)
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
            if serde_json::from_slice::<Value>(&bytes[cursor..end]).is_ok() {
                valid += 1;
                tail.error = None;
            } else {
                tail.parse_limited = true;
                tail.error = Some("malformed JSONL line ignored".to_string());
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
    let metadata = fs::metadata(path).ok()?;
    Some(file_identity_from_metadata(&metadata))
}

fn file_identity_from_metadata(metadata: &fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
    #[cfg(not(unix))]
    {
        FileIdentity {
            modified_ms: metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as u64),
        }
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

    fn session_file(dir: &tempfile::TempDir, id: &str, cwd: &str) -> PathBuf {
        let path = dir.path().join("session.jsonl");
        fs::write(
            &path,
            format!("{{\"type\":\"session\",\"version\":3,\"id\":\"{id}\",\"cwd\":\"{cwd}\"}}\n"),
        )
        .unwrap();
        path
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
        assert_eq!(validate_candidate(&candidate, &cwd).unwrap().0, path);
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
        fs::write(
            &path,
            format!("{{\"type\":\"session\",\"version\":3,\"id\":\"second\",\"cwd\":\"{cwd}\"}}\n"),
        )
        .unwrap();
        assert_eq!(read_header(&path).unwrap().session_id, "second");
        assert_eq!(
            collector.tail_session(&path, 2).unwrap().offset,
            fs::metadata(&path).unwrap().len()
        );
        let replacement = dir.path().join("replacement.jsonl");
        fs::write(
            &replacement,
            format!("{{\"type\":\"session\",\"version\":3,\"id\":\"third\",\"cwd\":\"{cwd}\"}}\n"),
        )
        .unwrap();
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
            format!(
                "{{\"type\":\"session\",\"version\":3,\"id\":\"owned-b\",\"cwd\":\"{cwd}\"}}\n{suffix}"
            ),
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
        fs::write(
            &replacement,
            format!("{{\"type\":\"session\",\"version\":3,\"id\":\"owned\",\"cwd\":\"{cwd}\"}}\n"),
        )
        .unwrap();
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
        fs::write(
            &shared_path,
            format!("{{\"type\":\"session\",\"version\":3,\"id\":\"shared\",\"cwd\":\"{cwd}\"}}\n"),
        )
        .unwrap();
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
        fs::write(
            &b,
            format!(
                "{{\"type\":\"session\",\"version\":3,\"id\":\"budget-b\",\"cwd\":\"{cwd}\"}}\n"
            ),
        )
        .unwrap();
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
            "{{\"type\":\"session\",\"version\":3,\"id\":\"regrow\",\"cwd\":\"{cwd}\",\"replaced\":true}}\n{{}}\n{{}}\n{{}}\n"
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
}
