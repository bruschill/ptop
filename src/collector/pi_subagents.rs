use crate::model::{
    FleetChild, FleetExecution, FleetIdentitySource, FleetProcessTerminal,
    FleetProcessTerminalState, FleetRun, FleetRunMode, FleetRunState, FleetTelemetry, FleetUsage,
    FleetUsageAccounting, SourceHealth,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SUPPORTED_LIFECYCLE_VERSION: u64 = 3;
const MAX_STATUS_BYTES: u64 = 256 * 1024;
const MAX_STATUS_READ_BYTES: usize = 2 * 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 512;
const MAX_STATUS_FILES: usize = 128;
const MAX_RUNS_PER_SESSION: usize = 20;
const MAX_CHILDREN_PER_RUN: usize = 64;
const MAX_CHILD_DEPTH: usize = 3;
const MAX_ID_BYTES: usize = 256;
const MAX_SESSION_REFERENCE_BYTES: usize = 128 * 1024;
const MAX_LABEL_BYTES: usize = 160;
const MISSING_RUNNER_STALE_AFTER_MS: u64 = 30_000;
const UNKNOWN_RUNNER_STALE_AFTER_MS: u64 = 24 * 60 * 60 * 1_000;
pub(crate) const COMPLETED_RUN_RETENTION_DAYS: u64 = 30;
const COMPLETED_RUN_RETENTION_MS: u64 = COMPLETED_RUN_RETENTION_DAYS * 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Copy)]
struct StatusCapabilities {
    identity: bool,
    lifecycle: bool,
    usage: bool,
    children: bool,
    process_terminal: bool,
}

// Capability matrix for extension-owned lifecycle artifacts. Unknown and
// missing versions expose identity only, with every other field unavailable.
const LIFECYCLE_V3_CAPABILITIES: StatusCapabilities = StatusCapabilities {
    identity: true,
    lifecycle: true,
    usage: true,
    children: true,
    process_terminal: true,
};

fn capabilities(version: Option<u64>) -> Option<StatusCapabilities> {
    (version == Some(SUPPORTED_LIFECYCLE_VERSION)).then_some(LIFECYCLE_V3_CAPABILITIES)
}

#[derive(Debug, Clone)]
pub(crate) struct PiSubagentParent {
    pub(crate) session_id: String,
    pub(crate) session_file: PathBuf,
}

pub(crate) struct PiSubagentsCollector {
    async_root: PathBuf,
}

impl PiSubagentsCollector {
    pub(crate) fn new() -> Self {
        Self {
            async_root: default_async_root(),
        }
    }

    #[cfg(test)]
    fn with_async_root(async_root: PathBuf) -> Self {
        Self { async_root }
    }

    pub(crate) fn collect(
        &self,
        parents: &[PiSubagentParent],
        live_pids: &HashSet<u32>,
        verified_runner_runs: &HashMap<u32, String>,
        observed_at_ms: u64,
    ) -> HashMap<String, FleetTelemetry> {
        let aliases = parent_aliases(parents);
        let mut by_session: HashMap<String, FleetTelemetry> = parents
            .iter()
            .map(|parent| {
                (
                    parent.session_id.clone(),
                    FleetTelemetry::unavailable(
                        observed_at_ms,
                        "pi-subagents status root unavailable",
                    ),
                )
            })
            .collect();
        if parents.is_empty() {
            return by_session;
        }

        let scan = match status_candidates(&self.async_root) {
            Ok(scan) => scan,
            Err(reason) => {
                for telemetry in by_session.values_mut() {
                    telemetry.reason = Some(reason.to_string());
                }
                return by_session;
            }
        };

        for telemetry in by_session.values_mut() {
            telemetry.source_health = SourceHealth::Healthy;
            telemetry.reason = None;
            telemetry.omitted_statuses = scan.omitted as u32;
        }

        let mut remaining_bytes = MAX_STATUS_READ_BYTES;
        let mut pending = Vec::new();
        for candidate in scan.candidates {
            if candidate.length > MAX_STATUS_BYTES {
                mark_malformed(&mut by_session);
                continue;
            }
            let length = match usize::try_from(candidate.length) {
                Ok(length) if length <= remaining_bytes => length,
                _ => {
                    mark_omitted(&mut by_session, 1);
                    continue;
                }
            };
            remaining_bytes = remaining_bytes.saturating_sub(length);

            let bytes = match read_status_file(&candidate.path, &candidate.identity, length) {
                Ok(bytes) => bytes,
                Err(_) => {
                    mark_malformed(&mut by_session);
                    continue;
                }
            };
            let parsed = match parse_status(
                &bytes,
                &candidate.run_dir_name,
                candidate.modified_at_ms,
                observed_at_ms,
                live_pids,
                verified_runner_runs,
            ) {
                Ok(parsed) => parsed,
                Err(_) => {
                    mark_malformed(&mut by_session);
                    continue;
                }
            };
            let Some(parent_id) = aliases.get(&parsed.session_id).cloned() else {
                continue;
            };
            let telemetry = by_session
                .get_mut(&parent_id)
                .expect("parent alias points to a known session");
            telemetry.scanned_statuses = telemetry.scanned_statuses.saturating_add(1);
            telemetry.source_updated_at_ms = max_option(
                telemetry.source_updated_at_ms,
                Some(parsed.run.source_updated_at_ms),
            );
            if parsed.unsupported {
                telemetry.unsupported_statuses = telemetry.unsupported_statuses.saturating_add(1);
            }
            if !within_retention(&parsed.run, observed_at_ms) {
                telemetry.omitted_statuses = telemetry.omitted_statuses.saturating_add(1);
                continue;
            }
            pending.push((parent_id, parsed.run));
        }

        let mut embedded_ids_by_session: HashMap<String, HashSet<String>> = HashMap::new();
        let mut embedded_ids_by_parent: HashMap<String, HashMap<String, HashSet<String>>> =
            HashMap::new();
        for (parent_id, run) in &pending {
            let mut embedded = HashSet::new();
            collect_child_ids(&run.children, &mut embedded);
            embedded_ids_by_session
                .entry(parent_id.clone())
                .or_default()
                .extend(embedded.iter().cloned());
            embedded_ids_by_parent
                .entry(parent_id.clone())
                .or_default()
                .insert(run.run_id.clone(), embedded);
        }
        let mut published_ids_by_session: HashMap<String, HashSet<String>> = HashMap::new();
        for (parent_id, run) in pending {
            let represented_by_parent = run.parent_run_id.as_ref().is_some_and(|parent_run_id| {
                embedded_ids_by_parent
                    .get(&parent_id)
                    .and_then(|parents| parents.get(parent_run_id))
                    .is_some_and(|embedded| embedded.contains(&run.run_id))
            }) || (run.nested
                && embedded_ids_by_session
                    .get(&parent_id)
                    .is_some_and(|embedded| embedded.contains(&run.run_id)));
            let duplicate = !published_ids_by_session
                .entry(parent_id.clone())
                .or_default()
                .insert(run.run_id.clone());
            let telemetry = by_session
                .get_mut(&parent_id)
                .expect("pending run belongs to a known session");
            if represented_by_parent || duplicate {
                telemetry.omitted_statuses = telemetry.omitted_statuses.saturating_add(1);
            } else {
                telemetry.runs.push(run);
            }
        }

        for telemetry in by_session.values_mut() {
            telemetry.runs.sort_by(|left, right| {
                right
                    .source_updated_at_ms
                    .cmp(&left.source_updated_at_ms)
                    .then_with(|| left.run_id.cmp(&right.run_id))
            });
            if telemetry.runs.len() > MAX_RUNS_PER_SESSION {
                telemetry.omitted_statuses = telemetry
                    .omitted_statuses
                    .saturating_add((telemetry.runs.len() - MAX_RUNS_PER_SESSION) as u32);
                telemetry.runs.truncate(MAX_RUNS_PER_SESSION);
            }
            telemetry.stale = telemetry.runs.iter().any(|run| run.stale);
            if telemetry.malformed_statuses > 0 {
                telemetry.source_health = SourceHealth::Error;
                telemetry.reason = Some(format!(
                    "{} malformed or unsafe status file(s) ignored",
                    telemetry.malformed_statuses
                ));
            } else if telemetry.stale {
                telemetry.source_health = SourceHealth::Stale;
                telemetry.reason = Some("one or more active run snapshots are stale".to_string());
            } else if telemetry.unsupported_statuses > 0 {
                telemetry.reason = Some(format!(
                    "{} unsupported lifecycle artifact version(s)",
                    telemetry.unsupported_statuses
                ));
            }
        }

        by_session
    }
}

