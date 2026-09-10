# ptop

**Like [btop](https://github.com/aristocratos/btop), but for Pi coding agents.**

ptop monitors local Pi processes, owned session telemetry, child processes, subagent runs, listening ports, and project state from one terminal UI.

The previous Claude Code, Codex CLI, and OpenCode monitor remains available through `ptop --legacy` during the transition.

## What ptop shows

- Local Pi processes, child processes, listening ports, and project state.
- Context and usage telemetry when a session file can be attached with high-confidence ownership evidence.
- Local `pi-subagents` run state, usage, children, and terminal proof from supported `status.json` files.
- Unknown, inferred, estimated, and partial telemetry as distinct states instead of reporting missing data as zero.
- Run details inside the selected session on compact terminals and in a dedicated Runs panel at 140 columns or wider.
- Structured Pi telemetry in `ptop --json` for local tools and dashboards.

ptop is read-only and makes no network calls. It does not require API keys or authentication.

## Install

### macOS / Linux

> [!IMPORTANT]
> On Linux, ensure `sqlite3` is installed to enable monitoring for OpenCode sessions.

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/bruschill/ptop/releases/latest/download/ptop-installer.sh | sh
```

### Cargo

```bash
cargo install ptop
```

### Windows

Native support, with no WSL required. Uses `sysinfo` for process info and host CPU/MEM metrics, and `netstat -ano` for listening ports. Windows Pi support is process-only; Pi terminal jump and kill controls are disabled until trusted identity checks are available. Windows has no load average, so LOAD is reported as 0. OpenCode discovery in legacy mode additionally requires the `sqlite3` CLI (`winget install SQLite.SQLite`); without it ptop prints a one-time warning to stderr.

```powershell
powershell -c "irm https://github.com/bruschill/ptop/releases/latest/download/ptop-installer.ps1 | iex"
```

Or `cargo install ptop` from any terminal with Git in PATH. Claude Code config is resolved automatically from `%USERPROFILE%\.claude`.

### Other

Pre-built binaries for all platforms are available on the [GitHub Releases](https://github.com/bruschill/ptop/releases) page.

## Usage

```bash
ptop                    # Launch the ptop TUI
ptop --once             # Print a ptop snapshot and exit
ptop --json             # Print one ptop JSON snapshot and exit
ptop --legacy           # Use Claude/Codex/OpenCode collection
ptop --setup            # Install the legacy Claude rate-limit hook
ptop --theme dracula    # Launch with a packaged theme
ptop --theme-file ./my-theme.toml  # Launch with a user theme
ptop --mouse            # Enable mouse click/scroll navigation
```

Recommended terminal size: **120x40** or larger. The minimum is 80x24, and panels hide when space is limited. Pi run metadata stays in the selected-session detail at 80x24 and 100x24. At 140 columns or wider, available runs move to a dedicated Runs panel.
Mouse capture is off by default so terminal drag selection and copy keep working. Launch with `--mouse` if you prefer click targets and wheel navigation.

### Terminal Jump

Press `Enter` to focus the terminal running the selected agent. ptop supports cmux, tmux, and iTerm2 on macOS when it can verify the target session.

```bash
tmux new -s work
# pane 0: ptop
# pane 1: claude (project A)
# pane 2: claude (project B)
# → Enter on a session in ptop jumps to its pane
```

## Modes and supported agents

ptop is the default Pi monitor. Run `ptop --legacy` to monitor Claude Code, Codex CLI, and OpenCode with the previous collectors.

| Feature | Pi | Claude Code | Codex CLI | OpenCode |
| --- | :---: | :---: | :---: | :---: |
| Process discovery | ✅ | ✅ | ✅ | ✅ |
| Owned session telemetry | macOS/Linux | ✅ | ✅ | ✅ |
| Token tracking | Attached sessions | ✅ | ✅ | ✅ |
| Context window | Attached sessions | ✅ | ✅ | ❌ |
| Parent activity state | Unknown | ✅ | ✅ | ✅ |
| Current task text | Hidden | ✅ | ✅ | ❌ |
| Account rate limit | — | ✅ | ✅ | ❌ |
| Git status | ✅ | ✅ | ✅ | ✅ |
| Children / ports | ✅ | ✅ | ✅ | ✅ |
| Subagent run metadata | `status.json` | ✅ | ❌ | ❌ |
| Memory status | — | ✅ | ❌ | ❌ |

Pi telemetry requires unambiguous ownership. Windows stays process-only. See [Pi support and release gates](docs/pi-support.md) for platform boundaries, privacy rules, parser limits, compatibility policy, and validation commands.

OpenCode support reads the local SQLite database at `~/.local/share/opencode/opencode.db` (also the default location on Windows; `%LOCALAPPDATA%\opencode` and `%APPDATA%\opencode` are probed as fallbacks) and requires `sqlite3` in `PATH` (on Windows: `winget install SQLite.SQLite`).

## Themes

12 packaged themes are included, with 4 colorblind-friendly options (`high-contrast`, `protanopia`, `deuteranopia`, `tritanopia`). Press `t` to cycle packaged themes, launch one with `--theme <name>`, or load a Theme Format v1 TOML file with `--theme-file <path>`. Packaged and user themes use the same loader. See [Theme files](docs/themes.md).

| btop (default) | dracula | catppuccin |
|:-:|:-:|:-:|
| ![btop](https://raw.githubusercontent.com/bruschill/ptop/main/assets/themes/btop.png) | ![dracula](https://raw.githubusercontent.com/bruschill/ptop/main/assets/themes/dracula.png) | ![catppuccin](https://raw.githubusercontent.com/bruschill/ptop/main/assets/themes/catppuccin.png) |

| tokyo-night | gruvbox | nord |
|:-:|:-:|:-:|
| ![tokyo-night](https://raw.githubusercontent.com/bruschill/ptop/main/assets/themes/tokyo-night.png) | ![gruvbox](https://raw.githubusercontent.com/bruschill/ptop/main/assets/themes/gruvbox.png) | ![nord](https://raw.githubusercontent.com/bruschill/ptop/main/assets/themes/nord.png) |

Colorblind-friendly themes:

| high-contrast | protanopia |
|:-:|:-:|
| ![high-contrast](https://raw.githubusercontent.com/bruschill/ptop/main/assets/themes/high-contrast.png) | ![protanopia](https://raw.githubusercontent.com/bruschill/ptop/main/assets/themes/protanopia.png) |

| deuteranopia | tritanopia |
|:-:|:-:|
| ![deuteranopia](https://raw.githubusercontent.com/bruschill/ptop/main/assets/themes/deuteranopia.png) | ![tritanopia](https://raw.githubusercontent.com/bruschill/ptop/main/assets/themes/tritanopia.png) |

Light themes (`light` — Solarized cream, `white` — GitHub-style pure white) for bright terminals:

| light | white |
|:-:|:-:|
| ![light](https://raw.githubusercontent.com/bruschill/ptop/main/assets/themes/light.png) | ![white](https://raw.githubusercontent.com/bruschill/ptop/main/assets/themes/white.png) |

## Configuration

The `ptop/config.toml` file under your platform config directory supports:

```toml
# Choose either a packaged theme or a Theme Format v1 file.
theme = "btop"
# theme_file = "themes/low-glare.toml"
# Hide agent CLIs from the TUI (case-insensitive).
# Use "pi" in the default mode, or legacy CLI names with --legacy.
hidden_agents = ["codex"]
# Additional Claude Code profile roots to scan.
# ptop also auto-discovers ~/.claude and ~/.claude-* roots that contain
# both sessions/ and projects/.
claude_config_dirs = ["~/.claude-personal", "~/.claude-work-team"]
# UI language. Omit or leave empty to auto-detect from LANG.
language = "zh"
# Panel visibility. Pi mode always suppresses quota and MCP panels.
show_context = true
show_quota = true
show_tokens = true
show_projects = true
show_ports = true
show_sessions = true
show_mcp = true
```

### Supported Languages

| Code | Language            |
| ---- | ------------------- |
| `en` | English (default)   |
| `zh` | Simplified Chinese  |

When `language` is unset, ptop auto-detects from `LANG`. Any value starting with `zh` switches to Simplified Chinese; other values use English.

## Key Bindings

| Key                | Action                               |
| ------------------ | ------------------------------------ |
| `↑`/`↓` or `k`/`j` | Select session                       |
| `Enter`            | Jump to session terminal             |
| `x`                | Kill selected session                |
| `X`                | Kill all orphan ports                |
| `t`                | Cycle theme                          |
| `1`-`7`            | Toggle panel visibility              |
| `Esc`              | Open/close config page               |
| `q`                | Quit                                 |
| `r`                | Force refresh                        |

## Library / JSON snapshot

The `ptop` package is also a library crate, so local tools can reuse its data-collection
layer in-process without rescanning or subprocesses and serialize the same
state the TUI renders.

```bash
ptop --json    # one-shot JSON snapshot for scripts
```

For long-running consumers, build an `App`, refresh it with
`App::tick_no_summaries()` (which never spawns `claude --print`, so it doesn't
touch your Claude quota), and call `App::to_snapshot(interval_ms)` to get a
JSON-serializable [`Snapshot`]:

```rust,no_run
use ptop::app::App;
use ptop::{config, theme::Theme};

let cfg = config::load_config();
let mut app = App::new_pi(
    Theme::default(), &cfg.hidden_agents, cfg.panels,
);
app.tick_no_summaries();
let json = serde_json::to_string(&app.to_snapshot(2_000)).unwrap();
```

`App` is not `Send` (it owns the collectors), so keep it on one thread and pass
the serialized JSON elsewhere.

## Privacy

ptop reads local files and local process/open-file metadata only. No API keys, no auth. The default Pi mode is metadata-only and does not generate summaries or make network calls. It reads owned parent session JSONL for identity and numeric telemetry, but does not retain or publish prompt text, assistant text, tool arguments, or tool results. Its subagent adapter reads supported `status.json` files only, not child transcripts, prompt files, events, output logs, or tool-argument files. Legacy mode can generate session summaries through `claude --print`; that command may call the Claude API.

The JSON snapshot includes `monitor_mode`, `token_rate_value`, and structured Pi `telemetry` fields. Process-only Pi rows use `null` for authoritative context, usage, and token-rate values; the legacy numeric fields remain compatibility placeholders. Pi output never includes child command arguments, prompts, task text, tool arguments, tool results, or transcript content. It reduces child commands to executable labels. Legacy snapshots can include `summary`, `chat_messages`, working directories, config roots, tool-call previews, child process commands, token counts, and port metadata. Treat snapshots as local/private data. Do not write them to shared logs or expose them on a network without access controls.

## Acknowledgements

ptop grew from [graykode/abtop](https://github.com/graykode/abtop). Pi agent support started in [graykode/abtop#63](https://github.com/graykode/abtop/pull/63), opened by [@ptahdunbar](https://github.com/ptahdunbar).

Huge thanks to [@tbouquet](https://github.com/tbouquet) for driving much of ptop's recent shape: themes, config overlay and panel toggles, session filtering, subagent tree view, the context window gauge with compaction detection, and many fixes and security improvements.

## License

MIT
