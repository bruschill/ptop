//! Conservative, privacy-safe pi-subagents package availability detection.
use crate::model::FleetAvailability;
use serde_json::Value;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

const MAX_SETTINGS_BYTES: u64 = 256 * 1024;
const MAX_PACKAGE_ENTRIES: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Evidence {
    Installed,
    Absent,
    Ambiguous,
}

pub(crate) fn detect(session_file: &Path, cwd: &Path) -> FleetAvailability {
    let Some(home) = dirs::home_dir() else {
        return FleetAvailability::Unknown;
    };
    let root = home.join(".pi").join("agent");
    if !safe_chain_beneath(&root, &home) || !safe_full_chain(&home) {
        return FleetAvailability::Unknown;
    }
    detect_at_root(session_file, cwd, &root)
}

#[cfg(test)]
pub(crate) fn detect_at_root(
    session_file: &Path,
    cwd: &Path,
    agent_root: &Path,
) -> FleetAvailability {
    detect_impl(session_file, cwd, agent_root)
}

#[cfg(not(test))]
fn detect_at_root(session_file: &Path, cwd: &Path, agent_root: &Path) -> FleetAvailability {
    detect_impl(session_file, cwd, agent_root)
}

fn detect_impl(session_file: &Path, cwd: &Path, agent_root: &Path) -> FleetAvailability {
    let sessions = agent_root.join("sessions");
    if !session_file.starts_with(&sessions)
        || !safe_chain_beneath(agent_root, agent_root)
        || !safe_chain_beneath(&sessions, agent_root)
        || !safe_chain_beneath(session_file, &sessions)
    {
        return FleetAvailability::Unknown;
    }

    let global = settings_evidence(&agent_root.join("settings.json"));
    let project_pi = cwd.join(".pi");
    let project = match fs::symlink_metadata(&project_pi) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_dir()
                || !safe_chain_beneath(&project_pi, cwd)
                || !safe_full_chain(cwd) =>
        {
            Evidence::Ambiguous
        }
        Ok(_) => settings_evidence(&project_pi.join("settings.json")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Evidence::Absent,
        Err(_) => Evidence::Ambiguous,
    };
    match (global, project) {
        (Evidence::Installed, _) | (_, Evidence::Installed) => FleetAvailability::Installed,
        (Evidence::Absent, Evidence::Absent) => FleetAvailability::NotInstalled,
        _ => FleetAvailability::Unknown,
    }
}

fn safe_component(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn safe_chain_beneath(path: &Path, root: &Path) -> bool {
    !path
        .components()
        .any(|component| component == std::path::Component::ParentDir)
        && path.starts_with(root)
        && path
            .ancestors()
            .take_while(|ancestor| *ancestor != root)
            .all(safe_component)
        && safe_component(root)
}

fn safe_full_chain(path: &Path) -> bool {
    path.ancestors().all(safe_component)
}

#[derive(Clone, Eq, PartialEq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    length: u64,
}

fn identity(metadata: &fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
    #[cfg(not(unix))]
    {
        FileIdentity {
            length: metadata.len(),
        }
    }
}

