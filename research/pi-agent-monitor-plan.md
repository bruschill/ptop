# Plan: Polished Pi-specific monitor with fleet support

## 1. Recommended direction and smallest coherent MVP

Build a Pi-first local monitor around a passive, stateful `PiCollector`.

The first release starts only `PiCollector` from the Pi product entry point. Keep `AgentCollector` and `MultiCollector` available for legacy modes during stabilization.

MVP scope:

- Local top-level Pi processes.
- Process-only rows before session telemetry exists.
- High-confidence attachment to owned Pi JSONL.
- Metadata-only `pi-subagents` workflow/run state when supported.
- Existing process tree, ports, orphan handling, git, themes, jump, and guarded kill controls.
- Precision, completeness, provenance, freshness, and attachment state shown in TUI, JSON, and text snapshots from the first slice.

Do not cherry-pick `001caf7`.

---

## 2. Product definition and explicit assumptions

**Product:** a local terminal monitor for Pi coding-agent activity. “Fleet” means activity on the current machine only:

1. Top-level Pi sessions.
2. Local Pi subagents, including supported foreground/background metadata.
3. Local workflow/run state.

It excludes remote hosts, cloud aggregation, shared team telemetry, terminal scraping, and provider-account quota.

**Assumptions**

- Pi JSONL is append-only but may not persist the active leaf, task, model, or context state authoritatively.
- `pi-subagents/status.json` is authoritative only for schema versions and fields the adapter explicitly supports.
- `events.jsonl` supplies transition history only and never overrides a valid status snapshot.
- Windows remains process-only until `ProcInfo` has trusted session-identity evidence such as cwd or environment data.
- Exact foreground-child state is unavailable unless the supported status schema provides it.
- The implementation estimate is 3–4 weeks for one engineer. It may approach 5 weeks if an exact-state bridge or Windows attachment is included.

---

## 3. Scope, privacy boundary, and non-goals

### Included

- Passive local process discovery.
- Owned top-level session JSONL parsing for telemetry.
- Incremental, bounded JSONL parsing and tree reconstruction.
- Capability-detected telemetry adapters.
- Metadata-only subagent/workflow status support.
- Separate lifetime accounting and current-context calculation.
- Pi-safe snapshots, text output, and TUI.

### Privacy boundary

The collector may read an **owned top-level Pi session JSONL locally** to derive telemetry. It must not retain or expose prompt text, message bodies, task text, tool arguments, tool results, or output content.

Subagent adapters may read allowlisted status metadata and transition metadata only. They must not open child transcripts, task text, output logs, or tool arguments by default.

Apply this rule to:

- TUI details
- JSON and text snapshots
- logs and diagnostics
- caches
- summary generation
- error messages

Safe labels may describe state or activity without copying private text.

### Non-goals

- Remote or networked fleet support.
- Provider quota display.
- Cwd-only ownership or newest-file heuristics.
- Claimed exact state from passive telemetry.
- `claude --print`, summaries, or any provider-backed enrichment in Pi mode.
- Windows transcript attachment or process controls before supported and tested.

---

## 4. Current-state evidence and implementation seams

Relevant current paths:

- Collector extension: `src/collector/mod.rs`
- Shared process data: `src/collector/process.rs`
- Existing collectors: `src/collector/claude.rs`, `src/collector/codex.rs`, `src/collector/opencode.rs`
- Session model: `src/model/session.rs`
- Snapshots: `src/snapshot.rs`
- App state and controls: `src/app.rs`
- CLI modes and text snapshots: `src/lib.rs`
- Entrypoint only: `src/main.rs`
- UI: `src/ui/`
- Configuration: `src/config.rs`
- Terminal jump registry: `src/jump/`
- CI: `.github/workflows/ci.yml`

Constraints confirmed by the current code:

- `ProcInfo` has PID, PPID, RSS, CPU, and command, but no cwd or environment identity.
- `AgentSession` cannot represent unknown telemetry safely because context fields are required numerics.
- `SessionView` in `src/snapshot.rs` also requires numeric context and usage fields.
- `src/lib.rs::print_snapshot` separately prints numeric values.
- `--once` can invoke summary generation through the app flow.
- Existing quota UI is Claude/Codex-specific.
- Existing session detail views are transcript-oriented and must not be populated for Pi.
- Existing kill confirmation is row-index based and command validation is insufficient for new Pi entity types.

