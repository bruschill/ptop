# abtop

**Like [btop](https://github.com/aristocratos/btop), but for your AI coding agents.**

The default Pi Fleet mode shows local Pi coding-agent processes, child processes, open ports, and project state. It reports session telemetry as unavailable until it can attach a session file with high-confidence ownership evidence.

The previous Claude Code, Codex CLI, and OpenCode monitor remains available through `abtop --legacy` during the Pi rollout.

![demo](https://raw.githubusercontent.com/graykode/abtop/main/assets/demo.gif)

## Why

- Running 3+ agents across projects? See them all in one screen.
- Using legacy mode? Watch supported provider quota in real time.
- Agent spawned a server and forgot to kill it? Orphan port detection.
- When telemetry is available, see per-session context warnings.

All read-only. No API keys. No auth.

## Install

### macOS / Linux

> [!IMPORTANT]
> On Linux, ensure `sqlite3` is installed to enable monitoring for OpenCode sessions.

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/graykode/abtop/releases/latest/download/abtop-installer.sh | sh
```

### Cargo

```bash
cargo install abtop
```

### Windows

Native support, with no WSL required. Uses `sysinfo` for process info and host CPU/MEM metrics, and `netstat -ano` for listening ports. Windows Pi support is process-only; Pi terminal jump and kill controls are disabled until trusted identity checks are available. Windows has no load average, so LOAD is reported as 0. OpenCode discovery in legacy mode additionally requires the `sqlite3` CLI (`winget install SQLite.SQLite`); without it abtop prints a one-time warning to stderr.

```powershell
powershell -c "irm https://github.com/graykode/abtop/releases/latest/download/abtop-installer.ps1 | iex"
```

Or `cargo install abtop` from any terminal with Git in PATH. Claude Code config is resolved automatically from `%USERPROFILE%\.claude`.

### Other

Pre-built binaries for all platforms are available on the [GitHub Releases](https://github.com/graykode/abtop/releases) page.

## Usage

```bash
abtop                    # Launch the Pi Fleet TUI
abtop --once             # Print a Pi Fleet snapshot and exit
abtop --json             # Print one Pi Fleet JSON snapshot and exit
abtop --legacy           # Use Claude/Codex/OpenCode collection
abtop --setup            # Install the legacy Claude rate-limit hook
abtop --theme dracula    # Launch with a specific theme
abtop --mouse            # Enable mouse click/scroll navigation
```

Recommended terminal size: **120x40** or larger. The minimum is 80x24, and panels hide when space is limited. Pi run metadata stays in the selected-session detail at 80x24 and 100x24. At 140 columns or wider, available runs move to a dedicated Runs panel.
Mouse capture is off by default so terminal drag selection and copy keep working. Launch with `--mouse` if you prefer click targets and wheel navigation.

### Terminal Jump

Press `Enter` to focus the terminal running the selected agent. abtop supports cmux, tmux, and iTerm2 on macOS.

```bash
tmux new -s work
# pane 0: abtop
# pane 1: claude (project A)
# pane 2: claude (project B)
# → Enter on a session in abtop jumps to its pane
```

## Supported Agents

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

Pi telemetry requires unambiguous ownership. Windows stays process-only. See [Pi Fleet support and release gates](docs/pi-fleet.md) for platform boundaries, privacy rules, parser limits, compatibility policy, and validation commands.

OpenCode support reads the local SQLite database at `~/.local/share/opencode/opencode.db` (also the default location on Windows; `%LOCALAPPDATA%\opencode` and `%APPDATA%\opencode` are probed as fallbacks) and requires `sqlite3` in `PATH` (on Windows: `winget install SQLite.SQLite`).

## Themes

12 built-in themes, including 4 colorblind-friendly options (`high-contrast`, `protanopia`, `deuteranopia`, `tritanopia`). Press `t` to cycle at runtime, or launch with `--theme <name>`. Your choice is saved to `~/.config/abtop/config.toml`.

| btop (default) | dracula | catppuccin |
|:-:|:-:|:-:|
| ![btop](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/btop.png) | ![dracula](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/dracula.png) | ![catppuccin](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/catppuccin.png) |

| tokyo-night | gruvbox | nord |
|:-:|:-:|:-:|
| ![tokyo-night](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/tokyo-night.png) | ![gruvbox](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/gruvbox.png) | ![nord](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/nord.png) |

Colorblind-friendly themes:

| high-contrast | protanopia |
|:-:|:-:|
| ![high-contrast](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/high-contrast.png) | ![protanopia](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/protanopia.png) |

| deuteranopia | tritanopia |
|:-:|:-:|
| ![deuteranopia](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/deuteranopia.png) | ![tritanopia](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/tritanopia.png) |

Light themes (`light` — Solarized cream, `white` — GitHub-style pure white) for bright terminals:

| light | white |
|:-:|:-:|
| ![light](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/light.png) | ![white](https://raw.githubusercontent.com/graykode/abtop/main/assets/themes/white.png) |

## Configuration

`~/.config/abtop/config.toml` supports:

```toml
theme = "btop"
# Hide agent CLIs from the TUI (case-insensitive).
# Use "pi" for Pi Fleet, or legacy CLI names with --legacy.
hidden_agents = ["codex"]
# Additional Claude Code profile roots to scan.
# abtop also auto-discovers ~/.claude and ~/.claude-* roots that contain
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

When `language` is unset, abtop auto-detects from `LANG`. Any value starting with `zh` switches to Simplified Chinese; other values use English.

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

abtop is also a library crate, so local tools can reuse its data-collection
layer in-process without rescanning or subprocesses and serialize the same
state the TUI renders.

```bash
abtop --json    # one-shot JSON snapshot for scripts
```

For long-running consumers, build an `App`, refresh it with
`App::tick_no_summaries()` (which never spawns `claude --print`, so it doesn't
touch your Claude quota), and call `App::to_snapshot(interval_ms)` to get a
JSON-serializable [`Snapshot`]:

```rust,no_run
use abtop::app::App;
use abtop::{config, theme::Theme};

let cfg = config::load_config();
let mut app = App::new_pi(
    Theme::default(), &cfg.hidden_agents, cfg.panels,
);
app.tick_no_summaries();
let json = serde_json::to_string(&app.to_snapshot(2_000)).unwrap();
```

`App` is not `Send` (it owns the collectors), so keep it on one thread and pass
the serialized JSON elsewhere. [abtop-web-ui](https://github.com/XKHoshizora/abtop-web-ui)
is a reference consumer: a local-first web dashboard built on exactly this API.

## Privacy

abtop reads local files and local process/open-file metadata only. No API keys, no auth. Pi Fleet mode is metadata-only and does not generate summaries or make network calls. It reads owned parent session JSONL for identity and numeric telemetry, but does not retain or publish prompt text, assistant text, tool arguments, or tool results. Its subagent adapter reads supported `status.json` files only, not child transcripts, prompt files, events, output logs, or tool-argument files. Legacy mode can generate session summaries through `claude --print`; that command may call the Claude API.

The JSON snapshot includes `monitor_mode`, `token_rate_value`, and structured Pi `telemetry` fields. Process-only Pi rows use `null` for authoritative context, usage, and token-rate values; the legacy numeric fields remain compatibility placeholders. Pi output never includes child command arguments, prompts, task text, tool arguments, tool results, or transcript content. It reduces child commands to executable labels. Legacy snapshots can include `summary`, `chat_messages`, working directories, config roots, tool-call previews, child process commands, token counts, and port metadata. Treat snapshots as local/private data. Do not write them to shared logs or expose them on a network without access controls.

## Acknowledgements

Huge thanks to [@tbouquet](https://github.com/tbouquet) for driving much of abtop's recent shape — themes, config overlay and panel toggles, session filtering, subagent tree view, the context window gauge with compaction detection, plus a steady stream of fixes and security hardening along the way.

## License

MIT
