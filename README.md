# ptop

**Like [abtop](https://github.com/graykode/abtop), but for Pi coding agents**

ptop is a local terminal monitor for Pi processes, owned session telemetry, fleet runs, child processes, listening ports, and project state.

![ptop demo using the madeofcode theme](https://raw.githubusercontent.com/bruschill/ptop/main/assets/demo.png)

The demo uses the user-created [madeofcode](https://github.com/bruschill/madeofcode) theme.

## What ptop shows

- Pi sessions with status, model, thinking level, context, tokens, memory, and current telemetry state.
- `pi-subagents` fleet runs and child lifecycle metadata.
- Per-project Git branch and working-tree counts.
- Child processes, listening ports, port conflicts, and orphan ports.
- Host CPU, memory, and load metrics.
- Process-only Pi rows when telemetry cannot be attached safely.

ptop reads local process and filesystem metadata. It does not call an agent API.

## Supported Pi packages

- [`pi-subagents`](https://github.com/nicobailon/pi-subagents): fleet run and child lifecycle telemetry.

## Install

### macOS / Linux

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/bruschill/ptop/releases/latest/download/ptop-installer.sh | sh
```

### Cargo

```bash
cargo install ptop
```

### Windows

```powershell
powershell -c "irm https://github.com/bruschill/ptop/releases/latest/download/ptop-installer.ps1 | iex"
```

On Windows, ptop shows Pi processes, host metrics, listening ports, and Git status. Session-file attachment and `pi-subagents` fleet telemetry are unavailable; terminal jump and process kill controls are disabled until trusted identity checks are available. Windows reports load average as 0.

Pre-built binaries are available on the [GitHub Releases](https://github.com/bruschill/ptop/releases) page.

## Usage

```bash
ptop                    # Launch the TUI
ptop --once             # Print one text snapshot and exit
ptop --json             # Print one JSON snapshot and exit
ptop --demo             # Show collector-free Pi demo data
ptop --theme dracula    # Use a packaged theme
ptop --theme-file ./my-theme.toml  # Use a Theme Format v1 file
ptop --mouse            # Enable mouse click and scroll navigation
ptop --exit-on-jump     # Quit after Enter jumps to a session terminal
ptop --update           # Update ptop
ptop --version          # Print the version
```

Unknown and removed options fail with a clear error.

### Terminal jump

Press `Enter` to focus the terminal that owns the selected Pi process. On macOS, ptop tries Herdr, cmux, tmux, then iTerm2. On Linux, it tries Herdr, cmux, then tmux. When ptop and the selected Pi share a Herdr server, ptop focuses the exact pane and its tab, then brings that workspace forward while ptop keeps running. The process command and start identity are checked again before a jump. Windows does not support terminal jump.

Example with tmux:

```bash
tmux new -s work
# pane 0: ptop
# pane 1: pi (project A)
# pane 2: pi (project B)
# Enter on a session in ptop jumps to its pane
```

## Pi telemetry

ptop always discovers live Pi processes first. It attaches a Pi JSONL session only when ownership is unambiguous and the session identity and working directory match. A process remains visible as `process only` when attachment is missing, stale, unsupported, or ambiguous.

Model and provider names come from Pi metadata. A value such as `claude-opus-4-6` identifies the model used by Pi, not another monitored agent.

Fleet data comes from supported `pi-subagents` `status.json` files. Parent usage and run usage remain separate to prevent double counting. See [Pi support and release gates](docs/pi-support.md) for the full telemetry, platform, parser, and privacy contracts.

## Themes

Use `--theme <name>` or set `theme` in the configuration file. Packaged themes:

`btop`, `dracula`, `catppuccin`, `tokyo-night`, `gruvbox`, `nord`, `high-contrast`, `protanopia`, `deuteranopia`, `tritanopia`, `light`, and `white`.

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

Light themes (`light`, a Solarized cream theme, and `white`, a GitHub-style pure white theme):

| light | white |
|:-:|:-:|
| ![light](https://raw.githubusercontent.com/bruschill/ptop/main/assets/themes/light.png) | ![white](https://raw.githubusercontent.com/bruschill/ptop/main/assets/themes/white.png) |

You can create your own theme as a Theme Format v1 TOML file and load it with `--theme-file <path>`. See [Theme files](docs/themes.md).

## Configuration

ptop stores `config.toml` under the platform configuration directory:

- Linux: `$XDG_CONFIG_HOME/ptop/config.toml`, normally `~/.config/ptop/config.toml`.
- macOS: `~/Library/Application Support/ptop/config.toml`.
- Windows: `%APPDATA%\ptop\config.toml`.

```toml
# Choose either a packaged theme or a Theme Format v1 file.
theme = "btop"
# theme_file = "themes/low-glare.toml"

# UI language. Empty means auto-detect from LANG.
language = "en"

# Panel visibility.
show_context = true
show_tokens = true
show_projects = true
show_ports = true
show_sessions = true
```

Unknown keys are ignored and preserved when ptop rewrites known settings. This keeps obsolete configuration keys harmless.

### Supported languages

- English
- Simplified Chinese

## Key bindings

| Key | Action |
|---|---|
| `↑`/`↓` or `k`/`j` | Select a session |
| `←`/`→`, `Shift+Tab`/`Tab` | Change compact-layout tab |
| `w`, `u`, `s` | Open Work, Usage, or System tab |
| `+` / `-` | Maximize or restore the active compact section |
| `Enter` | Jump to the selected session terminal |
| `x` | Confirm and kill the selected Pi process |
| `X` | Kill verified orphan-port processes |
| `/` | Filter sessions |
| `Esc` | Clear the filter or close an overlay |
| `1`–`5` | Toggle context, tokens, projects, ports, or sessions |
| `t` | Cycle the theme |
| `v` | Open the view menu |
| `c` | Open configuration |
| `r` | Refresh |
| `?` | Show help |
| `q` | Quit |

Mouse input is disabled by default. Pass `--mouse` to enable clicks and scrolling.

## Library and JSON snapshot

```bash
ptop --json
```

The JSON snapshot includes host metrics, aggregate values, authoritative Pi telemetry, bounded token history, fleet runs, privacy-safe child process labels, and orphan ports. Unknown telemetry uses `null` in authoritative fields instead of a numeric placeholder.

Library example:

```rust,no_run
use ptop::app::App;
use ptop::{config, theme::Theme};

let cfg = config::load_config();
let mut app = App::new(Theme::default(), cfg.panels);
app.tick();
let json = serde_json::to_string(&app.to_snapshot(2_000)).unwrap();
# let _ = json;
```

`App::to_snapshot` is a pure read. Call `App::tick` before taking a fresh snapshot.

## Privacy

ptop does not retain or publish prompt text, assistant text, tool arguments, tool results, or child transcripts. Pi session parsing keeps identity, safe metadata, and structured numeric telemetry only. Fleet collection reads supported lifecycle status metadata, not child output logs or transcript files.

TUI, text snapshots, and JSON snapshots reduce child and orphan commands to executable labels where output leaves the collector. ptop performs no network requests. Package installation and `ptop --update` are separate user-requested operations.

## Acknowledgements

ptop grew from [graykode/abtop](https://github.com/graykode/abtop). Pi agent support started in [graykode/abtop#63](https://github.com/graykode/abtop/pull/63), opened by [@ptahdunbar](https://github.com/ptahdunbar).

## License

MIT
