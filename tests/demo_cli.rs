use std::process::Command;

fn ptop() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ptop"))
}

#[test]
fn pi_demo_once_uses_safe_fixture_data_without_task_descriptions() {
    let output = ptop().args(["--demo", "--once"]).output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("ptop — 3 Pi processes"), "{stdout}");
    assert!(stdout.contains("provider/model: anthropic/claude-sonnet-4-6"));
    assert!(!stdout.contains("security review"));
    assert!(!stdout.contains("payment boundary"));
}

#[test]
fn pi_demo_json_uses_the_supported_fleet_contract() {
    let output = ptop().args(["--demo", "--json"]).output().unwrap();

    assert!(output.status.success());
    let snapshot: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let fleet = &snapshot["sessions"][0]["telemetry"]["fleet"];
    assert_eq!(fleet["foreground_visibility"], "unavailable");
    assert_eq!(fleet["background_visibility"], "supported");
    assert_eq!(fleet["runs"][0]["lifecycle_version"], 3);
}

#[test]
fn removed_and_unknown_options_are_rejected() {
    for option in ["--legacy", "--setup", "--unknown"] {
        let output = ptop().arg(option).output().unwrap();

        assert!(!output.status.success(), "{option} unexpectedly succeeded");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(option), "{stderr}");
    }
}