fn parent_aliases(parents: &[PiSubagentParent]) -> HashMap<String, String> {
    let mut aliases = HashMap::new();
    let mut conflicts = HashSet::new();
    for parent in parents {
        for alias in [
            parent.session_id.clone(),
            parent.session_file.to_string_lossy().into_owned(),
        ] {
            if conflicts.contains(&alias) {
                continue;
            }
            if aliases
                .get(&alias)
                .is_some_and(|owner| owner != &parent.session_id)
            {
                aliases.remove(&alias);
                conflicts.insert(alias);
            } else {
                aliases.insert(alias, parent.session_id.clone());
            }
        }
    }
    aliases
}

fn collect_child_ids(children: &[FleetChild], ids: &mut HashSet<String>) {
    for child in children {
        if let Some(run_id) = &child.run_id {
            ids.insert(run_id.clone());
        } else if child.identity_source == FleetIdentitySource::RunId {
            ids.insert(child.id.clone());
        }
        collect_child_ids(&child.children, ids);
    }
}

struct StatusScan {
    candidates: Vec<StatusCandidate>,
    omitted: usize,
}

struct StatusCandidate {
    path: PathBuf,
    run_dir_name: String,
    length: u64,
    modified_at_ms: u64,
    identity: StatusFileIdentity,
}

#[derive(Clone)]
struct StatusFileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    length: u64,
    #[cfg(not(unix))]
    modified: Option<SystemTime>,
}

impl PartialEq for StatusFileIdentity {
    fn eq(&self, other: &Self) -> bool {
        #[cfg(unix)]
        {
            self.device == other.device && self.inode == other.inode
        }
        #[cfg(not(unix))]
        {
            self.length == other.length && self.modified == other.modified
        }
    }
}

fn status_file_identity(metadata: &fs::Metadata) -> StatusFileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        StatusFileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
    #[cfg(not(unix))]
    {
        StatusFileIdentity {
            length: metadata.len(),
            modified: metadata.modified().ok(),
        }
    }
}

fn status_candidates(async_root: &Path) -> Result<StatusScan, &'static str> {
    let root_metadata = match fs::symlink_metadata(async_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err("pi-subagents status root unavailable")
        }
        Err(_) => return Err("pi-subagents status root is unreadable"),
    };
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err("pi-subagents status root is not a regular directory");
    }
    let canonical_root =
        fs::canonicalize(async_root).map_err(|_| "pi-subagents status root is unreadable")?;
    let entries =
        fs::read_dir(&canonical_root).map_err(|_| "pi-subagents status root is unreadable")?;
    let mut candidates = Vec::new();
    let mut omitted: usize = 0;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_DIRECTORY_ENTRIES {
            omitted = omitted.saturating_add(1);
            break;
        }
        let Ok(entry) = entry else {
            omitted = omitted.saturating_add(1);
            continue;
        };
        let run_dir = entry.path();
        let Ok(run_metadata) = fs::symlink_metadata(&run_dir) else {
            omitted = omitted.saturating_add(1);
            continue;
        };
        if run_metadata.file_type().is_symlink() || !run_metadata.is_dir() {
            continue;
        }
        let status_path = run_dir.join("status.json");
        let Ok(status_path_metadata) = fs::symlink_metadata(&status_path) else {
            continue;
        };
        if status_path_metadata.file_type().is_symlink() || !status_path_metadata.is_file() {
            omitted = omitted.saturating_add(1);
            continue;
        }
        let Ok(canonical_status) = fs::canonicalize(&status_path) else {
            omitted = omitted.saturating_add(1);
            continue;
        };
        if !canonical_status.starts_with(&canonical_root) {
            omitted = omitted.saturating_add(1);
            continue;
        }
        let Some(run_dir_name) = run_dir.file_name().and_then(|name| name.to_str()) else {
            omitted = omitted.saturating_add(1);
            continue;
        };
        candidates.push(StatusCandidate {
            path: canonical_status,
            run_dir_name: run_dir_name.to_string(),
            length: status_path_metadata.len(),
            modified_at_ms: system_time_ms(status_path_metadata.modified().ok()).unwrap_or(0),
            identity: status_file_identity(&status_path_metadata),
        });
    }
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.modified_at_ms));
    if candidates.len() > MAX_STATUS_FILES {
        omitted = omitted.saturating_add(candidates.len() - MAX_STATUS_FILES);
        candidates.truncate(MAX_STATUS_FILES);
    }
    Ok(StatusScan {
        candidates,
        omitted,
    })
}

