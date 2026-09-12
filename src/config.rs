use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug)]
pub struct PanelVisibility {
    pub context: bool,
    pub tokens: bool,
    pub projects: bool,
    pub ports: bool,
    pub sessions: bool,
}

impl Default for PanelVisibility {
    fn default() -> Self {
        Self {
            context: true,
            tokens: true,
            projects: true,
            ports: true,
            sessions: true,
        }
    }
}

#[derive(Debug)]
pub struct AppConfig {
    pub theme: String,
    pub theme_file: Option<PathBuf>,
    pub panels: PanelVisibility,
    /// UI language override. Empty string means auto-detect from `LANG`.
    /// Recognized values: "en", "zh" (anything starting with "zh" maps to Simplified Chinese).
    pub language: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            theme: "btop".to_string(),
            theme_file: None,
            panels: PanelVisibility::default(),
            language: String::new(),
        }
    }
}

pub(crate) fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|directory| directory.join("ptop").join("config.toml"))
}

/// Compatibility loader for library callers that rely on an infallible result.
pub fn load_config() -> AppConfig {
    try_load_config().unwrap_or_default()
}

/// Load configuration and report read, encoding, or selected-theme syntax errors.
pub fn try_load_config() -> Result<AppConfig, String> {
    load_config_for_startup(false)
}

pub(crate) fn load_config_for_startup(
    ignore_theme_selection_errors: bool,
) -> Result<AppConfig, String> {
    let Some(path) = config_path() else {
        return Ok(AppConfig::default());
    };
    load_config_from_path(&path, ignore_theme_selection_errors)
}

fn load_config_from_path(
    path: &Path,
    ignore_theme_selection_errors: bool,
) -> Result<AppConfig, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AppConfig::default())
        }
        Err(error) => return Err(format!("cannot read config '{}': {error}", path.display())),
    };
    let content = std::str::from_utf8(&bytes)
        .map_err(|_| format!("config '{}' is not valid UTF-8", path.display()))?;
    parse_config_body_checked(content, path, ignore_theme_selection_errors)
}

#[cfg(test)]
fn parse_config_body(content: &str) -> AppConfig {
    parse_config_body_checked(content, Path::new("config.toml"), true).unwrap_or_default()
}

fn parse_config_body_checked(
    content: &str,
    path: &Path,
    ignore_theme_selection_errors: bool,
) -> Result<AppConfig, String> {
    let mut config = AppConfig::default();
    let mut in_table = false;
    let mut seen_theme = false;
    let mut seen_theme_file = false;
    let mut theme_error = None;
    let mut theme_file_error = None;

    for (index, original_line) in content.lines().enumerate() {
        let line = original_line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            in_table = true;
            continue;
        }
        if in_table {
            continue;
        }

        let Some((key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let line_number = index + 1;

        if key == "theme" || key == "theme_file" {
            let seen = if key == "theme" {
                &mut seen_theme
            } else {
                &mut seen_theme_file
            };
            if *seen && !ignore_theme_selection_errors {
                return Err(format!(
                    "config '{}':{line_number}: duplicate top-level '{key}'",
                    path.display()
                ));
            }
            *seen = true;

            let parsed = if key == "theme" {
                parse_theme_name(raw_value)
            } else {
                parse_toml_string(raw_value)
            };
            let value = match parsed {
                Ok(value) => value,
                Err(_) if ignore_theme_selection_errors => continue,
                Err(message) => {
                    let error = format!(
                        "config '{}':{line_number}: invalid {key}: {message}",
                        path.display()
                    );
                    if key == "theme" {
                        theme_error = Some(error);
                    } else {
                        theme_file_error = Some(error);
                    }
                    continue;
                }
            };
            if key == "theme" {
                config.theme = value;
            } else if value.is_empty() {
                if !ignore_theme_selection_errors {
                    theme_file_error = Some(format!(
                        "config '{}':{line_number}: theme_file must not be empty",
                        path.display()
                    ));
                }
            } else {
                config.theme_file = Some(PathBuf::from(value));
            }
            continue;
        }

        let value = strip_inline_comment(raw_value).trim();
        match key {
            "language" => {
                config.language = parse_toml_string(raw_value)
                    .unwrap_or_else(|_| value.trim_matches('"').trim_matches('\'').to_string())
            }
            "show_context" => config.panels.context = parse_bool(value).unwrap_or(true),
            "show_tokens" => config.panels.tokens = parse_bool(value).unwrap_or(true),
            "show_projects" => config.panels.projects = parse_bool(value).unwrap_or(true),
            "show_ports" => config.panels.ports = parse_bool(value).unwrap_or(true),
            "show_sessions" => config.panels.sessions = parse_bool(value).unwrap_or(true),
            _ => {}
        }
    }

    if !ignore_theme_selection_errors {
        if seen_theme_file {
            if let Some(error) = theme_file_error {
                return Err(error);
            }
        } else if let Some(error) = theme_error {
            return Err(error);
        }
    }
    Ok(config)
}

fn parse_theme_name(raw: &str) -> Result<String, String> {
    parse_toml_string(raw).or_else(|_| {
        let legacy = strip_inline_comment(raw).trim();
        if !legacy.is_empty()
            && legacy
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            Ok(legacy.to_string())
        } else {
            Err("expected a quoted string or an unquoted built-in name".to_string())
        }
    })
}

