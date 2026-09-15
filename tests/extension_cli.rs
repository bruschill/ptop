#![cfg(unix)]

use std::process::Command;

fn ptop() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ptop"))
}

fn isolated() -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::Builder::new()
        .prefix("ptop-extension-cli-")
        .tempdir_in(std::env::current_dir().unwrap())
        .unwrap();
    let home = root.path().join("home");
    let agent = root.path().join("agent");
    std::fs::create_dir(&home).unwrap();
    (root, agent)
}

fn lifecycle(agent: &std::path::Path, command: &str) -> std::process::Output {
    let (root, _) = isolated();
    // A deliberately broken config must not be read by lifecycle dispatch.
    let config = root.path().join("config");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(config.join("config.toml"), "not = [valid").unwrap();
    ptop()
        .args(["extension", command])
        .env("HOME", root.path().join("home"))
        .env("PI_CODING_AGENT_DIR", agent)
        .env("XDG_CONFIG_HOME", config)
        .output()
        .unwrap()
}

#[test]
fn compiled_lifecycle_cli_installs_statuses_and_removes_in_an_isolated_agent_dir() {
    let (_root, agent) = isolated();
    let install = lifecycle(&agent, "install");
    assert!(install.status.success(), "{:?}", install);
    assert!(String::from_utf8_lossy(&install.stdout).contains("installed"));

    let status = lifecycle(&agent, "status");
    assert!(status.status.success());
    assert_eq!(
        String::from_utf8_lossy(&status.stdout).trim(),
        "extension target: current"
    );

    let remove = lifecycle(&agent, "remove");
    assert!(remove.status.success());
    assert!(String::from_utf8_lossy(&remove.stdout).contains("removed"));
    assert!(!agent.join("extensions/ptop-live-harness.ts").exists());
}

#[test]
fn compiled_lifecycle_cli_rejects_removed_commands_before_config_or_terminal_startup() {
    let (_root, agent) = isolated();
    for command in ["update", "restore", "force"] {
        let output = lifecycle(&agent, command);
        assert_eq!(output.status.code(), Some(1), "{command}: {output:?}");
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr).trim(),
            "usage: ptop extension <install|status|remove>"
        );
        assert!(!agent.exists(), "{command} created an agent path");
    }
}

#[test]
fn compiled_status_is_read_only_and_bypasses_broken_config() {
    let (_root, agent) = isolated();
    let output = lifecycle(&agent, "status");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "extension target: absent"
    );
    assert!(output.stderr.is_empty());
    assert!(!agent.exists());
}
