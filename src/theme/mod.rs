use ratatui::style::Color;
use serde::Deserialize;
use std::fmt;
#[cfg(not(unix))]
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_THEME_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gradient {
    pub start: (u8, u8, u8),
    pub mid: (u8, u8, u8),
    pub end: (u8, u8, u8),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeSource {
    Packaged { id: &'static str },
    File { path: PathBuf },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub name: String,
    pub source: ThemeSource,

    // base
    pub main_bg: Color,
    pub main_fg: Color,
    pub title: Color,
    pub hi_fg: Color,
    pub selected_bg: Color,
    pub selected_fg: Color,
    pub inactive_fg: Color,
    pub graph_text: Color,
    pub meter_bg: Color,
    pub proc_misc: Color,
    pub div_line: Color,
    pub session_id: Color,

    // semantic colors
    pub status_fg: Color,
    pub warning_fg: Color,

    // box borders
    pub cpu_box: Color,
    pub mem_box: Color,
    pub net_box: Color,
    pub proc_box: Color,

    // Pi session label
    pub pi_agent: Color,

    // gradients
    pub cpu_grad: Gradient,
    pub proc_grad: Gradient,
    pub used_grad: Gradient,
    pub free_grad: Gradient,
    pub cached_grad: Gradient,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeRequest {
    BuiltIn(String),
    File(PathBuf),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ThemeCatalog;

pub const THEME_NAMES: &[&str] = &[
    "btop",
    "dracula",
    "catppuccin",
    "tokyo-night",
    "gruvbox",
    "nord",
    "light",
    "white",
    "high-contrast",
    "protanopia",
    "deuteranopia",
    "tritanopia",
];

struct BuiltInTheme {
    id: &'static str,
    document: &'static str,
}

const BUILT_INS: &[BuiltInTheme] = &[
    BuiltInTheme {
        id: "btop",
        document: include_str!("builtins/btop.toml"),
    },
    BuiltInTheme {
        id: "dracula",
        document: include_str!("builtins/dracula.toml"),
    },
    BuiltInTheme {
        id: "catppuccin",
        document: include_str!("builtins/catppuccin.toml"),
    },
    BuiltInTheme {
        id: "tokyo-night",
        document: include_str!("builtins/tokyo-night.toml"),
    },
    BuiltInTheme {
        id: "gruvbox",
        document: include_str!("builtins/gruvbox.toml"),
    },
    BuiltInTheme {
        id: "nord",
        document: include_str!("builtins/nord.toml"),
    },
    BuiltInTheme {
        id: "light",
        document: include_str!("builtins/light.toml"),
    },
    BuiltInTheme {
        id: "white",
        document: include_str!("builtins/white.toml"),
    },
    BuiltInTheme {
        id: "high-contrast",
        document: include_str!("builtins/high-contrast.toml"),
    },
    BuiltInTheme {
        id: "protanopia",
        document: include_str!("builtins/protanopia.toml"),
    },
    BuiltInTheme {
        id: "deuteranopia",
        document: include_str!("builtins/deuteranopia.toml"),
    },
    BuiltInTheme {
        id: "tritanopia",
        document: include_str!("builtins/tritanopia.toml"),
    },
];

impl ThemeCatalog {
    pub fn packaged() -> Self {
        Self
    }

    pub fn packaged_names(&self) -> &'static [&'static str] {
        THEME_NAMES
    }

    pub fn load(&self, request: &ThemeRequest) -> Result<Theme, ThemeLoadError> {
        match request {
            ThemeRequest::BuiltIn(name) => {
                let built_in = BUILT_INS
                    .iter()
                    .find(|theme| theme.id == name)
                    .ok_or_else(|| ThemeLoadError::UnknownBuiltIn { name: name.clone() })?;
                decode_validate_theme(
                    built_in.document.as_bytes(),
                    ThemeOrigin::Packaged { id: built_in.id },
                )
            }
            ThemeRequest::File(path) => {
                let bytes = read_theme_file(path)?;
                decode_validate_theme(
                    &bytes,
                    ThemeOrigin::File {
                        path: path.to_path_buf(),
                    },
                )
            }
        }
    }
}

impl Theme {
    /// Compatibility helper for callers that select packaged themes by name.
    pub fn by_name(name: &str) -> Option<Self> {
        ThemeCatalog::packaged()
            .load(&ThemeRequest::BuiltIn(name.to_string()))
            .ok()
    }
}

impl Default for Theme {
    fn default() -> Self {
        ThemeCatalog::packaged()
            .load(&ThemeRequest::BuiltIn("btop".to_string()))
            .expect("the embedded btop theme must be valid")
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ThemeLoadError {
    UnknownBuiltIn { name: String },
    MissingFile { path: PathBuf },
    ReadFile { path: PathBuf, message: String },
    NonRegularFile { path: PathBuf },
    TooLarge { path: PathBuf, limit: usize },
    InvalidUtf8 { path: PathBuf },
    InvalidTheme { path: PathBuf, message: String },
    InvalidPackaged { id: &'static str, message: String },
}

impl fmt::Display for ThemeLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownBuiltIn { name } => write!(
                f,
                "unknown theme '{name}'. available: {}",
                THEME_NAMES.join(", ")
            ),
            Self::MissingFile { path } => {
                write!(f, "theme file '{}' does not exist", path.display())
            }
            Self::ReadFile { path, message } => {
                write!(f, "cannot read theme file '{}': {message}", path.display())
            }
            Self::NonRegularFile { path } => {
                write!(f, "theme path '{}' is not a regular file", path.display())
            }
            Self::TooLarge { path, limit } => write!(
                f,
                "theme file '{}' exceeds the {limit}-byte limit",
                path.display()
            ),
            Self::InvalidUtf8 { path } => {
                write!(f, "theme file '{}' is not valid UTF-8", path.display())
            }
            Self::InvalidTheme { path, message } => {
                write!(f, "invalid theme file '{}': {message}", path.display())
            }
            Self::InvalidPackaged { id, message } => {
                write!(f, "invalid packaged theme '{id}': {message}")
            }
        }
    }
}

impl std::error::Error for ThemeLoadError {}

fn read_theme_file(path: &Path) -> Result<Vec<u8>, ThemeLoadError> {
    #[cfg(not(unix))]
    {
        let metadata = fs::metadata(path).map_err(|error| map_file_open_error(path, error))?;
        if !metadata.is_file() {
            return Err(ThemeLoadError::NonRegularFile {
                path: path.to_path_buf(),
            });
        }
        if metadata.len() > MAX_THEME_BYTES as u64 {
            return Err(ThemeLoadError::TooLarge {
                path: path.to_path_buf(),
                limit: MAX_THEME_BYTES,
            });
        }
    }

    #[cfg(unix)]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;

        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .map_err(|error| map_file_open_error(path, error))?
    };
    #[cfg(not(unix))]
    let file = std::fs::File::open(path).map_err(|error| map_file_open_error(path, error))?;

    let opened_metadata = file.metadata().map_err(|error| ThemeLoadError::ReadFile {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    if !opened_metadata.is_file() {
        return Err(ThemeLoadError::NonRegularFile {
            path: path.to_path_buf(),
        });
    }

    let mut bytes = Vec::with_capacity(opened_metadata.len().min(MAX_THEME_BYTES as u64) as usize);
    file.take((MAX_THEME_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| ThemeLoadError::ReadFile {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    if bytes.len() > MAX_THEME_BYTES {
        return Err(ThemeLoadError::TooLarge {
            path: path.to_path_buf(),
            limit: MAX_THEME_BYTES,
        });
    }
    Ok(bytes)
}

fn map_file_open_error(path: &Path, error: std::io::Error) -> ThemeLoadError {
    if error.kind() == std::io::ErrorKind::NotFound {
        ThemeLoadError::MissingFile {
            path: path.to_path_buf(),
        }
    } else {
        ThemeLoadError::ReadFile {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    }
}

#[derive(Clone)]
enum ThemeOrigin {
    Packaged { id: &'static str },
    File { path: PathBuf },
}

impl ThemeOrigin {
    fn source(&self) -> ThemeSource {
        match self {
            Self::Packaged { id } => ThemeSource::Packaged { id },
            Self::File { path } => ThemeSource::File { path: path.clone() },
        }
    }

    fn invalid(&self, message: impl Into<String>) -> ThemeLoadError {
        let message = message.into();
        match self {
            Self::Packaged { id } => ThemeLoadError::InvalidPackaged { id, message },
            Self::File { path } => ThemeLoadError::InvalidTheme {
                path: path.clone(),
                message,
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeDocument {
    format: u32,
    name: String,
    colors: ThemeColors,
    gradients: ThemeGradients,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeColors {
    main_bg: String,
    main_fg: String,
    title: String,
    hi_fg: String,
    selected_bg: String,
    selected_fg: String,
    inactive_fg: String,
    graph_text: String,
    meter_bg: String,
    proc_misc: String,
    div_line: String,
    session_id: String,
    status_fg: String,
    warning_fg: String,
    cpu_box: String,
    mem_box: String,
    net_box: String,
    proc_box: String,
    pi_agent: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeGradients {
    cpu: [String; 3],
    process: [String; 3],
    used: [String; 3],
    free: [String; 3],
    cached: [String; 3],
}

fn decode_validate_theme(bytes: &[u8], origin: ThemeOrigin) -> Result<Theme, ThemeLoadError> {
    let text = std::str::from_utf8(bytes).map_err(|_| match &origin {
        ThemeOrigin::Packaged { .. } => origin.invalid("document is not valid UTF-8"),
        ThemeOrigin::File { path } => ThemeLoadError::InvalidUtf8 { path: path.clone() },
    })?;
    let document: ThemeDocument = toml::from_str(text)
        .map_err(|error| origin.invalid(format!("TOML parse error: {error}")))?;

    if document.format != 1 {
        return Err(origin.invalid(format!(
            "unsupported format {}; expected format = 1",
            document.format
        )));
    }
    if document.name.trim().is_empty() {
        return Err(origin.invalid("name must not be blank"));
    }
    if document.name.chars().any(char::is_control) {
        return Err(origin.invalid("name must not contain control characters"));
    }

    let colors = document.colors;
    let gradients = document.gradients;
    Ok(Theme {
        name: document.name,
        source: origin.source(),
        main_bg: parse_color("colors.main_bg", &colors.main_bg, &origin)?,
        main_fg: parse_color("colors.main_fg", &colors.main_fg, &origin)?,
        title: parse_color("colors.title", &colors.title, &origin)?,
        hi_fg: parse_color("colors.hi_fg", &colors.hi_fg, &origin)?,
        selected_bg: parse_color("colors.selected_bg", &colors.selected_bg, &origin)?,
        selected_fg: parse_color("colors.selected_fg", &colors.selected_fg, &origin)?,
        inactive_fg: parse_color("colors.inactive_fg", &colors.inactive_fg, &origin)?,
        graph_text: parse_color("colors.graph_text", &colors.graph_text, &origin)?,
        meter_bg: parse_color("colors.meter_bg", &colors.meter_bg, &origin)?,
        proc_misc: parse_color("colors.proc_misc", &colors.proc_misc, &origin)?,
        div_line: parse_color("colors.div_line", &colors.div_line, &origin)?,
        session_id: parse_color("colors.session_id", &colors.session_id, &origin)?,
        status_fg: parse_color("colors.status_fg", &colors.status_fg, &origin)?,
        warning_fg: parse_color("colors.warning_fg", &colors.warning_fg, &origin)?,
        cpu_box: parse_color("colors.cpu_box", &colors.cpu_box, &origin)?,
        mem_box: parse_color("colors.mem_box", &colors.mem_box, &origin)?,
        net_box: parse_color("colors.net_box", &colors.net_box, &origin)?,
        proc_box: parse_color("colors.proc_box", &colors.proc_box, &origin)?,
        pi_agent: parse_color("colors.pi_agent", &colors.pi_agent, &origin)?,
        cpu_grad: parse_gradient("gradients.cpu", &gradients.cpu, &origin)?,
        proc_grad: parse_gradient("gradients.process", &gradients.process, &origin)?,
        used_grad: parse_gradient("gradients.used", &gradients.used, &origin)?,
        free_grad: parse_gradient("gradients.free", &gradients.free, &origin)?,
        cached_grad: parse_gradient("gradients.cached", &gradients.cached, &origin)?,
    })
}

fn parse_color(field: &str, value: &str, origin: &ThemeOrigin) -> Result<Color, ThemeLoadError> {
    let rgb = parse_rgb(field, value, origin)?;
    Ok(Color::Rgb(rgb.0, rgb.1, rgb.2))
}

fn parse_rgb(
    field: &str,
    value: &str,
    origin: &ThemeOrigin,
) -> Result<(u8, u8, u8), ThemeLoadError> {
    if value.len() != 7
        || !value.starts_with('#')
        || !value[1..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(origin.invalid(format!(
            "{field} must be a color in #RRGGBB form, got '{value}'"
        )));
    }
    let parse_pair = |range| u8::from_str_radix(&value[range], 16);
    Ok((
        parse_pair(1..3).expect("validated hexadecimal pair"),
        parse_pair(3..5).expect("validated hexadecimal pair"),
        parse_pair(5..7).expect("validated hexadecimal pair"),
    ))
}

fn parse_gradient(
    field: &str,
    values: &[String; 3],
    origin: &ThemeOrigin,
) -> Result<Gradient, ThemeLoadError> {
    Ok(Gradient {
        start: parse_rgb(&format!("{field}[0]"), &values[0], origin)?,
        mid: parse_rgb(&format!("{field}[1]"), &values[1], origin)?,
        end: parse_rgb(&format!("{field}[2]"), &values[2], origin)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;

    fn catalog() -> ThemeCatalog {
        ThemeCatalog::packaged()
    }

    #[test]
    fn every_packaged_theme_loads_through_the_catalog() {
        assert_eq!(THEME_NAMES.len(), BUILT_INS.len());
        for name in THEME_NAMES {
            let theme = catalog()
                .load(&ThemeRequest::BuiltIn((*name).to_string()))
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(matches!(
                theme.source,
                ThemeSource::Packaged { id } if id == *name
            ));
        }
    }

    #[test]
    fn unknown_packaged_theme_is_rejected() {
        assert!(matches!(
            catalog().load(&ThemeRequest::BuiltIn("missing".to_string())),
            Err(ThemeLoadError::UnknownBuiltIn { .. })
        ));
        assert!(Theme::by_name("missing").is_none());
    }

    #[test]
    fn default_is_packaged_btop() {
        let theme = Theme::default();
        assert_eq!(theme.name, "btop");
        assert_eq!(theme.source, ThemeSource::Packaged { id: "btop" });
    }

    #[test]
    fn packaged_palettes_match_golden_hashes() {
        let expected = [
            ("btop", 0xf09666fd28077cf0),
            ("dracula", 0xe76f775e8721b001),
            ("catppuccin", 0x94e92eaa54f46523),
            ("tokyo-night", 0x636a0be69c047804),
            ("gruvbox", 0xd5602a0944617265),
            ("nord", 0x743d3ce8d3e55c08),
            ("light", 0xc19b033786a39898),
            ("white", 0x7bfb7876668462c8),
            ("high-contrast", 0xa2704b8e025b5601),
            ("protanopia", 0xe8f38f64f731c316),
            ("deuteranopia", 0x8eb8c4c4ea3c0346),
            ("tritanopia", 0x08b450b897c2e52f),
        ];

        for (name, hash) in expected {
            let theme = catalog()
                .load(&ThemeRequest::BuiltIn(name.to_string()))
                .unwrap();
            assert_eq!(palette_hash(&theme), hash, "{name} palette changed");
        }
    }

    #[test]
    fn packaged_and_file_sources_use_the_same_decoder() {
        let packaged = catalog()
            .load(&ThemeRequest::BuiltIn("btop".to_string()))
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("btop.toml");
        fs::write(&path, BUILT_INS[0].document).unwrap();
        let mut from_file = catalog().load(&ThemeRequest::File(path.clone())).unwrap();

        assert_eq!(from_file.source, ThemeSource::File { path: path.clone() });
        assert_ne!(packaged.source, from_file.source);
        from_file.source = packaged.source.clone();
        assert_eq!(from_file, packaged);
    }

    #[test]
    fn packaged_identity_does_not_come_from_the_display_name() {
        let renamed = BUILT_INS[0]
            .document
            .replace("name = \"btop\"", "name = \"Custom label\"");
        let theme = decode_validate_theme(renamed.as_bytes(), ThemeOrigin::Packaged { id: "btop" })
            .unwrap();
        assert_eq!(theme.name, "Custom label");
        assert_eq!(theme.source, ThemeSource::Packaged { id: "btop" });
    }

    #[test]
    fn malformed_packaged_theme_uses_the_packaged_error_path() {
        assert!(matches!(
            decode_validate_theme(b"format = 1", ThemeOrigin::Packaged { id: "broken" }),
            Err(ThemeLoadError::InvalidPackaged { id: "broken", .. })
        ));
    }

    #[test]
    fn strict_schema_and_color_validation_reject_bad_documents() {
        let cases = [
            (
                BUILT_INS[0]
                    .document
                    .replacen("format = 1", "format = 2", 1),
                "unsupported format",
            ),
            (
                BUILT_INS[0]
                    .document
                    .replacen("name = \"btop\"", "name = \"   \"", 1),
                "name must not be blank",
            ),
            (
                BUILT_INS[0]
                    .document
                    .replacen("name = \"btop\"", "name = \"\\u001Bbad\"", 1),
                "control characters",
            ),
            (
                BUILT_INS[0]
                    .document
                    .replacen("main_bg = \"#191919\"", "main_bg = \"red\"", 1),
                "#RRGGBB",
            ),
            (
                format!("{}\nunknown = true\n", BUILT_INS[0].document),
                "unknown field",
            ),
            (
                BUILT_INS[0].document.replacen(
                    "main_fg = \"#CCCCCC\"",
                    "main_fg = \"#CCCCCC\"\nextra = \"#000000\"",
                    1,
                ),
                "unknown field",
            ),
            (
                BUILT_INS[0].document.replacen(
                    "main_fg = \"#CCCCCC\"",
                    "main_fg = \"#CCCCCC\"\nmain_fg = \"#000000\"",
                    1,
                ),
                "duplicate key",
            ),
        ];

        for (document, expected) in cases {
            let error =
                decode_validate_theme(document.as_bytes(), ThemeOrigin::Packaged { id: "test" })
                    .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn rejects_invalid_utf8_and_nonregular_paths() {
        let dir = tempfile::tempdir().unwrap();
        let invalid_utf8 = dir.path().join("invalid.toml");
        fs::write(&invalid_utf8, [0xff, 0xfe]).unwrap();
        assert!(matches!(
            catalog().load(&ThemeRequest::File(invalid_utf8)),
            Err(ThemeLoadError::InvalidUtf8 { .. })
        ));
        assert!(matches!(
            catalog().load(&ThemeRequest::File(dir.path().to_path_buf())),
            Err(ThemeLoadError::NonRegularFile { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_fifos_without_blocking() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::time::{Duration, Instant};

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("theme.fifo");
        let native_path = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(native_path.as_ptr(), 0o600) }, 0);

        let started = Instant::now();
        assert!(matches!(
            catalog().load(&ThemeRequest::File(path)),
            Err(ThemeLoadError::NonRegularFile { .. })
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn accepts_exact_limit_and_rejects_one_byte_over() {
        let dir = tempfile::tempdir().unwrap();
        let exact_path = dir.path().join("exact.toml");
        let mut exact = BUILT_INS[0].document.as_bytes().to_vec();
        exact.push(b'#');
        exact.resize(MAX_THEME_BYTES, b'x');
        fs::write(&exact_path, exact).unwrap();
        catalog()
            .load(&ThemeRequest::File(exact_path))
            .expect("an exact-size valid document should load");

        let oversized_path = dir.path().join("oversized.toml");
        let mut file = File::create(&oversized_path).unwrap();
        file.write_all(&vec![b'x'; MAX_THEME_BYTES + 1]).unwrap();
        assert!(matches!(
            catalog().load(&ThemeRequest::File(oversized_path)),
            Err(ThemeLoadError::TooLarge { .. })
        ));
    }

    fn palette_hash(theme: &Theme) -> u64 {
        let mut bytes = Vec::new();
        for color in [
            theme.main_bg,
            theme.main_fg,
            theme.title,
            theme.hi_fg,
            theme.selected_bg,
            theme.selected_fg,
            theme.inactive_fg,
            theme.graph_text,
            theme.meter_bg,
            theme.proc_misc,
            theme.div_line,
            theme.session_id,
            theme.status_fg,
            theme.warning_fg,
            theme.cpu_box,
            theme.mem_box,
            theme.net_box,
            theme.proc_box,
            theme.pi_agent,
        ] {
            let Color::Rgb(red, green, blue) = color else {
                panic!("packaged themes must use RGB colors")
            };
            bytes.extend([red, green, blue]);
        }
        for gradient in [
            &theme.cpu_grad,
            &theme.proc_grad,
            &theme.used_grad,
            &theme.free_grad,
            &theme.cached_grad,
        ] {
            for point in [gradient.start, gradient.mid, gradient.end] {
                bytes.extend([point.0, point.1, point.2]);
            }
        }

        bytes.into_iter().fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        })
    }
}