#[derive(Deserialize)]
struct StringValue {
    value: String,
}

fn parse_toml_string(raw: &str) -> Result<String, String> {
    toml::from_str::<StringValue>(&format!("value = {raw}"))
        .map(|parsed| parsed.value)
        .map_err(|error| error.to_string())
}

fn strip_inline_comment(raw: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in raw.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && quote == Some('"') {
            escaped = true;
            continue;
        }
        if character == '"' || character == '\'' {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
            }
            continue;
        }
        if character == '#' && quote.is_none() {
            return &raw[..index];
        }
    }
    raw
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

pub fn save_theme(name: &str) -> Result<(), String> {
    write_with_edits(&[
        ("theme", Some(quote_toml_string(name))),
        ("theme_file", None),
    ])
}

pub fn save_theme_file(path: &Path) -> Result<(), String> {
    let value = path
        .to_str()
        .ok_or("theme file paths saved in config must be valid UTF-8")?;
    write_with_edits(&[
        ("theme", None),
        ("theme_file", Some(quote_toml_string(value))),
    ])
}

pub fn save_panel_visibility(panels: &PanelVisibility) -> Result<(), String> {
    write_with_updates(&[
        ("show_context", panels.context.to_string()),
        ("show_tokens", panels.tokens.to_string()),
        ("show_projects", panels.projects.to_string()),
        ("show_ports", panels.ports.to_string()),
        ("show_sessions", panels.sessions.to_string()),
    ])
}

fn quote_toml_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            character if character.is_control() => {
                quoted.push_str(&format!("\\u{:04X}", character as u32));
            }
            character => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

fn write_with_updates(updates: &[(&str, String)]) -> Result<(), String> {
    let edits: Vec<(&str, Option<String>)> = updates
        .iter()
        .map(|(key, value)| (*key, Some(value.clone())))
        .collect();
    write_with_edits(&edits)
}

/// Read the config, safely update top-level scalar keys, and preserve other lines.
fn write_with_edits(edits: &[(&str, Option<String>)]) -> Result<(), String> {
    let path = config_path().ok_or("no config directory")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.to_string()),
    };
    let new_content = rewrite_config_lines(&content, edits)?;
    std::fs::write(&path, new_content).map_err(|error| error.to_string())
}

/// Compatibility helper used by existing tests for update-only rewrites.
#[cfg(test)]
fn rewrite_kv_lines(content: &str, updates: &[(&str, String)]) -> String {
    let edits: Vec<(&str, Option<String>)> = updates
        .iter()
        .map(|(key, value)| (*key, Some(value.clone())))
        .collect();
    rewrite_config_lines(content, &edits).expect("test config must use supported syntax")
}

