//! Private Protocol v1, AF_UNIX transport, and registry for the optional live harness.
//!
//! It retains and publishes only safe protocol identity and reduced state; event payloads and
//! tool identifiers are discarded.
#![allow(dead_code)]

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use crate::model::{PiLiveHarnessProvenance, PiLiveHarnessTelemetry, PiLivePhase, SourceHealth};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::HashSet;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::collections::{BTreeMap, HashMap as IdentityMap, HashSet as IdentitySet, VecDeque};
use std::fmt;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::fs;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::io::{self, Read};
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::mem::{self, MaybeUninit};
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::os::unix::ffi::OsStrExt;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::os::unix::fs::{FileTypeExt, MetadataExt};
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::os::unix::net::UnixStream;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::time::{Duration, Instant};

const PROTOCOL_MAGIC: &str = "ptop-live";
const PROTOCOL_VERSION: u64 = 1;
const MAX_FRAME_BYTES: usize = 512;
const MAX_BUFFER_BYTES: usize = 1024;
const MAX_SESSION_ID_BYTES: usize = 256;
const EPOCH_HEX_BYTES: usize = 32;
const MAX_TOOL_IDS: usize = 32;
/// Matches the protocol identifier bound; Pi documents toolCallId as a string only.
const MAX_TOOL_CALL_ID_BYTES: usize = 256;
const MAX_NESTING: u8 = 4;
const HEARTBEAT_MS: u64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Idle,
    Generating,
    ToolRunning,
    Compacting,
    WaitingForUser,
}

impl Phase {
    const ALL: [Self; 5] = [
        Self::Idle,
        Self::Generating,
        Self::ToolRunning,
        Self::Compacting,
        Self::WaitingForUser,
    ];

    fn wire_name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Generating => "generating",
            Self::ToolRunning => "tool_running",
            Self::Compacting => "compacting",
            Self::WaitingForUser => "waiting_for_user",
        }
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl From<Phase> for PiLivePhase {
    fn from(value: Phase) -> Self {
        match value {
            Phase::Idle => Self::Idle,
            Phase::Generating => Self::Generating,
            Phase::ToolRunning => Self::ToolRunning,
            Phase::Compacting => Self::Compacting,
            Phase::WaitingForUser => Self::WaitingForUser,
        }
    }
}

#[derive(Deserialize)]
#[serde(field_identifier, rename_all = "snake_case")]
enum WireKey {
    Magic,
    Version,
    Epoch,
    Sequence,
    SessionId,
    Phase,
    PendingMessages,
}

/// The single authoritative Protocol v1 schema. Required nullable fields are represented
/// as `Option<Option<T>>`: absent is outer `None`; explicit JSON null is `Some(None)`.
struct WireFrame {
    magic: String,
    version: u64,
    epoch: String,
    sequence: u64,
    session_id: String,
    phase: Option<Phase>,
    pending_messages: Option<bool>,
}

impl<'de> Deserialize<'de> for WireFrame {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct WireFrameVisitor;

        impl<'de> Visitor<'de> for WireFrameVisitor {
            type Value = WireFrame;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a complete Protocol v1 frame object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut magic = None;
                let mut version = None;
                let mut epoch = None;
                let mut sequence = None;
                let mut session_id = None;
                let mut phase = None;
                let mut pending_messages = None;
                while let Some(key) = map.next_key::<WireKey>()? {
                    match key {
                        WireKey::Magic => set_once(&mut magic, map.next_value()?, "magic")?,
                        WireKey::Version => set_once(&mut version, map.next_value()?, "version")?,
                        WireKey::Epoch => set_once(&mut epoch, map.next_value()?, "epoch")?,
                        WireKey::Sequence => {
                            set_once(&mut sequence, map.next_value()?, "sequence")?
                        }
                        WireKey::SessionId => {
                            set_once(&mut session_id, map.next_value()?, "session_id")?
                        }
                        WireKey::Phase => set_once(&mut phase, map.next_value()?, "phase")?,
                        WireKey::PendingMessages => {
                            set_once(&mut pending_messages, map.next_value()?, "pending_messages")?
                        }
                    }
                }
                Ok(WireFrame {
                    magic: magic.ok_or_else(|| de::Error::missing_field("magic"))?,
                    version: version.ok_or_else(|| de::Error::missing_field("version"))?,
                    epoch: epoch.ok_or_else(|| de::Error::missing_field("epoch"))?,
                    sequence: sequence.ok_or_else(|| de::Error::missing_field("sequence"))?,
                    session_id: session_id.ok_or_else(|| de::Error::missing_field("session_id"))?,
                    phase: phase.ok_or_else(|| de::Error::missing_field("phase"))?,
                    pending_messages: pending_messages
                        .ok_or_else(|| de::Error::missing_field("pending_messages"))?,
                })
            }
        }

        deserializer.deserialize_map(WireFrameVisitor)
    }
}

fn set_once<E: de::Error, T>(slot: &mut Option<T>, value: T, name: &'static str) -> Result<(), E> {
    if slot.replace(value).is_some() {
        return Err(E::duplicate_field(name));
    }
    Ok(())
}

#[derive(Clone, PartialEq, Eq)]
struct LiveFrame {
    epoch: String,
    sequence: u64,
    session_id: String,
    phase: Option<Phase>,
    pending_messages: Option<bool>,
}

impl fmt::Debug for LiveFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LiveFrame")
            .field("sequence", &self.sequence)
            .field("phase", &self.phase)
            .field("pending_messages", &self.pending_messages)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtocolError {
    InvalidLength,
    BufferLimit,
    InvalidUtf8,
    InvalidSchema,
    InvalidIdentity,
    InvalidInitialFrame,
    InvalidSequence,
    ReusedEpoch,
    Closed,
}

impl TryFrom<WireFrame> for LiveFrame {
    type Error = ProtocolError;

    fn try_from(frame: WireFrame) -> Result<Self, Self::Error> {
        if frame.magic != PROTOCOL_MAGIC || frame.version != PROTOCOL_VERSION {
            return Err(ProtocolError::InvalidSchema);
        }
        if !valid_epoch(&frame.epoch)
            || !valid_safe_identifier(&frame.session_id, MAX_SESSION_ID_BYTES)
        {
            return Err(ProtocolError::InvalidIdentity);
        }
        Ok(Self {
            epoch: frame.epoch,
            sequence: frame.sequence,
            session_id: frame.session_id,
            phase: frame.phase,
            pending_messages: frame.pending_messages,
        })
    }
}

fn valid_epoch(value: &str) -> bool {
    value.len() == EPOCH_HEX_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_safe_identifier(value: &str, maximum_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum_bytes
        && value.chars().all(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}'
                )
        })
}

#[derive(Default)]
struct FrameDecoder {
    buffer: Vec<u8>,
    closed: bool,
}

impl FrameDecoder {
    fn push(&mut self, bytes: &[u8]) -> Result<(), ProtocolError> {
        if self.closed {
            return Err(ProtocolError::Closed);
        }
        if self.buffer.len().saturating_add(bytes.len()) > MAX_BUFFER_BYTES {
            self.buffer.clear();
            self.closed = true;
            return Err(ProtocolError::BufferLimit);
        }
        self.buffer.extend_from_slice(bytes);
        Ok(())
    }

    fn has_complete_frame(&self) -> bool {
        if self.buffer.len() < 4 {
            return false;
        }
        let length =
            u32::from_be_bytes(self.buffer[..4].try_into().expect("prefix length")) as usize;
        (1..=MAX_FRAME_BYTES).contains(&length) && self.buffer.len() >= 4 + length
    }

