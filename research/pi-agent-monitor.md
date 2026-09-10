# Research: Pi coding-agent monitor integration

## Summary
Pi support fits the existing collector architecture: add a stateful `PiCollector`, register it in `MultiCollector`, and reuse the existing process-tree, ports, git, snapshot, kill, and terminal-jump paths. The reliable baseline is process discovery plus an incrementally tailed Pi session JSONL.

Do not cherry-pick the guide branch. It adds Pi beside Claude and Codex against an old codebase, while this checkout has since gained OpenCode, richer session state, file auditing, MCP views, new UI modules, and safer process handling. The branches have diverged by 240 current-side commits and 3 guide-side commits from their merge base (`git rev-list --left-right --count HEAD...001caf7`). Port the collector ideas into the current architecture instead.

For a monitor specifically for Pi, use two telemetry layers:

1. A passive core collector for every Pi installation. It reads processes and session JSONL without changing Pi.
2. Capability-detected adapters for Pi extensions. The first should understand the installed `pi-subagents` lifecycle artifacts. A small opt-in bridge can later provide exact parent-session live state and context usage.

## Source scope

- Repository citations are relative to this checkout.
- Installed Pi citations are relative to `/Users/bruschill/.asdf/installs/nodejs/26.2.0/lib/node_modules/@earendil-works/pi-coding-agent/`, version 0.85.1.
- Installed `pi-subagents` citations are relative to `/Users/bruschill/.pi/agent/git/github.com/nicobailon/pi-subagents/`.
- The guide is fixed at commit [`001caf7`](https://github.com/ptahdunbar/abtop/tree/001caf7b1994243760f755e8e063d54654177abf).

## Product scope: Pi support vs a Pi-specific monitor

The guide implements additive Pi support. A Pi-specific product also needs UI changes:

- Start only `PiCollector` by default, but keep `AgentCollector` and `MultiCollector` until the Pi path is stable. They isolate collection from shared process, port, git, and orphan handling.
- Replace the agent column with provider/model or session name because every top-level row is Pi.
- Replace the Claude/Codex quota panel. Pi has no provider-independent account quota source. A Pi fleet panel showing foreground/background children and workflow state is more useful.
- Make context explicitly `unknown`, `estimated`, or `exact`. Different Pi providers and custom models make an unlabeled percentage misleading.
- Add Pi-specific session detail: session name, provider, model, thinking level, inferred or exact active branch, compaction count, queues, current tool, extension children, and cost when present. These require new domain and snapshot fields; they do not fit the current three-field `SubAgent` or existing `AgentSession` unchanged.
- Keep process, project, ports, tokens, context, jump, kill, snapshot, and theme code. These are already agent-neutral.

## Findings

1. **The passive integration points are small, but a Pi-specific product needs schema work.** Add `src/collector/pi.rs`; expose and construct it in `src/collector/mod.rs`; allow `"pi"` in hidden-agent and kill validation in `src/app.rs`; and add Pi fixtures/unit tests. `MultiCollector` already shares process/port data, refreshes git state, and manages orphan ports. `AgentSession` covers baseline model, effort, usage, task, history, process children, file audit, and context fields, but it has no separate provider, session name, cost, queue, workflow, telemetry precision, or active-leaf fields. `SubAgent` has only name, status, and tokens. Pi-specific fleet support therefore also changes `src/model/session.rs`, `src/snapshot.rs`, and several `src/ui/` modules. [collector contract: `src/collector/mod.rs:95-110`; registration: `src/collector/mod.rs:321-344`; model: `src/model/session.rs:99-104,133-202`; snapshot: `src/snapshot.rs:84-166`; kill validation: `src/app.rs:710-738,1001-1009`]

2. **Process discovery must work without a session file.** Detect an actual Pi CLI or RPC process by its executable or installed `pi-coding-agent` path and create a process-only row immediately. Persistent files may not exist before the first assistant response, `--no-session` runs never create one, and `PI_SESSION_FILE` is added only to transient shell-tool descendants. On macOS and Linux, descendant environment inspection (`ps eww` or `/proc/<pid>/environ`) can provide a high-confidence session marker while a shell tool is running. Otherwise use an open file descriptor or a unique cwd plus validated JSONL header as a fallback. Never assign a newest file when two Pi processes share a cwd. [Pi `README.md:619-643`; Pi `docs/environment-variables.md:10-43`; repository `src/collector/process.rs:1-178,242-276`]

   Windows needs an explicit scope decision. The repository uses `sysinfo` for Windows process command lines, CPU, and memory, but `ProcInfo` has no cwd or environment. Initial support can show process-only Pi rows on Windows while leaving session identity and transcript metrics unknown. Full Windows attachment requires extending process discovery with a trustworthy cwd/session source or requiring the opt-in bridge. [repository `Cargo.toml:26-33`; repository `src/collector/process.rs:6-12,108-163`]

   Persistent sessions default to `~/.pi/agent/sessions/--<encoded resolved cwd>--/<encoded ISO timestamp>_<UUID>.jsonl`. Encoding replaces `/`, `\\`, and `:` with hyphens. Directory and config roots can be overridden by `PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR`, and `--session-dir`. This means the guide’s slash-only `--Users-...--` encoder is not portable, and hardcoding `~/.pi/agent` misses valid sessions. [Pi `dist/core/session-manager.d.ts:151-159,242-267`; Pi `docs/environment-variables.md:45-61`]

3. **The JSONL is an append-only tree, and it does not always persist the active leaf.** Header is `type:"session"`, version 3, id, timestamp, cwd, optional `parentSession`; all other entries have `id`, `parentId`, and timestamp. Relevant entry types include message, model/thinking changes, compaction, branch summary, custom/custom_message, labels, and session info. `/fork` and `/clone` create a file with `parentSession`. Pi can also move its in-memory leaf to an existing entry without appending anything; only the next appended entry reveals that branch choice. A passive monitor can reconstruct every branch and infer the active branch from the latest append, but task, model, and context remain inferred while Pi is idle after an unpersisted tree jump. Exact live leaf identity requires the bridge. [Pi `docs/session-format.md:18-54,105-233,308-328`; Pi `dist/core/session-manager.d.ts:4-106,184-222`; Pi `dist/core/session-manager.js:1045-1079`]

4. **Use `usage.totalTokens` as a context baseline, not as an exact standalone answer.** Pi’s own `calculateContextTokens()` prefers `usage.totalTokens` and falls back to summing `input`, `output`, `cacheRead`, and `cacheWrite`. `getContextUsage()` starts from the latest valid assistant usage on the active branch and estimates messages after it. After compaction it returns unknown until a later successful assistant response supplies a valid baseline. A passive monitor can mirror this algorithm on its inferred branch, but the result remains estimated because the active leaf may be unpersisted and the effective model window may be unresolved. Return unknown rather than assume a 200K window. [Pi `dist/core/compaction/compaction.js:86-160`; Pi `dist/core/agent-session.js:2708-2742`; Pi `dist/core/extensions/types.d.ts:193-198`]

   Lifetime token and cost totals are a different calculation over all session entries. Count assistant usage, optional `toolResult.usage` for nested LLM work, and compaction/branch-summary usage exactly once. This includes compacted and abandoned branches because it represents billed work, not current context. [Pi `docs/session-format.md:81-117,238-261`; Pi `dist/core/agent-session.js:2656-2706`]

5. **Model metadata is provider-specific and available only indirectly to an external Rust monitor.** Messages contain provider/model; model changes contain provider/modelId; model definitions provide `contextWindow` and `maxTokens`, defaulting to 128K/16,384 only for custom models. The effective catalog can include built-ins, `models.json`, cached catalog data, and extensions, so a hardcoded model-name table will be wrong. Phase 1 should show provider/model and `context_window=0`/unknown unless a conservative, documented local catalog parser resolves the model. A Pi extension can obtain the exact active model and `ctx.getContextUsage()` but requires user opt-in. [Pi `docs/session-format.md:130-142`; Pi `docs/models.md:143-166`; Pi `dist/core/extensions/types.d.ts:177-202`]

6. **Active status and task are inherently heuristic without an opt-in extension.** Persisted messages arrive only at `message_end`; live streaming, queued messages, tool start/end, UI prompts, retries, and settled state are event-only. From JSONL, display the latest tool call on the inferred branch as recent activity, not necessarily a currently running tool. Use file mtime plus parent/child CPU to classify `Thinking`/`Executing`/`Waiting`, and mark branch-derived details as inferred until another entry confirms the leaf. An optional Pi extension can emit redacted status events (`agent_start`, `tool_execution_start/end`, `ui_prompt_start/end`, `agent_settled`) and exact leaf identity to a private local file. [Pi `dist/core/agent-session.d.ts:39-101`; Pi `dist/core/extensions/types.d.ts:330-398,461-500`; repository `src/model/session.rs:39-80`]

7. **Core Pi has no built-in subagents, but this Pi installation does.** Assistant content has `toolCall {id,name,arguments}` and tool-result messages provide name, error state, and optional nested usage. Include nested `toolResult.usage` in lifetime totals, but do not also add it when an extension’s aggregate status already includes the same work. Core Pi’s README says subagents and background bash are extension features, so the passive collector must not invent generic child semantics. Show unknown custom tools generically, retain current redaction rules, and only extract file access for known read/write/edit argument shapes. [Pi `docs/session-format.md:49-117`; Pi `README.md:548-563`; repository `src/model/session.rs:7-36,176-196`]

   The installed `pi-subagents` extension is a concrete exception and should get a dedicated adapter. Foreground children are Pi sessions inside the parent process, so process-tree discovery cannot find them. Background children run in a detached runner and write `status.json`, `events.jsonl`, bounded output logs, child transcripts, and artifact metadata. The extension says `status.json` is the authoritative snapshot and consumers should not scrape terminal output. It exposes run and session identity, state, steps, models, token/cost totals, tool and turn counts, child session files, and nested children. Read only status metadata by default; transcripts, task text, output logs, and tool arguments are sensitive. [`pi-subagents` `README.md:43-45,92-98`; `docs/observability.md:7-13,146-188,200-226,237-263`]

8. **Children, ports, git, jump, and privacy reuse existing code; Pi adds no account quota source.** Shell commands are Pi descendants and inherit Pi markers, so the existing recursive children/port/orphan machinery and 10-second git cache work unchanged after correct parent PID mapping. Existing jump takes a PID and supports cmux, tmux, and iTerm2, so Pi requires no new jumper. Pi supports many providers and docs expose rate-limit status only to extensions from HTTP response headers; no persistent/account-level rate-limit API is documented. Keep quota blank for Pi, rather than attribute a provider quota to all Pi sessions. [Pi `docs/environment-variables.md:10-43`; repository `src/collector/mod.rs:419-497`; repository `src/jump/mod.rs:43-79`; Pi `dist/core/extensions/types.d.ts:314-329`]

9. **Performance and safety require a stateful tailer.** Pi session files are append-only, and Pi itself reads UTF-8 in 1 MiB chunks while carrying an incomplete line buffer. Mirror the existing Claude collector’s offset/cache/partial-line/rotation approach, keyed by canonical session-file path plus file identity where available. On first sight scan bounded full content; thereafter parse appended complete lines only; reset on shrink/replacement; cap text, task, chat, tool, and file-access collections; ignore malformed JSON lines. Reject symlinks and validate header/type/cwd before use. [Pi implementation: `https://github.com/earendil-works/pi-mono/blob/v0.85.1/packages/coding-agent/src/core/session-manager.ts#L374-L459`; repository incremental design: `src/collector/claude.rs:382-473,1292-1374`; repository terminal sanitization/redaction: `src/collector/mod.rs:18-80`]

10. **The clean architecture is a passive collector plus optional telemetry adapters.** Keep `PiCollector` responsible for stable core data and define a small adapter interface for extension-owned status. Merge records by canonical session path or session ID, never cwd alone. The `pi-subagents` adapter can provide exact background child state now. A future Pi monitor bridge can provide parent streaming/tool/UI/context state. If an adapter is absent or its schema version is unknown, the passive session row still works and reports reduced precision.

## Guide-branch claims: verified vs stale/assumption

- **Verified direction:** A `PiCollector`, shared process data, child walk, port mapping, git reuse, and no default quota source are sound design choices. [guide collector at commit `001caf7`: `https://github.com/ptahdunbar/abtop/blob/001caf7b1994243760f755e8e063d54654177abf/src/collector/pi.rs`; current contracts above]
- **Stale or incorrect, high severity:** It treats the last assistant `usage.totalTokens` encountered in raw file order, divided by a hardcoded window, as exact current context. Pi does use `totalTokens` as the preferred baseline, but only on the active branch, with trailing-message estimates and special post-compaction unknown handling. The guide implements none of those conditions. [guide link above; Pi `dist/core/compaction/compaction.js:86-160`; Pi `dist/core/agent-session.js:2708-2742`]
- **Stale or incorrect, high severity:** It hardcodes model windows and says Pi emits no context-window metadata. Model records define `contextWindow`; resolving the effective record externally remains non-trivial, but a 200K fallback is not reliable. [guide link above; Pi `docs/models.md:143-166`]
- **Stale or incorrect, medium severity:** It uses a slash-only session-dir encoder and treats `lsof` or newest-file fallback as certain ownership. Current encoding also replaces backslashes and colons and resolves cwd. Newest-file selection is ambiguous for simultaneous same-cwd sessions. [Pi `dist/core/session-manager.d.ts:151-159`; Pi `docs/environment-variables.md:24-43`]
- **Does not compile, blocker:** The guide refers to `SessionStatus::Working`, `context_window_for_model(&result.model)`, and `ProcInfo.started_at_ms`; this checkout exposes `Thinking`/`Executing`/`Waiting`, a three-argument context helper, and no process start field. [guide link above; repository `src/model/session.rs:39-80`; repository `src/collector/mod.rs:141-154`; repository `src/collector/process.rs:6-12`]
- **Stale and partial:** It omits compaction/branch-summary usage, `session_info` naming, `custom_message`, extension-created tools, and current extension lifecycle visibility. Its `subagents=[]` choice is correct for core Pi but incomplete for installations with `pi-subagents`. [Pi `docs/session-format.md:145-233`; Pi `dist/core/extensions/types.d.ts:330-500`; `pi-subagents` `docs/observability.md:146-226`]

## Phased implementation plan

1. **Foundation and process-only rows:** Add `PiCollector` and `pi` hidden/kill recognition. Detect direct wrappers, Node/Bun installs, and RPC mode without bare-string false positives. Create a safe row before a session file exists. Attach persistent JSONL only from high-confidence ownership evidence. Define Windows v1 as process-only or extend `ProcInfo` with a trustworthy cwd/session mapping.
2. **Parser and tailer:** Implement typed defensive parsing for headers, messages, model/thinking entries, compaction, branch summaries, session info, and content blocks. Cache offsets, carry partial UTF-8/JSON lines, reset on rotation, and preserve bounded data. Keep all-entry billed totals separate from inferred-branch context. Count assistant, nested tool-result, compaction, and branch-summary usage once.
3. **Branch and context semantics:** Build the inferred branch by `parentId`; mirror Pi’s last-valid-assistant `totalTokens` baseline, trailing-message estimate, and post-compaction unknown rule. Store telemetry precision (`unknown`, `inferred`, or `exact`) and never present an unpersisted leaf guess as authoritative.
4. **Domain, snapshot, and Pi-first UI:** Add provider, session name, cost, queue, workflow, active-leaf precision, and richer child/run records to the model. Update snapshot serialization and consumers. Replace the quota panel with fleet/runtime data; update sessions, context, tokens, and detail views. Reuse ports, git, orphan handling, kill, and jump.
5. **`pi-subagents` adapter:** Discover supported async roots and versioned `status.json` files. Join runs to parent and child session IDs, render workflow/child state, and use `events.jsonl` only for transition history. Define deduplication between Pi transcript usage and extension aggregates. Do not read output logs or transcripts for the default view.
6. **Optional precision bridge:** Publish a user-installed Pi extension that writes a private 0600 status record keyed by PID/session ID with exact leaf identity, active tool metadata, `getContextUsage()`, active model context window, queue/UI wait state, and provider 429/retry hints. It must never log prompts, tool output, API keys, headers, or raw arguments.
7. **Optional model resolution:** Parse safe local `models.json` and catalog cache only when their schema/version is known. Mark extension, dynamic, and OAuth catalog entries unresolved. Never read or expose `auth.json`.

## Rough sizing

Assuming one engineer familiar with this Rust codebase:

- Cross-platform process-only rows plus passive session attachment: 4 to 7 engineer-days.
- Incremental parser, branch/context semantics, privacy hardening, and integration tests: 3 to 5 additional days.
- Domain/snapshot changes, Pi-specific UI, and the `pi-subagents` adapter: 5 to 8 additional days.
- Optional exact-state bridge: 2 to 3 additional days.

The smallest useful release is process discovery plus a passive collector that labels branch and context precision honestly. A polished Pi-specific monitor with fleet support is roughly a 3-to-4-week feature for one engineer. The bridge and full Windows session attachment can extend that toward five weeks.

## Tests to add

- Process matching: direct wrapper, Node/Bun-installed CLI, RPC mode, reject `pip` and unrelated `pi`, pre-transcript and ephemeral process-only rows, transient descendant environment markers, same-cwd collision, and Windows process-only behavior.
- JSONL: v1/v2/v3 migration tolerance, malformed or truncated final line, rotation, an unpersisted leaf move followed by append, compaction, model change, session name, tool-call/result error, and nested tool usage.
- Accounting: lifetime totals include assistant, nested tool-result, compaction, and branch-summary usage exactly once. Context tests cover last-assistant `totalTokens`, component fallback, trailing estimates, unresolved windows, and post-compaction unknown state.
- Domain and serialization: provider, session name, cost, queue, workflow/run metadata, active-leaf precision, backward-compatible snapshots, and bounded richer child records.
- Privacy and safety: symlink rejection, oversized fields, terminal controls, secrets, and no tool arguments beyond a permitted path/command summary.
- `pi-subagents`: versioned status files, queued/running/terminal states, foreground versus background semantics, nested children, usage deduplication, stale-run reconciliation, unknown fields, and retention cleanup.
- Integration: Pi sessions participate in hidden agents, child ports/orphans, git refresh, kill verification, terminal jump, snapshot serialization, and the Pi-specific fleet panel.

## Sources retained

- Installed Pi 0.85.1 README, session, environment, model, extension, SDK, RPC, JSON, and compaction documentation: first-party behavior and storage contracts.
- Installed declarations under Pi `dist/core/` and `dist/modes/`: current shipped type contracts where prose documentation is incomplete.
- [Pi mono v0.85.1 source](https://github.com/earendil-works/pi-mono/tree/v0.85.1/packages/coding-agent/src): session-path encoding, JSONL reading, context calculation, and runtime events.
- Current checkout `src/collector/*`, `src/model/session.rs`, `src/app.rs`, `src/jump/mod.rs`, and `src/ui/*`: exact integration surface.
- Installed `pi-subagents` README, `docs/observability.md`, and lifecycle/status source: current extension-specific child and workflow telemetry.
- Guide branch: retained only as a historical design sketch because it conflicts with Pi 0.85.1 and this checkout.

## Gaps and decisions still needed

- Pi exposes no stable external PID-to-session registry. Passive mapping remains ambiguous for idle same-cwd sessions, pending sessions, and ephemeral runs.
- Pi does not persist every active-leaf move. Passive branch, task, model, and context state can remain inferred until the next append.
- Pi exposes no provider-independent account quota API or persisted exact parent context measurement.
- Foreground `pi-subagents` children share the parent process. Exact live foreground state requires extension cooperation; process scanning cannot provide it.
- Windows process discovery exists, but the current process model does not expose the cwd or environment needed for passive session ownership.
- Effective model metadata can be dynamic or extension-provided.
- A dedicated `pi-subagents` adapter couples the monitor to an extension schema. Gate it by lifecycle artifact version and ignore unknown fields.
- Decide whether the first release is additive Pi support or changes the product default to Pi-only. The collector work is shared; the panel and configuration changes are not.

No source code was built or tested because this task produced research only. Implementation should validate with `cargo test`, `cargo clippy -- -D warnings`, and fixture-driven `--once` or snapshot output.