fn read_status_file(
    path: &Path,
    expected_identity: &StatusFileIdentity,
    expected_length: usize,
) -> Result<Vec<u8>, ()> {
    let mut file = File::open(path).map_err(|_| ())?;
    let opened_metadata = file.metadata().map_err(|_| ())?;
    if !opened_metadata.is_file()
        || status_file_identity(&opened_metadata) != *expected_identity
        || opened_metadata.len() != expected_length as u64
    {
        return Err(());
    }
    let mut bytes = Vec::with_capacity(expected_length);
    file.by_ref()
        .take(expected_length as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if bytes.len() != expected_length {
        return Err(());
    }
    let path_metadata = fs::symlink_metadata(path).map_err(|_| ())?;
    if path_metadata.file_type().is_symlink()
        || !path_metadata.is_file()
        || status_file_identity(&path_metadata) != *expected_identity
    {
        return Err(());
    }
    Ok(bytes)
}

struct ParsedStatus {
    session_id: String,
    run: FleetRun,
    unsupported: bool,
}

fn parse_status(
    bytes: &[u8],
    run_dir_name: &str,
    modified_at_ms: u64,
    observed_at_ms: u64,
    live_pids: &HashSet<u32>,
    verified_runner_runs: &HashMap<u32, String>,
) -> Result<ParsedStatus, ()> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    let run_id = identity_string(object.get("runId"), MAX_ID_BYTES).ok_or(())?;
    if run_id != run_dir_name
        || Path::new(&run_id)
            .file_name()
            .and_then(|name| name.to_str())
            != Some(&run_id)
    {
        return Err(());
    }
    let session_id =
        identity_string(object.get("sessionId"), MAX_SESSION_REFERENCE_BYTES).ok_or(())?;
    let lifecycle_version = object.get("lifecycleArtifactVersion").and_then(json_u64);
    let Some(supported) = capabilities(lifecycle_version) else {
        return Ok(ParsedStatus {
            session_id,
            run: FleetRun {
                lifecycle_version,
                run_id,
                parent_run_id: None,
                nested: false,
                mode: FleetRunMode::Unknown,
                state: FleetRunState::Unknown,
                execution: FleetExecution::Background,
                runner_pid: None,
                started_at_ms: None,
                updated_at_ms: None,
                ended_at_ms: None,
                source_updated_at_ms: modified_at_ms.min(observed_at_ms),
                stale: false,
                process_terminal: None,
                usage: FleetUsage::separate_run_aggregate(),
                children: Vec::new(),
                omitted_children: 0,
                reason: Some(match lifecycle_version {
                    Some(version) => format!("unsupported lifecycle artifact version {version}"),
                    None => "missing lifecycle artifact version".to_string(),
                }),
            },
            unsupported: true,
        });
    };
    debug_assert!(supported.identity);

    let mode = parse_mode(object.get("mode")).ok_or(())?;
    let state = parse_state(object.get("state")).ok_or(())?;
    let parent_run_id = identity_string(object.get("parentWorkflowRunId"), MAX_ID_BYTES);
    let nested = object
        .get("isNested")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let runner_pid = object
        .get("pid")
        .and_then(json_u64)
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0);
    let started_at_ms = object.get("startedAt").and_then(json_u64).ok_or(())?;
    let updated_at_ms = object.get("lastUpdate").and_then(json_u64);
    let ended_at_ms = object.get("endedAt").and_then(json_u64);
    let source_updated_at_ms = modified_at_ms
        .max(updated_at_ms.unwrap_or(started_at_ms))
        .min(observed_at_ms);
    let age_ms = observed_at_ms.saturating_sub(source_updated_at_ms);
    let stale = state.is_active()
        && match runner_pid {
            Some(pid)
                if verified_runner_runs.get(&pid).map(String::as_str) == Some(run_id.as_str()) =>
            {
                false
            }
            Some(pid) if !live_pids.contains(&pid) => age_ms > MISSING_RUNNER_STALE_AFTER_MS,
            Some(_) | None => age_ms > UNKNOWN_RUNNER_STALE_AFTER_MS,
        };
    let usage = if supported.usage {
        parse_usage(object.get("totalTokens"), object.get("totalCost"))
    } else {
        FleetUsage::separate_run_aggregate()
    };
    let process_terminal = supported
        .process_terminal
        .then(|| parse_process_terminal(object.get("processTerminal"), &run_id))
        .flatten();
    let mut child_budget = MAX_CHILDREN_PER_RUN;
    let mut seen_child_ids = HashSet::new();
    let mut omitted_children = 0_u32;
    let children = if supported.children {
        parse_child_array(
            object.get("steps"),
            0,
            "step",
            FleetExecution::Unknown,
            &mut child_budget,
            &mut seen_child_ids,
            &mut omitted_children,
        )
    } else {
        Vec::new()
    };

    debug_assert!(supported.lifecycle);
    Ok(ParsedStatus {
        session_id,
        run: FleetRun {
            lifecycle_version,
            run_id,
            parent_run_id,
            nested,
            mode,
            state,
            execution: FleetExecution::Background,
            runner_pid,
            started_at_ms: Some(started_at_ms),
            updated_at_ms,
            ended_at_ms,
            source_updated_at_ms,
            stale,
            process_terminal,
            usage,
            children,
            omitted_children,
            reason: stale.then(|| "active status snapshot is stale".to_string()),
        },
        unsupported: false,
    })
}

