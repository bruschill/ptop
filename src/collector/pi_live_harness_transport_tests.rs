use super::*;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

fn isolated_runtime() -> (tempfile::TempDir, PathBuf, u32) {
    let base = tempfile::tempdir_in("/tmp").unwrap();
    fs::set_permissions(base.path(), fs::Permissions::from_mode(0o1777)).unwrap();
    let uid = unsafe { libc::geteuid() };
    let runtime = base.path().join(format!("ptop-{uid}"));
    fs::create_dir(&runtime).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    (base, runtime, uid)
}

fn socket_path(runtime: &Path, digit: char) -> PathBuf {
    runtime.join(format!("s-{}.sock", digit.to_string().repeat(24)))
}

fn bind_private_socket(path: &Path) -> UnixListener {
    let listener = UnixListener::bind(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    listener
}

#[test]
fn discovery_returns_complete_sorted_identity_set_and_honors_exclusions() {
    let (base, runtime, uid) = isolated_runtime();
    let later_path = socket_path(&runtime, 'b');
    let earlier_path = socket_path(&runtime, 'a');
    let _later = bind_private_socket(&later_path);
    let _earlier = bind_private_socket(&earlier_path);
    fs::write(runtime.join("unrelated"), b"ignored").unwrap();

    let candidates =
        enumerate_runtime_candidates_at(base.path(), uid, uid, &IdentitySet::new()).unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].path, earlier_path);
    assert_eq!(candidates[1].path, later_path);
    assert_ne!(candidates[0].inode, 0);

    let excluded = IdentitySet::from([candidates[0].clone()]);
    let remaining = enumerate_runtime_candidates_at(base.path(), uid, uid, &excluded).unwrap();
    assert_eq!(remaining, vec![candidates[1].clone()]);
}

#[test]
fn discovery_reads_entry_sixty_five_only_to_prove_exhaustion() {
    let (base, runtime, uid) = isolated_runtime();
    for index in 0..MAX_SOCKET_CANDIDATES {
        fs::write(runtime.join(format!("entry-{index:02}")), b"").unwrap();
    }
    assert!(
        enumerate_runtime_candidates_at(base.path(), uid, uid, &IdentitySet::new())
            .unwrap()
            .is_empty()
    );

    fs::write(runtime.join("entry-64"), b"").unwrap();
    assert_eq!(
        enumerate_runtime_candidates_at(base.path(), uid, uid, &IdentitySet::new()),
        Err(DiscoveryError::Exhausted)
    );
}

#[test]
fn matching_symlink_wrong_mode_and_unsafe_directories_fail_closed() {
    let (base, runtime, uid) = isolated_runtime();
    let victim = runtime.join("victim");
    fs::write(&victim, b"unchanged").unwrap();
    symlink(&victim, socket_path(&runtime, 'a')).unwrap();
    assert_eq!(
        enumerate_runtime_candidates_at(base.path(), uid, uid, &IdentitySet::new()),
        Err(DiscoveryError::Unavailable)
    );
    assert_eq!(fs::read(&victim).unwrap(), b"unchanged");

    fs::remove_file(socket_path(&runtime, 'a')).unwrap();
    let socket = socket_path(&runtime, 'b');
    let _listener = bind_private_socket(&socket);
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o660)).unwrap();
    assert_eq!(
        enumerate_runtime_candidates_at(base.path(), uid, uid, &IdentitySet::new()),
        Err(DiscoveryError::Unavailable)
    );

    fs::set_permissions(&socket, fs::Permissions::from_mode(0o4600)).unwrap();
    assert_eq!(
        enumerate_runtime_candidates_at(base.path(), uid, uid, &IdentitySet::new()),
        Err(DiscoveryError::Unavailable)
    );

    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        enumerate_runtime_candidates_at(base.path(), uid, uid, &IdentitySet::new()),
        Err(DiscoveryError::Unavailable)
    );

    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(base.path(), fs::Permissions::from_mode(0o0777)).unwrap();
    assert_eq!(
        enumerate_runtime_candidates_at(base.path(), uid, uid, &IdentitySet::new()),
        Err(DiscoveryError::Unavailable)
    );
}

#[test]
fn generation_rejects_reenumeration_mutation() {
    let (base, runtime, uid) = isolated_runtime();
    let first = socket_path(&runtime, 'a');
    let _first_listener = bind_private_socket(&first);
    let baseline =
        enumerate_runtime_candidates_at(base.path(), uid, uid, &IdentitySet::new()).unwrap();
    let generation = DiscoveryGeneration::new(baseline.clone());
    assert!(generation.is_unchanged(&baseline));

    let second = socket_path(&runtime, 'b');
    let _second_listener = bind_private_socket(&second);
    let changed =
        enumerate_runtime_candidates_at(base.path(), uid, uid, &IdentitySet::new()).unwrap();
    assert!(!generation.is_unchanged(&changed));
}

