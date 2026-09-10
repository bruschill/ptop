# Themes

ptop includes 12 packaged themes and accepts user-provided Theme Format v1 TOML files.

## Select a theme

Use a packaged theme:

```bash
ptop --theme nord
```

Use a theme file:

```bash
ptop --theme-file ./examples/themes/low-glare.toml
```

The two flags are mutually exclusive. Theme selection uses this order:

1. `--theme-file PATH`
2. `--theme NAME`
3. `theme_file` in the config file
4. `theme` in the config file
5. packaged `btop`

A relative command-line path is resolved from the directory where ptop starts. A relative config path is resolved from the config file's directory. `~/` is expanded when a home directory is available. Windows also accepts `~\`.

The config file is stored under the platform config directory returned by `dirs::config_dir()`:

- Linux: usually `~/.config/ptop/config.toml`
- macOS: usually `~/Library/Application Support/ptop/config.toml`
- Windows: usually `%APPDATA%\ptop\config.toml`

Example:

```toml
theme_file = "themes/low-glare.toml"
```

Press `t` to cycle packaged themes. If a file theme is active, the first press selects packaged `btop` and saves that selection.

## Theme Format v1

Every field is required. Unknown and duplicate fields are rejected.

```toml
format = 1
name = "My theme"

[colors]
main_bg = "#191919"
main_fg = "#CCCCCC"
title = "#EEEEEE"
hi_fg = "#B54040"
selected_bg = "#6A2F2F"
selected_fg = "#EEEEEE"
inactive_fg = "#404040"
graph_text = "#606060"
meter_bg = "#404040"
proc_misc = "#0DE756"
div_line = "#303030"
session_id = "#B0A070"
status_fg = "#DC4C4C"
warning_fg = "#DCA032"
cpu_box = "#556D59"
mem_box = "#6C6C4B"
net_box = "#5C588D"
proc_box = "#805252"
claude_agent = "#D97757"
codex_agent = "#7A9DFF"
opencode_agent = "#4ADE80"
pi_agent = "#C084FC"

[gradients]
cpu = ["#77CA9B", "#CBC06C", "#DC4C4C"]
process = ["#80D0A3", "#DCD179", "#D45454"]
used = ["#592B26", "#D9626D", "#FF4769"]
free = ["#384F21", "#B5E685", "#DCFF85"]
cached = ["#163350", "#74E6FC", "#26C5FF"]
```

Colors must use `#RRGGBB`. Each gradient requires exactly three colors: start, midpoint, and end. The theme name must be nonblank and cannot contain terminal control characters.

See [`examples/themes/low-glare.toml`](../examples/themes/low-glare.toml) for a complete file.

## Validation and errors

Theme files must be:

- regular files, or symlinks that resolve to regular files
- valid UTF-8
- no larger than 64 KiB
- valid Theme Format v1 documents

ptop validates the selected theme before entering terminal raw mode. Invalid files produce one stderr diagnostic and a nonzero exit. A valid command-line theme overrides an invalid configured theme selection.

Theme Format v1 does not support inheritance, imports, aliases, expressions, discovery directories, or live reload.

## Packaged themes

Packaged themes use the same Theme Format v1 documents and loader as user files. They are embedded in the binary, so installed builds do not need runtime theme assets.