fn parse_child_array(
    value: Option<&Value>,
    depth: usize,
    fallback_prefix: &str,
    default_execution: FleetExecution,
    budget: &mut usize,
    seen_ids: &mut HashSet<String>,
    omitted: &mut u32,
) -> Vec<FleetChild> {
    let Some(values) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    if depth >= MAX_CHILD_DEPTH {
        *omitted = omitted.saturating_add(values.len() as u32);
        return Vec::new();
    }
    let mut children = Vec::new();
    for (index, value) in values.iter().enumerate() {
        if *budget == 0 {
            *omitted = omitted.saturating_add(1);
            continue;
        }
        let Some(object) = value.as_object() else {
            *omitted = omitted.saturating_add(1);
            continue;
        };
        let explicit_run_id = identity_string(object.get("runId"), MAX_ID_BYTES);
        let (id, identity_source) = child_identity(object, index, fallback_prefix);
        if !seen_ids.insert(id.clone()) {
            *omitted = omitted.saturating_add(1);
            continue;
        }
        *budget -= 1;
        let state = parse_state(object.get("status").or_else(|| object.get("state")))
            .unwrap_or(FleetRunState::Unknown);
        let execution = match object.get("async").and_then(Value::as_bool) {
            Some(true) => FleetExecution::Background,
            Some(false) => FleetExecution::InProcess,
            None => default_execution,
        };
        // `label` and `description` are caller-controlled task text. Only the
        // agent identifier crosses the default monitor privacy boundary.
        let name = display_string(object.get("agent")).unwrap_or_else(|| id.clone());
        let model = display_string(object.get("model"));
        let current_tool = display_string(object.get("currentTool"));
        let activity = display_string(object.get("activityState"));
        let usage = parse_usage(
            object.get("tokens").or_else(|| object.get("totalTokens")),
            object.get("totalCost"),
        );
        let nested_prefix = format!("{id}/child");
        let mut nested = parse_child_array(
            object.get("children"),
            depth + 1,
            &nested_prefix,
            FleetExecution::Background,
            budget,
            seen_ids,
            omitted,
        );
        let nested_step_prefix = format!("{id}/step");
        let mut nested_steps = parse_child_array(
            object.get("steps"),
            depth + 1,
            &nested_step_prefix,
            FleetExecution::Unknown,
            budget,
            seen_ids,
            omitted,
        );
        nested.append(&mut nested_steps);
        children.push(FleetChild {
            id,
            run_id: explicit_run_id,
            identity_source,
            name,
            state,
            execution,
            model,
            current_tool,
            activity,
            started_at_ms: object.get("startedAt").and_then(json_u64),
            updated_at_ms: object
                .get("lastUpdate")
                .and_then(json_u64)
                .or_else(|| object.get("lastActivityAt").and_then(json_u64)),
            ended_at_ms: object.get("endedAt").and_then(json_u64),
            usage,
            children: nested,
        });
    }
    children
}

fn child_identity(
    object: &serde_json::Map<String, Value>,
    index: usize,
    fallback_prefix: &str,
) -> (String, FleetIdentitySource) {
    for (field, source) in [
        ("childId", FleetIdentitySource::ChildId),
        ("workflowKey", FleetIdentitySource::WorkflowKey),
        ("runId", FleetIdentitySource::RunId),
        ("id", FleetIdentitySource::RunId),
    ] {
        if let Some(id) = identity_string(object.get(field), MAX_ID_BYTES) {
            return (id, source);
        }
    }
    (
        format!("index:{fallback_prefix}:{index}"),
        FleetIdentitySource::Index,
    )
}

fn parse_usage(tokens: Option<&Value>, cost: Option<&Value>) -> FleetUsage {
    let token_object = tokens.and_then(Value::as_object);
    let cost_object = cost.and_then(Value::as_object);
    FleetUsage {
        input_tokens: token_object
            .and_then(|object| object.get("input"))
            .and_then(json_u64),
        output_tokens: token_object
            .and_then(|object| object.get("output"))
            .and_then(json_u64),
        total_tokens: token_object
            .and_then(|object| object.get("total"))
            .and_then(json_u64),
        reported_cost: cost_object
            .and_then(|object| object.get("costUsd"))
            .and_then(json_nonnegative_f64),
        accounting: FleetUsageAccounting::SeparateRunAggregate,
    }
}

fn parse_process_terminal(
    value: Option<&Value>,
    expected_run_id: &str,
) -> Option<FleetProcessTerminal> {
    let object = value?.as_object()?;
    if object.get("version").and_then(json_u64) != Some(1)
        || identity_string(object.get("runId"), MAX_ID_BYTES).as_deref() != Some(expected_run_id)
    {
        return None;
    }
    let runner_id = identity_string(object.get("runnerProcessInstanceId"), MAX_ID_BYTES)?;
    let state = match object.get("state")?.as_str()? {
        "pending" => FleetProcessTerminalState::Pending,
        "observed" => FleetProcessTerminalState::Observed,
        "unknown" => FleetProcessTerminalState::Unknown,
        "not-started" => FleetProcessTerminalState::NotStarted,
        _ => return None,
    };
    if object.get("resumeDisposition").is_some_and(|value| {
        !matches!(
            value.as_str(),
            Some("resumable" | "non-resumable" | "unavailable")
        )
    }) {
        return None;
    }
    let instances = match object.get("instances") {
        Some(Value::Array(instances)) if instances.iter().all(valid_process_instance) => {
            Some(instances)
        }
        Some(_) => return None,
        None => None,
    };
    let observed_at_ms = object.get("observedAt").and_then(json_u64);
    if state == FleetProcessTerminalState::Observed {
        let runner = instances?.iter().find(|instance| {
            instance
                .as_object()
                .and_then(|instance| instance.get("kind"))
                .and_then(Value::as_str)
                == Some("runner")
        })?;
        let runner_matches = runner.as_object().is_some_and(|runner| {
            identity_string(runner.get("processInstanceId"), MAX_ID_BYTES).as_deref()
                == Some(runner_id.as_str())
        });
        if observed_at_ms.is_none() || !runner_matches {
            return None;
        }
    }
    Some(FleetProcessTerminal {
        state,
        observed_at_ms,
    })
}

fn valid_process_instance(value: &Value) -> bool {
    let Some(instance) = value.as_object() else {
        return false;
    };
    if identity_string(instance.get("processInstanceId"), MAX_ID_BYTES).is_none()
        || instance
            .get("closeObservedAt")
            .and_then(Value::as_number)
            .is_none()
        || !matches!(
            instance.get("exitCode"),
            Some(Value::Null | Value::Number(_))
        )
        || !matches!(instance.get("signal"), Some(Value::Null | Value::String(_)))
    {
        return false;
    }
    match instance.get("kind").and_then(Value::as_str) {
        Some("runner") => !instance.contains_key("attempt"),
        Some("pi-writer") => {
            if instance.get("attempt").and_then(json_u64).is_none() {
                return false;
            }
            let Some(process_tree) = instance.get("processTree").and_then(Value::as_object) else {
                return false;
            };
            match process_tree.get("state").and_then(Value::as_str) {
                Some("observed") => {
                    process_tree.get("mechanism").and_then(Value::as_str)
                        == Some("posix-process-group")
                        && process_tree
                            .get("processGroupId")
                            .and_then(json_u64)
                            .is_some_and(|id| id > 0)
                        && process_tree
                            .get("verifiedAt")
                            .and_then(Value::as_number)
                            .is_some()
                }
                Some("unknown") => {
                    matches!(
                        process_tree.get("reason").and_then(Value::as_str),
                        Some("unsupported-platform" | "signal-failed" | "verification-failed")
                    ) && process_tree.get("diagnostic").is_none_or(Value::is_string)
                }
                _ => false,
            }
        }
        _ => false,
    }
}