#[test]
fn nonblocking_connect_revalidates_identity_and_authenticates_peer_pid() {
    let (base, runtime, uid) = isolated_runtime();
    let path = socket_path(&runtime, 'a');
    let listener = bind_private_socket(&path);
    let candidate = enumerate_runtime_candidates_at(base.path(), uid, uid, &IdentitySet::new())
        .unwrap()
        .pop()
        .unwrap();
    let (accepted_tx, accepted_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let accepted = listener.accept().unwrap().0;
        accepted_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        drop(accepted);
    });

    let mut stream = UnixLiveHarnessStream::connect(&candidate).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while !stream.connect_ready().unwrap() {
        assert!(Instant::now() < deadline, "nonblocking connect timed out");
        thread::sleep(Duration::from_millis(1));
    }
    accepted_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(stream.peer_pid().unwrap(), std::process::id());
    let status_flags = unsafe { libc::fcntl(stream.stream.as_raw_fd(), libc::F_GETFL) };
    let descriptor_flags = unsafe { libc::fcntl(stream.stream.as_raw_fd(), libc::F_GETFD) };
    assert_ne!(status_flags & libc::O_NONBLOCK, 0);
    assert_ne!(descriptor_flags & libc::FD_CLOEXEC, 0);

    release_tx.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn connect_refuses_mode_or_inode_change_after_enumeration() {
    let (base, runtime, uid) = isolated_runtime();
    let path = socket_path(&runtime, 'a');
    let listener = bind_private_socket(&path);
    let candidate = enumerate_runtime_candidates_at(base.path(), uid, uid, &IdentitySet::new())
        .unwrap()
        .pop()
        .unwrap();

    fs::set_permissions(&path, fs::Permissions::from_mode(0o4600)).unwrap();
    assert!(UnixLiveHarnessStream::connect(&candidate).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

    drop(listener);
    fs::remove_file(&path).unwrap();
    let _replacement = bind_private_socket(&path);
    assert!(UnixLiveHarnessStream::connect(&candidate).is_err());
}

#[test]
fn fixed_name_and_path_bounds_match_protocol_contract() {
    assert!(runtime_root_fits(Path::new(&format!(
        "/{}",
        "r".repeat(70)
    ))));
    assert!(!runtime_root_fits(Path::new(&format!(
        "/{}",
        "r".repeat(71)
    ))));
    assert!(socket_path_fits(Path::new(&format!(
        "/{}",
        "s".repeat(102)
    ))));
    assert!(!socket_path_fits(Path::new(&format!(
        "/{}",
        "s".repeat(103)
    ))));

    assert!(is_runtime_socket_name(b"s-0123456789abcdef01234567.sock"));
    assert!(!is_runtime_socket_name(b"s-0123456789ABCDEF01234567.sock"));
    assert!(!is_runtime_socket_name(b"x-0123456789abcdef01234567.sock"));
    assert_eq!(b"s-0123456789abcdef01234567.sock".len(), 31);

    let (base, runtime, uid) = isolated_runtime();
    assert!(runtime.as_os_str().as_bytes().len() <= MAX_RUNTIME_ROOT_BYTES);
    let path = socket_path(&runtime, 'a');
    assert!(path.as_os_str().as_bytes().len() < MAX_SOCKET_PATH_BYTES_WITH_NUL);
    let _listener = bind_private_socket(&path);
    assert_eq!(
        enumerate_runtime_candidates_at(base.path(), uid, uid, &IdentitySet::new())
            .unwrap()
            .len(),
        1
    );
}

#[derive(Default)]
struct FakeCounts {
    enumerations: Cell<usize>,
    connects: Cell<usize>,
    credentials: Cell<usize>,
    candidate_checks: Cell<usize>,
    reads: Cell<usize>,
    read_bytes: Cell<usize>,
    drops: Cell<usize>,
}

enum FakeRead {
    Data(Vec<u8>),
    WouldBlock,
    Eof,
    Error,
}

struct FakeStreamState {
    ready: bool,
    peer_pid: Option<u32>,
    reads: VecDeque<FakeRead>,
}

struct FakeStream {
    state: Rc<RefCell<FakeStreamState>>,
    counts: Rc<FakeCounts>,
    now: Rc<Cell<Instant>>,
    ready_advance: Rc<Cell<Duration>>,
    credential_advance: Rc<Cell<Duration>>,
}

impl Drop for FakeStream {
    fn drop(&mut self) {
        self.counts.drops.set(self.counts.drops.get() + 1);
    }
}

impl RegistryStream for FakeStream {
    fn connect_ready(&mut self) -> io::Result<bool> {
        self.now.set(self.now.get() + self.ready_advance.get());
        Ok(self.state.borrow().ready)
    }

    fn peer_pid(&self) -> io::Result<u32> {
        self.now.set(self.now.get() + self.credential_advance.get());
        self.counts
            .credentials
            .set(self.counts.credentials.get() + 1);
        self.state
            .borrow()
            .peer_pid
            .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "credentials"))
    }

    fn read_into(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.counts.reads.set(self.counts.reads.get() + 1);
        let Some(read) = self.state.borrow_mut().reads.pop_front() else {
            return Err(io::Error::from(io::ErrorKind::WouldBlock));
        };
        match read {
            FakeRead::Data(mut bytes) => {
                let length = bytes.len().min(buffer.len());
                buffer[..length].copy_from_slice(&bytes[..length]);
                if length < bytes.len() {
                    bytes.drain(..length);
                    self.state
                        .borrow_mut()
                        .reads
                        .push_front(FakeRead::Data(bytes));
                }
                self.counts
                    .read_bytes
                    .set(self.counts.read_bytes.get() + length);
                Ok(length)
            }
            FakeRead::WouldBlock => Err(io::Error::from(io::ErrorKind::WouldBlock)),
            FakeRead::Eof => Ok(0),
            FakeRead::Error => Err(io::Error::other("read")),
        }
    }
}

struct FakeStreamSpec {
    ready: bool,
    peer_pid: Option<u32>,
    reads: VecDeque<FakeRead>,
}

impl FakeStreamSpec {
    fn valid(pid: u32, frame: Vec<u8>) -> Self {
        Self {
            ready: true,
            peer_pid: Some(pid),
            reads: VecDeque::from([FakeRead::Data(frame)]),
        }
    }
}

type FakeEnumerations = Rc<RefCell<VecDeque<Result<Vec<SocketCandidate>, DiscoveryError>>>>;

struct FakeBackend {
    now: Rc<Cell<Instant>>,
    enumerations: FakeEnumerations,
    connections: Rc<RefCell<VecDeque<FakeStreamSpec>>>,
    streams: Rc<RefCell<Vec<Rc<RefCell<FakeStreamState>>>>>,
    counts: Rc<FakeCounts>,
    enumerate_advance: Rc<Cell<Duration>>,
    ready_advance: Rc<Cell<Duration>>,
    credential_advance: Rc<Cell<Duration>>,
    candidate_advance: Rc<Cell<Duration>>,
    invalid_candidates: Rc<RefCell<IdentitySet<SocketCandidate>>>,
}

impl RegistryBackend for FakeBackend {
    fn now(&self) -> Instant {
        self.now.get()
    }

    fn candidate_current(&mut self, candidate: &SocketCandidate) -> bool {
        self.counts
            .candidate_checks
            .set(self.counts.candidate_checks.get() + 1);
        self.now.set(self.now.get() + self.candidate_advance.get());
        !self.invalid_candidates.borrow().contains(candidate)
    }

    fn enumerate(
        &mut self,
        excluded: &IdentitySet<SocketCandidate>,
    ) -> Result<Vec<SocketCandidate>, DiscoveryError> {
        self.counts
            .enumerations
            .set(self.counts.enumerations.get() + 1);
        self.now.set(self.now.get() + self.enumerate_advance.get());
        let mut result = self
            .enumerations
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| Ok(Vec::new()))?;
        result.retain(|candidate| !excluded.contains(candidate));
        Ok(result)
    }

    fn connect(&mut self, _candidate: &SocketCandidate) -> io::Result<Box<dyn RegistryStream>> {
        self.counts.connects.set(self.counts.connects.get() + 1);
        let spec = self
            .connections
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| io::Error::new(io::ErrorKind::ConnectionRefused, "connect"))?;
        let state = Rc::new(RefCell::new(FakeStreamState {
            ready: spec.ready,
            peer_pid: spec.peer_pid,
            reads: spec.reads,
        }));
        self.streams.borrow_mut().push(state.clone());
        Ok(Box::new(FakeStream {
            state,
            counts: self.counts.clone(),
            now: self.now.clone(),
            ready_advance: self.ready_advance.clone(),
            credential_advance: self.credential_advance.clone(),
        }))
    }
}