fn read_bounded_same_file(path: &Path) -> Result<Vec<u8>, ()> {
    if !safe_component(path) {
        return Err(());
    }
    let before = fs::symlink_metadata(path).map_err(|_| ())?;
    if !before.is_file() || before.len() > MAX_SETTINGS_BYTES {
        return Err(());
    }
    let id = identity(&before);
    let len = before.len();
    let mut file = File::open(path).map_err(|_| ())?;
    let opened = file.metadata().map_err(|_| ())?;
    if !opened.is_file() || identity(&opened) != id || opened.len() != len {
        return Err(());
    }
    let mut bytes = Vec::with_capacity((len as usize).min(64 * 1024));
    file.by_ref()
        .take(MAX_SETTINGS_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    let after = fs::symlink_metadata(path).map_err(|_| ())?;
    if bytes.len() as u64 != len || identity(&after) != id || after.len() != len || !after.is_file()
    {
        return Err(());
    }
    Ok(bytes)
}

fn settings_evidence(path: &Path) -> Evidence {
    let Ok(bytes) = read_bounded_same_file(path) else {
        return Evidence::Ambiguous;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return Evidence::Ambiguous;
    };
    let Some(object) = value.as_object() else {
        return Evidence::Ambiguous;
    };
    let Some(packages) = object.get("packages") else {
        return Evidence::Absent;
    };
    let Some(packages) = packages.as_array() else {
        return Evidence::Ambiguous;
    };
    if packages.len() > MAX_PACKAGE_ENTRIES {
        return Evidence::Ambiguous;
    }
    let mut result = Evidence::Absent;
    for package in packages {
        match package_evidence(package) {
            Evidence::Installed => result = Evidence::Installed,
            Evidence::Ambiguous => return Evidence::Ambiguous,
            Evidence::Absent => {}
        }
    }
    result
}

fn package_evidence(value: &Value) -> Evidence {
    let source = match value {
        Value::String(source) => source.trim(),
        Value::Object(object) => match object_source(object) {
            Some(source) => source.trim(),
            None => return Evidence::Ambiguous,
        },
        _ => return Evidence::Ambiguous,
    };
    if let Some(evidence) = npm_evidence(source) {
        return evidence;
    }
    match github_repo(source) {
        Ok(Some(repo)) => {
            if repo == "nicobailon/pi-subagents" {
                Evidence::Installed
            } else {
                Evidence::Absent
            }
        }
        Ok(None) => Evidence::Ambiguous,
        Err(()) => Evidence::Ambiguous,
    }
}

fn object_source(object: &serde_json::Map<String, Value>) -> Option<&str> {
    let source = object.get("source")?.as_str()?;
    for (key, value) in object {
        match key.as_str() {
            "source" => {}
            "autoload" if value.is_boolean() => {}
            "extensions" | "skills" | "prompts" | "themes"
                if value.as_array().is_some_and(|v| {
                    v.len() <= MAX_PACKAGE_ENTRIES && v.iter().all(Value::is_string)
                }) => {}
            _ => return None,
        }
    }
    Some(source)
}

fn npm_evidence(source: &str) -> Option<Evidence> {
    let spec = source.strip_prefix("npm:")?.trim();
    if spec == "pi-subagents"
        || spec
            .strip_prefix("pi-subagents@")
            .is_some_and(|range| !range.trim().is_empty())
    {
        Some(Evidence::Installed)
    } else if spec.is_empty() {
        Some(Evidence::Ambiguous)
    } else {
        Some(Evidence::Absent)
    }
}

fn github_repo(source: &str) -> Result<Option<&str>, ()> {
    let wrapped = source.starts_with("git:") && !source.starts_with("git://");
    let source = if wrapped {
        source.strip_prefix("git:").unwrap_or(source)
    } else {
        source
    };
    let path = if let Some(path) = source.strip_prefix("github.com/") {
        path
    } else if let Some(path) = source.strip_prefix("git@github.com:") {
        if !wrapped {
            return Ok(None);
        }
        path
    } else if let Some(path) = source.strip_prefix("http://github.com/") {
        path
    } else if let Some(path) = source.strip_prefix("https://github.com/") {
        path
    } else if let Some(path) = source.strip_prefix("ssh://git@github.com/") {
        path
    } else if let Some(path) = source.strip_prefix("git://github.com/") {
        path
    } else {
        return Ok(None);
    };
    if path.contains('#') {
        return Err(());
    }
    // Pi splits at the first ref delimiter, so refs may themselves contain @.
    let repo = match path.split_once('@') {
        Some((repo, reference)) if valid_ref(reference) => repo,
        Some(_) => return Err(()),
        None => path,
    };
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if repo.split('/').count() != 2 || repo.is_empty() {
        return Err(());
    }
    Ok(Some(repo))
}

fn valid_ref(reference: &str) -> bool {
    !reference.is_empty()
        && reference
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'/' | b'@'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn sources_trim_and_classify_without_substrings() {
        let installed = [
            " npm:pi-subagents@^1 ",
            "npm: pi-subagents",
            "npm:pi-subagents ",
            "git:github.com/nicobailon/pi-subagents",
            "git:github.com/nicobailon/pi-subagents@release@2",
            "git:git@github.com:nicobailon/pi-subagents.git",
            "git:git@github.com:nicobailon/pi-subagents.git@release@2",
            "http://github.com/nicobailon/pi-subagents",
            "https://github.com/nicobailon/pi-subagents.git@release@2",
            "ssh://git@github.com/nicobailon/pi-subagents@v1",
            "git://github.com/nicobailon/pi-subagents@v1",
        ];
        for source in installed {
            assert_eq!(
                package_evidence(&Value::String(source.into())),
                Evidence::Installed
            );
        }
        for source in [
            "npm:pi-subagents-extra",
            "https://github.com/nicobailon/pi-subagents-extra@v1",
        ] {
            assert_eq!(
                package_evidence(&Value::String(source.into())),
                Evidence::Absent
            );
        }
        for source in [
            "pi-subagents",
            "git@github.com:nicobailon/pi-subagents@v1",
            "git+https://github.com/nicobailon/pi-subagents",
            "./local",
        ] {
            assert_eq!(
                package_evidence(&Value::String(source.into())),
                Evidence::Ambiguous
            );
        }
    }

    #[test]
    fn complete_settings_distinguish_installed_from_not_installed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("agent");
        let session = root.join("sessions/project/session.jsonl");
        fs::create_dir_all(session.parent().unwrap()).unwrap();
        fs::write(&session, "x").unwrap();
        let project = temp.path().join("project");
        fs::create_dir_all(&project).unwrap();

        fs::write(root.join("settings.json"), r#"{"packages":[]}"#).unwrap();
        assert_eq!(
            detect_at_root(&session, &project, &root),
            FleetAvailability::NotInstalled
        );

        fs::write(
            root.join("settings.json"),
            r#"{"packages":["npm: pi-subagents"]}"#,
        )
        .unwrap();
        assert_eq!(
            detect_at_root(&session, &project, &root),
            FleetAvailability::Installed
        );
    }

    #[test]
    fn invalid_settings_and_custom_sessions_are_unknown() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("settings.json"), r#"{"packages":[]}"#).unwrap();
        let custom = temp.path().join("project/.pi/sessions/s.jsonl");
        fs::create_dir_all(custom.parent().unwrap()).unwrap();
        fs::write(&custom, "x").unwrap();
        assert_eq!(
            detect_at_root(&custom, &temp.path().join("project"), temp.path()),
            FleetAvailability::Unknown
        );
        fs::write(temp.path().join("sessions"), "not a directory").unwrap();
        assert_eq!(
            settings_evidence(&temp.path().join("settings.json")),
            Evidence::Absent
        );
        assert_eq!(
            settings_evidence(&temp.path().join("missing.json")),
            Evidence::Ambiguous
        );
        fs::write(temp.path().join("settings.json"), "[]").unwrap();
        assert_eq!(
            settings_evidence(&temp.path().join("settings.json")),
            Evidence::Ambiguous
        );
        fs::write(temp.path().join("settings.json"), r#"{"packages":"bad"}"#).unwrap();
        assert_eq!(
            settings_evidence(&temp.path().join("settings.json")),
            Evidence::Ambiguous
        );
    }

    #[cfg(unix)]
    #[test]
    fn project_pi_symlink_is_ambiguous() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("agent");
        let session = root.join("sessions/project/session.jsonl");
        fs::create_dir_all(session.parent().unwrap()).unwrap();
        fs::write(&session, "x").unwrap();
        fs::write(root.join("settings.json"), r#"{"packages":[]}"#).unwrap();
        let project = temp.path().join("project");
        let target = temp.path().join("project-pi");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("settings.json"), r#"{"packages":[]}"#).unwrap();
        fs::create_dir_all(&project).unwrap();
        symlink(&target, project.join(".pi")).unwrap();
        assert_eq!(
            detect_at_root(&session, &project, &root),
            FleetAvailability::Unknown
        );

        fs::remove_file(project.join(".pi")).unwrap();
        symlink(project.join("missing"), project.join(".pi")).unwrap();
        assert_eq!(
            detect_at_root(&session, &project, &root),
            FleetAvailability::Unknown
        );
    }
}
