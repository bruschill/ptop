# ptop

Local terminal monitor for Pi processes, sessions, and `pi-subagents` runs.

## Product rules

These rules are non-negotiable:

- Monitor Pi only. Do not add another top-level coding-agent collector, mode,
  label, quota source, or transcript view.
- Treat model and provider names such as Claude or Codex as Pi session
  metadata, not as separate monitored agents.
- Discover live Pi processes first. Keep a process-only row when telemetry is
  missing, stale, unsupported, or ambiguous.
- Fail closed when attaching telemetry or targeting a process. Never guess
  ownership from a path, session ID, PID, or display label.
- Preserve unknown values as unknown. Authoritative snapshot fields use `null`
  when unavailable, never a fabricated zero.
- Do not retain or publish prompt text, assistant text, tool arguments, tool
  results, child transcripts, or child output logs.
- Do not add network requests to monitoring paths. Installation and explicit
  update operations are outside the monitoring path.
- Keep `--demo` collector-free.

## Language and compatibility

Use English for code, comments, tests, fixtures, documentation, configuration,
scripts, UI strings, and repository communication. Preserve non-English text
only when it is an exact external identifier, protocol value, or short quote
with an English explanation.

Keep compatibility with the `rust-version` in `Cargo.toml`; do not use newer
standard-library APIs. Keep platform-specific code and imports behind narrow
`cfg` gates. CI must continue to compile Linux, macOS, and Windows process-only
support.

## Sources of truth

Read the relevant source before changing behavior:

- `README.md`: user-visible behavior, flags, key bindings, configuration, and
  privacy summary.
- `docs/pi-support.md`: platform, attachment, parser, fleet, polling, snapshot,
  privacy, validation, and compatibility contracts.
- `docs/themes.md`: Theme Format v1 and packaged theme requirements.
- `jump::jumpers()`: terminal backend order and fallback behavior.

Update `README.md` with every user-visible behavior change. Update the detailed
contract document when a parser, telemetry, platform, snapshot, or theme
contract changes. Do not let `AGENTS.md` become a second copy of those specs.

## Code ownership map

```text
src/main.rs                 Binary entry
src/lib.rs                  CLI, terminal lifecycle, input loop, text output
src/app.rs                  State, polling, selection, kill and jump controls
src/collector/pi.rs         Pi discovery, session attachment, JSONL tailing
src/collector/pi_subagents.rs
                            Extension-owned fleet status parsing
src/collector/process.rs    Process trees, ports, and Git status
src/model/                  Internal session and telemetry model
src/snapshot.rs             Public JSON-safe snapshot DTOs
src/ui/                     Layout, panels, menus, help, and click targets
src/jump/                   Ordered terminal jump adapters
src/herdr.rs                Shared Herdr process location and bounded CLI calls
src/config.rs               Theme, language, and five panel settings
src/theme/                  Theme decoder and packaged palettes
src/demo.rs                 Collector-free deterministic fixtures
src/host_info.rs            Host and aggregate metrics
src/locale.rs               UI strings
```

Keep tests close to the implementation unless they exercise the compiled CLI;
CLI integration tests belong in `tests/`.

## Collection and safety invariants

### Pi processes and parent sessions

`collector::pi::is_pi_command` accepts only executable-position Pi launches:

- the `pi` wrapper,
- supported `env` wrapping, or
- Node, Node.js, or Bun running the installed Pi package entry point.

Do not match arbitrary `pi` substrings, later arguments, or unrelated `cli.js`
files. Nested Pi processes are not top-level rows.

Attach JSONL only after ownership, header identity, working directory, and file
identity checks agree. Shared paths or session-ID claims are ambiguous and
must remain process-only. On supported Unix platforms, use process-start
identity to detect PID reuse. Windows remains process-only.

The tailer is stateful. It must:

- scan an attached file once and then read appended bytes,
- carry incomplete lines across reads,
- reset after truncation, replacement, or deletion,
- bound bytes, line size, candidate count, descriptors, semantic entries, and
  model catalogs, and
- report partial, stale, malformed, or unavailable data explicitly.

Structured telemetry must preserve precision, completeness, provenance,
source health, and observation time. Retain only identity, model/provider
metadata, thinking level, numeric usage, context size, compaction state, and
safe activity labels. Treat session and status files as potentially secret.

