use std::process::Command;

fn ptop() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ptop"))
}

#[test]
fn invalid_theme_file_fails_before_terminal_setup() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid.toml");
    std::fs::write(&path, "format = 1\n").unwrap();

    let output = ptop()
        .arg("--theme-file")
        .arg(&path)
        .arg("--once")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.contains(&0x1b));
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("invalid theme file"), "{error}");
    assert!(error.contains(&path.display().to_string()), "{error}");
}

#[test]
fn theme_independent_version_exit_does_not_load_a_theme() {
    let output = ptop()
        .arg("--version")
        .arg("--theme-file")
        .arg("does-not-exist.toml")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .starts_with("ptop "));
}

#[test]
fn conflicting_theme_flags_fail_with_one_actionable_error() {
    let output = ptop()
        .arg("--theme")
        .arg("btop")
        .arg("--theme-file")
        .arg("custom.toml")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.contains(&0x1b));
    let error = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        error.trim(),
        "--theme and --theme-file are mutually exclusive"
    );
}
