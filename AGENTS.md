# ptop

Local terminal monitor for Pi coding-agent processes, sessions, and `pi-subagents` runs.

## Language policy

English is mandatory for all project-facing work and communication.

- Write source code, comments, tests, fixtures, documentation, examples, configuration, scripts, and user-facing strings in English.
- Use English for GitHub issues, comments, pull requests, reviews, commits, branches, releases, labels, milestones, discussions, and CI messages.
- Preserve non-English text only when it is an exact external identifier, protocol value, or short direct quote with an English explanation.

## Product boundary

ptop monitors Pi only. Do not add another top-level coding-agent collector, mode, label, quota source, or transcript view.

Model and provider metadata reported by Pi may contain names such as Claude or Codex. Treat those values as Pi session metadata, not as separate monitored agents.

Keep:

- Pi process and session discovery.
- Safe Pi JSONL attachment and numeric telemetry.
- `pi-subagents` fleet and run telemetry.
- Process trees, child commands, ports, orphan detection, Git state, host metrics, and terminal jump.
- Process-only rows when telemetry attachment is unavailable or ambiguous.
- The privacy contract: no prompt text, assistant text, tool arguments, tool results, or child transcripts in product output.

## Architecture

```text
src/
├── main.rs                    # Binary entry
├── lib.rs                     # CLI parsing, terminal lifecycle, input loop, text snapshot
├── app.rs                     # Application state, polling, selection, kill/jump controls
├── config.rs                  # Theme, language, and five panel settings
├── demo.rs                    # Collector-free Pi demo fixtures
├── host_info.rs               # Host and aggregate metrics
├── locale.rs                  # UI strings
├── snapshot.rs                # JSON-safe snapshot DTOs
├── collector/
│   ├── mod.rs                 # Concrete Pi Collector and shared enrichment
│   ├── pi.rs                  # Pi process discovery, attachment, JSONL tailing
│   ├── pi_subagents.rs        # Fleet lifecycle status parser
│   └── process.rs             # Process tree, ports, and Git status
├── jump/
│   ├── mod.rs                 # Ordered terminal-jumper registry
│   ├── cmux.rs
│   ├── tmux.rs
│   └── iterm2.rs
├── model/
│   ├── mod.rs
│   └── session.rs             # Pi sessions and structured telemetry
├── theme/
│   ├── mod.rs
│   └── builtins/*.toml
└── ui/
    ├── mod.rs                 # Layout, compact tabs, click targets
    ├── context.rs
    ├── tokens.rs
    ├── projects.rs
    ├── ports.rs
    ├── sessions.rs
    ├── runs.rs
    ├── header.rs
    ├── footer.rs
    ├── help.rs
    ├── config.rs
    └── view_menu.rs
```

## UI layout

Desktop panel order:

1. **Sessions** stays visible when enabled and receives priority height.
2. **Tokens, projects, and ports** share the middle row.
3. **Context** appears when the sessions panel has enough height and surplus space remains.
4. **Runs** is embedded in session detail, or promoted beside sessions on wide layouts.
5. **Header and footer** use one row each.

Compact layouts use Work, Usage, and System tabs. The five configurable panels are numbered consecutively:

1. context
2. tokens
3. projects
4. ports
5. sessions

## Pi process discovery

`collector::pi::is_pi_command` accepts only executable-position Pi launches:

- the `pi` wrapper
- supported `env` wrapping
- Node, Node.js, or Bun running the installed Pi package entry point

Do not match arbitrary `pi` substrings, later arguments, or unrelated `cli.js` files. Nested Pi processes are not top-level rows.

A process row exists without a session file. `PiCollector` attaches JSONL telemetry only after ownership, header identity, working directory, and file identity checks agree. Shared path or session-ID claims are ambiguous and must fail closed to a process-only row.

On supported Unix platforms, use process-start identity to detect PID reuse. Windows remains process-only.

## Pi parent telemetry

The JSONL tailer is stateful:

- Scan an attached file once, then read appended bytes.
- Carry incomplete lines between reads.
- Reset safely after truncation, replacement, or deletion.
- Bound total work, line size, candidate count, descriptor count, semantic entries, and model catalogs.
- Mark partial, stale, malformed, or unavailable data explicitly.