    /// Returns one complete frame only; extra bytes remain for the next call.
    fn next(&mut self) -> Result<Option<LiveFrame>, ProtocolError> {
        if self.closed {
            return Err(ProtocolError::Closed);
        }
        if self.buffer.len() < 4 {
            return Ok(None);
        }
        let length =
            u32::from_be_bytes(self.buffer[..4].try_into().expect("prefix length")) as usize;
        if !(1..=MAX_FRAME_BYTES).contains(&length) {
            return self.reject(ProtocolError::InvalidLength);
        }
        let total = 4 + length;
        if self.buffer.len() < total {
            return Ok(None);
        }
        let payload: Vec<u8> = self.buffer.drain(..total).skip(4).collect();
        let payload = std::str::from_utf8(&payload).map_err(|_| ProtocolError::InvalidUtf8);
        let result = payload
            .and_then(|json| {
                serde_json::from_str::<WireFrame>(json).map_err(|_| ProtocolError::InvalidSchema)
            })
            .and_then(LiveFrame::try_from);
        match result {
            Ok(frame) => Ok(Some(frame)),
            Err(error) => self.reject(error),
        }
    }

    fn reject<T>(&mut self, error: ProtocolError) -> Result<T, ProtocolError> {
        self.buffer.clear();
        self.closed = true;
        Err(error)
    }
}

/// Connection-scoped sequence validation. The registry supplies `previous_epoch` after a closed
/// stream so an immediate reconnect cannot reuse its old epoch.
struct ProtocolConnection {
    attached_session_id: String,
    previous_epoch: Option<String>,
    epoch: Option<String>,
    next_sequence: u64,
    closed: bool,
}

impl fmt::Debug for ProtocolConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProtocolConnection")
            .field("has_previous_epoch", &self.previous_epoch.is_some())
            .field("has_epoch", &self.epoch.is_some())
            .field("next_sequence", &self.next_sequence)
            .field("closed", &self.closed)
            .finish()
    }
}

impl ProtocolConnection {
    fn new(attached_session_id: &str, previous_epoch: Option<&str>) -> Self {
        Self {
            attached_session_id: attached_session_id.to_owned(),
            previous_epoch: previous_epoch.map(str::to_owned),
            epoch: None,
            next_sequence: 0,
            closed: false,
        }
    }

    fn epoch(&self) -> Option<&str> {
        self.epoch.as_deref()
    }

    fn accept(&mut self, frame: LiveFrame) -> Result<(), ProtocolError> {
        if self.closed {
            return Err(ProtocolError::Closed);
        }
        if frame.session_id != self.attached_session_id {
            return self.reject(ProtocolError::InvalidIdentity);
        }
        match &self.epoch {
            None => {
                if frame.sequence != 0 || frame.phase.is_some() || frame.pending_messages.is_some()
                {
                    return self.reject(ProtocolError::InvalidInitialFrame);
                }
                if self.previous_epoch.as_deref() == Some(frame.epoch.as_str()) {
                    return self.reject(ProtocolError::ReusedEpoch);
                }
                self.epoch = Some(frame.epoch);
                self.next_sequence = 1;
            }
            Some(epoch) => {
                if frame.epoch != *epoch || frame.sequence != self.next_sequence {
                    return self.reject(ProtocolError::InvalidSequence);
                }
                let Some(next_sequence) = self.next_sequence.checked_add(1) else {
                    return self.reject(ProtocolError::InvalidSequence);
                };
                self.next_sequence = next_sequence;
            }
        }
        Ok(())
    }

    fn reject(&mut self, error: ProtocolError) -> Result<(), ProtocolError> {
        self.closed = true;
        Err(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HeartbeatAction {
    Start(ReducedState),
    Wait,
    Close,
}

#[derive(Clone, Copy)]
struct InProgressFrame {
    deadline_ms: u64,
}

/// A pure state machine for extension emission. It stores only the latest safe reduced state.
struct HeartbeatSchedule {
    next_due_ms: u64,
    in_progress: Option<InProgressFrame>,
    latest: ReducedState,
    initial_emitted: bool,
    closed: bool,
}

impl HeartbeatSchedule {
    fn new() -> Self {
        Self {
            next_due_ms: 0,
            in_progress: None,
            latest: ReducedState::unknown(),
            initial_emitted: false,
            closed: false,
        }
    }

    fn update(&mut self, state: ReducedState) {
        self.latest = state;
    }

    fn poll(&mut self, now_ms: u64) -> HeartbeatAction {
        if self.closed {
            return HeartbeatAction::Close;
        }
        if let Some(frame) = self.in_progress {
            if now_ms >= frame.deadline_ms {
                self.closed = true;
                return HeartbeatAction::Close;
            }
            return HeartbeatAction::Wait;
        }
        if now_ms < self.next_due_ms {
            return HeartbeatAction::Wait;
        }
        let deadline_ms = self.next_boundary_after(now_ms);
        self.in_progress = Some(InProgressFrame { deadline_ms });
        self.next_due_ms = deadline_ms;
        let state = if self.initial_emitted {
            self.latest
        } else {
            self.initial_emitted = true;
            ReducedState::unknown()
        };
        HeartbeatAction::Start(state)
    }

    fn complete(&mut self, now_ms: u64) -> HeartbeatAction {
        let Some(frame) = self.in_progress else {
            return self.poll(now_ms);
        };
        if now_ms >= frame.deadline_ms {
            self.closed = true;
            return HeartbeatAction::Close;
        }
        self.in_progress = None;
        HeartbeatAction::Wait
    }

    fn next_boundary_after(&self, now_ms: u64) -> u64 {
        if now_ms == self.next_due_ms {
            self.next_due_ms.saturating_add(HEARTBEAT_MS)
        } else {
            // A late start abandons the old grid. This prevents a completed late
            // frame from being followed by a catch-up frame less than one interval later.
            now_ms.saturating_add(HEARTBEAT_MS)
        }
    }
}

/// Privacy-safe Pi 0.85.1 hook reductions. IDs are used only to match completion and are
/// bounded before cloning. Debug output never includes the ID.
enum ReducerEvent<'a> {
    SessionStart,
    SessionShutdown,
    AgentStart,
    AgentEnd,
    AgentSettled { is_idle: bool },
    ToolStart { tool_call_id: &'a str },
    ToolEnd { tool_call_id: &'a str },
    CompactStart,
    CompactEnd,
    PromptStart,
    PromptEnd,
}

impl fmt::Debug for ReducerEvent<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::SessionStart => "SessionStart",
            Self::SessionShutdown => "SessionShutdown",
            Self::AgentStart => "AgentStart",
            Self::AgentEnd => "AgentEnd",
            Self::AgentSettled { .. } => "AgentSettled",
            Self::ToolStart { .. } => "ToolStart",
            Self::ToolEnd { .. } => "ToolEnd",
            Self::CompactStart => "CompactStart",
            Self::CompactEnd => "CompactEnd",
            Self::PromptStart => "PromptStart",
            Self::PromptEnd => "PromptEnd",
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ReducedState {
    phase: Option<Phase>,
    pending_messages: Option<bool>,
}

impl ReducedState {
    fn unknown() -> Self {
        Self {
            phase: None,
            pending_messages: None,
        }
    }
}

impl fmt::Debug for ReducedState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReducedState")
            .field("phase", &self.phase)
            .field("pending_messages", &self.pending_messages)
            .finish()
    }
}

#[derive(Default)]
struct Reducer {
    known: bool,
    idle: bool,
    agent_spans: u8,
    compaction_spans: u8,
    prompt_spans: u8,
    tool_ids: HashSet<String>,
    shutdown: bool,
}

impl fmt::Debug for Reducer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Reducer")
            .field("known", &self.known)
            .field("idle", &self.idle)
            .field("agent_spans", &self.agent_spans)
            .field("compaction_spans", &self.compaction_spans)
            .field("prompt_spans", &self.prompt_spans)
            .field("tool_count", &self.tool_ids.len())
            .field("shutdown", &self.shutdown)
            .finish()
    }
}

