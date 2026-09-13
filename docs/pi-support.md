# Pi support and release gates

ptop monitors local Pi coding-agent processes only. Process discovery is independent from telemetry attachment, so every verified top-level Pi process remains visible even when no session file can be attached.

## Platform contract

| Capability | macOS | Linux | Windows |
|---|---:|---:|---:|
| Pi process discovery | Yes | Yes | Yes |
| Child process and port discovery | Yes | Yes | Yes |
| Owned session telemetry attachment | Yes | Yes | Process-only |
| `pi-subagents` fleet status | Yes | Yes | Unavailable |
| Git working-tree enrichment | Yes | Yes | Yes |
| Terminal jump | Herdr, cmux, tmux, iTerm2 | Herdr, cmux, tmux | Disabled |
| Herdr workspace presence | Yes | Yes | Disabled |
| Pi process kill | Yes | Yes | Disabled |

Windows uses `sysinfo` and `netstat -ano`. It reports load average as 0. Jump and kill controls remain disabled until ptop can apply the same trusted process-identity checks used on Unix.

On macOS and Linux, ptop checks that a controlled PID still belongs to a Pi process. Where available, it also compares an opaque process-start identity to prevent PID reuse from targeting another process. Herdr jumps use the selected Pi process's pane ID and server socket, and apply only when ptop is attached to the same Herdr server.

The interactive non-demo TUI reports workspace presence only when its inherited `HERDR_ENV`, socket, workspace ID, and pane ID are present and valid. On its background presence thread, ptop asks the owning Herdr server for the exact pane and workspace. When both IDs match, it captures the current workspace label, renames the workspace to `ptop`, and publishes the display token `ptop=running` with a 30-second TTL refreshed every 10 seconds. A missing server, timeout, malformed response, or identity mismatch suppresses the affected operation without blocking TUI startup.

On normal exit, ptop stops the presence loop and attempts to restore the captured label before its background thread ends. It first requires the exact workspace to exist with the label `ptop`. A label changed before that check is preserved. A crash, forced termination, closed workspace, unavailable server, or failed command can prevent restoration. If the workspace was already named `ptop`, ptop does not claim ownership of that name or restore it later.

Herdr 0.9.0 exposes unconditional workspace rename commands, not an atomic compare-and-set operation. Pane validation, label reads, workspace renaming, and workspace reporting are therefore separate commands. A pane move or label change between a check and its following rename can target or overwrite a value that changed concurrently. Automatic renaming is supported only as best-effort behavior for one ptop TUI per workspace. With multiple ptop instances, an instance that captured the original label can restore it while another remains active.

A pane moved between validation and reporting can refresh the old workspace once. Later mismatches suppress refreshes, and the TTL removes stale presence metadata. ptop does not clear the shared token on exit because another ptop process may still be refreshing it. This local Herdr path is outside `Collector::collect` and does not change snapshots, agent state, waits, notifications, or rollups.

## Telemetry attachment contract

A process row does not require telemetry. ptop attaches a Pi parent session JSONL only when all required ownership evidence agrees:

1. The process is a supported top-level Pi launch, not a nested Pi child.
2. The candidate file is owned through a supported process or Herdr discovery path.
3. The JSONL header session ID and working directory match the candidate claim.
4. The file identity remains stable while it is read.
5. No other live Pi process claims the same session path or session ID.

Missing, stale, unsupported, invalid, or ambiguous evidence produces a visible `process only` row. It never causes ptop to guess an attachment.

Telemetry values include metadata for precision (`unknown`, `inferred`, `estimated`, or `exact`), completeness (`unknown`, `partial`, or `complete`), provenance, observation time, and source health. Authoritative JSON fields use `null` for unavailable values. Numeric compatibility fields must not be treated as authoritative when the structured value is `null`.

Model and provider values are Pi metadata. Their names do not select another collector or monitored agent.

## Fleet and run telemetry

