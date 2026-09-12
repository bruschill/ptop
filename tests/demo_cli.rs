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
fn legacy_demo_once_remains_available_when_explicitly_requested() {
    let output = ptop()
        .args(["--legacy", "--demo", "--once"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("ptop — 5 sessions"), "{stdout}");
}