impl Reducer {
    fn reduce(&mut self, event: ReducerEvent<'_>) -> Option<ReducedState> {
        match event {
            ReducerEvent::SessionStart => *self = Self::default(),
            ReducerEvent::SessionShutdown => {
                self.shutdown = true;
                self.clear();
                return None;
            }
            ReducerEvent::AgentStart => {
                self.recover_for_activity();
                if self.agent_spans != 0 {
                    self.invalidate();
                } else {
                    self.agent_spans = 1;
                    self.idle = false;
                }
            }
            ReducerEvent::AgentEnd => {
                if !self.known || self.agent_spans == 0 {
                    self.invalidate();
                } else {
                    self.agent_spans -= 1;
                }
            }
            ReducerEvent::AgentSettled { is_idle } => {
                if is_idle {
                    self.clear();
                    self.known = true;
                    self.idle = true;
                } else {
                    self.invalidate();
                }
            }
            ReducerEvent::ToolStart { tool_call_id } => {
                self.recover_for_activity();
                if tool_call_id.len() > MAX_TOOL_CALL_ID_BYTES
                    || self.tool_ids.len() == MAX_TOOL_IDS
                    || !self.tool_ids.insert(tool_call_id.to_owned())
                {
                    self.invalidate();
                } else {
                    self.idle = false;
                }
            }
            ReducerEvent::ToolEnd { tool_call_id } => {
                if !self.known || !self.tool_ids.remove(tool_call_id) {
                    self.invalidate();
                }
            }
            ReducerEvent::CompactStart => {
                self.recover_for_activity();
                if self.compaction_spans == MAX_NESTING {
                    self.invalidate();
                } else {
                    self.compaction_spans += 1;
                    self.idle = false;
                }
            }
            ReducerEvent::CompactEnd => {
                if !self.known || self.compaction_spans == 0 {
                    self.invalidate();
                } else {
                    self.compaction_spans -= 1;
                }
            }
            ReducerEvent::PromptStart => {
                self.recover_for_activity();
                if self.prompt_spans == MAX_NESTING {
                    self.invalidate();
                } else {
                    self.prompt_spans += 1;
                    self.idle = false;
                }
            }
            ReducerEvent::PromptEnd => {
                if !self.known || self.prompt_spans == 0 {
                    self.invalidate();
                } else {
                    self.prompt_spans -= 1;
                }
            }
        }
        (!self.shutdown).then(|| self.state())
    }

    fn state(&self) -> ReducedState {
        let phase = if !self.known {
            None
        } else if self.prompt_spans != 0 {
            Some(Phase::WaitingForUser)
        } else if self.compaction_spans != 0 {
            Some(Phase::Compacting)
        } else if !self.tool_ids.is_empty() {
            Some(Phase::ToolRunning)
        } else if self.agent_spans != 0 {
            Some(Phase::Generating)
        } else if self.idle {
            Some(Phase::Idle)
        } else {
            None
        };
        ReducedState {
            phase,
            pending_messages: None,
        }
    }

    fn recover_for_activity(&mut self) {
        if !self.known {
            self.clear();
            self.known = true;
            self.idle = false;
        }
    }

    fn invalidate(&mut self) {
        self.clear();
        self.known = false;
        self.idle = false;
    }

    fn clear(&mut self) {
        self.agent_spans = 0;
        self.compaction_spans = 0;
        self.prompt_spans = 0;
        self.tool_ids.clear();
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const MAX_SOCKET_CANDIDATES: usize = 64;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const MAX_RUNTIME_ROOT_BYTES: usize = 71;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const MAX_SOCKET_PATH_BYTES_WITH_NUL: usize = 104;

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct SocketCandidate {
    path: PathBuf,
    device: u64,
    inode: u64,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl fmt::Debug for SocketCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SocketCandidate")
            .field("device", &self.device)
            .field("inode", &self.inode)
            .finish_non_exhaustive()
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoveryError {
    Unavailable,
    Exhausted,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug)]
struct DiscoveryGeneration {
    baseline: Vec<SocketCandidate>,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl DiscoveryGeneration {
    fn new(baseline: Vec<SocketCandidate>) -> Self {
        Self { baseline }
    }

    fn is_unchanged(&self, current: &[SocketCandidate]) -> bool {
        self.baseline == current
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn directory_identity(path: &Path, owner: u32, mode: u32) -> Result<(u64, u64), DiscoveryError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| DiscoveryError::Unavailable)?;
    if !metadata.file_type().is_dir() || metadata.uid() != owner || metadata.mode() & 0o7777 != mode
    {
        return Err(DiscoveryError::Unavailable);
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn runtime_root_fits(path: &Path) -> bool {
    path.as_os_str().as_bytes().len() <= MAX_RUNTIME_ROOT_BYTES
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn socket_path_fits(path: &Path) -> bool {
    path.as_os_str().as_bytes().len() < MAX_SOCKET_PATH_BYTES_WITH_NUL
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn is_runtime_socket_name(name: &[u8]) -> bool {
    name.len() == 31
        && name.starts_with(b"s-")
        && name[2..26]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        && &name[26..] == b".sock"
}

/// Enumerate one complete fixed-root candidate set. The caller supplies the
/// trusted root owner so tests can exercise the same checks in an isolated
/// directory; production always supplies root UID 0.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn enumerate_runtime_candidates_at(
    base: &Path,
    trusted_root_owner: u32,
    effective_uid: u32,
    excluded: &IdentitySet<SocketCandidate>,
) -> Result<Vec<SocketCandidate>, DiscoveryError> {
    let base_before = directory_identity(base, trusted_root_owner, 0o1777)?;
    let runtime_root = base.join(format!("ptop-{effective_uid}"));
    if !runtime_root_fits(&runtime_root) {
        return Err(DiscoveryError::Unavailable);
    }
    let root_before = directory_identity(&runtime_root, effective_uid, 0o700)?;
    let entries = fs::read_dir(&runtime_root).map_err(|_| DiscoveryError::Unavailable)?;
    let mut candidates = Vec::new();
    for (index, entry) in entries.enumerate() {
        let entry = entry.map_err(|_| DiscoveryError::Unavailable)?;
        if index == MAX_SOCKET_CANDIDATES {
            return Err(DiscoveryError::Exhausted);
        }
        let name = entry.file_name();
        if !is_runtime_socket_name(name.as_bytes()) {
            continue;
        }
        let path = entry.path();
        if !socket_path_fits(&path) {
            return Err(DiscoveryError::Unavailable);
        }
        let metadata = fs::symlink_metadata(&path).map_err(|_| DiscoveryError::Unavailable)?;
        if !metadata.file_type().is_socket()
            || metadata.uid() != effective_uid
            || metadata.mode() & 0o7777 != 0o600
        {
            return Err(DiscoveryError::Unavailable);
        }
        let candidate = SocketCandidate {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        if !excluded.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    let root_after = directory_identity(&runtime_root, effective_uid, 0o700)?;
    let base_after = directory_identity(base, trusted_root_owner, 0o1777)?;
    if root_before != root_after || base_before != base_after {
        return Err(DiscoveryError::Unavailable);
    }
    candidates.sort();
    Ok(candidates)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn enumerate_runtime_candidates(
    excluded: &IdentitySet<SocketCandidate>,
) -> Result<Vec<SocketCandidate>, DiscoveryError> {
    let effective_uid = unsafe { libc::geteuid() };
    #[cfg(target_os = "linux")]
    let base = Path::new("/tmp");
    #[cfg(target_vendor = "apple")]
    let base = Path::new("/private/tmp");
    enumerate_runtime_candidates_at(base, 0, effective_uid, excluded)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn current_socket_candidate(path: &Path, effective_uid: u32) -> io::Result<SocketCandidate> {
    if !socket_path_fits(path) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "socket path"));
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != effective_uid
        || metadata.mode() & 0o7777 != 0o600
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "socket identity",
        ));
    }
    Ok(SocketCandidate {
        path: path.to_path_buf(),
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
struct UnixLiveHarnessStream {
    stream: UnixStream,
    connected: bool,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl UnixLiveHarnessStream {
    fn connect(candidate: &SocketCandidate) -> io::Result<Self> {
        let effective_uid = unsafe { libc::geteuid() };
        if current_socket_candidate(&candidate.path, effective_uid)? != *candidate {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "socket replaced",
            ));
        }
        let path = candidate.path.as_os_str().as_bytes();
        if path.contains(&0) || path.len() >= MAX_SOCKET_PATH_BYTES_WITH_NUL {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "socket path"));
        }

        let descriptor = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        let result = (|| {
            let status_flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
            if status_flags < 0
                || unsafe {
                    libc::fcntl(descriptor, libc::F_SETFL, status_flags | libc::O_NONBLOCK)
                } < 0
            {
                return Err(io::Error::last_os_error());
            }
            let descriptor_flags = unsafe { libc::fcntl(descriptor, libc::F_GETFD) };
            if descriptor_flags < 0
                || unsafe {
                    libc::fcntl(
                        descriptor,
                        libc::F_SETFD,
                        descriptor_flags | libc::FD_CLOEXEC,
                    )
                } < 0
            {
                return Err(io::Error::last_os_error());
            }

            let mut address = unsafe { mem::zeroed::<libc::sockaddr_un>() };
            address.sun_family = libc::AF_UNIX as libc::sa_family_t;
            if path.len() >= address.sun_path.len() {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "socket path"));
            }
            for (destination, source) in address.sun_path.iter_mut().zip(path.iter().copied()) {
                *destination = source as libc::c_char;
            }
            let connected = unsafe {
                libc::connect(
                    descriptor,
                    (&raw const address).cast::<libc::sockaddr>(),
                    mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
                )
            } == 0;
            if !connected {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::EINPROGRESS) {
                    return Err(error);
                }
            }
            Ok(connected)
        })();
        match result {
            Ok(connected) => {
                let stream = unsafe { UnixStream::from_raw_fd(descriptor) };
                Ok(Self { stream, connected })
            }
            Err(error) => {
                unsafe { libc::close(descriptor) };
                Err(error)
            }
        }
    }

    fn connect_ready(&mut self) -> io::Result<bool> {
        if self.connected {
            return Ok(true);
        }
        let mut poll = libc::pollfd {
            fd: self.stream.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&raw mut poll, 1, 0) };
        if ready < 0 {
            return Err(io::Error::last_os_error());
        }
        if ready == 0 {
            return Ok(false);
        }
        if let Some(error) = self.stream.take_error()? {
            return Err(error);
        }
        if poll.revents & libc::POLLOUT == 0
            || poll.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                "socket connect",
            ));
        }
        self.connected = true;
        Ok(true)
    }

    fn peer_pid(&self) -> io::Result<u32> {
        platform_peer_pid(self.stream.as_raw_fd())
    }

    fn read_into(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buffer)
    }
}