ptop reads supported `pi-subagents` lifecycle `status.json` files and associates them with an owned Pi parent session. Lifecycle schema version 3 can expose run state, execution mode, usage, child state, and process-terminal metadata.

Parent session usage and run usage use separate accounting. Consumers must not add them together as if they were one token total.

Safety limits include:

- 256 KiB per status.json file.
- 2 MiB of status.json data per collection tick.
- 512 run-directory entries examined per collection tick.
- 128 status files per collection tick.
- 20 runs per parent session.
- 64 children per run.
- 3 nested child levels.
- Completed runs remain eligible for 30 days.
- Runs with no verifiable runner become stale after 24 hours.
- A run whose verified runner has disappeared becomes stale after 30 seconds.

Unknown lifecycle versions do not become trusted run telemetry.

## Privacy boundary

The parent JSONL parser retains identity, model/provider metadata, thinking level, numeric usage, context size, compaction state, and safe activity labels. It does not retain or publish:

- prompt text
- assistant text
- tool arguments or tool results
- chat transcripts or tool previews
- child transcripts, events, prompt files, or output logs

TUI, text snapshots, and JSON snapshots reduce child and orphan process commands to executable labels. Fleet status labels are bounded and terminal-control characters are removed before display.

ptop makes no network request while monitoring. It reads local files, process metadata, Git status, and listening-port state.

## Parent parser limits

The parent session tailer is stateful. It scans an attached file once, then reads only appended bytes. It handles incomplete lines, truncation, replacement, deletion, malformed records, and oversized lines without retaining raw transcript content.

Token history contains the latest 64 persisted assistant entries with complete component usage, in file order across all branches. Each point is the checked sum of input, output, cache-read, and cache-write tokens for one assistant turn. Entries with unavailable or overflowing component usage are omitted rather than represented as zero. The lifetime turn count still includes every persisted assistant entry.

Current limits:

- 2 MiB of Pi session JSONL per collection tick.
- 1 MiB per Pi session JSONL line.
- 8192 semantic tree entries per attached session.
- 128 attachment candidates per collection tick.
- 128 Pi processes considered for attachment per collection tick.
- 4096 open file descriptors per collection tick.
- Model catalog files are capped at 2 MiB and 1024 retained model entries.

These are defensive implementation limits, not a promise that every external Pi schema will remain compatible.

## Shared enrichment and polling

Process information is collected every two seconds. Port data and Git branch and working-tree status use the slower poll path, normally every ten seconds, with port-cache invalidation when the tracked PID set changes.

Orphan detection is cross-tick state. A child port becomes orphaned only after its parent Pi session disappears while the child remains alive and listening. Before sending a signal, ptop performs a fresh port scan and compares the current command with the tracked command.

## JSON snapshot contract

`ptop --json` and `App::to_snapshot` expose:

- host and aggregate metrics
- live Pi session identity and state
- structured context and usage telemetry
- bounded token history
- fleet runs and children
- privacy-safe child process labels
- orphan ports

Snapshots omit monitor-mode discriminators, account quota, MCP server state, per-session transcript data, tool previews, file-audit data, and old multi-agent identifiers.

`App::to_snapshot` is a pure read. Call `App::tick` before taking a fresh snapshot.

## Validation

Run the parser benchmark when parent parsing or its limits change:

```bash
cargo test --release pi_parser_release_benchmark -- --ignored --nocapture
```

Before release, run:

```bash
./scripts/check-rustfmt.sh
cargo test
cargo clippy -- -D warnings
cargo build --release
cargo run -- --demo --once
cargo run -- --once
cargo publish --dry-run
```

CI must pass on macOS, Linux, and Windows. Release builds must retain process-only rows, privacy suppression, bounded parsing, fleet accounting separation, process identity checks, port/orphan behavior, and terminal-jump behavior.

## Compatibility policy

Pi session files and `pi-subagents` lifecycle files are local implementation contracts. New schema versions remain unavailable until explicitly supported and tested. ptop fails closed on ambiguous ownership and preserves the process row instead of inventing telemetry.