struct FakeControl {
    now: Rc<Cell<Instant>>,
    enumerations: FakeEnumerations,
    connections: Rc<RefCell<VecDeque<FakeStreamSpec>>>,
    streams: Rc<RefCell<Vec<Rc<RefCell<FakeStreamState>>>>>,
    counts: Rc<FakeCounts>,
    enumerate_advance: Rc<Cell<Duration>>,
    ready_advance: Rc<Cell<Duration>>,
    credential_advance: Rc<Cell<Duration>>,
    candidate_advance: Rc<Cell<Duration>>,
    invalid_candidates: Rc<RefCell<IdentitySet<SocketCandidate>>>,
}

impl FakeControl {
    fn advance(&self, duration: Duration) {
        self.now.set(self.now.get() + duration);
    }

    fn push_enumeration(&self, result: Result<Vec<SocketCandidate>, DiscoveryError>) {
        self.enumerations.borrow_mut().push_back(result);
    }

    fn push_connection(&self, spec: FakeStreamSpec) {
        self.connections.borrow_mut().push_back(spec);
    }

    fn push_stream_read(&self, index: usize, read: FakeRead) {
        self.streams.borrow()[index]
            .borrow_mut()
            .reads
            .push_back(read);
    }

    fn set_peer_pid(&self, index: usize, peer_pid: Option<u32>) {
        self.streams.borrow()[index].borrow_mut().peer_pid = peer_pid;
    }

    fn invalidate_candidate(&self, candidate: SocketCandidate) {
        self.invalid_candidates.borrow_mut().insert(candidate);
    }
}

fn fake_registry() -> (SidecarRegistry, FakeControl) {
    let started = Instant::now();
    let now = Rc::new(Cell::new(started));
    let enumerations = Rc::new(RefCell::new(VecDeque::new()));
    let connections = Rc::new(RefCell::new(VecDeque::new()));
    let streams = Rc::new(RefCell::new(Vec::new()));
    let counts = Rc::new(FakeCounts::default());
    let enumerate_advance = Rc::new(Cell::new(Duration::ZERO));
    let ready_advance = Rc::new(Cell::new(Duration::ZERO));
    let credential_advance = Rc::new(Cell::new(Duration::ZERO));
    let candidate_advance = Rc::new(Cell::new(Duration::ZERO));
    let invalid_candidates = Rc::new(RefCell::new(IdentitySet::new()));
    let backend = FakeBackend {
        now: now.clone(),
        enumerations: enumerations.clone(),
        connections: connections.clone(),
        streams: streams.clone(),
        counts: counts.clone(),
        enumerate_advance: enumerate_advance.clone(),
        ready_advance: ready_advance.clone(),
        credential_advance: credential_advance.clone(),
        candidate_advance: candidate_advance.clone(),
        invalid_candidates: invalid_candidates.clone(),
    };
    (
        SidecarRegistry::with_backend(Box::new(backend)),
        FakeControl {
            now,
            enumerations,
            connections,
            streams,
            counts,
            enumerate_advance,
            ready_advance,
            credential_advance,
            candidate_advance,
            invalid_candidates,
        },
    )
}

fn candidate(index: u64) -> SocketCandidate {
    SocketCandidate {
        path: PathBuf::from(format!("/tmp/ptop-501/s-{index:024x}.sock")),
        device: 1,
        inode: index + 100,
    }
}

fn attachment(pid: u32, start_id: &str, session_id: &str) -> VerifiedSidecarAttachment {
    VerifiedSidecarAttachment::new(pid, start_id, session_id)
}

fn epoch_digit(index: usize) -> char {
    char::from(b"0123456789abcdef"[index % 16])
}

fn wire_frame(epoch_digit: char, sequence: u64, session_id: &str, phase: Option<&str>) -> Vec<u8> {
    let body = serde_json::json!({
        "magic": PROTOCOL_MAGIC,
        "version": PROTOCOL_VERSION,
        "epoch": epoch_digit.to_string().repeat(EPOCH_HEX_BYTES),
        "sequence": sequence,
        "session_id": session_id,
        "phase": phase,
        "pending_messages": if sequence == 0 { serde_json::Value::Null } else { serde_json::Value::Bool(false) },
    });
    let mut body = serde_json::to_vec(&body).unwrap();
    let mut encoded = (body.len() as u32).to_be_bytes().to_vec();
    encoded.append(&mut body);
    encoded
}

fn stage_initial_frame(
    registry: &mut SidecarRegistry,
    control: &FakeControl,
    candidate: SocketCandidate,
    attachment: &VerifiedSidecarAttachment,
    observed_at_ms: u64,
) {
    control.push_enumeration(Ok(vec![candidate.clone()]));
    control.push_enumeration(Ok(vec![candidate]));
    control.push_connection(FakeStreamSpec::valid(
        attachment.pid,
        wire_frame('a', 0, &attachment.session_id, None),
    ));
    registry.poll_at(std::slice::from_ref(attachment), observed_at_ms);
}

#[test]
fn live_frame_is_private_pending_until_next_tick_revalidation() {
    let (mut registry, control) = fake_registry();
    let attachment = attachment(10, "start", "session");
    stage_initial_frame(&mut registry, &control, candidate(1), &attachment, 12_345);

    assert_eq!(registry.pending_sequence(10), Some(0));
    assert_eq!(
        registry.private_health(10),
        PrivateObservationHealth::Unavailable,
        "the read tick cannot commit its frame"
    );
    assert_eq!(control.counts.credentials.get(), 1);

    registry.poll_at(std::slice::from_ref(&attachment), 67_890);
    assert_eq!(
        registry.private_health(10),
        PrivateObservationHealth::Healthy
    );
    assert_eq!(registry.private_sequence(10), Some(0));
    assert_eq!(registry.private_observed_at_ms(10), Some(12_345));
    assert_eq!(registry.pending_sequence(10), None);
    assert_eq!(
        registry.telemetry(10),
        Some(PiLiveHarnessTelemetry {
            phase: None,
            pending_messages: None,
            source_health: SourceHealth::Healthy,
            provenance: PiLiveHarnessProvenance::ExtensionAfUnixV1,
            observed_at_ms: Some(12_345),
            stale: false,
            reason: None,
        })
    );
    assert_eq!(control.counts.candidate_checks.get(), 1);
    assert_eq!(control.counts.credentials.get(), 2);
}

#[test]
fn process_or_attachment_change_discards_pending_frame() {
    for replacement in [
        None,
        Some(attachment(10, "reused-start", "session")),
        Some(attachment(10, "start", "replacement-session")),
    ] {
        let (mut registry, control) = fake_registry();
        let original = attachment(10, "start", "session");
        stage_initial_frame(&mut registry, &control, candidate(1), &original, 1_000);

        let current = replacement.as_slice();
        registry.poll_at(current, 2_000);

        assert!(registry.accepted_pids().is_empty());
        assert_eq!(registry.pending_sequence(10), None);
        assert_eq!(
            registry.private_health(10),
            PrivateObservationHealth::Unavailable
        );
        assert_eq!(control.counts.candidate_checks.get(), 0);
    }
}