#[cfg(target_os = "linux")]
fn platform_peer_pid(descriptor: libc::c_int) -> io::Result<u32> {
    let mut credentials = MaybeUninit::<libc::ucred>::zeroed();
    let mut length = mem::size_of::<libc::ucred>() as libc::socklen_t;
    let status = unsafe {
        libc::getsockopt(
            descriptor,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &raw mut length,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != mem::size_of::<libc::ucred>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "peer credentials",
        ));
    }
    let credentials = unsafe { credentials.assume_init() };
    if credentials.pid <= 0
        || credentials.uid != unsafe { libc::geteuid() }
        || credentials.gid != unsafe { libc::getegid() }
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "peer credentials",
        ));
    }
    u32::try_from(credentials.pid)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "peer pid"))
}

#[cfg(target_vendor = "apple")]
#[repr(C)]
#[derive(Clone, Copy)]
struct AuditToken {
    value: [u32; 8],
}

#[cfg(target_vendor = "apple")]
#[link(name = "bsm")]
extern "C" {
    fn audit_token_to_pid(token: AuditToken) -> libc::pid_t;
}

#[cfg(target_vendor = "apple")]
fn socket_option<T>(descriptor: libc::c_int, option: libc::c_int) -> io::Result<T> {
    let mut value = MaybeUninit::<T>::zeroed();
    let mut length = mem::size_of::<T>() as libc::socklen_t;
    let status = unsafe {
        libc::getsockopt(
            descriptor,
            libc::SOL_LOCAL,
            option,
            value.as_mut_ptr().cast(),
            &raw mut length,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != mem::size_of::<T>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "peer credentials",
        ));
    }
    Ok(unsafe { value.assume_init() })
}

