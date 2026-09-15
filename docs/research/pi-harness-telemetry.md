# Research: Privacy-safe Pi harness telemetry for ptop

## Status

This document is a research basis for later work. Phase 0 and Phase 1 are
complete. The owner approved and `main` implements the Phase 2 aggregate and
bounded-history slices specified in
[`pi-harness-telemetry-phase-2.md`](pi-harness-telemetry-phase-2.md) and
[`pi-harness-telemetry-phase-2-history.md`](pi-harness-telemetry-phase-2-history.md).
Phase 3 is complete and release-accepted. Its optional local AF_UNIX extension
uses kernel-authenticated endpoint identity under ptop's existing benign
same-user threat model. The implementation includes the private protocol and
reducer, embedded extension lifecycle/runtime, credentialed transport, two-tick
validation, public DTO, and optional selected-session presentation.

## Summary

Pi persists privacy-safe metadata that can improve ptop without retaining
conversation payloads. Useful candidates include assistant terminal outcomes,
message-level model attribution, numeric usage, reported cost, compaction
metrics, and bounded persisted-tree facts.

The safest first work is not new UI. It is fixing and documenting the accounting
contract:

- Aggregate every successfully parsed persisted branch within parser limits.
- Keep parent session usage separate from `pi-subagents` fleet usage.
- Keep tool-result, compaction, and branch-summary usage in the parent total, but
  do not add it again when showing a breakdown.
- Mark totals partial when required assistant usage is absent or when an
  observed usage object is malformed, incomplete, oversized, or overflowing.
- Treat absent optional tool-result, compaction, and branch-summary usage as no
  contribution, matching Pi. Track optional coverage separately if needed.
- Preserve unavailable components and reported cost as unavailable. Do not
  replace them with zero.
- Use Pi's attribution key, `provider/(responseModel ?? model)`, without assigning
  undocumented business meanings to `model` or `responseModel`.

Passive JSONL does not prove current streaming state, queue depth, retry state,
an in-flight tool, UI-prompt state, or the active leaf after navigation without
an append. RPC is not an acceptable monitoring transport because it is a
content-bearing, mutating control protocol over a Pi subprocess. The implemented
optional Unix AF_UNIX design uses endpoint identity rather than byte authorship
and passes the global registry, freshness, installation, session-binding, public
API, and presentation gates in the Phase 3 plan.

**Evidence scope.** This research uses installed first-party Pi `0.85.1` and the
current ptop implementation and contracts. Where Pi documentation and installed
declarations differ, installed declarations define the supported implementation
surface. A contradiction still causes ptop to fail closed.

## Verified findings

### Persisted assistant metadata

Each persisted assistant message can contain `provider`, `model`, optional
`responseModel`, numeric `usage`, `stopReason`, and the entry timestamp. Its
content and error text are private.

The fields do not establish that `model` means "requested" or that
`responseModel` means "actual" or "billed." Pi's own usage display groups an
assistant message under:

```text
provider/(responseModel ?? model)
```

ptop should call this the **Pi attribution key**. If it retains both fields, name
them **message model** and **response-model field**. Do not infer stronger
semantics.

Safe extraction candidates are:

- bounded `provider`, `model`, and `responseModel` values;
- complete numeric usage components;
- independently optional reported cost;
- a bounded terminal-outcome enum; and
- a parseable entry timestamp, if the privacy contract approves retaining it.