### Fleet runs

Read only supported extension-owned lifecycle `status.json` files. Do not read
child transcripts, prompts, events, output logs, or handoff content. Unknown
lifecycle versions expose no trusted lifecycle details until support is added
explicitly.

Keep parent transcript usage separate from run usage. Preserve parser limits,
completed-run retention, bounded labels, source health, stale detection, child
depth, and process-terminal verification.

### Shared enrichment and controls

`Collector::collect` returns Pi sessions and orphan ports together. Refresh
process data every tick. Poll ports and Git on the slower interval, normally
ten seconds, and invalidate the port cache when the PID set changes.

Git counts come from `git -C {cwd} status --porcelain`. Track child ports across
ticks. A port becomes orphaned only while the child remains alive and listening
after its parent Pi session disappears. Process and port sources can race with
process exit; keep stale or unavailable state explicit instead of guessing.

Before killing an orphan, run a fresh port scan and require an exact current
command match. Before killing or jumping to a Pi session, revalidate the Pi
command and process-start identity. Reduce child and orphan commands to safe
executable labels before they reach TUI, text, or JSON output.

## UI and terminal behavior

Desktop panel order is:

1. Sessions, with priority height.
2. Tokens, projects, and ports in the middle row.
3. Context when sessions has enough height and surplus space remains.
4. Runs inside session detail, or beside sessions on wide layouts.
5. One-row header and footer.

Compact layouts use Work, Usage, and System tabs. The configurable panels keep
this numbering: context, tokens, projects, ports, sessions. Narrow terminals
must degrade without hiding all sessions.

Terminal jump order is Herdr, cmux, tmux, then iTerm2 on macOS. Each adapter
returns `NotApplicable`, `Jumped`, or `Failed(message)`. Try the next adapter
only for `NotApplicable`; stop on success or failure. Herdr jumps require the
same server socket and the exact selected pane. Windows jump and kill controls
remain disabled.

## Themes

Theme Format v1 has one Pi session color, `pi_agent`. Do not restore per-agent
colors. A schema change must update:

- every file under `src/theme/builtins/`,
- `examples/themes/low-glare.toml`,
- `docs/themes.md`,
- decoder and CLI tests, and
- palette hashes.

## Change checklist

- Parser changes: add malformed, partial, oversized, replacement, ambiguity,
  and privacy tests as applicable.
- Snapshot changes: preserve JSON compatibility where possible, use `null` for
  unavailable authoritative values, and update JSON tests and docs.
- UI changes: test wide, compact, and narrow layouts. Keep selection and click
  targets aligned with rendered rows.
- CLI or configuration changes: update help, README examples, parsing tests,
  and config round-trip tests. Preserve unknown configuration keys.
- Demo changes: keep fixtures deterministic and prove no collector runs.
- Platform changes: test the affected target and confirm other `cfg` branches
  still compile.
- Dependency changes: justify the dependency and keep the supported Rust floor.

## Verification

Run the narrowest relevant test while iterating. Before completing a material
change, match the CI checks:

```bash
./scripts/check-rustfmt.sh
cargo clippy --all-targets -- -D warnings -A clippy::uninlined-format-args
cargo test --all-targets
cargo build --release
```

For parent parser work, also run:

```bash
cargo test --release pi_parser_release_benchmark -- --ignored --nocapture
```

Useful smoke checks:

```bash
cargo run -- --once
cargo run -- --json
cargo run -- --demo --once
cargo run -- --exit-on-jump
```

## Commits and releases

Commit subjects use `<type>: <description>` with `feat`, `fix`, `refactor`,
`docs`, or `chore`.

For a release:

1. Set the same semver in `Cargo.toml` and `Cargo.lock`.
2. Run the full checks above, then `cargo publish --dry-run`.
3. Commit and push the version bump to `main`.
4. From a clean, current `main`, create and push an annotated `vX.Y.Z` tag.
5. Watch the `Release` and `Publish to crates.io` workflows.

Do not run `cargo publish` or `gh release create` manually. CI handles both. Do
not tag before the version bump reaches `main`. Do not reuse a release tag after
a failed publish; use a new patch version.