#[cfg(target_vendor = "apple")]
fn platform_peer_pid(descriptor: libc::c_int) -> io::Result<u32> {
    let peer_pid = socket_option::<libc::pid_t>(descriptor, libc::LOCAL_PEERPID)?;
    let effective_pid = socket_option::<libc::pid_t>(descriptor, libc::LOCAL_PEEREPID)?;
    let token = socket_option::<AuditToken>(descriptor, libc::LOCAL_PEERTOKEN)?;
    let token_pid = unsafe { audit_token_to_pid(token) };
    let mut peer_uid = 0;
    let mut peer_gid = 0;
    if unsafe { libc::getpeereid(descriptor, &raw mut peer_uid, &raw mut peer_gid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if peer_pid <= 0
        || peer_pid != effective_pid
        || peer_pid != token_pid
        || peer_uid != unsafe { libc::geteuid() }
        || peer_gid != unsafe { libc::getegid() }
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "peer credentials",
        ));
    }
    u32::try_from(peer_pid).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "peer pid"))
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const MAX_CONNECTS_PER_TICK: usize = 4;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const MAX_CREDENTIALS_PER_TICK: usize = 4;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const MAX_FRAMES_PER_TICK: usize = 16;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const MAX_BYTES_PER_TICK: usize = 8 * 1024;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const MAX_REGISTRY_DESCRIPTORS: usize = 8;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const MAX_PENDING_CONNECTS: usize = 4;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const MAX_PENDING_LIVE_FRAMES: usize = MAX_CREDENTIALS_PER_TICK;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const REGISTRY_WORK_BUDGET: Duration = Duration::from_millis(5);
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const STREAM_DEADLINE: Duration = Duration::from_secs(1);
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const OBSERVATION_DEADLINE: Duration = Duration::from_secs(3);
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const DISCOVERY_BACKOFF_SECONDS: [u64; 4] = [1, 2, 4, 10];

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
trait RegistryStream {
    fn connect_ready(&mut self) -> io::Result<bool>;
    fn peer_pid(&self) -> io::Result<u32>;
    fn read_into(&mut self, buffer: &mut [u8]) -> io::Result<usize>;
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl RegistryStream for UnixLiveHarnessStream {
    fn connect_ready(&mut self) -> io::Result<bool> {
        Self::connect_ready(self)
    }

    fn peer_pid(&self) -> io::Result<u32> {
        Self::peer_pid(self)
    }

    fn read_into(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        Self::read_into(self, buffer)
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
trait RegistryBackend {
    fn now(&self) -> Instant;
    fn candidate_current(&mut self, candidate: &SocketCandidate) -> bool;
    fn enumerate(
        &mut self,
        excluded: &IdentitySet<SocketCandidate>,
    ) -> Result<Vec<SocketCandidate>, DiscoveryError>;
    fn connect(&mut self, candidate: &SocketCandidate) -> io::Result<Box<dyn RegistryStream>>;
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
struct UnixRegistryBackend;

#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
struct DisabledRegistryBackend;

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl RegistryBackend for UnixRegistryBackend {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn candidate_current(&mut self, candidate: &SocketCandidate) -> bool {
        let effective_uid = unsafe { libc::geteuid() };
        current_socket_candidate(&candidate.path, effective_uid)
            .is_ok_and(|current| current == *candidate)
    }

    fn enumerate(
        &mut self,
        excluded: &IdentitySet<SocketCandidate>,
    ) -> Result<Vec<SocketCandidate>, DiscoveryError> {
        enumerate_runtime_candidates(excluded)
    }

    fn connect(&mut self, candidate: &SocketCandidate) -> io::Result<Box<dyn RegistryStream>> {
        UnixLiveHarnessStream::connect(candidate)
            .map(|stream| Box::new(stream) as Box<dyn RegistryStream>)
    }
}

#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
impl RegistryBackend for DisabledRegistryBackend {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn candidate_current(&mut self, _candidate: &SocketCandidate) -> bool {
        false
    }

    fn enumerate(
        &mut self,
        _excluded: &IdentitySet<SocketCandidate>,
    ) -> Result<Vec<SocketCandidate>, DiscoveryError> {
        Err(DiscoveryError::Unavailable)
    }

    fn connect(&mut self, _candidate: &SocketCandidate) -> io::Result<Box<dyn RegistryStream>> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "disabled"))
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct VerifiedSidecarAttachment {
    pid: u32,
    start_id: String,
    session_id: String,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl VerifiedSidecarAttachment {
    pub(super) fn new(pid: u32, start_id: &str, session_id: &str) -> Self {
        Self {
            pid,
            start_id: start_id.to_owned(),
            session_id: session_id.to_owned(),
        }
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
struct RegistryProbe {
    candidate: SocketCandidate,
    stream: Box<dyn RegistryStream>,
    deadline: Instant,
    attachment: Option<VerifiedSidecarAttachment>,
    decoder: FrameDecoder,
    protocol: Option<ProtocolConnection>,
    partial_deadline: Option<Instant>,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
struct AcceptedRegistryStream {
    candidate: SocketCandidate,
    stream: Box<dyn RegistryStream>,
    attachment: VerifiedSidecarAttachment,
    decoder: FrameDecoder,
    protocol: ProtocolConnection,
    partial_deadline: Option<Instant>,
    latest: LiveFrame,
    latest_received_at: Instant,
    latest_observed_at_ms: u64,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Clone)]
struct PendingLiveFrame {
    attachment: VerifiedSidecarAttachment,
    candidate: SocketCandidate,
    peer_pid: u32,
    epoch: String,
    sequence: u64,
    phase: Option<Phase>,
    pending_messages: Option<bool>,
    received_at: Instant,
    observed_at_ms: u64,
    read_tick: u64,
    credential_valid: bool,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrivateObservationHealth {
    Healthy,
    Stale,
    Unavailable,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[allow(dead_code)]
struct PrivateLiveObservation {
    attachment: VerifiedSidecarAttachment,
    health: PrivateObservationHealth,
    epoch: Option<String>,
    sequence: Option<u64>,
    phase: Option<Phase>,
    pending_messages: Option<bool>,
    received_at: Option<Instant>,
    observed_at_ms: Option<u64>,
    stale_tick: Option<u64>,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
struct RegistryGeneration {
    identity: DiscoveryGeneration,
    unresolved: VecDeque<SocketCandidate>,
    probes: Vec<RegistryProbe>,
    matches: Vec<AcceptedRegistryStream>,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
struct RegistryTickBudget {
    started: Instant,
    connects: usize,
    credentials: usize,
    frames: usize,
    bytes: usize,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl RegistryTickBudget {
    fn new(started: Instant) -> Self {
        Self {
            started,
            connects: 0,
            credentials: 0,
            frames: 0,
            bytes: 0,
        }
    }

    fn elapsed(&self, now: Instant) -> bool {
        now.checked_duration_since(self.started)
            .is_some_and(|elapsed| elapsed >= REGISTRY_WORK_BUDGET)
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(super) struct SidecarRegistry {
    backend: Box<dyn RegistryBackend>,
    attachments: Vec<VerifiedSidecarAttachment>,
    generation: Option<RegistryGeneration>,
    accepted: BTreeMap<u32, AcceptedRegistryStream>,
    pending: BTreeMap<u32, PendingLiveFrame>,
    observations: BTreeMap<u32, PrivateLiveObservation>,
    previous_epochs: IdentityMap<VerifiedSidecarAttachment, String>,
    next_discovery: Option<Instant>,
    backoff_index: usize,
    tick: u64,
    observed_at_ms: u64,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
impl SidecarRegistry {
    pub(super) fn new() -> Self {
        #[cfg(test)]
        let backend: Box<dyn RegistryBackend> = Box::new(DisabledRegistryBackend);
        #[cfg(not(test))]
        let backend: Box<dyn RegistryBackend> = Box::new(UnixRegistryBackend);
        Self::with_backend(backend)
    }

    fn with_backend(backend: Box<dyn RegistryBackend>) -> Self {
        Self {
            backend,
            attachments: Vec::new(),
            generation: None,
            accepted: BTreeMap::new(),
            pending: BTreeMap::new(),
            observations: BTreeMap::new(),
            previous_epochs: IdentityMap::new(),
            next_discovery: None,
            backoff_index: 0,
            tick: 0,
            observed_at_ms: 0,
        }
    }

    pub(super) fn poll_at(
        &mut self,
        attachments: &[VerifiedSidecarAttachment],
        observed_at_ms: u64,
    ) {
        let started = self.backend.now();
        self.tick = self.tick.saturating_add(1);
        self.observed_at_ms = observed_at_ms;
        let mut budget = RegistryTickBudget::new(started);
        let mut current = attachments.to_vec();
        current.sort();
        current.dedup();
        if current != self.attachments {
            if let Some(mut generation) = self.generation.take() {
                self.reconcile_generation_attachments(&mut generation, &current);
                self.generation = Some(generation);
            }
            let stale_pids: Vec<u32> = self
                .accepted
                .iter()
                .filter_map(|(pid, stream)| (!current.contains(&stream.attachment)).then_some(*pid))
                .collect();
            for pid in stale_pids {
                if let Some(stream) = self.accepted.remove(&pid) {
                    self.remember_stream_epoch(&stream);
                }
            }
            self.pending
                .retain(|_, pending| current.contains(&pending.attachment));
            self.observations
                .retain(|_, observation| current.contains(&observation.attachment));
            self.attachments = current;
            self.previous_epochs
                .retain(|attachment, _| self.attachments.contains(attachment));
            self.backoff_index = 0;
            self.next_discovery = Some(started);
        }
        if self.attachments.is_empty() {
            self.generation = None;
            self.accepted.clear();
            self.pending.clear();
            self.observations.clear();
            self.previous_epochs.clear();
            self.next_discovery = None;
            return;
        }

        self.advance_observation_health(started);
        let credential_limit = if self.discovery_needs_credential_turn(started) {
            MAX_CREDENTIALS_PER_TICK - 1
        } else {
            MAX_CREDENTIALS_PER_TICK
        };
        self.commit_pending(&mut budget, credential_limit);
        self.service_accepted(&mut budget);
        if budget.elapsed(self.backend.now()) {
            return;
        }
        if self.generation.is_none()
            && self
                .next_discovery
                .is_some_and(|next_discovery| started >= next_discovery)
        {
            self.start_generation(&mut budget);
        }
        if let Some(mut generation) = self.generation.take() {
            if !self.advance_generation(&mut generation, &mut budget) {
                return;
            }
            if generation.unresolved.is_empty() && generation.probes.is_empty() {
                self.finish_generation(generation, &mut budget);
            } else {
                self.generation = Some(generation);
            }
        }
    }

    #[cfg(test)]
    fn poll(&mut self, attachments: &[VerifiedSidecarAttachment]) {
        self.poll_at(attachments, self.tick.saturating_mul(1_000));
    }

    fn advance_observation_health(&mut self, now: Instant) {
        let mut observations = BTreeMap::new();
        for (pid, observation) in std::mem::take(&mut self.observations) {
            if !self.attachments.contains(&observation.attachment) {
                continue;
            }
            let observation = match observation.health {
                PrivateObservationHealth::Healthy
                    if observation
                        .received_at
                        .is_some_and(|received| now >= received + OBSERVATION_DEADLINE) =>
                {
                    let observed_at_ms = observation.observed_at_ms;
                    let mut stale = Self::empty_observation(
                        observation.attachment,
                        PrivateObservationHealth::Stale,
                        Some(self.tick),
                    );
                    stale.observed_at_ms = observed_at_ms;
                    stale
                }
                PrivateObservationHealth::Stale
                    if observation.stale_tick.is_some_and(|tick| tick < self.tick) =>
                {
                    Self::empty_observation(
                        observation.attachment,
                        PrivateObservationHealth::Unavailable,
                        None,
                    )
                }
                _ => observation,
            };
            observations.insert(pid, observation);
        }
        self.observations = observations;
    }

    fn discovery_needs_credential_turn(&self, now: Instant) -> bool {
        let descriptor_count = self.accepted.len()
            + self.generation.as_ref().map_or(0, |generation| {
                generation.probes.len() + generation.matches.len()
            });
        if descriptor_count >= MAX_REGISTRY_DESCRIPTORS {
            return false;
        }
        self.generation.as_ref().is_some_and(|generation| {
            !generation.unresolved.is_empty() || !generation.probes.is_empty()
        }) || (self.generation.is_none()
            && self
                .next_discovery
                .is_some_and(|next_discovery| now >= next_discovery))
    }

    fn commit_pending(&mut self, budget: &mut RegistryTickBudget, credential_limit: usize) {
        let now = self.backend.now();
        let mut pids: Vec<u32> = self
            .pending
            .iter()
            .filter_map(|(pid, pending)| (pending.read_tick < self.tick).then_some(*pid))
            .collect();
        pids.sort_by_key(|pid| {
            let pending = self.pending.get(pid).expect("pending frame");
            (pending.received_at, *pid)
        });

        for pid in pids {
            if budget.credentials == credential_limit || budget.elapsed(self.backend.now()) {
                break;
            }
            let pending = self.pending.get(&pid).expect("pending frame").clone();
            if now >= pending.received_at + OBSERVATION_DEADLINE {
                self.pending.remove(&pid);
                continue;
            }
            let current = self
                .attachments
                .iter()
                .filter(|attachment| attachment.pid == pid)
                .collect::<Vec<_>>();
            if current.len() != 1 || *current[0] != pending.attachment {
                self.invalidate_stream(pid);
                continue;
            }
            let stream_valid = self.accepted.get(&pid).is_some_and(|stream| {
                stream.attachment == pending.attachment
                    && stream.candidate == pending.candidate
                    && stream.protocol.epoch() == Some(pending.epoch.as_str())
                    && stream.latest.sequence == pending.sequence
            });
            if !pending.credential_valid || pending.peer_pid != pid || !stream_valid {
                self.invalidate_stream(pid);
                continue;
            }
            if !self.backend.candidate_current(&pending.candidate) {
                self.invalidate_stream(pid);
                continue;
            }
            if budget.elapsed(self.backend.now()) {
                break;
            }
            budget.credentials += 1;
            let peer_pid = self
                .accepted
                .get(&pid)
                .and_then(|stream| stream.stream.peer_pid().ok());
            if peer_pid != Some(pid) {
                self.invalidate_stream(pid);
                continue;
            }
            if budget.elapsed(self.backend.now()) {
                break;
            }
            self.observations.insert(
                pid,
                PrivateLiveObservation {
                    attachment: pending.attachment,
                    health: PrivateObservationHealth::Healthy,
                    epoch: Some(pending.epoch),
                    sequence: Some(pending.sequence),
                    phase: pending.phase,
                    pending_messages: pending.pending_messages,
                    received_at: Some(pending.received_at),
                    observed_at_ms: Some(pending.observed_at_ms),
                    stale_tick: None,
                },
            );
            self.pending.remove(&pid);
        }
    }

    fn empty_observation(
        attachment: VerifiedSidecarAttachment,
        health: PrivateObservationHealth,
        stale_tick: Option<u64>,
    ) -> PrivateLiveObservation {
        PrivateLiveObservation {
            attachment,
            health,
            epoch: None,
            sequence: None,
            phase: None,
            pending_messages: None,
            received_at: None,
            observed_at_ms: None,
            stale_tick,
        }
    }

    fn invalidate_stream(&mut self, pid: u32) {
        self.pending.remove(&pid);
        if let Some(stream) = self.accepted.remove(&pid) {
            self.remember_stream_epoch(&stream);
        }
        let mut attachments = self
            .attachments
            .iter()
            .filter(|attachment| attachment.pid == pid);
        if let Some(attachment) = attachments.next().cloned() {
            if attachments.next().is_none() {
                self.observations.insert(
                    pid,
                    Self::empty_observation(
                        attachment,
                        PrivateObservationHealth::Unavailable,
                        None,
                    ),
                );
                return;
            }
        }
        self.observations.remove(&pid);
    }

    fn reconcile_generation_attachments(
        &mut self,
        generation: &mut RegistryGeneration,
        current: &[VerifiedSidecarAttachment],
    ) {
        let mut probes = Vec::with_capacity(generation.probes.len());
        for probe in generation.probes.drain(..) {
            if probe
                .attachment
                .as_ref()
                .is_none_or(|attachment| current.contains(attachment))
            {
                probes.push(probe);
            } else {
                self.remember_probe_epoch(&probe);
            }
        }
        generation.probes = probes;

        let mut matches = Vec::with_capacity(generation.matches.len());
        for stream in generation.matches.drain(..) {
            if current.contains(&stream.attachment) {
                matches.push(stream);
            } else {
                self.remember_stream_epoch(&stream);
            }
        }
        generation.matches = matches;
    }

    fn excluded_candidates(&self) -> IdentitySet<SocketCandidate> {
        self.accepted
            .values()
            .map(|stream| stream.candidate.clone())
            .collect()
    }

    fn start_generation(&mut self, budget: &mut RegistryTickBudget) {
        let excluded = self.excluded_candidates();
        let candidates = match self.backend.enumerate(&excluded) {
            Ok(candidates) if !budget.elapsed(self.backend.now()) => candidates,
            _ => {
                self.schedule_retry(self.backend.now());
                return;
            }
        };
        self.generation = Some(RegistryGeneration {
            identity: DiscoveryGeneration::new(candidates.clone()),
            unresolved: candidates.into(),
            probes: Vec::new(),
            matches: Vec::new(),
        });
    }

    fn advance_generation(
        &mut self,
        generation: &mut RegistryGeneration,
        budget: &mut RegistryTickBudget,
    ) -> bool {
        self.service_generation_matches(generation, budget);
        self.advance_probes(generation, budget);
        while budget.connects < MAX_CONNECTS_PER_TICK
            && generation.probes.len() < MAX_PENDING_CONNECTS
            && generation.probes.len() < MAX_CREDENTIALS_PER_TICK.saturating_sub(budget.credentials)
            && generation.probes.len() < MAX_FRAMES_PER_TICK.saturating_sub(budget.frames)
            && MAX_BYTES_PER_TICK.saturating_sub(budget.bytes) >= MAX_FRAME_BYTES + 4
            && !generation.unresolved.is_empty()
            && !budget.elapsed(self.backend.now())
        {
            if self.descriptor_count(generation) >= MAX_REGISTRY_DESCRIPTORS {
                self.discard_generation(generation);
                self.schedule_retry(self.backend.now());
                return false;
            }
            let candidate = generation.unresolved.pop_front().expect("candidate");
            budget.connects += 1;
            if let Ok(stream) = self.backend.connect(&candidate) {
                generation.probes.push(RegistryProbe {
                    candidate,
                    stream,
                    deadline: self.backend.now() + STREAM_DEADLINE,
                    attachment: None,
                    decoder: FrameDecoder::default(),
                    protocol: None,
                    partial_deadline: None,
                });
            }
        }
        self.advance_probes(generation, budget);
        true
    }

    fn advance_probes(
        &mut self,
        generation: &mut RegistryGeneration,
        budget: &mut RegistryTickBudget,
    ) {
        let mut remaining = Vec::new();
        for mut probe in generation.probes.drain(..) {
            if budget.frames == MAX_FRAMES_PER_TICK
                || budget.bytes == MAX_BYTES_PER_TICK
                || budget.elapsed(self.backend.now())
            {
                remaining.push(probe);
                continue;
            }
            let now = self.backend.now();
            if now >= probe.deadline {
                self.remember_probe_epoch(&probe);
                continue;
            }
            let ready = match probe.stream.connect_ready() {
                Ok(ready) => ready,
                Err(_) => {
                    self.remember_probe_epoch(&probe);
                    continue;
                }
            };
            if !ready {
                remaining.push(probe);
                continue;
            }
            if probe.attachment.is_none() {
                if budget.credentials == MAX_CREDENTIALS_PER_TICK
                    || budget.elapsed(self.backend.now())
                {
                    remaining.push(probe);
                    continue;
                }
                budget.credentials += 1;
                let peer_pid = match probe.stream.peer_pid() {
                    Ok(peer_pid) => peer_pid,
                    Err(_) => continue,
                };
                let Some(attachment) = self.unique_attachment(peer_pid) else {
                    continue;
                };
                let previous_epoch = self.previous_epochs.get(&attachment).map(String::as_str);
                probe.protocol = Some(ProtocolConnection::new(
                    &attachment.session_id,
                    previous_epoch,
                ));
                probe.attachment = Some(attachment);
                probe.deadline = now + STREAM_DEADLINE;
            }
            if budget.elapsed(self.backend.now()) {
                remaining.push(probe);
                continue;
            }
            match read_registry_frame(
                probe.stream.as_mut(),
                &mut probe.decoder,
                probe.protocol.as_mut().expect("credentialed protocol"),
                &mut probe.partial_deadline,
                budget,
                now,
            ) {
                Ok(Some(frame)) => {
                    generation.matches.push(AcceptedRegistryStream {
                        candidate: probe.candidate,
                        stream: probe.stream,
                        attachment: probe.attachment.expect("credentialed attachment"),
                        decoder: probe.decoder,
                        protocol: probe.protocol.expect("credentialed protocol"),
                        partial_deadline: probe.partial_deadline,
                        latest: frame,
                        latest_received_at: self.backend.now(),
                        latest_observed_at_ms: self.observed_at_ms,
                    });
                }
                Ok(None) => remaining.push(probe),
                Err(()) => self.remember_probe_epoch(&probe),
            }
        }
        generation.probes = remaining;
    }

    fn service_generation_matches(
        &mut self,
        generation: &mut RegistryGeneration,
        budget: &mut RegistryTickBudget,
    ) {
        self.expire_generation_match_deadlines(generation, self.backend.now());
        for _ in 0..2 {
            let mut remaining = Vec::with_capacity(generation.matches.len());
            for mut stream in generation.matches.drain(..) {
                if budget.frames == MAX_FRAMES_PER_TICK
                    || budget.bytes == MAX_BYTES_PER_TICK
                    || budget.elapsed(self.backend.now())
                {
                    remaining.push(stream);
                    continue;
                }
                let now = self.backend.now();
                match read_registry_frame(
                    stream.stream.as_mut(),
                    &mut stream.decoder,
                    &mut stream.protocol,
                    &mut stream.partial_deadline,
                    budget,
                    now,
                ) {
                    Ok(Some(frame)) => {
                        stream.latest = frame;
                        stream.latest_received_at = self.backend.now();
                        stream.latest_observed_at_ms = self.observed_at_ms;
                        remaining.push(stream);
                    }
                    Ok(None) => remaining.push(stream),
                    Err(()) => self.remember_stream_epoch(&stream),
                }
            }
            generation.matches = remaining;
        }
    }

    fn expire_generation_match_deadlines(
        &mut self,
        generation: &mut RegistryGeneration,
        now: Instant,
    ) {
        let mut current = Vec::with_capacity(generation.matches.len());
        for stream in generation.matches.drain(..) {
            if stream
                .partial_deadline
                .is_some_and(|deadline| now >= deadline)
                && !stream.decoder.has_complete_frame()
            {
                self.remember_stream_epoch(&stream);
            } else {
                current.push(stream);
            }
        }
        generation.matches = current;
    }

    fn finish_generation(
        &mut self,
        mut generation: RegistryGeneration,
        budget: &mut RegistryTickBudget,
    ) {
        self.expire_generation_match_deadlines(&mut generation, self.backend.now());
        if budget.elapsed(self.backend.now()) {
            self.discard_generation(&mut generation);
            self.schedule_retry(self.backend.now());
            return;
        }
        let excluded = self.excluded_candidates();
        let current = match self.backend.enumerate(&excluded) {
            Ok(current) if !budget.elapsed(self.backend.now()) => current,
            _ => {
                self.discard_generation(&mut generation);
                self.schedule_retry(self.backend.now());
                return;
            }
        };
        if !generation.identity.is_unchanged(&current) {
            self.discard_generation(&mut generation);
            self.schedule_retry(self.backend.now());
            return;
        }

        let mut counts = BTreeMap::<u32, usize>::new();
        for pid in self.accepted.keys() {
            counts.insert(*pid, 1);
        }
        for stream in &generation.matches {
            *counts.entry(stream.attachment.pid).or_default() += 1;
        }
        let ambiguous: IdentitySet<u32> = counts
            .into_iter()
            .filter_map(|(pid, count)| (count > 1).then_some(pid))
            .collect();
        for pid in &ambiguous {
            self.invalidate_stream(*pid);
        }
        for stream in generation.matches {
            if ambiguous.contains(&stream.attachment.pid)
                || !self.attachments.contains(&stream.attachment)
            {
                self.remember_stream_epoch(&stream);
                continue;
            }
            self.stage_stream(&stream);
            self.accepted.insert(stream.attachment.pid, stream);
        }
        self.schedule_retry(self.backend.now());
    }

    fn descriptor_count(&self, generation: &RegistryGeneration) -> usize {
        self.accepted.len() + generation.probes.len() + generation.matches.len()
    }

    fn unique_attachment(&self, pid: u32) -> Option<VerifiedSidecarAttachment> {
        let mut matches = self
            .attachments
            .iter()
            .filter(|attachment| attachment.pid == pid);
        let first = matches.next()?.clone();
        matches.next().is_none().then_some(first)
    }

    fn discard_generation(&mut self, generation: &mut RegistryGeneration) {
        for probe in &generation.probes {
            self.remember_probe_epoch(probe);
        }
        for stream in &generation.matches {
            self.remember_stream_epoch(stream);
        }
        generation.unresolved.clear();
        generation.probes.clear();
        generation.matches.clear();
    }

    fn remember_probe_epoch(&mut self, probe: &RegistryProbe) {
        if let (Some(attachment), Some(epoch)) = (
            probe.attachment.as_ref(),
            probe.protocol.as_ref().and_then(ProtocolConnection::epoch),
        ) {
            self.previous_epochs
                .insert(attachment.clone(), epoch.to_owned());
        }
    }

    fn remember_stream_epoch(&mut self, stream: &AcceptedRegistryStream) {
        if let Some(epoch) = stream.protocol.epoch() {
            self.previous_epochs
                .insert(stream.attachment.clone(), epoch.to_owned());
        }
    }

    fn stage_stream(&mut self, stream: &AcceptedRegistryStream) {
        if !self.pending.contains_key(&stream.attachment.pid)
            && self.pending.len() == MAX_PENDING_LIVE_FRAMES
        {
            return;
        }
        self.pending.insert(
            stream.attachment.pid,
            PendingLiveFrame {
                attachment: stream.attachment.clone(),
                candidate: stream.candidate.clone(),
                peer_pid: stream.attachment.pid,
                epoch: stream.latest.epoch.clone(),
                sequence: stream.latest.sequence,
                phase: stream.latest.phase,
                pending_messages: stream.latest.pending_messages,
                received_at: stream.latest_received_at,
                observed_at_ms: stream.latest_observed_at_ms,
                read_tick: self.tick,
                credential_valid: true,
            },
        );
    }

    fn service_accepted(&mut self, budget: &mut RegistryTickBudget) {
        let now = self.backend.now();
        let expired: Vec<u32> = self
            .accepted
            .iter()
            .filter_map(|(pid, stream)| {
                (stream
                    .partial_deadline
                    .is_some_and(|deadline| now >= deadline)
                    && !stream.decoder.has_complete_frame())
                .then_some(*pid)
            })
            .collect();
        for pid in expired {
            self.invalidate_stream(pid);
        }
        for _ in 0..MAX_FRAMES_PER_TICK {
            let mut decoded_frame = false;
            let mut pids: Vec<u32> = self.accepted.keys().copied().collect();
            pids.sort_by_key(|pid| {
                let (rank, received_at) = match self.observations.get(pid) {
                    None => (0, now),
                    Some(observation) => match observation.health {
                        PrivateObservationHealth::Unavailable => (0, now),
                        PrivateObservationHealth::Stale => (1, now),
                        PrivateObservationHealth::Healthy => {
                            (2, observation.received_at.unwrap_or(now))
                        }
                    },
                };
                (rank, received_at, *pid)
            });
            for pid in pids {
                if self
                    .pending
                    .get(&pid)
                    .is_some_and(|pending| pending.read_tick < self.tick)
                    || (!self.pending.contains_key(&pid)
                        && self.pending.len() == MAX_PENDING_LIVE_FRAMES)
                {
                    continue;
                }
                if budget.frames == MAX_FRAMES_PER_TICK
                    || budget.bytes == MAX_BYTES_PER_TICK
                    || budget.elapsed(self.backend.now())
                {
                    return;
                }
                let Some(mut stream) = self.accepted.remove(&pid) else {
                    continue;
                };
                let now = self.backend.now();
                match read_registry_frame(
                    stream.stream.as_mut(),
                    &mut stream.decoder,
                    &mut stream.protocol,
                    &mut stream.partial_deadline,
                    budget,
                    now,
                ) {
                    Ok(Some(frame)) => {
                        decoded_frame = true;
                        stream.latest = frame;
                        stream.latest_received_at = self.backend.now();
                        stream.latest_observed_at_ms = self.observed_at_ms;
                        self.stage_stream(&stream);
                        self.accepted.insert(pid, stream);
                    }
                    Ok(None) => {
                        self.accepted.insert(pid, stream);
                    }
                    Err(()) => {
                        self.remember_stream_epoch(&stream);
                        self.invalidate_stream(pid);
                    }
                }
            }
            if !decoded_frame {
                break;
            }
        }
    }

    fn schedule_retry(&mut self, now: Instant) {
        let seconds = DISCOVERY_BACKOFF_SECONDS[self.backoff_index];
        self.backoff_index = (self.backoff_index + 1).min(DISCOVERY_BACKOFF_SECONDS.len() - 1);
        self.next_discovery = Some(now + Duration::from_secs(seconds));
    }

    pub(super) fn telemetry(&self, pid: u32) -> Option<PiLiveHarnessTelemetry> {
        let observation = self.observations.get(&pid)?;
        match observation.health {
            PrivateObservationHealth::Healthy => Some(PiLiveHarnessTelemetry {
                phase: observation.phase.map(Into::into),
                pending_messages: observation.pending_messages,
                source_health: SourceHealth::Healthy,
                provenance: PiLiveHarnessProvenance::ExtensionAfUnixV1,
                observed_at_ms: observation.observed_at_ms,
                stale: false,
                reason: None,
            }),
            PrivateObservationHealth::Stale => Some(PiLiveHarnessTelemetry {
                phase: None,
                pending_messages: None,
                source_health: SourceHealth::Stale,
                provenance: PiLiveHarnessProvenance::ExtensionAfUnixV1,
                observed_at_ms: observation.observed_at_ms,
                stale: true,
                reason: Some("live harness observation expired".to_string()),
            }),
            PrivateObservationHealth::Unavailable => None,
        }
    }

    #[cfg(test)]
    pub(super) fn inject_healthy_for_test(
        &mut self,
        attachment: VerifiedSidecarAttachment,
        observed_at_ms: u64,
    ) {
        self.observations.insert(
            attachment.pid,
            PrivateLiveObservation {
                attachment,
                health: PrivateObservationHealth::Healthy,
                epoch: Some("00000000000000000000000000000000".to_string()),
                sequence: Some(0),
                phase: None,
                pending_messages: None,
                received_at: Some(self.backend.now()),
                observed_at_ms: Some(observed_at_ms),
                stale_tick: None,
            },
        );
    }

    #[cfg(test)]
    fn accepted_pids(&self) -> Vec<u32> {
        self.accepted.keys().copied().collect()
    }

    #[cfg(test)]
    fn accepted_sequence(&self, pid: u32) -> Option<u64> {
        self.accepted.get(&pid).map(|stream| stream.latest.sequence)
    }

    #[cfg(test)]
    fn pending_sequence(&self, pid: u32) -> Option<u64> {
        self.pending.get(&pid).map(|pending| pending.sequence)
    }

    #[cfg(test)]
    fn pending_pids(&self) -> Vec<u32> {
        self.pending.keys().copied().collect()
    }

    #[cfg(test)]
    fn private_health(&self, pid: u32) -> PrivateObservationHealth {
        self.observations
            .get(&pid)
            .map_or(PrivateObservationHealth::Unavailable, |observation| {
                observation.health
            })
    }

    #[cfg(test)]
    fn private_sequence(&self, pid: u32) -> Option<u64> {
        self.observations
            .get(&pid)
            .and_then(|observation| observation.sequence)
    }

    #[cfg(test)]
    fn private_observed_at_ms(&self, pid: u32) -> Option<u64> {
        self.observations
            .get(&pid)
            .and_then(|observation| observation.observed_at_ms)
    }

    #[cfg(test)]
    fn next_discovery_delay(&self, now: Instant) -> Option<Duration> {
        self.next_discovery
            .and_then(|next| next.checked_duration_since(now))
    }

    #[cfg(test)]
    fn previous_epoch(&self, attachment: &VerifiedSidecarAttachment) -> Option<&str> {
        self.previous_epochs.get(attachment).map(String::as_str)
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn read_registry_frame(
    stream: &mut dyn RegistryStream,
    decoder: &mut FrameDecoder,
    protocol: &mut ProtocolConnection,
    partial_deadline: &mut Option<Instant>,
    budget: &mut RegistryTickBudget,
    now: Instant,
) -> Result<Option<LiveFrame>, ()> {
    if partial_deadline.is_some_and(|deadline| now >= deadline) && !decoder.has_complete_frame() {
        return Err(());
    }
    match decoder.next() {
        Ok(Some(frame)) => {
            protocol.accept(frame.clone()).map_err(|_| ())?;
            budget.frames += 1;
            *partial_deadline = if decoder.buffer.is_empty() {
                None
            } else {
                Some(partial_deadline.unwrap_or(now + STREAM_DEADLINE))
            };
            return Ok(Some(frame));
        }
        Ok(None) => {}
        Err(_) => return Err(()),
    }
    let remaining = MAX_BYTES_PER_TICK.saturating_sub(budget.bytes);
    if remaining == 0 || budget.frames == MAX_FRAMES_PER_TICK {
        return Ok(None);
    }
    let mut bytes = [0_u8; MAX_BUFFER_BYTES];
    let maximum = remaining.min(bytes.len());
    let read = match stream.read_into(&mut bytes[..maximum]) {
        Ok(0) => return Err(()),
        Ok(read) => read,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            return Ok(None);
        }
        Err(_) => return Err(()),
    };
    budget.bytes += read;
    decoder.push(&bytes[..read]).map_err(|_| ())?;
    match decoder.next() {
        Ok(Some(frame)) => {
            protocol.accept(frame.clone()).map_err(|_| ())?;
            budget.frames += 1;
            *partial_deadline = (!decoder.buffer.is_empty()).then_some(now + STREAM_DEADLINE);
            Ok(Some(frame))
        }
        Ok(None) => {
            if partial_deadline.is_none() {
                *partial_deadline = Some(now + STREAM_DEADLINE);
            }
            Ok(None)
        }
        Err(_) => Err(()),
    }
}

#[cfg(test)]
#[path = "pi_live_harness_tests.rs"]
mod tests;

#[cfg(all(test, any(target_os = "linux", target_vendor = "apple")))]
#[path = "pi_live_harness_transport_tests.rs"]
mod transport_tests;