fn parse_mode(value: Option<&Value>) -> Option<FleetRunMode> {
    match value?.as_str()? {
        "single" => Some(FleetRunMode::Single),
        "parallel" => Some(FleetRunMode::Parallel),
        "chain" => Some(FleetRunMode::Chain),
        "workflow" => Some(FleetRunMode::Workflow),
        _ => None,
    }
}

fn parse_state(value: Option<&Value>) -> Option<FleetRunState> {
    match value?.as_str()? {
        "queued" | "pending" => Some(FleetRunState::Queued),
        "running" => Some(FleetRunState::Running),
        "complete" | "completed" => Some(FleetRunState::Complete),
        "failed" => Some(FleetRunState::Failed),
        "partial" => Some(FleetRunState::Partial),
        "paused" => Some(FleetRunState::Paused),
        "stopped" => Some(FleetRunState::Stopped),
        "rejected" => Some(FleetRunState::Rejected),
        _ => None,
    }
}

fn identity_string(value: Option<&Value>, max_bytes: usize) -> Option<String> {
    let value = value?.as_str()?;
    (!value.is_empty()
        && value.len() <= max_bytes
        && !value.chars().any(|character| character.is_control()))
    .then(|| value.to_string())
}

fn display_string(value: Option<&Value>) -> Option<String> {
    let raw = value?.as_str()?;
    let sanitized = crate::collector::sanitize_terminal_text(raw);
    let bounded: String = sanitized.chars().take(MAX_LABEL_BYTES).collect();
    (!bounded.is_empty()).then_some(bounded)
}

fn json_u64(value: &Value) -> Option<u64> {
    value.as_u64()
}

