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
}