Do not extract `api` without a documented user need and an explicit change to
[the ptop privacy contract](../pi-support.md#privacy-boundary). Do not retain
content, error messages, thinking text, images, tool calls, arguments, or
results.

The declared stop-reason union is `pending`, `stop`, `length`, `toolUse`,
`error`, `aborted`, and `deferred`. `pending` is an unexpected nonterminal value
in persisted input, not a terminal outcome. Count known terminal values and an
unknown/future bucket. Never inspect error text to create finer error classes.

**Evidence:** [Pi session format](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/docs/session-format.md),
[pi-ai message types](https://github.com/earendil-works/pi/blob/v0.85.1/packages/ai/src/types.ts),
and [Pi usage attribution](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/src/core/usage-totals.ts).

### Parent usage accounting

Pi and ptop aggregate usage from assistant messages, tool-result messages,
compactions, and branch summaries across persisted entries. Pi assigns
non-assistant usage to the display bucket `Tools/summaries`.

ptop should not use that display text as its domain model. A bounded internal
enum such as `UnattributedToolOrSummary` can map to UI text later. The bucket is
a breakdown of the parent total, not another amount to add. Call nested usage
**tool-reported usage**, not "tool-LLM usage," because the declaration does not
promise that an LLM produced it.

Reconciliation is conditional, and usage applicability depends on record type:

- Assistant usage is required by the installed Pi type. Missing, null,
  malformed, or incomplete assistant usage makes component totals partial.
- Tool-result, compaction, and branch-summary usage is optional. An absent field
  contributes nothing and does not make totals partial, matching Pi. A present
  but malformed or incomplete usage object does make totals partial.
- Component totals reconcile only when every required or observed usage record
  has all four valid components and no checked addition overflows.
- Reported cost reconciles only when every cost needed by that view is present,
  valid, and finite.
- `totalTokens` must not substitute for missing components in component
  accounting. ptop currently uses it as a context baseline when valid.
- Oversized, parser-limited, or overflowing data produces an explicit partial
  reason.

Current ptop marks an observed incomplete usage object partial. It does not
necessarily mark missing required assistant usage partial. Phase 0 must fix that
case before new usage reporting. If ptop later needs to describe how often
optional records supplied usage, model that as separate coverage metadata rather
than aggregate completeness.

There is no proof that tool-reported usage is disjoint from `pi-subagents` fleet
accounting. Keep fleet usage separate and never combine it with the parent total.

**Evidence:** [Pi usage attribution](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/src/core/usage-totals.ts),
[ptop parent accounting](../../src/collector/pi.rs), and
[the fleet accounting contract](../pi-support.md#fleet-and-run-telemetry).

### Persisted tree metadata

Pi entries use IDs and parent links to form an append-only persisted tree.
Append order is not active-branch order. Tree navigation can change Pi's
in-memory leaf without appending another entry, so passive JSONL cannot prove the
current active leaf, active path, active depth, active-path compactions, or
latest active-path outcome.

ptop currently exposes an `active_leaf_id` inferred from the last persisted
entry. That field is not authoritative. Before adding more topology telemetry,
resolve whether it remains an explicitly inferred compatibility field or becomes
`null` without trusted live evidence.

Entry IDs are internal linkage data, not approved public telemetry. Do not add
new IDs to snapshots by default. Persisted topology counts are possible only
after a bounded whole-tree validator exists. The validator must define:

- which known and unknown entry types participate in linkage;
- maximum ID and parent-ID lengths;
- unique IDs and duplicate handling;
- root, missing-parent, cycle, and disconnected-node handling;
- child-count and total-node bounds;
- parser-limit behavior; and
- partial or unavailable output for every ambiguity.

A **leaf** in passive telemetry can only mean a validated persisted-graph leaf.
It must never mean the live active-session leaf.

**Evidence:** [Pi session manager](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/src/core/session-manager.ts),
[ptop tree parsing](../../src/collector/pi.rs), and
[ptop snapshot conversion](../../src/snapshot.rs).

### Compaction and branch summaries

Compaction and branch-summary entries can provide numeric usage and bounded
operational metadata. Their summaries, retained messages, file lists, and
arbitrary `details` remain private.

Potential safe fields are the event kind, `tokensBefore`, complete numeric
summary-generation usage, and a bounded provenance enum such as Pi or extension
when directly known. Timestamp retention needs the same privacy decision as
assistant timestamps. Trigger reason is not persisted, so ptop must not label a
compaction manual, threshold-triggered, or overflow-triggered.

Pi's session-format documentation describes `retainedTail`, which can contain
messages. The installed `CompactionEntry` declaration does not include it.
ptop must not parse or retain its contents.

Current ptop already returns unknown context until it observes a valid assistant
baseline after the latest compaction. Once that baseline exists, it does not use
the compaction summary as the baseline. Add a `retainedTail` fixture to confirm
content suppression and this guard. A blanket rule that invalidates context even
after a later valid baseline would be a new conservative schema policy, not a
fix for demonstrated current behavior.

**Evidence:** [Pi compaction guide](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/docs/compaction.md),
[Pi session format](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/docs/session-format.md),
and [Pi session manager](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/src/core/session-manager.ts).

### Live harness state

Pi's in-process session and RPC protocol expose richer state than persisted
JSONL. That is evidence that the state exists, not permission to use RPC as a
monitoring transport. RPC uses subprocess stdin/stdout, returns content-bearing
state, and supports commands that mutate Pi. ptop must not launch, attach to, or
control RPC for an existing process.

The installed extension API has broad lifecycle coverage, including session,
session-tree, agent, turn, message, tool execution, UI prompt, compaction,
model-selection, and thinking-selection events. It can sample
`hasPendingMessages()`. It does not expose queue-change or retry lifecycle events,
so exact queue depth and retry progress require new Pi hooks.

Extension events can include forbidden data. Never write event objects, prompt
text, assistant text, message deltas, queue text, tool arguments, tool results,
raw errors, extension paths, or UI titles to ptop telemetry.

`appendEntry` is not a live-state transport. It persists a tree node, advances
Pi's leaf, grows the transcript, and consumes ptop's bounded semantic-entry
budget.

**Evidence:** [AgentSession](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/src/core/agent-session.ts),
[extension types](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/src/core/extensions/types.ts),
and [RPC types](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/src/modes/rpc/rpc-types.ts).

## UI direction

Start with compact session-detail information. Do not lead with a graph or a
large table before completeness states are understandable.

1. Show bounded terminal-outcome counts and a compact Pi-attribution summary.
2. Show whether component totals and reported cost are complete, partial, or
   unavailable, with bounded reasons.
3. Add a recent 64-point component-usage graph only after gaps and attribution
   changes are clear. Keep one Pi session palette. Provider or model names are
   metadata, not separate monitored agents.
4. Add compaction markers without summary text, file lists, or unsupported
   trigger labels.
5. Put full model tables and validated persisted-tree topology behind progressive
   disclosure.
6. If a sidecar passes its feasibility gate, start with generic badges such as
   `Generating`, `Running tool`, `Compacting`, `Waiting for user`, and
   `Pending message`. Do not display arbitrary tool names.

Every new view must preserve usable wide, compact, and narrow layouts.

## Collection boundaries

### Passive JSONL

Passive extraction remains subject to the existing ownership, header, working
directory, file-identity, version, size, and ambiguity checks. Failure preserves
a process-only row.

Within those checks, passive JSONL can support bounded persisted assistant
outcomes, Pi-attribution model data, complete numeric usage, reported cost,
compaction metadata, and eventually validated persisted-tree facts. It cannot
prove live state or the active tree cursor.

All-branch reporting means all successfully parsed persisted entries within
limits. It does not mean ptop saw bytes or entries that exceeded a limit. Every
published aggregate needs precision, completeness, provenance, source health,
and observation time consistent with [the Pi support contract](../pi-support.md).

### Cross-session history

Do not add historical storage or a default arbitrary-session crawl. Revisit only
for a specific product requirement with:

- an explicit local-only setting;
- bounded aggregate retention;
- clear reset and deletion behavior;
- no transcript index or payload retention;
- no network or sync; and
- a documented distinction between live-session and historical totals.

### Optional sidecar gate: approved AF_UNIX endpoint identity

A sidecar is now an optional Unix product design, not a file-based authorship
experiment. Local AF_UNIX IPC is permitted for the optional Pi extension.
Internet, IP, HTTP, RPC, and other network monitoring remain prohibited.

ptop starts sidecar discovery only after independently completing its current
owned JSONL attachment: process-first discovery, header/CWD/file identity,
ambiguity, and Unix process-start checks. The socket path and all self-reported
fields are discovery hints only. ptop accepts only one live candidate whose
kernel peer credential identifies the exact currently verified Pi PID, whose
first bounded frame matches the already attached session identity, and whose
existing process-start identity still matches before and after connection/frame
handling. Linux uses `SO_PEERCRED`; Darwin support requires runtime-successful
peer PID/token APIs. Missing or ambiguous APIs fail closed.

This is kernel-authenticated **endpoint identity**, not per-byte authorship. A
connected descriptor can be transferred to a different writer. Malicious
same-UID processes, malicious extensions inside Pi, root/kernel attackers, and
deliberate descriptor transfer are outside the benign same-user threat model,
consistent with current local attachment. Never claim that every byte came from
Pi.

The extension may reduce only generic phase and a sampled pending-message
boolean. Unknown is `null` for both values, never a fabricated idle/false value
or an `unknown` phase string. It must reduce events immediately and never retain,
write, or log payloads, event objects, raw errors, titles, tool names, or paths.
`--demo` never discovers or connects. A PiCollector-owned global registry scans
one complete bounded generation, not once per session, stages frames until the
next normal process snapshot confirms identity, and leaves existing streams alive
while new discovery is unavailable. Failure leaves valid JSONL telemetry
unchanged, or otherwise leaves a process-only row. Regular-file sidecars remain
rejected: permissions, atomic replacement, inode matching, and sequence checks
do not authenticate writers.

The approved contract, measured Linux/Darwin probe evidence, and remaining
implementation conditions are in
[`pi-harness-telemetry-phase-3.md`](pi-harness-telemetry-phase-3.md). The
reviewable execution plan and fixture matrix are in
[`pi-harness-telemetry-phase-3-plan.md`](pi-harness-telemetry-phase-3-plan.md).

## Revised roadmap

### Phase 0: contract and correctness

- Mark missing required assistant usage partial. Treat absent optional
  tool-result, compaction, and branch-summary usage as no contribution; mark a
  present malformed usage object partial.
- Define component-token, optional-usage coverage, `totalTokens`, and
  reported-cost semantics.
- Define Pi attribution-key fields without calling them requested or actual.
- Keep parent, unattributed tool/summary, and fleet accounting separate.
- Remove `api` from the proposal.
- Decide whether timestamps, IDs, and provenance are internal-only or public.
- Resolve the existing inferred `active_leaf_id` contract.
- Define bounded enums, strings, point counts, model buckets, and partial reasons.
- Add a `retainedTail` fixture that proves payload suppression and preserves the
  existing post-compaction assistant-baseline guard.

### Phase 1: passive parser and internal model

- Add bounded assistant terminal outcomes.
- Add bounded message model, response-model field, provider, and Pi attribution
  data.
- Add complete component usage points and optional reported cost.
- Add declared compaction metrics without content.
- Preserve all successfully parsed persisted branches within current limits.
- Keep unattributed tool/summary usage separate in the breakdown.
- Add no topology output until the whole-tree validator and privacy contract are
  complete.
- Publish no active-leaf or active-path claims from passive JSONL.

Do not publish new UI or snapshot fields during this phase.

### Phase 2: snapshots and UI

The approved first-slice DTO, UI, compatibility, and caller interface is in
[`pi-harness-telemetry-phase-2.md`](pi-harness-telemetry-phase-2.md) and is
implemented on `main`. The next bounded-history and summary-marker slice is
approved in
[`pi-harness-telemetry-phase-2-history.md`](pi-harness-telemetry-phase-2-history.md).

- Add compatibility-reviewed optional DTOs. Authoritative unavailable values use
  `null`.
- Add explicit component and reported-cost reconciliation status.
- Deliver compact session-detail outcomes and model attribution first.
- Add bounded graphs, full tables, compaction markers, and validated topology
  only through progressive disclosure.
- Preserve existing JSON compatibility where possible.
- Test wide, compact, and narrow layouts and aligned click targets.

### Phase 3: optional Unix AF_UNIX live telemetry

- Implement the owner-approved endpoint-identity contract through the
  reviewable slices in
  [`pi-harness-telemetry-phase-3-plan.md`](pi-harness-telemetry-phase-3-plan.md),
  including explicit local embedded-extension `install`, `status`, and `remove`
  commands; complete global discovery generations; and cross-tick frame
  acceptance.
- Publish generic phase and sampled pending-message state only; keep exact queue
  depth, retry progress, and arbitrary tool names out of scope.
- Keep Windows process-only, fleet separate, and `--demo` collector-free.
- Fail closed on runtime API, credential, discovery, frame, freshness, or
  ambiguity failure while preserving valid JSONL telemetry or the process row.
- Phase 3 is complete and release-accepted. The Live row appears only when the
  selected-session detail has genuine surplus capacity.

### Deferred: cross-session history

Keep history deferred until a concrete product requirement justifies a new
retention contract.

## Required validation

### Usage and attribution fixtures

- Assistant records with complete, omitted, null, non-object, incomplete,
  negative, wrong-typed, and overflowing usage.
- Optional tool-result, compaction, and branch-summary usage when absent,
  complete, present but malformed, and present but incomplete.
- Invalid, missing, and overflowing reported cost.
- `model` with and without `responseModel`.
- Bounded and oversized provider/model values.
- Every known stop reason, persisted `pending`, and an unknown future value.
- Parent/tool-summary/fleet separation and conditional reconciliation.
- Branches abandoned by later navigation.

### Compaction fixtures

- Declared compaction fields with and without usage.
- Branch-summary usage.
- Summary and `details` privacy suppression.
- `retainedTail` presence without content retention.
- Unknown fields and future versions.
- The existing unknown-until-post-compaction-baseline behavior when
  `retainedTail` is present.

### Tree-validator fixtures before topology work

- Duplicate IDs, missing parents, cycles, multiple roots, disconnected nodes,
  oversized IDs, excessive children, unknown entry types, and semantic-entry
  limit exhaustion.
- Navigation to an earlier leaf without an appended entry.
- A clear distinction between persisted graph leaves and the unknown live leaf.

### Sidecar implementation fixtures

- Normal lifecycle, session replacement, reload, crash, session switch,
  reconnect, competing connections, EOF, stale expiry, sequence rollback, and
  PID reuse.
- Symlink, wrong owner, permissive directory/socket, wrong type, bounded
  candidate exhaustion, malformed/oversized framing and schema, unknown version,
  control/bidi strings, wrong peer PID, unsupported Darwin API, and mutation
  tests for every fail-closed guard.
- A Pi process launched with `--no-session`, no extension, Linux/macOS
  differences, Windows process-only behavior, JSON/text privacy sentinels,
  fleet exclusion, and `--demo` zero access.
- Proof that neither extension nor collector logs payloads or uses Internet, IP,
  HTTP, RPC, TCP, or UDP monitoring.

## Sources

- [Pi coding-agent package manifest](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/package.json)
- [Pi session format](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/docs/session-format.md)
- [Pi compaction guide](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/docs/compaction.md)
- [Pi extensions guide](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/docs/extensions.md)
- [Pi RPC guide](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/docs/rpc.md)
- [Pi SDK guide](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/docs/sdk.md)
- [Pi session manager](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/src/core/session-manager.ts)
- [AgentSession](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/src/core/agent-session.ts)
- [pi-ai message types](https://github.com/earendil-works/pi/blob/v0.85.1/packages/ai/src/types.ts)
- [Pi usage attribution](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/src/core/usage-totals.ts)
- [Extension types](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/src/core/extensions/types.ts)
- [RPC types](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/src/modes/rpc/rpc-types.ts)
- [ptop Pi contract](../pi-support.md)
- [ptop model](../../src/model/session.rs)
- [ptop Pi collector](../../src/collector/pi.rs)
- [ptop snapshots](../../src/snapshot.rs)

The upstream links are pinned to Pi `v0.85.1`, matching the installed package
used for this research.