#[test]
fn credential_or_socket_change_discards_only_affected_pending_frame() {
    let attachment = attachment(10, "start", "session");

    let (mut credential_registry, credential_control) = fake_registry();
    stage_initial_frame(
        &mut credential_registry,
        &credential_control,
        candidate(1),
        &attachment,
        1_000,
    );
    credential_control.set_peer_pid(0, None);
    credential_registry.poll_at(std::slice::from_ref(&attachment), 2_000);
    assert!(credential_registry.accepted_pids().is_empty());
    assert_eq!(
        credential_registry.private_health(10),
        PrivateObservationHealth::Unavailable
    );

    let (mut socket_registry, socket_control) = fake_registry();
    let socket = candidate(1);
    stage_initial_frame(
        &mut socket_registry,
        &socket_control,
        socket.clone(),
        &attachment,
        1_000,
    );
    socket_control.invalidate_candidate(socket);
    socket_registry.poll_at(std::slice::from_ref(&attachment), 2_000);
    assert!(socket_registry.accepted_pids().is_empty());
    assert_eq!(socket_control.counts.credentials.get(), 1);
    assert_eq!(
        socket_registry.private_health(10),
        PrivateObservationHealth::Unavailable
    );
}

#[test]
fn established_frames_are_staged_with_their_read_tick_timestamp() {
    let (mut registry, control) = fake_registry();
    let attachment = attachment(10, "start", "session");
    stage_initial_frame(&mut registry, &control, candidate(1), &attachment, 1_000);
    control.push_stream_read(
        0,
        FakeRead::Data(wire_frame('a', 1, "session", Some("idle"))),
    );

    registry.poll_at(std::slice::from_ref(&attachment), 2_000);
    assert_eq!(registry.private_sequence(10), Some(0));
    assert_eq!(registry.pending_sequence(10), Some(1));
    assert_eq!(registry.private_observed_at_ms(10), Some(1_000));

    registry.poll_at(std::slice::from_ref(&attachment), 3_000);
    assert_eq!(registry.private_sequence(10), Some(1));
    assert_eq!(registry.private_observed_at_ms(10), Some(2_000));
}

#[test]
fn committed_frame_is_stale_for_one_tick_then_unavailable() {
    let (mut registry, control) = fake_registry();
    let attachment = attachment(10, "start", "session");
    stage_initial_frame(&mut registry, &control, candidate(1), &attachment, 1_000);
    control.push_stream_read(
        0,
        FakeRead::Data(wire_frame('a', 1, "session", Some("generating"))),
    );
    registry.poll_at(std::slice::from_ref(&attachment), 2_000);
    registry.poll_at(std::slice::from_ref(&attachment), 3_000);
    assert_eq!(
        registry.telemetry(10),
        Some(PiLiveHarnessTelemetry {
            phase: Some(PiLivePhase::Generating),
            pending_messages: Some(false),
            source_health: SourceHealth::Healthy,
            provenance: PiLiveHarnessProvenance::ExtensionAfUnixV1,
            observed_at_ms: Some(2_000),
            stale: false,
            reason: None,
        })
    );

    control.advance(Duration::from_secs(3));
    registry.poll_at(std::slice::from_ref(&attachment), 6_000);
    assert_eq!(registry.private_health(10), PrivateObservationHealth::Stale);
    assert_eq!(registry.private_sequence(10), None);
    assert_eq!(
        registry.telemetry(10),
        Some(PiLiveHarnessTelemetry {
            phase: None,
            pending_messages: None,
            source_health: SourceHealth::Stale,
            provenance: PiLiveHarnessProvenance::ExtensionAfUnixV1,
            observed_at_ms: Some(2_000),
            stale: true,
            reason: Some("live harness observation expired".to_string()),
        })
    );

    registry.poll_at(std::slice::from_ref(&attachment), 6_001);
    assert_eq!(
        registry.private_health(10),
        PrivateObservationHealth::Unavailable
    );
    assert_eq!(registry.telemetry(10), None);
}

#[test]
fn pending_capacity_and_priority_give_eight_streams_fair_credential_turns() {
    let (mut registry, control) = fake_registry();
    let candidates: Vec<_> = (1..=8).map(candidate).collect();
    control.push_enumeration(Ok(candidates.clone()));
    control.push_enumeration(Ok(candidates));
    let attachments: Vec<_> = (0..8)
        .map(|index| attachment(10 + index, &format!("start-{index}"), &format!("s-{index}")))
        .collect();
    for (index, attachment) in attachments.iter().enumerate() {
        control.push_connection(FakeStreamSpec::valid(
            attachment.pid,
            wire_frame(epoch_digit(index), 0, &attachment.session_id, None),
        ));
    }

    registry.poll(&attachments);
    registry.poll(&attachments);
    assert_eq!(registry.pending_pids(), vec![10, 11, 12, 13]);
    for (index, attachment) in attachments.iter().enumerate() {
        control.push_stream_read(
            index,
            FakeRead::Data(wire_frame(
                epoch_digit(index),
                1,
                &attachment.session_id,
                Some("idle"),
            )),
        );
    }

    let credentials_before = control.counts.credentials.get();
    registry.poll(&attachments);
    assert_eq!(
        control.counts.credentials.get() - credentials_before,
        MAX_CREDENTIALS_PER_TICK
    );
    assert_eq!(registry.pending_pids(), vec![14, 15, 16, 17]);

    let credentials_before = control.counts.credentials.get();
    registry.poll(&attachments);
    assert_eq!(
        control.counts.credentials.get() - credentials_before,
        MAX_CREDENTIALS_PER_TICK
    );
    assert_eq!(registry.pending_pids(), vec![10, 11, 12, 13]);

    registry.poll(&attachments);
    assert!(registry.pending_pids().is_empty());
    for attachment in attachments {
        assert_eq!(
            registry.private_health(attachment.pid),
            PrivateObservationHealth::Healthy
        );
        assert_eq!(registry.private_sequence(attachment.pid), Some(1));
    }
}

