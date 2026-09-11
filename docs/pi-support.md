# Pi support and release gates

ptop discovers local Pi processes by default, then attaches telemetry only when it can prove ownership of the session file without ambiguity. Use `ptop --legacy` for the Claude Code, Codex CLI, and OpenCode collectors during stabilization.

## Platform contract

| Platform | Process discovery | Owned session telemetry | Controls |
| --- | --- | --- | --- |
| macOS | Supported and tested | Supported through a directly open session JSONL file or a validated Herdr pane claim | Terminal jump and kill require the existing backend-specific identity checks |
| Linux | Supported and tested | Supported through bounded `/proc` environment markers, directly open session JSONL files, or a validated Herdr pane claim | tmux jump and kill require the existing identity checks |
| Windows | Supported and tested | Process-only | Pi terminal jump and kill are disabled |

Windows stays process-only until the process layer exposes a trustworthy working directory and process-start identity. Other platforms are not part of the release-tested Pi contract.

## Telemetry contract

A Pi process row always remains visible. Failed, incomplete, conflicting, or unsupported telemetry does not remove that row.

- `process only` means no owned session JSONL is attached.
- `attached` means ownership passed the high-confidence identity checks.
- In Herdr, ptop accepts a session path only when the Pi process names the pane, Herdr reports that PID in the pane's foreground process group, and two pane snapshots have the same session path and revision.
- Herdr `working` maps to executing; `idle`, `done`, and `blocked` map to waiting.
- Unknown values render as `—`, not zero.
- `~` marks inferred values.
- `≈` marks estimated values.
- `+` after tokens marks a partial total.
- Structured `telemetry` fields in JSON are authoritative for Pi.
- Legacy numeric JSON fields remain compatibility placeholders for Pi consumers.
- Unknown `status.json` schema versions expose bounded identity metadata only. They do not claim lifecycle, usage, child, or terminal state.
- Fleet usage remains a separate run aggregate because parent session totals may already include child usage.

Runs remain in the selected-session detail at 80x24 and 100x24. At 140 columns or wider, an available selected-session run list moves to a dedicated Runs panel. The same run is not rendered in both places.

## Privacy boundary

ptop reads local process metadata, owned parent session JSONL files, model catalog metadata, and supported `pi-subagents` `status.json` files. It does not read child session transcripts, prompt files, `events.jsonl`, output logs, or tool-argument files from the subagent run directory.

The owned parent JSONL parser inspects records for identity, usage, model metadata, and context size. It does not retain or publish prompt text, assistant text, tool arguments, or tool results. Pi snapshots and TUI rows reduce child process commands to executable names.

ptop does not generate summaries or call `claude --print`. `--once`, `--json`, the TUI, and library callers using `tick_no_summaries()` keep that behavior.

## Parser and retention limits

The limits below are release contracts. Exceeding a limit keeps the process row visible and marks telemetry partial, omitted, unavailable, or unhealthy instead of publishing an incomplete value as complete.

### Parent session JSONL

- 2 MiB of Pi session JSONL per collection tick across attached sessions.
- 1 MiB per Pi session JSONL line.
- 8192 semantic tree entries per attached session.
- 128 attachment candidates per collection tick.
- 128 Pi processes considered for attachment per collection tick.
- 4096 open file descriptors per collection tick.
- Herdr discovery checks at most 32 Pi roots per refresh, reads at most 256 KiB per command, allows 300 ms per command, and stops after 1 second total.
- No retained partial-line buffer. An incomplete final line is reread from its newline boundary on the next tick.
- Model catalog files are capped at 2 MiB and 1024 retained model entries.
- Tail and model caches are removed when their owned live attachment disappears.

### Fleet status metadata

- 256 KiB per status.json file.
- 2 MiB of status.json data per collection tick.
- 512 run-directory entries examined per collection tick.
- 128 status files per collection tick.
- 20 runs per parent session.
- 64 children per run across 3 nested child levels.
- Completed runs remain eligible for 30 days.
- Active snapshots with no verifiable runner become stale after 24 hours. A snapshot whose verified runner has disappeared becomes stale after 30 seconds.

## Performance benchmark

Run the bounded parser benchmark in release mode:

```bash
cargo test --release pi_parser_release_benchmark -- --ignored --nocapture
```

The benchmark parses and reconstructs up to one full 2 MiB tick budget. The release gate requires it to finish within one second. The byte, entry, and retention caps remain the primary deterministic controls. Wall-clock results still depend on the host.

## Release checklist

Run these checks before publishing:

```bash
scripts/check-rustfmt.sh "$(git merge-base HEAD origin/main)"
cargo clippy --all-targets -- -D warnings -A clippy::uninlined-format-args
cargo test --all-targets
cargo build --release
cargo test --release pi_parser_release_benchmark -- --ignored --nocapture
```

CI runs the build, Clippy, and test suite on macOS, Linux, and Windows. It also runs explicit legacy-constructor regression tests and the parser benchmark on Linux. The rustfmt gate checks Rust files changed from the provided base, including committed, staged, and unstaged changes, because the older codebase does not yet pass a whole-repository Rust 1.88 formatting check. The Clippy gate allows the existing `uninlined_format_args` style lint so unrelated formatting does not block the release; every other warning remains denied.

Before changing a support claim, verify:

- Privacy tests still exclude private content and prohibited subagent artifact paths.
- Ambiguous ownership still leaves a process-only row.
- JSON, text, and TUI output still distinguish unknown from known zero.
- 80x24, 100x24, and wide desktop layout tests pass.
- Legacy collector construction and snapshot compatibility tests pass.
- Platform-specific tests match the documented support table.

## Stabilization policy

Pi and `pi-subagents` files are local implementation contracts that may change. New schema versions remain unsupported until capability tests define each accepted field. Add fields to the structured Pi snapshot contract instead of changing legacy meanings. Keep `--legacy` available until a separate compatibility decision removes it.