---

## 5. Architecture, identity, attachment, and accounting contracts

### Collector boundary

Implement `PiCollector: AgentCollector` in `src/collector/pi.rs`.

Keep Pi-specific normalized parsing types inside `PiCollector`, then project top-level session rows through the existing collector contract. Do not introduce a second general fleet pipeline unless later requirements require one.

Workflow and run records are metadata associated with a parent session. They must not become fake process sessions with PIDs, process metrics, jump, or kill behavior.

### Process identity and controls

Use process identity as:

- PID
- validated executable/command identity
- process start identity where the platform exposes it

Where start identity is unavailable, expose the remaining PID-reuse limitation.

Bind kill confirmation to stable entity and process identity, not table position. A confirmed process-only row may have unknown activity and remain killable if its process identity is validated. Child controls must not silently kill a shared parent process.

Disable unsupported Windows jump/kill controls until implemented and tested.

### Attachment evidence

Attach a JSONL only when evidence is consistent and high confidence. The ownership spike must define and test a ranked evidence table covering:

1. Validated Pi executable or installed CLI path, including Node/Bun and RPC launches.
2. Open-file evidence.
3. Transient descendant `PI_SESSION_FILE` markers attributed to the correct Pi ancestor.
4. Supported session-directory overrides.
5. Session-header validation.
6. Conflicts among process evidence, canonical path, and session ID.

Reject attachment when evidence is ambiguous, conflicting, symlinked, or depends only on cwd. Never choose a newest file.

Merge passive and adapter data only when canonical session path or session ID agrees. If both exist and disagree, do not merge.

### Usage and context

Keep these independent:

- **Lifetime usage:** observed session-level input/output/cache/cost accounting across all persisted branches.
- **Current context:** inferred active-branch context.

Accounting contract:

- Deduplicate persisted usage by stable entry identity.
- Account for abandoned branches, assistant usage, nested tool-result usage, and branch-summary or compaction usage according to source ownership.
- Keep session-inclusive and child-exclusive totals distinct.
- Adapter aggregates replace or annotate overlapping passive totals. Never add them automatically.
- Do not sum parent-inclusive totals with child totals in fleet aggregates.
- If overlap is unknown, show separate totals or mark aggregate unavailable.
- Track completeness separately from precision. Exact observed entries do not prove a complete lifetime total.
- Treat source-provided cost as reported, not verified billing truth.

Context contract:

1. Reconstruct branches by `parentId`.
2. Infer the active leaf from persisted evidence and label that inference.
3. Use the last valid assistant `totalTokens` baseline on the inferred branch.
4. Define valid baselines explicitly, including treatment of failed or aborted assistant responses.
5. Use component fallback only when `totalTokens` is absent and the necessary components are valid.
6. Estimate trailing messages only when safely derivable without retaining content.
7. After compaction, expose unknown context until a later valid assistant baseline.
8. Expose unknown context if provider-plus-model window resolution is unavailable.
9. Handle missing parents and malformed entries without inventing branch state.
10. An unpersisted leaf move remains inferred and undetectable until a later append.

Use separate provenance/reason fields for branch uncertainty, trailing estimate, compaction, missing window, and parse limitations. A single precision label is insufficient.

---

## 6. Domain, snapshot, freshness, and UI rules

### Telemetry representation

Add types equivalent to:

```rust
enum TelemetryPrecision {
    Unknown,
    Inferred,
    Estimated,
    Exact,
}
```

Each relevant value also carries:

- provenance/reason
- completeness
- source update time
- collector observation time
- last successful parse time
- source health and current error
- stale state
- attachment confidence

Do not treat a successful reread of unchanged data as a source update.

### Snapshot compatibility policy

Use an additive compatibility bridge:

- Preserve legacy numeric snapshot fields unchanged for legacy collectors.
- For Pi records, legacy numeric placeholders remain compatibility-only and are documented as non-authoritative.
- Add authoritative structured optional fields for context, usage, precision, completeness, provenance, freshness, and attachment state.
- New Pi-aware consumers must use structured fields.
- `src/lib.rs::print_snapshot` must render structured Pi values and never present unknown as `0`.

Test legacy records unchanged, plus Pi records for unknown, partial, estimated, known-zero, and exact values.

### UI rules