#[test]
fn sustained_pending_commits_reserve_a_credential_turn_for_discovery() {
    let (mut registry, control) = fake_registry();
    let first_candidates: Vec<_> = (1..=4).map(candidate).collect();
    control.push_enumeration(Ok(first_candidates.clone()));
    control.push_enumeration(Ok(first_candidates));
    let mut attachments: Vec<_> = (0..4)
        .map(|index| attachment(10 + index, &format!("start-{index}"), &format!("s-{index}")))
        .collect();
    for (index, attachment) in attachments.iter().enumerate() {
        control.push_connection(FakeStreamSpec::valid(
            attachment.pid,
            wire_frame(epoch_digit(index), 0, &attachment.session_id, None),
        ));
    }
    registry.poll_at(&attachments, 0);
    for (index, attachment) in attachments.iter().enumerate() {
        control.push_stream_read(
            index,
            FakeRead::Data(wire_frame(
                epoch_digit(index),
                1,
                &attachment.session_id,
                Some("idle"),
            )),
        );
    }
    control.advance(Duration::from_secs(2));
    registry.poll_at(&attachments, 2_000);

    let fifth = attachment(20, "start-5", "s-5");
    attachments.push(fifth.clone());
    control.push_enumeration(Ok(vec![candidate(5)]));
    control.push_enumeration(Ok(vec![candidate(5)]));
    control.push_connection(FakeStreamSpec::valid(
        fifth.pid,
        wire_frame('e', 0, &fifth.session_id, None),
    ));
    for (index, attachment) in attachments[..4].iter().enumerate() {
        control.push_stream_read(
            index,
            FakeRead::Data(wire_frame(
                epoch_digit(index),
                2,
                &attachment.session_id,
                Some("idle"),
            )),
        );
    }

    control.advance(Duration::from_secs(2));
    let credentials_before = control.counts.credentials.get();
    registry.poll_at(&attachments, 4_000);
    assert_eq!(
        control.counts.credentials.get() - credentials_before,
        MAX_CREDENTIALS_PER_TICK
    );
    assert_eq!(registry.accepted_pids(), vec![10, 11, 12, 13, 20]);
    assert_eq!(control.counts.connects.get(), 5);
}

#[test]
fn complete_buffered_heartbeats_survive_eight_stream_two_second_cadence() {
    let (mut registry, control) = fake_registry();
    let candidates: Vec<_> = (1..=8).map(candidate).collect();
    control.push_enumeration(Ok(candidates.clone()));
    control.push_enumeration(Ok(candidates));
    let attachments: Vec<_> = (0..8)
        .map(|index| attachment(10 + index, &format!("start-{index}"), &format!("s-{index}")))
        .collect();
    for (index, attachment) in attachments.iter().enumerate() {
        control.push_connection(FakeStreamSpec::valid(
            attachment.pid,
            wire_frame(epoch_digit(index), 0, &attachment.session_id, None),
        ));
    }
    registry.poll_at(&attachments, 0);
    control.advance(Duration::from_secs(2));
    registry.poll_at(&attachments, 2_000);
    assert_eq!(registry.accepted_pids().len(), 8);

    for (index, attachment) in attachments.iter().enumerate() {
        let mut combined = Vec::new();
        for sequence in 1..=5 {
            combined.extend_from_slice(&wire_frame(
                epoch_digit(index),
                sequence,
                &attachment.session_id,
                Some("idle"),
            ));
        }
        control.push_stream_read(index, FakeRead::Data(combined));
    }

    control.advance(Duration::from_secs(2));
    registry.poll_at(&attachments, 4_000);
    assert_eq!(registry.accepted_pids().len(), 8);
    assert_eq!(registry.pending_pids(), vec![10, 11, 12, 13]);
    for pid in 10..14 {
        assert_eq!(registry.accepted_sequence(pid), Some(4));
    }

    control.advance(Duration::from_secs(2));
    registry.poll_at(&attachments, 6_000);
    assert_eq!(registry.accepted_pids().len(), 8);
    assert_eq!(registry.pending_pids(), vec![14, 15, 16, 17]);
    for pid in 10..18 {
        assert_eq!(registry.accepted_sequence(pid), Some(4));
    }

    control.advance(Duration::from_secs(2));
    registry.poll_at(&attachments, 8_000);
    assert_eq!(registry.accepted_pids().len(), 8);
    for pid in 10..14 {
        assert_eq!(registry.accepted_sequence(pid), Some(5));
        assert_eq!(registry.pending_sequence(pid), Some(5));
    }
    for pid in 14..18 {
        assert_eq!(
            registry.private_health(pid),
            PrivateObservationHealth::Healthy
        );
        assert_eq!(registry.private_sequence(pid), Some(4));
    }
}

#[test]
fn registry_commits_only_after_complete_unchanged_generation() {
    let (mut registry, control) = fake_registry();
    let candidates = vec![
        candidate(1),
        candidate(2),
        candidate(3),
        candidate(4),
        candidate(5),
    ];
    control.push_enumeration(Ok(candidates.clone()));
    control.push_enumeration(Ok(candidates.clone()));
    let attachments: Vec<_> = (0..5)
        .map(|index| {
            attachment(
                10 + index,
                &format!("start-{index}"),
                &format!("session-{index}"),
            )
        })
        .collect();
    for (index, attachment) in attachments.iter().enumerate() {
        control.push_connection(FakeStreamSpec::valid(
            attachment.pid,
            wire_frame(epoch_digit(index), 0, &attachment.session_id, None),
        ));
    }

    registry.poll(&attachments);
    assert!(
        registry.accepted_pids().is_empty(),
        "partial generation cannot commit"
    );
    assert_eq!(control.counts.connects.get(), MAX_CONNECTS_PER_TICK);
    assert_eq!(control.counts.credentials.get(), MAX_CREDENTIALS_PER_TICK);

    registry.poll(&attachments);
    assert_eq!(registry.accepted_pids(), vec![10, 11, 12, 13, 14]);
    assert_eq!(control.counts.enumerations.get(), 2);
}

#[test]
fn registry_rejects_changed_generation_wrong_identity_and_duplicate_matches() {
    let attachment = attachment(10, "start", "session");

    let (mut changed, changed_control) = fake_registry();
    changed_control.push_enumeration(Ok(vec![candidate(1)]));
    changed_control.push_enumeration(Ok(vec![candidate(1), candidate(2)]));
    changed_control.push_connection(FakeStreamSpec::valid(
        10,
        wire_frame('a', 0, "session", None),
    ));
    changed.poll(std::slice::from_ref(&attachment));
    assert!(changed.accepted_pids().is_empty());

    let (mut wrong, wrong_control) = fake_registry();
    wrong_control.push_enumeration(Ok(vec![candidate(1), candidate(2), candidate(3)]));
    wrong_control.push_enumeration(Ok(vec![candidate(1), candidate(2), candidate(3)]));
    wrong_control.push_connection(FakeStreamSpec::valid(
        99,
        wire_frame('a', 0, "session", None),
    ));
    wrong_control.push_connection(FakeStreamSpec::valid(
        10,
        wire_frame('b', 0, "wrong-session", None),
    ));
    wrong_control.push_connection(FakeStreamSpec {
        ready: true,
        peer_pid: None,
        reads: VecDeque::new(),
    });
    wrong.poll(std::slice::from_ref(&attachment));
    assert!(wrong.accepted_pids().is_empty());

    let (mut duplicate, duplicate_control) = fake_registry();
    duplicate_control.push_enumeration(Ok(vec![candidate(1), candidate(2)]));
    duplicate_control.push_enumeration(Ok(vec![candidate(1), candidate(2)]));
    duplicate_control.push_connection(FakeStreamSpec::valid(
        10,
        wire_frame('a', 0, "session", None),
    ));
    duplicate_control.push_connection(FakeStreamSpec::valid(
        10,
        wire_frame('b', 0, "session", None),
    ));
    duplicate.poll(&[attachment]);
    assert!(duplicate.accepted_pids().is_empty());
}