Structured telemetry records precision, completeness, provenance, source health, and observation time. Authoritative snapshot fields use `null` when unavailable. Do not turn unknown values into known zero.

The parent parser may retain identity, model/provider metadata, thinking level, numeric usage, context size, compaction state, and safe activity labels. It must not retain or publish prompt text, assistant text, tool arguments, or tool results.

## Fleet telemetry

`collector/pi_subagents.rs` reads supported extension-owned lifecycle `status.json` files. Keep run usage separate from parent transcript usage to prevent double counting.

Do not read child transcript, prompt, event, or output-log files. Unknown lifecycle versions expose no trusted lifecycle details until explicitly supported.

Preserve parser limits, completed-run retention, bounded labels, source health, stale detection, child depth, and process-terminal verification.

## Shared process enrichment

`Collector::collect` returns Pi sessions and orphan ports together.

- Process data is refreshed every tick.
- Port and Git polling use the slower interval, normally every ten seconds.
- A changed PID set invalidates the port cache.
- Git counts come from `git -C {cwd} status --porcelain`.
- Child ports are tracked across ticks.
- A port becomes orphaned only while the child remains alive and listening after its parent Pi session disappears.

Before killing an orphan, perform a fresh port scan and require an exact current-command match. Before killing or jumping to a Pi session, revalidate the Pi command and process-start identity.

## Terminal jump

`jumpers()` is the ordered source of truth:

1. cmux, using `CMUX_WORKSPACE_ID` from the process environment.
2. tmux, using pane process-tree ownership.
3. iTerm2 on macOS, using controlling TTY and AppleScript.

Each adapter returns:

- `NotApplicable`: try the next adapter.
- `Jumped`: stop successfully.
- `Failed(message)`: stop and show the backend error.

Windows jump and kill controls are disabled.

## Privacy

All monitoring is local and read-only except explicit kill actions, terminal focus, configuration writes, and user-requested update/install operations.

- Never include prompt text, assistant text, tool arguments, tool results, or child transcripts in the TUI, text snapshot, or JSON snapshot.
- Reduce child and orphan commands to executable labels in output.
- Treat session files and status files as potentially secret.
- Keep `--demo` collector-free.
- Do not add network calls to monitoring paths.

## Theme contract

Theme Format v1 has one Pi session color, `pi_agent`. Do not restore per-agent colors. Update all packaged themes, `examples/themes/low-glare.toml`, `docs/themes.md`, decoder tests, and palette hashes when the schema changes.

## Commands

```bash
cargo build
cargo run
cargo run -- --once
cargo run -- --json
cargo run -- --demo --once
cargo run -- --exit-on-jump
cargo test
cargo clippy -- -D warnings
./scripts/check-rustfmt.sh
```

Parser benchmark:

```bash
cargo test --release pi_parser_release_benchmark -- --ignored --nocapture
```

## Commit convention

```text
<type>: <description>
```

Types: `feat`, `fix`, `refactor`, `docs`, `chore`.

## Release process

1. Update the same semver in `Cargo.toml` and `Cargo.lock`.
2. Run:
   ```bash
   cargo test
   cargo clippy -- -D warnings
   cargo build --release
   cargo publish --dry-run
   ```
3. Commit and push the version bump to `main`.
4. From a clean, current `main`, create and push an annotated `vX.Y.Z` tag.
5. Watch the `Release` and `Publish to crates.io` workflows.

Do not run `cargo publish` or `gh release create` manually. CI handles both. Do not tag before the version bump reaches `main`. Do not reuse a release tag after a failed publish; use a new patch version.

## Common failure modes

- Pi session and fleet files are implementation details. Parse defensively.
- File paths and session IDs can collide or change. Require unambiguous ownership.
- Files can disappear between discovery and read. Handle `NotFound` without dropping the process row.
- Large or partial JSONL records must stay bounded and must not poison later appends.
- `lsof`, `/proc`, and `netstat` data can race with process exit. Show stale data safely.
- PID reuse can target a different process. Recheck command and start identity before controls.
- Orphan ports require cross-tick history and a fresh ownership check before a signal.
- Terminal widths below the supported layout must degrade without hiding all sessions.