Deliver truthful presentation in every vertical slice:

- Exact: `82%`
- Inferred: `~82%`
- Estimated: `≈82%`
- Unknown: `—`

Never draw an unknown context bar as zero.

Pi session detail is metadata-only: identity, process, attachment state, source health, model metadata, context provenance, workflow/run metadata, child metadata, ports, and git state.

Hide quota in Pi mode.

Use existing header metrics. Do not add a separate Host panel.

Runs belong in selected-session detail initially. Promote them to a mid-row panel only when tested width permits.

Test layouts at:

- 80×24
- 100×24
- larger desktop width

Include long identities, many children, stale warnings, and narrow-mode overflow behavior.

---

## 7. Vertical phases

### Phase 1: Pi-only process monitor

**Paths:** `src/collector/pi.rs`, `src/collector/mod.rs`, `src/model/session.rs`, `src/snapshot.rs`, `src/app.rs`, `src/lib.rs`, `src/config.rs`, relevant `src/ui/` files.

**Deliver:**

- Pi product entry mode that starts only `PiCollector`.
- Independent top-level process rows, including same-cwd processes.
- Process identity and platform-limited controls.
- Unknown telemetry, attachment state, source health, metadata-only detail, quota suppression, and Pi-safe JSON/text/TUI output.
- Summary suppression in TUI, `--once`, and all Pi entry paths.

**Exit:** a Pi process without a transcript renders safely and never appears to have `0%` context or quota.

### Phase 2: Owned session telemetry

**Paths:** `src/collector/pi.rs`, `src/model/session.rs`, `src/snapshot.rs`, fixtures.

**Deliver:**

- Concrete ownership evidence rules.
- Canonical identity conflict handling.
- Stateful tailer with offsets, partial-line buffers, atomic replacement, truncation, deletion, malformed-write recovery, and cache eviction.
- Bounded initial scan with progress or partial-completeness state.
- High-confidence attachment only.

**Exit:** ambiguous or conflicting ownership preserves process-only rows; normal appends do not reread the full file.

### Phase 3: Usage and context

**Paths:** `src/collector/pi.rs`, `src/model/session.rs`, `src/snapshot.rs`, `src/lib.rs`, `src/ui/context.rs`, `src/ui/tokens.rs`, `src/ui/sessions.rs`.

**Deliver:**

- Branch reconstruction and active-leaf provenance.
- Accounting ownership and deduplication.
- Context baseline, fallback, compaction, and window-resolution semantics.
- Precision, completeness, provenance, and unknown-state rendering in JSON, text, and TUI.

**Exit:** Pi usage and context values communicate what is observed, inferred, estimated, incomplete, or unknown.

### Phase 4: Local fleet metadata

**Paths:** `src/collector/pi.rs` or `src/collector/pi_subagents.rs`, `src/model/session.rs`, `src/snapshot.rs`, selected detail UI.

**Deliver:**

- Capability matrix for supported `status.json` versions and fields.
- Status snapshot reader as authoritative metadata source.
- Optional transition-only `events.jsonl` reader.
- Foreground/background state only where supported.
- Workflow/run identity, freshness, terminal-state reconciliation, and retention policy.
- Canonical deduplication without fake process rows.

**Exit:** supported metadata enriches the parent session; unsupported schemas and unavailable fields remain visibly unavailable.

### Phase 5: Polish and release gates

**Paths:** `src/ui/mod.rs`, `src/ui/config.rs`, `src/app.rs`, `src/lib.rs`, `.github/workflows/ci.yml`, `README.md`, configuration docs.

**Deliver:**

- Responsive Runs promotion only where width permits.
- Platform coverage and explicit Windows boundary.
- Performance limits, benchmarks, and documentation.
- Legacy regression validation and stabilization guidance.

**Exit:** documented privacy, platform, performance, and compatibility gates pass.

---

## 8. Spikes, tests, and measurable limits

### Required spikes

1. **Ownership spike:** produce the ranked attachment evidence table and fixtures before Phase 2.
2. **JSONL semantics spike:** establish scrubbed branch, compaction, valid-baseline, and unpersisted-leaf fixtures before Phase 3.
3. **Status schema spike:** create the capability matrix before Phase 4.
4. **Performance spike:** establish parser budgets before Phase 2 implementation is finalized.

### Test coverage