#[test]
fn failed_connects_still_stop_at_the_global_attempt_budget() {
    let (mut registry, control) = fake_registry();
    let candidates: Vec<_> = (1..=5).map(candidate).collect();
    control.push_enumeration(Ok(candidates.clone()));
    control.push_enumeration(Ok(candidates));
    let attachment = attachment(10, "start", "session");

    registry.poll(std::slice::from_ref(&attachment));
    assert_eq!(control.counts.connects.get(), MAX_CONNECTS_PER_TICK);
    registry.poll(&[attachment]);
    assert_eq!(control.counts.connects.get(), MAX_CONNECTS_PER_TICK + 1);
}

#[test]
fn exhausted_credential_budget_does_not_start_self_expiring_connections() {
    let (mut registry, control) = fake_registry();
    let candidates: Vec<_> = (1..=8).map(candidate).collect();
    control.push_enumeration(Ok(candidates));
    let attachments: Vec<_> = (0..8)
        .map(|index| {
            attachment(
                30 + index,
                &format!("start-{index}"),
                &format!("session-{index}"),
            )
        })
        .collect();
    for (index, attachment) in attachments.iter().enumerate() {
        control.push_connection(FakeStreamSpec {
            ready: false,
            peer_pid: Some(attachment.pid),
            reads: VecDeque::from([FakeRead::Data(wire_frame(
                epoch_digit(index),
                0,
                &attachment.session_id,
                None,
            ))]),
        });
    }

    registry.poll(&attachments);
    assert_eq!(control.counts.connects.get(), 4);
    for stream in control.streams.borrow().iter() {
        stream.borrow_mut().ready = true;
    }
    registry.poll(&attachments);
    assert_eq!(control.counts.credentials.get(), 4);
    assert_eq!(
        control.counts.connects.get(),
        4,
        "no new connection starts after the credential budget is consumed"
    );
    registry.poll(&attachments);
    assert_eq!(control.counts.connects.get(), 8);
}

#[test]
fn registry_enforces_descriptor_connect_credential_and_frame_budgets() {
    let (mut registry, control) = fake_registry();
    let candidates: Vec<_> = (1..=9).map(candidate).collect();
    control.push_enumeration(Ok(candidates.clone()));
    let attachments: Vec<_> = (0..9)
        .map(|index| {
            attachment(
                20 + index,
                &format!("start-{index}"),
                &format!("session-{index}"),
            )
        })
        .collect();
    for (index, attachment) in attachments.iter().enumerate() {
        control.push_connection(FakeStreamSpec::valid(
            attachment.pid,
            wire_frame(epoch_digit(index), 0, &attachment.session_id, None),
        ));
    }

    registry.poll(&attachments);
    assert_eq!(control.counts.connects.get(), 4);
    assert_eq!(control.counts.credentials.get(), 4);
    registry.poll(&attachments);
    assert_eq!(control.counts.connects.get(), 8);
    assert_eq!(control.counts.credentials.get(), 8);
    registry.poll(&attachments);
    assert!(registry.accepted_pids().is_empty());
    assert_eq!(control.counts.connects.get(), MAX_REGISTRY_DESCRIPTORS);
    assert_eq!(control.counts.drops.get(), MAX_REGISTRY_DESCRIPTORS);

    let (mut frames, frame_control) = fake_registry();
    let eight_candidates: Vec<_> = (1..=8).map(candidate).collect();
    frame_control.push_enumeration(Ok(eight_candidates.clone()));
    frame_control.push_enumeration(Ok(eight_candidates));
    let eight_attachments: Vec<_> = (0..8)
        .map(|index| attachment(40 + index, &format!("s-{index}"), &format!("q-{index}")))
        .collect();
    for (index, attachment) in eight_attachments.iter().enumerate() {
        frame_control.push_connection(FakeStreamSpec::valid(
            attachment.pid,
            wire_frame(epoch_digit(index), 0, &attachment.session_id, None),
        ));
    }
    frames.poll(&eight_attachments);
    frames.poll(&eight_attachments);
    assert_eq!(frames.accepted_pids().len(), 8);
    frames.poll(&eight_attachments);
    frames.poll(&eight_attachments);
    let reads_before = frame_control.counts.reads.get();
    for (index, attachment) in eight_attachments.iter().enumerate() {
        frame_control.push_stream_read(
            index,
            FakeRead::Data(wire_frame(
                epoch_digit(index),
                1,
                &attachment.session_id,
                Some("idle"),
            )),
        );
        frame_control.push_stream_read(
            index,
            FakeRead::Data(wire_frame(
                epoch_digit(index),
                2,
                &attachment.session_id,
                Some("idle"),
            )),
        );
        frame_control.push_stream_read(
            index,
            FakeRead::Data(wire_frame(
                epoch_digit(index),
                3,
                &attachment.session_id,
                Some("idle"),
            )),
        );
        frame_control.push_stream_read(
            index,
            FakeRead::Data(wire_frame(
                epoch_digit(index),
                4,
                &attachment.session_id,
                Some("idle"),
            )),
        );
    }
    frames.poll(&eight_attachments);
    assert_eq!(
        frame_control.counts.reads.get() - reads_before,
        MAX_FRAMES_PER_TICK
    );
    assert_eq!(
        eight_attachments
            .iter()
            .filter(|attachment| frames.accepted_sequence(attachment.pid) == Some(4))
            .count(),
        MAX_PENDING_LIVE_FRAMES
    );
}

