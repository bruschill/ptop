//! Shared Herdr process-location and command helpers.
//!
//! Pi processes launched in Herdr inherit an opaque pane ID and the server
//! socket that owns it. This module reads those markers without exposing them
//! through snapshots and runs bounded Herdr CLI requests against that socket.

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use serde_json::Value;
#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::io::Read;
#[cfg(any(target_os = "linux", target_vendor = "apple", test))]
use std::path::Path;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::process::Stdio;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const MAX_HERDR_JSON_BYTES: usize = 256 * 1024;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const MAX_HERDR_ENV_BYTES: usize = 256 * 1024;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const HERDR_PRESENCE_COMMAND_TIMEOUT: Duration = Duration::from_millis(1_000);
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const HERDR_PRESENCE_REFRESH_INTERVAL: Duration = Duration::from_secs(10);
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const HERDR_PRESENCE_TTL_MS: u64 = 30_000;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
const PTOP_WORKSPACE_LABEL: &str = "ptop";

#[derive(Default)]
pub(crate) struct WorkspacePresence {
    stop: Option<std::sync::mpsc::Sender<()>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Drop for WorkspacePresence {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspacePresenceTarget {
    binary: std::ffi::OsString,
    socket_path: String,
    workspace_id: String,
    pane_id: String,
}

#[cfg(any(target_os = "linux", target_vendor = "apple", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HerdrProcessMarker {
    pub(crate) pane_id: String,
    pub(crate) socket_path: String,
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
pub(crate) fn run_bounded_herdr_json(
    binary: &std::ffi::OsStr,
    socket_path: &str,
    args: &[String],
    timeout: Duration,
) -> Option<Value> {
    let bytes = run_bounded_herdr(binary, socket_path, args, timeout)?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn run_bounded_herdr_command(
    binary: &std::ffi::OsStr,
    socket_path: &str,
    args: &[String],
    timeout: Duration,
) -> bool {
    run_bounded_herdr(binary, socket_path, args, timeout).is_some()
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn run_bounded_herdr(
    binary: &std::ffi::OsStr,
    socket_path: &str,
    args: &[String],
    timeout: Duration,
) -> Option<Vec<u8>> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    let mut command = std::process::Command::new(binary);
    command
        .args(args)
        .env("HERDR_SOCKET_PATH", socket_path)
        .process_group(0)
        .stdin(Stdio::null())
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
    Some(bytes)
}

pub(crate) fn start_workspace_presence(enabled: bool) -> WorkspacePresence {
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    {
        if enabled {
            if let Some(target) =
                current_workspace_presence_target_from(|name| std::env::var_os(name))
            {
                return start_workspace_presence_with(target, HERDR_PRESENCE_REFRESH_INTERVAL);
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    let _ = enabled;

    WorkspacePresence::default()
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn start_workspace_presence_with(
    target: WorkspacePresenceTarget,
    refresh_interval: Duration,
) -> WorkspacePresence {
    let (stop, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::Builder::new()
        .name("ptop-herdr-presence".to_string())
        .spawn(move || {
            let restore = rename_workspace_for_ptop(&target);
            run_workspace_presence_worker(receiver, refresh_interval, || {
                report_workspace_presence(&target);
            });
            if let Some(previous_label) = restore {
                restore_workspace_label(&target, &previous_label);
            }
        })
        .ok();
    let stop = worker.as_ref().map(|_| stop);

    WorkspacePresence { stop, worker }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn run_workspace_presence_worker(
    receiver: std::sync::mpsc::Receiver<()>,
    refresh_interval: Duration,
    mut report: impl FnMut(),
) {
    report();
    while matches!(
        receiver.recv_timeout(refresh_interval),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ) {
        report();
    }
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn rename_workspace_for_ptop(target: &WorkspacePresenceTarget) -> Option<String> {
    rename_workspace_for_ptop_with(
        target,
        |args| {
            run_bounded_herdr_json(
                &target.binary,
                &target.socket_path,
                args,
                HERDR_PRESENCE_COMMAND_TIMEOUT,
            )
        },
        |args| {
            run_bounded_herdr_command(
                &target.binary,
                &target.socket_path,
                args,
                HERDR_PRESENCE_COMMAND_TIMEOUT,
            )
        },
    )
}

// Herdr 0.9.0 has no conditional rename, so the read and rename can race.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn rename_workspace_for_ptop_with(
    target: &WorkspacePresenceTarget,
    mut get: impl FnMut(&[String]) -> Option<Value>,
    mut rename: impl FnMut(&[String]) -> bool,
) -> Option<String> {
    let pane = get(&pane_get_args(target))?;
    if !pane_matches_target(&pane, target) {
        return None;
    }

    let workspace = get(&workspace_get_args(&target.workspace_id))?;
    let previous_label = workspace_label(&workspace, &target.workspace_id)?.to_string();
    if previous_label == PTOP_WORKSPACE_LABEL {
        return None;
    }

    rename(&workspace_rename_args(
        &target.workspace_id,
        PTOP_WORKSPACE_LABEL,
    ))
    .then_some(previous_label)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn restore_workspace_label(target: &WorkspacePresenceTarget, previous_label: &str) {
    let _ = restore_workspace_label_with(
        target,
        previous_label,
        |args| {
            run_bounded_herdr_json(
                &target.binary,
                &target.socket_path,
                args,
                HERDR_PRESENCE_COMMAND_TIMEOUT,
            )
        },
        |args| {
            run_bounded_herdr_command(
                &target.binary,
                &target.socket_path,
                args,
                HERDR_PRESENCE_COMMAND_TIMEOUT,
            )
        },
    );
}

// The label check prevents stale restoration, but Herdr cannot make it atomic.
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn restore_workspace_label_with(
    target: &WorkspacePresenceTarget,
    previous_label: &str,
    mut get_workspace: impl FnMut(&[String]) -> Option<Value>,
    mut rename: impl FnMut(&[String]) -> bool,
) -> bool {
    if previous_label == PTOP_WORKSPACE_LABEL {
        return false;
    }
    let Some(workspace) = get_workspace(&workspace_get_args(&target.workspace_id)) else {
        return false;
    };
    if workspace_label(&workspace, &target.workspace_id) != Some(PTOP_WORKSPACE_LABEL) {
        return false;
    }

    rename(&workspace_rename_args(&target.workspace_id, previous_label))
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn report_workspace_presence(target: &WorkspacePresenceTarget) {
    let _ = report_workspace_presence_with(
        target,
        |args| {
            run_bounded_herdr_json(
                &target.binary,
                &target.socket_path,
                args,
                HERDR_PRESENCE_COMMAND_TIMEOUT,
            )
        },
        |args| {
            run_bounded_herdr_command(
                &target.binary,
                &target.socket_path,
                args,
                HERDR_PRESENCE_COMMAND_TIMEOUT,
            )
        },
    );
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn report_workspace_presence_with(
    target: &WorkspacePresenceTarget,
    mut get_pane: impl FnMut(&[String]) -> Option<Value>,
    mut report: impl FnMut(&[String]) -> bool,
) -> bool {
    let Some(pane) = get_pane(&pane_get_args(target)) else {
        return false;
    };
    if !pane_matches_target(&pane, target) {
        return false;
    }

    report(&workspace_presence_report_args(&target.workspace_id))
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn pane_matches_target(pane: &Value, target: &WorkspacePresenceTarget) -> bool {
    pane.pointer("/result/pane").is_some_and(|pane| {
        pane.get("pane_id").and_then(Value::as_str) == Some(target.pane_id.as_str())
            && pane.get("workspace_id").and_then(Value::as_str)
                == Some(target.workspace_id.as_str())
    })
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn workspace_label<'a>(workspace: &'a Value, workspace_id: &str) -> Option<&'a str> {
    let workspace = workspace.pointer("/result/workspace")?;
    (workspace.get("workspace_id").and_then(Value::as_str) == Some(workspace_id))
        .then(|| workspace.get("label").and_then(Value::as_str))
        .flatten()
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn pane_get_args(target: &WorkspacePresenceTarget) -> Vec<String> {
    vec![
        "pane".to_string(),
        "get".to_string(),
        target.pane_id.clone(),
    ]
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn workspace_get_args(workspace_id: &str) -> Vec<String> {
    vec![
        "workspace".to_string(),
        "get".to_string(),
        workspace_id.to_string(),
    ]
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn workspace_rename_args(workspace_id: &str, label: &str) -> Vec<String> {
    vec![
        "workspace".to_string(),
        "rename".to_string(),
        workspace_id.to_string(),
        label.to_string(),
    ]
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn workspace_presence_report_args(workspace_id: &str) -> Vec<String> {
    vec![
        "workspace".to_string(),
        "report-metadata".to_string(),
        workspace_id.to_string(),
        "--source".to_string(),
        "ptop:presence".to_string(),
        "--token".to_string(),
        "ptop=running".to_string(),
        "--ttl-ms".to_string(),
        HERDR_PRESENCE_TTL_MS.to_string(),
    ]
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn current_workspace_presence_target_from(
    mut env: impl FnMut(&str) -> Option<std::ffi::OsString>,
) -> Option<WorkspacePresenceTarget> {
    if env("HERDR_ENV").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return None;
    }

    let socket_path = env("HERDR_SOCKET_PATH")?.into_string().ok()?;
    if socket_path.is_empty() || socket_path.len() > 4096 || !Path::new(&socket_path).is_absolute()
    {
        return None;
    }
    let workspace_id = bounded_herdr_id(env("HERDR_WORKSPACE_ID")?)?;
    let pane_id = bounded_herdr_id(env("HERDR_PANE_ID")?)?;
    let binary = env("HERDR_BIN_PATH")
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| "herdr".into());

    Some(WorkspacePresenceTarget {
        binary,
        socket_path,
        workspace_id,
        pane_id,
    })
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn bounded_herdr_id(value: std::ffi::OsString) -> Option<String> {
    let value = value.into_string().ok()?;
    (!value.is_empty() && value.len() <= 128).then_some(value)
}

#[cfg(any(target_os = "linux", target_vendor = "apple", test))]
pub(crate) fn parse_herdr_process_marker(env: &[u8]) -> Option<HerdrProcessMarker> {
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

#[cfg(target_os = "linux")]
pub(crate) fn process_herdr_marker(pid: u32) -> Option<HerdrProcessMarker> {
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
pub(crate) fn process_herdr_marker(pid: u32) -> Option<HerdrProcessMarker> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_socket() -> &'static str {
        if cfg!(windows) {
            r"C:\herdr socket\herdr.sock"
        } else {
            "/tmp/herdr socket/herdr.sock"
        }
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    fn presence_target() -> WorkspacePresenceTarget {
        WorkspacePresenceTarget {
            binary: "herdr".into(),
            socket_path: "/tmp/herdr.sock".to_string(),
            workspace_id: "w1X".to_string(),
            pane_id: "w1X:p2".to_string(),
        }
    }

    #[test]
    fn process_marker_accepts_an_absolute_socket_path_with_spaces() {
        let env = format!(
            "HERDR_ENV=1\0HERDR_PANE_ID=w1X:p1\0HERDR_SOCKET_PATH={}\0",
            test_socket()
        );

        assert_eq!(
            parse_herdr_process_marker(env.as_bytes()),
            Some(HerdrProcessMarker {
                pane_id: "w1X:p1".to_string(),
                socket_path: test_socket().to_string(),
            })
        );
    }

    #[test]
    fn process_marker_rejects_incomplete_or_relative_markers() {
        let incomplete = format!(
            "HERDR_PANE_ID=w1X:p1\0HERDR_SOCKET_PATH={}\0",
            test_socket()
        );
        let relative = b"HERDR_ENV=1\0HERDR_PANE_ID=w1X:p1\0HERDR_SOCKET_PATH=relative.sock\0";

        assert!(parse_herdr_process_marker(incomplete.as_bytes()).is_none());
        assert!(parse_herdr_process_marker(relative).is_none());
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn command_runner_bounds_time_and_output() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-herdr");
        fs::write(
            &script,
            "#!/bin/sh\nif [ \"$1\" = ok ]; then exit 0; fi\nif [ \"$1\" = fail ]; then exit 1; fi\nif [ \"$1\" = stdin ]; then [ /dev/fd/0 -ef /dev/null ]; exit; fi\nif [ \"$1\" = sleep ]; then sleep 2; printf '{}\\n'; exit; fi\nprintf '{\"padding\":\"'\ndd if=/dev/zero bs=1024 count=300 2>/dev/null | tr '\\000' x\nprintf '\"}\\n'\n",
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
        assert!(run_bounded_herdr_command(
            script.as_os_str(),
            "/tmp/herdr.sock",
            &["ok".to_string()],
            Duration::from_secs(1),
        ));
        assert!(!run_bounded_herdr_command(
            script.as_os_str(),
            "/tmp/herdr.sock",
            &["fail".to_string()],
            Duration::from_secs(1),
        ));
        assert!(run_bounded_herdr_command(
            script.as_os_str(),
            "/tmp/herdr.sock",
            &["stdin".to_string()],
            Duration::from_secs(1),
        ));
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn presence_lifecycle_renames_then_restores_the_workspace() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-herdr");
        let state = script.with_extension("state");
        let log = script.with_extension("log");
        fs::write(&state, "project alpha").unwrap();
        fs::write(
            &script,
            "#!/bin/sh\nstate=\"${0}.state\"\nlog=\"${0}.log\"\nif [ \"$1 $2\" = \"pane get\" ]; then printf '{\"result\":{\"pane\":{\"pane_id\":\"w1X:p2\",\"workspace_id\":\"w1X\"}}}\\n'; exit; fi\nif [ \"$1 $2\" = \"workspace get\" ]; then label=$(cat \"$state\"); printf '{\"result\":{\"workspace\":{\"workspace_id\":\"w1X\",\"label\":\"%s\"}}}\\n' \"$label\"; exit; fi\nif [ \"$1 $2\" = \"workspace rename\" ]; then printf '%s' \"$4\" > \"$state\"; printf '%s\\n' \"$4\" >> \"$log\"; printf '{}\\n'; exit; fi\nif [ \"$1 $2\" = \"workspace report-metadata\" ]; then printf '{}\\n'; exit; fi\nexit 1\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();

        let mut target = presence_target();
        target.binary = script.into_os_string();
        let presence = start_workspace_presence_with(target, Duration::from_secs(60));
        let deadline = Instant::now() + Duration::from_secs(1);
        while fs::read_to_string(&state).unwrap_or_default() != "ptop" && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(fs::read_to_string(&state).unwrap(), "ptop");

        drop(presence);

        assert_eq!(fs::read_to_string(&state).unwrap(), "project alpha");
        assert_eq!(fs::read_to_string(&log).unwrap(), "ptop\nproject alpha\n");
    }

    #[test]
    fn disabled_presence_does_not_start_a_worker() {
        let presence = start_workspace_presence(false);

        assert!(presence.stop.is_none());
        assert!(presence.worker.is_none());
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn current_presence_target_requires_exact_herdr_location() {
        let valid = |name: &str| match name {
            "HERDR_ENV" => Some("1".into()),
            "HERDR_SOCKET_PATH" => Some("/tmp/herdr socket/server.sock".into()),
            "HERDR_WORKSPACE_ID" => Some("w1X".into()),
            "HERDR_PANE_ID" => Some("w1X:p2".into()),
            "HERDR_BIN_PATH" => Some("/opt/herdr/bin/herdr".into()),
            _ => None,
        };
        let target = current_workspace_presence_target_from(valid).unwrap();
        assert_eq!(target.socket_path, "/tmp/herdr socket/server.sock");
        assert_eq!(target.workspace_id, "w1X");
        assert_eq!(target.pane_id, "w1X:p2");
        assert_eq!(target.binary, std::ffi::OsStr::new("/opt/herdr/bin/herdr"));

        for missing in [
            "HERDR_ENV",
            "HERDR_SOCKET_PATH",
            "HERDR_WORKSPACE_ID",
            "HERDR_PANE_ID",
        ] {
            assert!(current_workspace_presence_target_from(|name| {
                (name != missing).then(|| valid(name)).flatten()
            })
            .is_none());
        }
        assert!(current_workspace_presence_target_from(|name| match name {
            "HERDR_ENV" => Some("1".into()),
            "HERDR_SOCKET_PATH" => Some("relative.sock".into()),
            "HERDR_WORKSPACE_ID" => Some("w1X".into()),
            "HERDR_PANE_ID" => Some("w1X:p2".into()),
            _ => None,
        })
        .is_none());
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn verified_workspace_is_renamed_and_its_previous_label_is_retained() {
        let target = presence_target();
        let mut get_args = Vec::new();
        let mut rename_args = Vec::new();

        let previous_label = rename_workspace_for_ptop_with(
            &target,
            |args| {
                get_args.push(args.to_vec());
                if args == pane_get_args(&target) {
                    Some(serde_json::json!({
                        "result": { "pane": {
                            "pane_id": "w1X:p2",
                            "workspace_id": "w1X"
                        } }
                    }))
                } else {
                    Some(serde_json::json!({
                        "result": { "workspace": {
                            "workspace_id": "w1X",
                            "label": "project alpha"
                        } }
                    }))
                }
            },
            |args| {
                rename_args.push(args.to_vec());
                true
            },
        );

        assert_eq!(previous_label.as_deref(), Some("project alpha"));
        assert_eq!(
            get_args,
            [
                pane_get_args(&target),
                workspace_get_args(&target.workspace_id)
            ]
        );
        assert_eq!(
            rename_args,
            [workspace_rename_args(&target.workspace_id, "ptop")]
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn workspace_rename_requires_an_exact_pane_and_workspace() {
        let target = presence_target();
        let rename_called = std::cell::Cell::new(false);

        let previous_label = rename_workspace_for_ptop_with(
            &target,
            |_| {
                Some(serde_json::json!({
                    "result": { "pane": {
                        "pane_id": "w1X:p2",
                        "workspace_id": "w2"
                    } }
                }))
            },
            |_| {
                rename_called.set(true);
                true
            },
        );

        assert!(previous_label.is_none());
        assert!(!rename_called.get());
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn preexisting_ptop_workspace_label_is_not_owned() {
        let target = presence_target();
        let rename_called = std::cell::Cell::new(false);

        let previous_label = rename_workspace_for_ptop_with(
            &target,
            |args| {
                if args == pane_get_args(&target) {
                    Some(serde_json::json!({
                        "result": { "pane": {
                            "pane_id": "w1X:p2",
                            "workspace_id": "w1X"
                        } }
                    }))
                } else {
                    Some(serde_json::json!({
                        "result": { "workspace": {
                            "workspace_id": "w1X",
                            "label": "ptop"
                        } }
                    }))
                }
            },
            |_| {
                rename_called.set(true);
                true
            },
        );

        assert!(previous_label.is_none());
        assert!(!rename_called.get());
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn previous_workspace_label_is_restored_only_while_ptop_still_owns_it() {
        let target = presence_target();
        let mut rename_args = Vec::new();
        assert!(restore_workspace_label_with(
            &target,
            "project alpha",
            |_| {
                Some(serde_json::json!({
                    "result": { "workspace": {
                        "workspace_id": "w1X",
                        "label": "ptop"
                    } }
                }))
            },
            |args| {
                rename_args.push(args.to_vec());
                true
            },
        ));
        assert_eq!(
            rename_args,
            [workspace_rename_args(&target.workspace_id, "project alpha")]
        );

        let rename_called = std::cell::Cell::new(false);
        assert!(!restore_workspace_label_with(
            &target,
            "project alpha",
            |_| {
                Some(serde_json::json!({
                    "result": { "workspace": {
                        "workspace_id": "w1X",
                        "label": "renamed by user"
                    } }
                }))
            },
            |_| {
                rename_called.set(true);
                true
            },
        ));
        assert!(!rename_called.get());
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn presence_request_is_display_only_and_expires() {
        assert_eq!(
            workspace_presence_report_args("w1X"),
            vec![
                "workspace",
                "report-metadata",
                "w1X",
                "--source",
                "ptop:presence",
                "--token",
                "ptop=running",
                "--ttl-ms",
                "30000",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn presence_requires_the_reported_pane_to_remain_in_its_workspace() {
        let target = presence_target();
        let mut pane_args = Vec::new();
        let mut report_args = Vec::new();
        assert!(report_workspace_presence_with(
            &target,
            |args| {
                pane_args.push(args.to_vec());
                Some(serde_json::json!({
                    "result": { "pane": { "pane_id": "w1X:p2", "workspace_id": "w1X" } }
                }))
            },
            |args| {
                report_args.push(args.to_vec());
                true
            },
        ));
        assert_eq!(pane_args, [vec!["pane", "get", "w1X:p2"]]);
        assert_eq!(report_args, [workspace_presence_report_args("w1X")]);

        let report_called = std::cell::Cell::new(false);
        assert!(!report_workspace_presence_with(
            &target,
            |_| {
                Some(serde_json::json!({
                    "result": { "pane": { "pane_id": "w1X:p2", "workspace_id": "w2" } }
                }))
            },
            |_| {
                report_called.set(true);
                true
            },
        ));
        assert!(!report_called.get());
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn presence_worker_reports_once_before_stopping() {
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(()).unwrap();
        let mut reports = 0;

        run_workspace_presence_worker(receiver, Duration::from_secs(60), || reports += 1);

        assert_eq!(reports, 1);
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn presence_worker_refreshes_until_its_sender_disconnects() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut sender = Some(sender);
        let mut reports = 0;

        run_workspace_presence_worker(receiver, Duration::ZERO, || {
            reports += 1;
            if reports == 3 {
                sender.take();
            }
        });

        assert_eq!(reports, 3);
    }
}