Add scrubbed durable fixtures for:

- Process-only discovery, identical Pi command PID reuse, and same-cwd processes.
- Unique, ambiguous, conflicting, symlinked, forked, cloned, replacement, and multi-process session ownership.
- Append, partial lines, malformed lines, large lines, truncation, atomic replacement, deletion, and interrupted initial scans.
- Missing parents, branch changes, compaction, failed/aborted assistant responses, absent totals, unknown windows, and unpersisted leaf movement.
- Nested usage, adapter overlap, parent-inclusive versus child-exclusive totals, and incomplete scans.
- Supported, malformed, missing, and unknown `status.json` versions.
- Foreground/background unavailable versus supported states.
- JSON and text output for every telemetry state.
- 80×24, 100×24, and desktop rendering.
- Pi `--once` and TUI paths proving no `claude --print` invocation.
- No reads from prohibited child transcript, prompt, output-log, or tool-argument paths.

### Performance limits

Before implementation, set measured thresholds for:

- Maximum per-tick parsing work.
- Maximum accepted JSONL line size.
- Maximum retained tree metadata per session.
- Maximum retained partial-line buffer.
- Initial-scan budget and progress behavior.
- Stale cache and terminal-run retention periods.

When a limit is exceeded, retain process visibility, mark telemetry partial or unavailable, and avoid publishing an incomplete lifetime total as complete.

---

## 9. Rollout, validation, and residual risks

### Rollout

1. Release Pi mode as the Pi product entry point with Pi-only collection from day one.
2. Preserve legacy multi-agent constructors and behavior during stabilization.
3. Validate Linux/macOS attachment through targeted tests before claiming support.
4. Advertise Windows as process-only.
5. Make any broader default or legacy deprecation a separate compatibility decision.

### Release gates

- Privacy tests pass.
- No ambiguous ownership attachment.
- JSON, text, and TUI distinguish unknown from zero.
- Parser remains within measured budgets.
- Existing collector tests remain green.
- Platform support statements match tested behavior.

### Residual risks

- Passive telemetry cannot prove current active state when Pi does not persist it.
- Pi and subagent schemas may change.
- Some process launches may lack sufficient ownership evidence.
- Model-window data may be unavailable.
- Cost may remain unavailable or only source-reported.
- Exact foreground-child state may require future extension cooperation.

---

## 10. User decisions and issue checklist

### Decisions with defaults

1. **Product identity:** use `ptop` for the project, package, crate, binary, repository, and persistent paths; keep Pi as the default monitor mode.
2. **Activation:** Pi product entry point is Pi-only in its first release.
3. **Legacy support:** retain Claude/Codex/OpenCode modes during stabilization.
4. **Snapshot policy:** additive authoritative Pi telemetry fields; legacy numeric fields remain compatibility-only for Pi.
5. **Cost:** show only reported source values; otherwise `—`.
6. **Content visibility:** metadata-only Pi monitoring. Future content access requires explicit opt-in.
7. **Windows:** advertise process-only support only.
8. **Exact-state bridge:** defer until Pi exposes a stable documented interface.

### Dependency-ordered issue checklist

- [ ] Add Pi-only entry mode and passive `PiCollector`.
- [ ] Add safe process identity, process-only rows, and platform-limited controls.
- [ ] Add unknown telemetry, attachment/source health, metadata detail, quota suppression, and summary suppression in all Pi paths.
- [ ] Complete and approve the ownership-evidence spike.
- [ ] Implement canonical identity conflict handling and high-confidence attachment.
- [ ] Add bounded incremental JSONL parsing and lifecycle recovery.
- [ ] Add ownership, rotation, ambiguity, replacement, and PID-reuse fixtures.
- [ ] Complete JSONL semantics and performance spikes.
- [ ] Implement branch-aware accounting and context contracts.
- [ ] Add precision, completeness, provenance, freshness, and structured snapshot fields.
- [ ] Update JSON and `print_snapshot` output for Pi telemetry.
- [ ] Complete status-schema capability matrix.
- [ ] Implement metadata-only subagent/workflow adapter and retention reconciliation.
- [ ] Add responsive selected-detail workflow state and promote Runs only where layout permits.
- [ ] Add privacy, performance, platform, layout, and legacy regression gates.
- [ ] Document support boundaries, configuration, snapshot policy, and rollout.