#[test]
fn registry_stops_reads_at_the_global_eight_kibibyte_budget() {
    let (mut registry, control) = fake_registry();
    let long_session = (1..=MAX_SESSION_ID_BYTES)
        .rev()
        .map(|length| "\"".repeat(length))
        .find(|session| {
            wire_frame('a', 1, session, Some("waiting_for_user")).len() <= MAX_FRAME_BYTES + 4
        })
        .unwrap();
    let later_length = wire_frame('a', 1, &long_session, Some("waiting_for_user")).len();
    assert!(later_length * MAX_FRAMES_PER_TICK > MAX_BYTES_PER_TICK);

    let candidates: Vec<_> = (1..=8).map(candidate).collect();
    control.push_enumeration(Ok(candidates.clone()));
    control.push_enumeration(Ok(candidates));
    let attachments: Vec<_> = (0..8)
        .map(|index| attachment(70 + index, &format!("start-{index}"), &long_session))
        .collect();
    for (index, attachment) in attachments.iter().enumerate() {
        control.push_connection(FakeStreamSpec::valid(
            attachment.pid,
            wire_frame(epoch_digit(index), 0, &attachment.session_id, None),
        ));
    }
    registry.poll(&attachments);
    registry.poll(&attachments);
    assert_eq!(registry.accepted_pids().len(), 8);
    registry.poll(&attachments);
    registry.poll(&attachments);

    for (index, attachment) in attachments.iter().enumerate() {
        for sequence in 1..=4 {
            control.push_stream_read(
                index,
                FakeRead::Data(wire_frame(
                    epoch_digit(index),
                    sequence,
                    &attachment.session_id,
                    Some("waiting_for_user"),
                )),
            );
        }
    }
    let bytes_before = control.counts.read_bytes.get();
    registry.poll(&attachments);
    assert_eq!(
        control.counts.read_bytes.get() - bytes_before,
        MAX_BYTES_PER_TICK
    );
    assert!(
        attachments[..MAX_PENDING_LIVE_FRAMES]
            .iter()
            .any(|attachment| registry.accepted_sequence(attachment.pid) != Some(4)),
        "the byte cap stops the last otherwise-valid frame"
    );
}

#[test]
fn registry_rechecks_elapsed_budget_between_transport_operations() {
    let attachment = attachment(10, "start", "session");

    let (mut readiness, readiness_control) = fake_registry();
    readiness_control.push_enumeration(Ok(vec![candidate(1)]));
    readiness_control.push_enumeration(Ok(vec![candidate(1)]));
    readiness_control.push_connection(FakeStreamSpec::valid(
        10,
        wire_frame('a', 0, "session", None),
    ));
    readiness_control
        .ready_advance
        .set(REGISTRY_WORK_BUDGET + Duration::from_millis(1));
    readiness.poll(std::slice::from_ref(&attachment));
    assert_eq!(readiness_control.counts.credentials.get(), 0);
    assert_eq!(readiness_control.counts.reads.get(), 0);

    let (mut credentials, credentials_control) = fake_registry();
    credentials_control.push_enumeration(Ok(vec![candidate(1)]));
    credentials_control.push_enumeration(Ok(vec![candidate(1)]));
    credentials_control.push_connection(FakeStreamSpec::valid(
        10,
        wire_frame('a', 0, "session", None),
    ));
    credentials_control
        .credential_advance
        .set(REGISTRY_WORK_BUDGET + Duration::from_millis(1));
    credentials.poll(std::slice::from_ref(&attachment));
    assert_eq!(credentials_control.counts.credentials.get(), 1);
    assert_eq!(credentials_control.counts.reads.get(), 0);

    let (mut candidate_check, candidate_control) = fake_registry();
    stage_initial_frame(
        &mut candidate_check,
        &candidate_control,
        candidate(1),
        &attachment,
        1_000,
    );
    candidate_control
        .candidate_advance
        .set(REGISTRY_WORK_BUDGET + Duration::from_millis(1));
    candidate_check.poll_at(std::slice::from_ref(&attachment), 2_000);
    assert_eq!(candidate_check.pending_sequence(10), Some(0));
    assert_eq!(
        candidate_check.private_health(10),
        PrivateObservationHealth::Unavailable
    );
    assert_eq!(candidate_control.counts.credentials.get(), 1);

    let (mut credential_check, credential_check_control) = fake_registry();
    stage_initial_frame(
        &mut credential_check,
        &credential_check_control,
        candidate(1),
        &attachment,
        1_000,
    );
    credential_check_control
        .credential_advance
        .set(REGISTRY_WORK_BUDGET + Duration::from_millis(1));
    credential_check.poll_at(std::slice::from_ref(&attachment), 2_000);
    assert_eq!(credential_check.pending_sequence(10), Some(0));
    assert_eq!(
        credential_check.private_health(10),
        PrivateObservationHealth::Unavailable
    );
    assert_eq!(credential_check_control.counts.credentials.get(), 2);
}

#[test]
fn registry_enforces_elapsed_connect_and_partial_frame_deadlines() {
    let attachment = attachment(10, "start", "session");
    let (mut elapsed, elapsed_control) = fake_registry();
    elapsed_control.push_enumeration(Ok(vec![candidate(1)]));
    elapsed_control
        .enumerate_advance
        .set(REGISTRY_WORK_BUDGET + Duration::from_millis(1));
    elapsed.poll(std::slice::from_ref(&attachment));
    assert_eq!(elapsed_control.counts.connects.get(), 0);
    assert_eq!(
        elapsed.next_discovery_delay(elapsed_control.now.get()),
        Some(Duration::from_secs(1))
    );

    let (mut connecting, connecting_control) = fake_registry();
    connecting_control.push_enumeration(Ok(vec![candidate(1)]));
    connecting_control.push_enumeration(Ok(vec![candidate(1)]));
    connecting_control.push_connection(FakeStreamSpec {
        ready: false,
        peer_pid: Some(10),
        reads: VecDeque::new(),
    });
    connecting.poll(std::slice::from_ref(&attachment));
    connecting_control.advance(STREAM_DEADLINE);
    connecting.poll(std::slice::from_ref(&attachment));
    assert!(connecting.accepted_pids().is_empty());
    assert_eq!(connecting_control.counts.drops.get(), 1);

    let (mut partial, partial_control) = fake_registry();
    partial_control.push_enumeration(Ok(vec![candidate(1)]));
    partial_control.push_enumeration(Ok(vec![candidate(1)]));
    let encoded = wire_frame('a', 0, "session", None);
    partial_control.push_connection(FakeStreamSpec {
        ready: true,
        peer_pid: Some(10),
        reads: VecDeque::from([FakeRead::Data(encoded[..6].to_vec())]),
    });
    partial.poll(std::slice::from_ref(&attachment));
    partial_control.advance(STREAM_DEADLINE);
    partial.poll(&[attachment]);
    assert!(partial.accepted_pids().is_empty());
    assert_eq!(partial_control.counts.drops.get(), 1);
}

#[test]
fn discovery_backoff_is_one_two_four_then_ten_seconds() {
    let (mut registry, control) = fake_registry();
    let attachment = attachment(10, "start", "session");
    for _ in 0..10 {
        control.push_enumeration(Ok(Vec::new()));
    }

    for delay in [1, 2, 4, 10] {
        registry.poll(std::slice::from_ref(&attachment));
        assert_eq!(
            registry.next_discovery_delay(control.now.get()),
            Some(Duration::from_secs(delay))
        );
        let calls = control.counts.enumerations.get();
        control.advance(Duration::from_secs(delay).saturating_sub(Duration::from_millis(1)));
        registry.poll(std::slice::from_ref(&attachment));
        assert_eq!(control.counts.enumerations.get(), calls);
        control.advance(Duration::from_millis(1));
    }
    registry.poll(&[attachment]);
    assert_eq!(
        registry.next_discovery_delay(control.now.get()),
        Some(Duration::from_secs(10))
    );
}