fn rewrite_config_lines(content: &str, edits: &[(&str, Option<String>)]) -> Result<String, String> {
    if content.contains("\"\"\"") || content.contains("'''") {
        return Err("cannot safely rewrite config containing multiline strings".to_string());
    }

    let mut found = vec![false; edits.len()];
    let mut output = Vec::new();
    let mut in_table = false;
    let mut first_table_index = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if first_table_index.is_none() {
                first_table_index = Some(output.len());
            }
            in_table = true;
            output.push(line.to_string());
            continue;
        }

        if !in_table {
            if let Some((key, _)) = trimmed.split_once('=') {
                let key = key.trim();
                if let Some(index) = edits.iter().position(|(candidate, _)| *candidate == key) {
                    if found[index] {
                        return Err(format!("cannot safely rewrite duplicate top-level '{key}'"));
                    }
                    found[index] = true;
                    if let Some(value) = &edits[index].1 {
                        output.push(format!("{} = {value}", edits[index].0));
                    }
                    continue;
                }
            }
        }
        output.push(line.to_string());
    }

    let additions: Vec<String> = edits
        .iter()
        .enumerate()
        .filter_map(|(index, (key, value))| {
            if found[index] {
                None
            } else {
                value.as_ref().map(|value| format!("{key} = {value}"))
            }
        })
        .collect();
    let insertion_index = first_table_index.unwrap_or(output.len());
    output.splice(insertion_index..insertion_index, additions);
    Ok(output.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme_update(name: &str) -> Vec<(&'static str, String)> {
        vec![("theme", format!("\"{}\"", name))]
    }

    #[test]
    fn obsolete_keys_are_ignored_and_preserved_by_rewrites() {
        let before = "theme = \"btop\"\nhidden_agents = [\"codex\"]\nclaude_config_dirs = [\"~/.claude-work\"]\nshow_quota = false\nshow_mcp = false\n";
        let config = parse_config_body(before);
        assert!(config.panels.context);
        assert!(config.panels.tokens);

        let after = rewrite_kv_lines(before, &theme_update("dracula"));
        assert!(after.contains("theme = \"dracula\""));
        for key in [
            "hidden_agents",
            "claude_config_dirs",
            "show_quota",
            "show_mcp",
        ] {
            assert!(after.contains(key), "{key} line dropped:\n{after}");
        }
    }

    #[test]
    fn rewrite_theme_preserves_arbitrary_unknown_keys() {
        let before = "# user comment\nfuture_key = 42\ntheme = \"btop\"\n";
        let after = rewrite_kv_lines(before, &theme_update("nord"));
        assert!(after.contains("# user comment"));
        assert!(after.contains("future_key = 42"));
        assert!(after.contains("theme = \"nord\""));
    }

    #[test]
    fn rewrite_theme_appends_when_missing() {
        let before = "future_key = 42\n";
        let after = rewrite_kv_lines(before, &theme_update("gruvbox"));
        assert!(after.contains("future_key = 42"));
        assert!(after.contains("theme = \"gruvbox\""));
    }

    #[test]
    fn rewrite_panels_replaces_existing_and_appends_missing() {
        let before = "theme = \"btop\"\nshow_tokens = true\n";
        let updates: Vec<(&str, String)> = vec![
            ("show_tokens", "false".to_string()),
            ("show_projects", "false".to_string()),
        ];
        let after = rewrite_kv_lines(before, &updates);
        assert!(after.contains("show_tokens = false"));
        assert!(!after.contains("show_tokens = true"));
        assert!(after.contains("show_projects = false"));
        assert!(after.contains("theme = \"btop\""));
    }

    #[test]
    fn parse_bool_round_trips_visibility_keys() {
        assert_eq!(parse_bool("true"), Some(true));
        assert_eq!(parse_bool("False"), Some(false));
        assert_eq!(parse_bool("nope"), None);
    }

    #[test]
    fn rewrite_language_replaces_existing() {
        let before = "theme = \"btop\"\nlanguage = \"en\"\n";
        let updates: Vec<(&str, String)> = vec![("language", "\"zh\"".to_string())];
        let after = rewrite_kv_lines(before, &updates);
        assert!(after.contains("language = \"zh\""));
        assert!(!after.contains("language = \"en\""));
        assert!(after.contains("theme = \"btop\""));
    }

    #[test]
    fn parses_theme_file_with_spaces_hashes_and_backslashes() {
        let body = r#"theme_file = 'themes/low glare#1\custom.toml' # selected theme"#;
        let config = parse_config_body_checked(body, Path::new("config.toml"), false).unwrap();
        assert_eq!(
            config.theme_file,
            Some(PathBuf::from(r"themes/low glare#1\custom.toml"))
        );
    }

    #[test]
    fn duplicate_theme_selection_is_rejected_unless_cli_overrides_it() {
        let body = "theme_file = 42\ntheme_file = false\nfuture_key = 42\n";
        assert!(parse_config_body_checked(body, Path::new("config.toml"), false).is_err());

        let config = parse_config_body_checked(body, Path::new("config.toml"), true).unwrap();
        assert_eq!(config.theme_file, None);
    }

    #[test]
    fn only_the_winning_configured_theme_selection_must_be_valid() {
        let config = parse_config_body_checked(
            "theme_file = \"custom.toml\"\ntheme = []\n",
            Path::new("config.toml"),
            false,
        )
        .unwrap();
        assert_eq!(config.theme_file, Some(PathBuf::from("custom.toml")));

        let error = parse_config_body_checked(
            "theme_file = []\ntheme = \"nord\"\n",
            Path::new("config.toml"),
            false,
        )
        .unwrap_err();
        assert!(error.contains("invalid theme_file"));
    }

    #[test]
    fn switching_from_file_to_builtin_removes_file_selection_before_tables() {
        let body = "theme_file = \"custom.toml\"\n[future]\ntheme_file = \"nested.toml\"\n";
        let edits = [
            ("theme", Some(quote_toml_string("nord"))),
            ("theme_file", None),
        ];
        let rewritten = rewrite_config_lines(body, &edits).unwrap();
        let config =
            parse_config_body_checked(&rewritten, Path::new("config.toml"), false).unwrap();

        assert_eq!(config.theme, "nord");
        assert_eq!(config.theme_file, None);
        assert!(rewritten.contains("[future]\ntheme_file = \"nested.toml\""));
        assert!(rewritten.find("theme = \"nord\"").unwrap() < rewritten.find("[future]").unwrap());
    }

    #[test]
    fn panel_rewrites_preserve_a_quoted_theme_path() {
        let body = "theme_file = \"themes/low glare#1.toml\"\nshow_tokens = true\n";
        let rewritten =
            rewrite_config_lines(body, &[("show_tokens", Some("false".to_string()))]).unwrap();
        let config =
            parse_config_body_checked(&rewritten, Path::new("config.toml"), false).unwrap();

        assert_eq!(
            config.theme_file,
            Some(PathBuf::from("themes/low glare#1.toml"))
        );
        assert!(!config.panels.tokens);
    }

    #[test]
    fn unsafe_rewrites_fail_instead_of_corrupting_config() {
        assert!(rewrite_config_lines(
            "theme = \"btop\"\ntheme = \"nord\"\n",
            &[("theme", Some(quote_toml_string("dracula")))],
        )
        .is_err());
        assert!(rewrite_config_lines(
            "message = \"\"\"multi\nline\"\"\"\n",
            &[("theme", Some(quote_toml_string("dracula")))],
        )
        .is_err());
    }

    #[test]
    fn fallible_loader_reports_invalid_utf8() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        let error = load_config_from_path(&path, false).unwrap_err();
        assert!(error.contains("not valid UTF-8"));
        assert!(error.contains(&path.display().to_string()));
    }

    #[test]
    fn quoted_theme_paths_round_trip_as_toml_strings() {
        let value = r##"C:\Users\Brandon\themes\"night\"#1.toml"##;
        let quoted = quote_toml_string(value);
        assert_eq!(parse_toml_string(&quoted).unwrap(), value);
    }
}
