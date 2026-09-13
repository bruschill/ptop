//! Herdr backend.
//!
//! A Pi process launched in Herdr inherits the pane ID and server socket that
//! own it. When ptop is attached to that same server, focus the exact pane
//! through Herdr's agent command. Different servers are not interchangeable,
//! so those targets remain available to later terminal adapters.

use super::{JumpAttempt, TerminalJumper};
use crate::herdr::{process_herdr_marker, run_bounded_herdr_json, HerdrProcessMarker};
use std::path::Path;
use std::time::Duration;

const HERDR_JUMP_TIMEOUT_MS: u64 = 1_000;

pub struct HerdrJumper;

impl TerminalJumper for HerdrJumper {
    fn name(&self) -> &'static str {
        "herdr"
    }

    fn try_jump(&self, pid: u32) -> JumpAttempt {
        let current_socket = current_herdr_socket_from(|name| std::env::var(name).ok());
        let marker = process_herdr_marker(pid);
        try_jump_with(current_socket, marker, |marker, args| {
            let binary = std::env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
            run_bounded_herdr_json(
                &binary,
                &marker.socket_path,
                args,
                Duration::from_millis(HERDR_JUMP_TIMEOUT_MS),
            )
        })
    }
}

fn current_herdr_socket_from(mut env: impl FnMut(&str) -> Option<String>) -> Option<String> {
    if env("HERDR_ENV").as_deref() != Some("1") {
        return None;
    }
    let socket_path = env("HERDR_SOCKET_PATH")?;
    if socket_path.len() > 4096 || !Path::new(&socket_path).is_absolute() {
        return None;
    }
    Some(socket_path)
}

fn try_jump_with(
    current_socket: Option<String>,
    marker: Option<HerdrProcessMarker>,
    mut run: impl FnMut(&HerdrProcessMarker, &[String]) -> Option<serde_json::Value>,
) -> JumpAttempt {
    let (Some(current_socket), Some(marker)) = (current_socket, marker) else {
        return JumpAttempt::NotApplicable;
    };
    if current_socket != marker.socket_path {
        return JumpAttempt::NotApplicable;
    }

    let args = vec![
        "agent".to_string(),
        "focus".to_string(),
        marker.pane_id.clone(),
    ];
    let Some(focus_result) = run(&marker, &args) else {
        return JumpAttempt::Failed("focus command failed or timed out".to_string());
    };
    let Some(workspace_id) = focus_result
        .pointer("/result/agent/workspace_id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 128)
    else {
        return JumpAttempt::Failed(
            "focus response did not identify the target workspace".to_string(),
        );
    };

    let workspace_args = vec![
        "workspace".to_string(),
        "focus".to_string(),
        workspace_id.to_string(),
    ];
    if run(&marker, &workspace_args).is_some() {
        JumpAttempt::Jumped
    } else {
        JumpAttempt::Failed("workspace focus command failed or timed out".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn marker(socket_path: &str) -> HerdrProcessMarker {
        HerdrProcessMarker {
            pane_id: "w1X:p2".to_string(),
            socket_path: socket_path.to_string(),
        }
    }

    #[test]
    fn current_socket_requires_herdr_and_an_absolute_path() {
        let current = |name: &str| match name {
            "HERDR_ENV" => Some("1".to_string()),
            "HERDR_SOCKET_PATH" => Some("/tmp/herdr socket/server.sock".to_string()),
            _ => None,
        };
        let not_herdr = |name: &str| match name {
            "HERDR_ENV" => Some("0".to_string()),
            "HERDR_SOCKET_PATH" => Some("/tmp/herdr.sock".to_string()),
            _ => None,
        };
        let relative = |name: &str| match name {
            "HERDR_ENV" => Some("1".to_string()),
            "HERDR_SOCKET_PATH" => Some("herdr.sock".to_string()),
            _ => None,
        };

        assert_eq!(
            current_herdr_socket_from(current),
            Some("/tmp/herdr socket/server.sock".to_string())
        );
        assert!(current_herdr_socket_from(not_herdr).is_none());
        assert!(current_herdr_socket_from(relative).is_none());
    }

    #[test]
    fn same_server_focuses_the_exact_pi_pane_and_its_workspace() {
        let calls = RefCell::new(Vec::new());
        let attempt = try_jump_with(
            Some("/tmp/herdr.sock".to_string()),
            Some(marker("/tmp/herdr.sock")),
            |location, args| {
                assert_eq!(location.pane_id, "w1X:p2");
                assert_eq!(location.socket_path, "/tmp/herdr.sock");
                calls.borrow_mut().push(args.to_vec());
                Some(serde_json::json!({
                    "result": {
                        "agent": {
                            "workspace_id": "w1X"
                        }
                    }
                }))
            },
        );

        assert_eq!(attempt, JumpAttempt::Jumped);
        assert_eq!(
            *calls.borrow(),
            [
                ["agent", "focus", "w1X:p2"].map(str::to_string),
                ["workspace", "focus", "w1X"].map(str::to_string),
            ]
        );
    }

    #[test]
    fn another_server_falls_through_without_running_herdr() {
        let called = Cell::new(false);
        let attempt = try_jump_with(
            Some("/tmp/current.sock".to_string()),
            Some(marker("/tmp/target.sock")),
            |_, _| {
                called.set(true);
                Some(serde_json::json!({}))
            },
        );

        assert_eq!(attempt, JumpAttempt::NotApplicable);
        assert!(!called.get());
    }

    #[test]
    fn focus_failure_stops_at_the_owning_adapter() {
        let attempt = try_jump_with(
            Some("/tmp/herdr.sock".to_string()),
            Some(marker("/tmp/herdr.sock")),
            |_, _| None,
        );

        assert_eq!(
            attempt,
            JumpAttempt::Failed("focus command failed or timed out".to_string())
        );
    }
}