#[test]
fn one_sessions_failure_cannot_close_or_delay_another_sessions_stream() {
    let (mut registry, control) = fake_registry();
    let first_attachment = attachment(10, "start-a", "session-a");
    control.push_enumeration(Ok(vec![candidate(1)]));
    control.push_enumeration(Ok(vec![candidate(1)]));
    control.push_connection(FakeStreamSpec::valid(
        10,
        wire_frame('a', 0, "session-a", None),
    ));
    registry.poll(std::slice::from_ref(&first_attachment));
    assert_eq!(registry.accepted_pids(), vec![10]);

    control.push_stream_read(
        0,
        FakeRead::Data(wire_frame('a', 1, "session-a", Some("idle"))),
    );
    control.push_enumeration(Ok(vec![candidate(2)]));
    control.push_enumeration(Ok(vec![candidate(2)]));
    control.push_connection(FakeStreamSpec::valid(20, vec![0, 0, 0, 0]));
    let second_attachment = attachment(20, "start-b", "session-b");
    registry.poll(&[first_attachment.clone(), second_attachment]);
    assert_eq!(registry.accepted_pids(), vec![10]);
    assert_eq!(registry.accepted_sequence(10), Some(1));
    assert_eq!(
        registry.private_health(10),
        PrivateObservationHealth::Healthy
    );
    assert_eq!(registry.private_sequence(10), Some(0));
    assert_eq!(registry.pending_sequence(10), Some(1));
    assert_eq!(
        registry.private_health(20),
        PrivateObservationHealth::Unavailable
    );

    registry.poll(&[attachment(10, "reused", "session-a")]);
    assert!(
        registry.accepted_pids().is_empty(),
        "PID-reuse identity removes only stale stream"
    );
}

#[test]
fn accepted_stream_eof_or_read_error_closes_only_that_stream() {
    let (mut registry, control) = fake_registry();
    let candidates = vec![candidate(1), candidate(2)];
    control.push_enumeration(Ok(candidates.clone()));
    control.push_enumeration(Ok(candidates));
    let first = attachment(10, "start-a", "session-a");
    let second = attachment(20, "start-b", "session-b");
    control.push_connection(FakeStreamSpec::valid(
        10,
        wire_frame('a', 0, "session-a", None),
    ));
    control.push_connection(FakeStreamSpec::valid(
        20,
        wire_frame('b', 0, "session-b", None),
    ));
    registry.poll(&[first.clone(), second.clone()]);

    control.push_stream_read(0, FakeRead::Eof);
    control.push_stream_read(
        1,
        FakeRead::Data(wire_frame('b', 1, "session-b", Some("idle"))),
    );
    registry.poll(&[first, second.clone()]);
    assert_eq!(registry.accepted_pids(), vec![20]);
    assert_eq!(registry.accepted_sequence(20), Some(1));
    assert_eq!(
        registry.private_health(10),
        PrivateObservationHealth::Unavailable
    );
    assert_eq!(
        registry.private_health(20),
        PrivateObservationHealth::Healthy
    );
    assert_eq!(registry.private_sequence(20), Some(0));
    assert_eq!(registry.pending_sequence(20), Some(1));

    control.push_stream_read(1, FakeRead::Error);
    registry.poll(&[second]);
    assert!(registry.accepted_pids().is_empty());
    assert_eq!(
        registry.private_health(20),
        PrivateObservationHealth::Unavailable
    );
}

#[test]
fn complete_frame_plus_partial_next_frame_keeps_its_deadline() {
    let (mut registry, control) = fake_registry();
    let attachment = attachment(10, "start", "session");
    let socket = candidate(1);
    control.push_enumeration(Ok(vec![socket.clone()]));
    control.push_enumeration(Ok(vec![socket]));
    let mut combined = wire_frame('a', 0, "session", None);
    let later = wire_frame('a', 1, "session", Some("idle"));
    combined.extend_from_slice(&later[..6]);
    control.push_connection(FakeStreamSpec::valid(10, combined));

    registry.poll(std::slice::from_ref(&attachment));
    assert_eq!(registry.accepted_pids(), vec![10]);
    control.advance(STREAM_DEADLINE);
    registry.poll(&[attachment]);
    assert!(
        registry.accepted_pids().is_empty(),
        "partial bytes after a complete frame expire on the next deadline tick"
    );
}

#[test]
fn matched_stream_partial_deadline_expires_before_generation_commit() {
    let (mut registry, control) = fake_registry();
    let attachment = attachment(10, "start", "session");
    let candidates: Vec<_> = (1..=5).map(candidate).collect();
    control.push_enumeration(Ok(candidates.clone()));
    control.push_enumeration(Ok(candidates));
    let mut initial_and_partial = wire_frame('a', 0, "session", None);
    initial_and_partial.extend_from_slice(&wire_frame('a', 1, "session", Some("idle")));
    initial_and_partial.extend_from_slice(&wire_frame('a', 2, "session", Some("idle")));
    let partial = wire_frame('a', 3, "session", Some("idle"));
    initial_and_partial.extend_from_slice(&partial[..6]);
    control.push_connection(FakeStreamSpec::valid(10, initial_and_partial));
    for _ in 0..3 {
        control.push_connection(FakeStreamSpec::valid(99, wire_frame('b', 0, "other", None)));
    }
    control.push_connection(FakeStreamSpec::valid(99, wire_frame('c', 0, "other", None)));

    registry.poll(std::slice::from_ref(&attachment));
    assert!(registry.accepted_pids().is_empty());
    control.advance(STREAM_DEADLINE);
    registry.poll(std::slice::from_ref(&attachment));
    assert!(
        registry.accepted_pids().is_empty(),
        "an expired matched stream cannot commit after the baseline resolves"
    );
    assert_eq!(
        registry.previous_epoch(&attachment),
        Some("a".repeat(32).as_str())
    );
}

#[test]
fn unrelated_attachment_change_preserves_an_in_progress_match() {
    let (mut registry, control) = fake_registry();
    let first = attachment(10, "start-a", "session-a");
    let candidates: Vec<_> = (1..=5).map(candidate).collect();
    control.push_enumeration(Ok(candidates.clone()));
    control.push_enumeration(Ok(candidates));
    control.push_connection(FakeStreamSpec::valid(
        10,
        wire_frame('a', 0, "session-a", None),
    ));
    for _ in 0..4 {
        control.push_connection(FakeStreamSpec::valid(99, wire_frame('b', 0, "other", None)));
    }
    registry.poll(std::slice::from_ref(&first));
    assert!(registry.accepted_pids().is_empty());

    let second = attachment(20, "start-b", "session-b");
    registry.poll(&[first, second]);
    assert_eq!(registry.accepted_pids(), vec![10]);
    assert_eq!(registry.accepted_sequence(10), Some(0));
}