fn json_nonnegative_f64(value: &Value) -> Option<f64> {
    let value = value.as_f64()?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn within_retention(run: &FleetRun, observed_at_ms: u64) -> bool {
    !run.state.is_terminal()
        || observed_at_ms.saturating_sub(run.source_updated_at_ms) <= COMPLETED_RUN_RETENTION_MS
}

fn max_option(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn mark_malformed(by_session: &mut HashMap<String, FleetTelemetry>) {
    for telemetry in by_session.values_mut() {
        telemetry.malformed_statuses = telemetry.malformed_statuses.saturating_add(1);
    }
}

fn mark_omitted(by_session: &mut HashMap<String, FleetTelemetry>, count: u32) {
    for telemetry in by_session.values_mut() {
        telemetry.omitted_statuses = telemetry.omitted_statuses.saturating_add(count);
    }
}

fn system_time_ms(value: Option<SystemTime>) -> Option<u64> {
    value?
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
}

pub(crate) fn pi_subagent_runner_run_id(command: &str) -> Option<String> {
    let components: Vec<&str> = command.split_whitespace().collect();
    let runner_index = components.iter().position(|component| {
        let normalized = component.trim_matches(['\'', '"']).replace('\\', "/");
        normalized.contains("pi-subagents")
            && normalized.ends_with("/runs/background/subagent-runner.ts")
    })?;
    let config = components.get(runner_index + 1)?;
    let normalized = config.trim_matches(['\'', '"']).replace('\\', "/");
    let file_name = normalized.rsplit('/').next()?;
    let run_id = file_name
        .strip_prefix("async-cfg-")?
        .strip_suffix(".json")?;
    (!run_id.is_empty() && run_id.len() <= MAX_ID_BYTES && !run_id.chars().any(char::is_control))
        .then(|| run_id.to_string())
}

fn default_async_root() -> PathBuf {
    if let Some(root) = std::env::var_os("PI_SUBAGENTS_TEMP_ROOT").filter(|root| !root.is_empty()) {
        return PathBuf::from(root).join("async-subagent-runs");
    }
    std::env::temp_dir()
        .join(format!("pi-subagents-{}", temp_scope_id()))
        .join("async-subagent-runs")
}

fn temp_scope_id() -> String {
    #[cfg(unix)]
    {
        if let Ok(output) = std::process::Command::new("id").arg("-u").output() {
            if output.status.success() {
                let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !uid.is_empty() && uid.bytes().all(|byte| byte.is_ascii_digit()) {
                    return format!("uid-{uid}");
                }
            }
        }
    }
    for key in ["USERNAME", "USER", "LOGNAME"] {
        if let Ok(value) = std::env::var(key) {
            let sanitized = sanitize_scope_segment(&value);
            if sanitized != "unknown" {
                return format!("user-{sanitized}");
            }
        }
    }
    "shared".to_string()
}

fn sanitize_scope_segment(value: &str) -> String {
    let mut output = String::new();
    let mut prior_dash = false;
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            output.push(character);
            prior_dash = false;
        } else if !prior_dash && !output.is_empty() {
            output.push('-');
            prior_dash = true;
        }
    }
    while output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() {
        "unknown".to_string()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_status(root: &Path, run_id: &str, value: Value) {
        let run_dir = root.join(run_id);
        fs::create_dir_all(&run_dir).unwrap();
        let mut file = File::create(run_dir.join("status.json")).unwrap();
        file.write_all(serde_json::to_string(&value).unwrap().as_bytes())
            .unwrap();
    }

    fn collect_one_with_processes(
        root: &Path,
        observed_at_ms: u64,
        live_pids: HashSet<u32>,
        verified_runner_runs: HashMap<u32, String>,
    ) -> FleetTelemetry {
        let collector = PiSubagentsCollector::with_async_root(root.to_path_buf());
        collector
            .collect(
                &[PiSubagentParent {
                    session_id: "parent-session".to_string(),
                    session_file: root.join("parent-session.jsonl"),
                }],
                &live_pids,
                &verified_runner_runs,
                observed_at_ms,
            )
            .remove("parent-session")
            .unwrap()
    }

    fn collect_one_with_runners(
        root: &Path,
        observed_at_ms: u64,
        verified_runner_runs: HashMap<u32, String>,
    ) -> FleetTelemetry {
        let live_pids = verified_runner_runs.keys().copied().collect();
        collect_one_with_processes(root, observed_at_ms, live_pids, verified_runner_runs)
    }

    fn collect_one(root: &Path, observed_at_ms: u64) -> FleetTelemetry {
        collect_one_with_runners(root, observed_at_ms, HashMap::new())
    }

    #[test]
    fn capability_matrix_supports_only_installed_v3_contract() {
        assert!(capabilities(None).is_none());
        assert!(capabilities(Some(2)).is_none());
        let version = capabilities(Some(3)).unwrap();
        assert!(
            version.identity
                && version.lifecycle
                && version.usage
                && version.children
                && version.process_terminal
        );
        assert!(capabilities(Some(4)).is_none());
    }

    #[test]
    fn reads_supported_status_without_exposing_private_fields_or_double_counting() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("async-subagent-runs");
        let parent_session_file = root.join("parent-session.jsonl");
        write_status(
            &root,
            "run-1",
            serde_json::json!({
                "lifecycleArtifactVersion": 3,
                "runId": "run-1",
                "sessionId": parent_session_file,
                "mode": "workflow",
                "state": "running",
                "startedAt": 1_000,
                "lastUpdate": 2_000,
                "cwd": "/private/project",
                "outputFile": "/private/output.log",
                "totalTokens": {"input": 10, "output": 5, "total": 15},
                "totalCost": {"inputTokens": 10, "outputTokens": 5, "costUsd": 0.25},
                "processTerminal": {
                    "version": 1,
                    "runId": "run-1",
                    "runnerProcessInstanceId": "runner-instance-1",
                    "state": "observed",
                    "observedAt": 2_050,
                    "instances": [{
                        "kind": "runner",
                        "processInstanceId": "runner-instance-1",
                        "closeObservedAt": 2_050,
                        "exitCode": 0,
                        "signal": null
                    }],
                    "diagnostic": "private process detail"
                },
                "steps": [{
                    "childId": "child-1",
                    "agent": "reviewer",
                    "label": "Review auth\u{202e}test",
                    "description": "private delegated task",
                    "status": "running",
                    "async": true,
                    "model": "provider/model",
                    "currentTool": "read",
                    "currentToolArgs": "secret.txt",
                    "tokens": {"input": 7, "output": 3, "total": 10},
                    "totalCost": {"costUsd": 0.1},
                    "children": [{
                        "id": "nested-1",
                        "agent": "delegate",
                        "state": "running",
                        "totalTokens": {"input": 2, "output": 1, "total": 3},
                        "steps": [{"agent": "inner", "status": "pending"}]
                    }]
                }]
            }),
        );

        let fleet = collect_one(&root, 2_100);
        assert_eq!(fleet.source_health, SourceHealth::Healthy);
        assert_eq!(fleet.runs.len(), 1);
        let run = &fleet.runs[0];
        assert_eq!(run.mode, FleetRunMode::Workflow);
        assert_eq!(run.execution, FleetExecution::Background);
        assert_eq!(run.usage.total_tokens, Some(15));
        assert_eq!(
            run.usage.accounting,
            FleetUsageAccounting::SeparateRunAggregate
        );
        assert_eq!(
            run.process_terminal.as_ref().map(|proof| proof.state),
            Some(FleetProcessTerminalState::Observed)
        );
        assert_eq!(
            run.children[0].identity_source,
            FleetIdentitySource::ChildId
        );
        assert_eq!(run.children[0].name, "reviewer");
        assert_eq!(run.children[0].execution, FleetExecution::Background);
        let nested = &run.children[0].children[0];
        assert_eq!(nested.id, "nested-1");
        assert_eq!(nested.identity_source, FleetIdentitySource::RunId);
        assert_eq!(nested.execution, FleetExecution::Background);
        assert_eq!(nested.usage.total_tokens, Some(3));
        assert_eq!(
            nested.children[0].identity_source,
            FleetIdentitySource::Index
        );
        assert!(nested.children[0].id.starts_with("index:nested-1/step:"));
        let json = serde_json::to_string(&fleet).unwrap();
        for private in [
            "private delegated task",
            "/private/project",
            "secret.txt",
            "output.log",
            "private process detail",
            "Review auth",
        ] {
            assert!(!json.contains(private), "private field leaked: {private}");
        }
    }

    #[test]
    fn accepts_a_bounded_session_path_longer_than_an_identifier() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("async-subagent-runs");
        let long_session_path = format!("/tmp/{}/parent.jsonl", "deep/".repeat(80));
        assert!(long_session_path.len() > MAX_ID_BYTES);
        write_status(
            &root,
            "run-long-path",
            serde_json::json!({
                "lifecycleArtifactVersion": 3,
                "runId": "run-long-path",
                "sessionId": long_session_path.clone(),
                "mode": "single",
                "state": "running",
                "startedAt": 1
            }),
        );
        let collector = PiSubagentsCollector::with_async_root(root);
        let fleet = collector.collect(
            &[PiSubagentParent {
                session_id: "parent-session".to_string(),
                session_file: PathBuf::from(&long_session_path),
            }],
            &HashSet::new(),
            &HashMap::new(),
            2,
        );

        assert_eq!(fleet["parent-session"].runs.len(), 1);
    }

    #[test]
    fn conflicting_parent_aliases_are_rejected() {
        let aliases = parent_aliases(&[
            PiSubagentParent {
                session_id: "session-a".to_string(),
                session_file: PathBuf::from("/tmp/shared-session.jsonl"),
            },
            PiSubagentParent {
                session_id: "/tmp/shared-session.jsonl".to_string(),
                session_file: PathBuf::from("/tmp/other-session.jsonl"),
            },
        ]);

        assert!(!aliases.contains_key("/tmp/shared-session.jsonl"));
        assert_eq!(
            aliases.get("session-a").map(String::as_str),
            Some("session-a")
        );
    }

    #[test]
    fn terminal_runs_follow_the_extension_retention_window() {
        let retained = FleetRun {
            lifecycle_version: Some(3),
            run_id: "run-old".to_string(),
            parent_run_id: None,
            nested: false,
            mode: FleetRunMode::Single,
            state: FleetRunState::Complete,
            execution: FleetExecution::Background,
            runner_pid: None,
            started_at_ms: Some(1),
            updated_at_ms: Some(1),
            ended_at_ms: Some(1),
            source_updated_at_ms: 1,
            stale: false,
            process_terminal: None,
            usage: FleetUsage::separate_run_aggregate(),
            children: Vec::new(),
            omitted_children: 0,
            reason: None,
        };
        assert!(within_retention(&retained, COMPLETED_RUN_RETENTION_MS + 1));
        assert!(!within_retention(&retained, COMPLETED_RUN_RETENTION_MS + 2));

        let mut active = retained;
        active.state = FleetRunState::Running;
        assert!(within_retention(&active, u64::MAX));
    }

    #[test]
    fn unknown_and_missing_versions_keep_identity_but_not_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("async-subagent-runs");
        for (run_id, version) in [("run-old", Some(2_u64)), ("run-missing", None)] {
            let mut value = serde_json::json!({
                "runId": run_id,
                "sessionId": "parent-session",
                "mode": "single",
                "state": "running",
                "startedAt": 1
            });
            if let Some(version) = version {
                value["lifecycleArtifactVersion"] = Value::from(version);
            }
            write_status(&root, run_id, value);
        }

        let fleet = collect_one(&root, 2_000);
        assert_eq!(fleet.unsupported_statuses, 2);
        assert_eq!(fleet.runs.len(), 2);
        assert!(fleet
            .runs
            .iter()
            .all(|run| run.state == FleetRunState::Unknown && run.children.is_empty()));
    }

    #[test]
    fn malformed_status_is_ignored_and_marks_source_error() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("async-subagent-runs");
        let run_dir = root.join("broken");
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(run_dir.join("status.json"), b"{not-json").unwrap();

        let fleet = collect_one(&root, 2_000);
        assert_eq!(fleet.source_health, SourceHealth::Error);
        assert_eq!(fleet.malformed_statuses, 1);
        assert!(fleet.runs.is_empty());
    }

    #[test]
    fn workflow_child_status_is_not_duplicated_as_a_top_level_run() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("async-subagent-runs");
        write_status(
            &root,
            "workflow-run",
            serde_json::json!({
                "lifecycleArtifactVersion": 3,
                "runId": "workflow-run",
                "sessionId": "parent-session",
                "mode": "workflow",
                "state": "running",
                "startedAt": 1,
                "steps": [{
                    "workflowKey": "lane-one",
                    "runId": "child-run",
                    "agent": "worker",
                    "status": "running",
                    "async": true,
                    "tokens": {"input": 2, "output": 1, "total": 3}
                }]
            }),
        );
        write_status(
            &root,
            "child-run",
            serde_json::json!({
                "lifecycleArtifactVersion": 3,
                "runId": "child-run",
                "sessionId": "parent-session",
                "parentWorkflowRunId": "workflow-run",
                "mode": "single",
                "state": "running",
                "startedAt": 2,
                "totalTokens": {"input": 2, "output": 1, "total": 3}
            }),
        );

        let fleet = collect_one(&root, 3);
        assert_eq!(fleet.runs.len(), 1);
        assert_eq!(fleet.runs[0].run_id, "workflow-run");
        assert_eq!(fleet.runs[0].children[0].id, "lane-one");
        assert_eq!(
            fleet.runs[0].children[0].run_id.as_deref(),
            Some("child-run")
        );
        assert_eq!(fleet.omitted_statuses, 1);
    }

    #[test]
    fn child_status_remains_visible_when_parent_does_not_embed_it() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("async-subagent-runs");
        write_status(
            &root,
            "workflow-partial",
            serde_json::json!({
                "lifecycleArtifactVersion": 3,
                "runId": "workflow-partial",
                "sessionId": "parent-session",
                "mode": "workflow",
                "state": "running",
                "startedAt": 1,
                "steps": [{
                    "workflowKey": "child-visible",
                    "agent": "worker",
                    "status": "running"
                }]
            }),
        );
        write_status(
            &root,
            "child-visible",
            serde_json::json!({
                "lifecycleArtifactVersion": 3,
                "runId": "child-visible",
                "sessionId": "parent-session",
                "parentWorkflowRunId": "workflow-partial",
                "isNested": true,
                "mode": "single",
                "state": "running",
                "startedAt": 2
            }),
        );

        let fleet = collect_one(&root, 3);
        let run_ids: HashSet<&str> = fleet.runs.iter().map(|run| run.run_id.as_str()).collect();
        assert_eq!(
            run_ids,
            HashSet::from(["workflow-partial", "child-visible"])
        );
    }

    #[test]
    fn live_verified_runner_prevents_quiet_status_from_being_marked_stale() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("async-subagent-runs");
        write_status(
            &root,
            "run-live",
            serde_json::json!({
                "lifecycleArtifactVersion": 3,
                "runId": "run-live",
                "sessionId": "parent-session",
                "mode": "single",
                "state": "running",
                "pid": 42,
                "startedAt": 1,
                "lastUpdate": 1
            }),
        );
        let observed_at_ms =
            system_time_ms(Some(SystemTime::now())).unwrap() + UNKNOWN_RUNNER_STALE_AFTER_MS + 2;

        let fleet = collect_one_with_runners(
            &root,
            observed_at_ms,
            HashMap::from([(42, "run-live".to_string())]),
        );
        assert_eq!(fleet.source_health, SourceHealth::Healthy);
        assert!(!fleet.runs[0].stale);
        assert_eq!(fleet.runs[0].runner_pid, Some(42));
    }

    #[test]
    fn reused_runner_pid_does_not_keep_an_old_run_live_forever() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("async-subagent-runs");
        write_status(
            &root,
            "run-old",
            serde_json::json!({
                "lifecycleArtifactVersion": 3,
                "runId": "run-old",
                "sessionId": "parent-session",
                "mode": "single",
                "state": "running",
                "pid": 42,
                "startedAt": 1,
                "lastUpdate": 1
            }),
        );
        let observed_at_ms =
            system_time_ms(Some(SystemTime::now())).unwrap() + UNKNOWN_RUNNER_STALE_AFTER_MS + 2;

        let fleet = collect_one_with_processes(
            &root,
            observed_at_ms,
            HashSet::from([42]),
            HashMap::from([(42, "run-new".to_string())]),
        );
        assert!(fleet.runs[0].stale);
    }

    #[test]
    fn mismatched_process_terminal_proof_is_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("async-subagent-runs");
        write_status(
            &root,
            "run-proof",
            serde_json::json!({
                "lifecycleArtifactVersion": 3,
                "runId": "run-proof",
                "sessionId": "parent-session",
                "mode": "single",
                "state": "complete",
                "startedAt": 1,
                "processTerminal": {
                    "version": 1,
                    "runId": "another-run",
                    "runnerProcessInstanceId": "runner-1",
                    "state": "observed",
                    "observedAt": 2,
                    "instances": [{
                        "kind": "runner",
                        "processInstanceId": "runner-1",
                        "closeObservedAt": 2
                    }]
                }
            }),
        );

        let fleet = collect_one(&root, 2);
        assert_eq!(fleet.runs.len(), 1);
        assert!(fleet.runs[0].process_terminal.is_none());

        let incomplete_runner = serde_json::json!({
            "version": 1,
            "runId": "run-proof",
            "runnerProcessInstanceId": "runner-1",
            "state": "observed",
            "observedAt": 2,
            "instances": [{
                "kind": "runner",
                "processInstanceId": "runner-1",
                "closeObservedAt": 2
            }]
        });
        assert!(parse_process_terminal(Some(&incomplete_runner), "run-proof").is_none());

        let malformed_extra_instance = serde_json::json!({
            "version": 1,
            "runId": "run-proof",
            "runnerProcessInstanceId": "runner-1",
            "state": "observed",
            "observedAt": 2,
            "instances": [
                {
                    "kind": "runner",
                    "processInstanceId": "runner-1",
                    "closeObservedAt": 2,
                    "exitCode": 0,
                    "signal": null
                },
                {
                    "kind": "pi-writer",
                    "processInstanceId": "writer-1",
                    "closeObservedAt": 2,
                    "exitCode": 0,
                    "signal": null,
                    "attempt": 0
                }
            ]
        });
        assert!(parse_process_terminal(Some(&malformed_extra_instance), "run-proof").is_none());

        let wrong_first_runner = serde_json::json!({
            "version": 1,
            "runId": "run-proof",
            "runnerProcessInstanceId": "runner-expected",
            "state": "observed",
            "observedAt": 2,
            "instances": [
                {
                    "kind": "runner",
                    "processInstanceId": "runner-other",
                    "closeObservedAt": 2,
                    "exitCode": 0,
                    "signal": null
                },
                {
                    "kind": "runner",
                    "processInstanceId": "runner-expected",
                    "closeObservedAt": 2,
                    "exitCode": 0,
                    "signal": null
                }
            ]
        });
        assert!(parse_process_terminal(Some(&wrong_first_runner), "run-proof").is_none());

        let invalid_disposition = serde_json::json!({
            "version": 1,
            "runId": "run-proof",
            "runnerProcessInstanceId": "runner-1",
            "state": "pending",
            "resumeDisposition": "maybe"
        });
        assert!(parse_process_terminal(Some(&invalid_disposition), "run-proof").is_none());
    }

    #[test]
    fn runner_command_matching_requires_the_run_specific_config() {
        assert_eq!(
            pi_subagent_runner_run_id(
                "node /pkg/pi-subagents/src/runs/background/subagent-runner.ts /tmp/async-cfg-run-live.json"
            ),
            Some("run-live".to_string())
        );
        assert!(pi_subagent_runner_run_id("node server.js").is_none());
        assert!(pi_subagent_runner_run_id(
            "node /pkg/pi-subagents/src/runs/background/subagent-runner.ts /tmp/config"
        )
        .is_none());
    }

    #[test]
    fn stale_active_runs_are_flagged_without_inventing_a_terminal_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("async-subagent-runs");
        write_status(
            &root,
            "run-stale",
            serde_json::json!({
                "lifecycleArtifactVersion": 3,
                "runId": "run-stale",
                "sessionId": "parent-session",
                "mode": "single",
                "state": "running",
                "startedAt": 1,
                "lastUpdate": 1
            }),
        );

        let observed_at_ms =
            system_time_ms(Some(SystemTime::now())).unwrap() + UNKNOWN_RUNNER_STALE_AFTER_MS + 2;
        let fleet = collect_one(&root, observed_at_ms);
        assert_eq!(fleet.source_health, SourceHealth::Stale);
        assert!(fleet.runs[0].stale);
        assert_eq!(fleet.runs[0].state, FleetRunState::Running);
        assert!(fleet.runs[0].process_terminal.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_status_is_never_read() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("async-subagent-runs");
        let run_dir = root.join("run-link");
        fs::create_dir_all(&run_dir).unwrap();
        let target = temp.path().join("private.json");
        fs::write(&target, b"{\"secret\":\"must-not-read\"}").unwrap();
        symlink(&target, run_dir.join("status.json")).unwrap();

        let fleet = collect_one(&root, 2_000);
        assert!(fleet.runs.is_empty());
        assert_eq!(fleet.omitted_statuses, 1);
    }

    #[test]
    fn status_scan_ignores_child_transcripts_prompts_logs_and_tool_arguments() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("async-subagent-runs");
        write_status(
            &root,
            "run-private",
            serde_json::json!({
                "lifecycleArtifactVersion": 3,
                "runId": "run-private",
                "sessionId": "parent-session",
                "mode": "single",
                "state": "running",
                "startedAt": 1
            }),
        );
        let run_dir = root.join("run-private");
        for name in [
            "events.jsonl",
            "session.jsonl",
            "prompt.txt",
            "output-0.log",
            "tool-arguments.json",
        ] {
            fs::write(run_dir.join(name), "private sentinel").unwrap();
        }

        let scan = status_candidates(&root).unwrap();
        assert_eq!(scan.candidates.len(), 1);
        assert_eq!(
            scan.candidates[0].path,
            fs::canonicalize(run_dir.join("status.json")).unwrap()
        );
        let fleet = collect_one(&root, 2_000);
        assert_eq!(fleet.runs.len(), 1);
        assert!(!serde_json::to_string(&fleet)
            .unwrap()
            .contains("private sentinel"));
    }

    #[test]
    fn status_scan_caps_files_per_collection() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("async-subagent-runs");
        for index in 0..MAX_STATUS_FILES + 5 {
            write_status(
                &root,
                &format!("run-{index:03}"),
                serde_json::json!({
                    "lifecycleArtifactVersion": 3,
                    "runId": format!("run-{index:03}"),
                    "sessionId": "parent-session",
                    "state": "complete"
                }),
            );
        }

        let scan = status_candidates(&root).unwrap();
        assert_eq!(scan.candidates.len(), MAX_STATUS_FILES);
        assert!(scan.omitted >= 5);
    }

    #[test]
    fn documented_fleet_limits_match_release_constants() {
        let docs = include_str!("../../docs/pi-support.md");
        let expected = [
            format!("{} KiB per status.json file", MAX_STATUS_BYTES / 1024),
            format!(
                "{} MiB of status.json data per collection tick",
                MAX_STATUS_READ_BYTES / (1024 * 1024)
            ),
            format!(
                "{} run-directory entries examined per collection tick",
                MAX_DIRECTORY_ENTRIES
            ),
            format!("{} status files per collection tick", MAX_STATUS_FILES),
            format!("{} runs per parent session", MAX_RUNS_PER_SESSION),
            format!("{} children per run", MAX_CHILDREN_PER_RUN),
            format!("{} nested child levels", MAX_CHILD_DEPTH),
            format!(
                "Completed runs remain eligible for {} days",
                COMPLETED_RUN_RETENTION_DAYS
            ),
            format!(
                "no verifiable runner become stale after {} hours",
                UNKNOWN_RUNNER_STALE_AFTER_MS / (60 * 60 * 1_000)
            ),
            format!(
                "verified runner has disappeared becomes stale after {} seconds",
                MISSING_RUNNER_STALE_AFTER_MS / 1_000
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
    fn missing_root_reports_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let fleet = collect_one(&temp.path().join("missing"), 2_000);
        assert_eq!(fleet.source_health, SourceHealth::Unavailable);
        assert!(fleet.runs.is_empty());
    }
}
