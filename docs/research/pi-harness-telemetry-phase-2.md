# Pi harness telemetry Phase 2 design

## Status

Approved by the owner on 2026-09-13. This branch implements the exact first
Phase 2 delivery slice.

Phase 0 and Phase 1 are on `main`. Their parser, accounting, privacy, and
attachment contracts remain in force.

## Approved decisions

The owner approved the defaults below before implementation started.

| Decision | Approved behavior |
|---|---|
| First delivery | Add aggregate assistant outcomes, component reconciliation, reported-cost reconciliation, and Pi model attribution to JSON and selected-session detail. |
| Deferred work | Do not expose point history, graphs, compaction events, full model tables, persisted topology, or live sidecar state in this slice. |
| JSON location | Add one `harness` key under each existing `sessions[].telemetry` object. |
| Availability | Serialize `harness` as `null` for process-only rows, Pi session versions 1 and 2, unsupported versions, failed attachment, and an inconsistent private projection. A valid version-3 projection gets an object, including an empty known-zero session. |
| Unknown values | Use `null` for unavailable authoritative numbers and component groups. Never substitute zero. |
| Compatibility | Keep all existing JSON keys, values, nesting, enum spellings, and compatibility placeholders unchanged. The new key is additive. Keep `--once` text unchanged. |
| Rust API | Add the optional harness value to public `SessionTelemetry`. Treat this as a Rust source-compatibility change and ship it in the next minor release, not a patch release. |
| Internal seam | Continue reducing through `PiSemantic::session_data`. Carry the approved aggregate through private `PiSessionData` into `SessionTelemetry`. Do not change `Collector::collect` or add an `App` side map. |
| Ordering and bounds | Publish at most 64 named attribution buckets. Sort them by descending observed component total, then provider, then model. Keep strings at the existing 256-byte bound. Bound generated reason text to 160 bytes. |
| TUI location | Add compact lines to the existing selected Pi session detail. Add no panel, tab, key binding, mode, color role, or click target. |
| Text output | Do not add harness fields to `--once`. JSON and the TUI are the approved consumers for this slice. |

## Goals

1. Make Phase 1 aggregate telemetry available without exposing retained parser
   records.
2. Distinguish complete, partial, and unavailable component and cost totals.
3. Show compact persisted assistant outcomes and Pi model attribution for the
   selected live session.
4. Preserve process-first discovery, attachment checks, source health,
   observation metadata, fleet separation, and null-for-unknown behavior.
5. Keep the first UI small enough to work in wide, compact, and narrow layouts.

## Non-goals

This slice does not add:

- assistant or summary point histories to public Rust types or JSON;
- a token-component graph;
- compaction or branch-summary markers;
- summary text, retained-tail content, IDs, parent IDs, or tree topology;
- requested-model versus response-model claims;
- live active-leaf, active-path, or active-depth claims;
- a full attribution table or attribution drill-down;
- another collector, monitored-agent type, mode, or palette color;
- historical storage or arbitrary session-file scanning;
- sidecar discovery or live Pi harness state;
- network requests; or
- changes to fleet-run accounting.

Those items require separate approval after this slice is shipped and its
completeness labels are understandable in practice.

## Existing constraints

The design follows these current contracts:

- A live Pi process remains visible when telemetry is missing or rejected.
- Rich parsing is supported only for Pi session version 3.
- Parent transcript usage stays separate from fleet-run usage.
- Passive JSONL covers successfully parsed persisted entries across branches. It
  does not prove live state or the active tree cursor.
- Model and provider names are Pi metadata, not separate monitored agents.
- Prompt text, assistant text, tool arguments, tool results, summary payloads,
  retained-tail content, and child transcripts remain prohibited.
- The parser keeps at most 64 assistant observations, 64 named attribution keys,
  and 64 summary observations.
- `active_leaf_id` remains present for compatibility and always serializes as
  `null`.

## Exact public types

Add these serializable types in `src/model/session.rs`. Field names below are
also the JSON key names unless stated otherwise.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationStatus {
    Unavailable,
    Partial,
    Complete,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TokenComponents {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssistantOutcomeTelemetry {
    pub status: ReconciliationStatus,
    pub total: u32,
    pub stop: u32,
    pub length: u32,
    pub tool_use: u32,
    pub error: u32,
    pub aborted: u32,
    pub deferred: u32,
    pub pending: u32,
    pub unknown: u32,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComponentReconciliation {
    pub status: ReconciliationStatus,
    pub total: Option<TokenComponents>,
    pub assistant: Option<TokenComponents>,
    pub unattributed_tool_or_summary: Option<TokenComponents>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReportedCostReconciliation {
    pub status: ReconciliationStatus,
    pub total: Option<f64>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PiAttributionBucket {
    pub provider: String,
    pub model: String,
    pub components: TokenComponents,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PiAttributionTelemetry {
    pub named: Vec<PiAttributionBucket>,
    pub unavailable: TokenComponents,
    pub overflow: TokenComponents,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PiHarnessTelemetry {
    pub assistant_outcomes: AssistantOutcomeTelemetry,
    pub components: ComponentReconciliation,
    pub reported_cost: ReportedCostReconciliation,
    pub attribution: Option<PiAttributionTelemetry>,
}
```

Add this field to the existing public model:

```rust
pub struct SessionTelemetry {
    // Existing fields remain unchanged and in their current order.
    pub harness: Option<PiHarnessTelemetry>,
}
```

Add the same field to `SessionTelemetryView` and clone it from
`SessionTelemetry` in `App::to_snapshot`:

```rust
pub struct SessionTelemetryView {
    // Existing fields remain unchanged and in their current order.
    pub harness: Option<PiHarnessTelemetry>,
}
```

Do not flatten any new type. One nested key limits collisions and gives consumers
one availability boundary.

### Field semantics

#### `ReconciliationStatus`

- `complete`: all expected persisted data covered by that aggregate was parsed
  and reconciled.
- `partial`: at least one authoritative value is available, but one or more
  expected records, fields, or bytes were unavailable or invalid.
- `unavailable`: no authoritative numeric aggregate can be published.

This enum is separate from `TelemetryCompleteness`. The existing enum includes
`unknown`, while this DTO needs the user-facing distinction `unavailable`.

#### `assistant_outcomes`

- `total` equals the saturating sum of the eight outcome counters.
- Counters cover accepted persisted assistant entries across all parsed branches.
- `status` is `partial` when the tail scan or semantic reduction is incomplete,
  an entry is invalid, or duplicate IDs make coverage ambiguous.
- A valid, fully scanned version-3 session with no assistant entries is
  `complete` with zero counters.
- `reason` is `null` when complete. Partial reasons are generated from fixed
  ptop text, never source payload text.
- An unavailable outcome object is not emitted. The enclosing `harness` is
  `null` instead.

#### `components`

- `total` is the observed parent transcript component total.
- `assistant` contains only complete assistant component observations.
- `unattributed_tool_or_summary` contains complete tool-result, compaction, and
  branch-summary component observations. It is not model-attributed.
- All three component objects are present together when at least one complete
  component observation exists, or when a fully scanned session is known empty.
- All three are `null` when no authoritative component observation exists and
  the session cannot be established as known zero.
- Partial component objects are observed lower bounds. The TUI marks them
  partial and must not use them for rates or complete aggregates.
- When present, `total` must equal `assistant` plus
  `unattributed_tool_or_summary` component by component.
- Existing `telemetry.usage` totals remain unchanged. When the new `total` is
  present, its four values must equal the existing four authoritative usage
  component values.

#### `reported_cost`

- `complete` requires every expected cost to be present, valid, finite,
  non-negative, and covered by a complete scan.
- `total` is non-null only for `complete`, including complete zero cost.
- `partial` always has `total: null`.
- `unavailable` means no cost expectation exists and has `total: null`.
- Existing `telemetry.usage.reported_cost` remains unchanged and equals this
  `total` when status is `complete`.
- Cost status remains independent from component status.
- `reason` is `null` when complete. Otherwise it uses fixed ptop text such as
  `no reported cost observations`, `an expected reported cost is unavailable`,
  or `persisted session scan is incomplete`.

#### `attribution`

- `named` is usage attribution by the Pi `(provider, responseModel ?? model)`
  rule already implemented in Phase 1.
- It does not claim requested model, selected model, or a separate agent.
- `unavailable` is a fixed bucket for assistant component usage whose provider
  or model attribution was absent or invalid. The word describes attribution,
  not source availability.
- `overflow` is a fixed aggregate for valid named keys beyond the first 64
  retained keys.
- `named + unavailable + overflow` must equal `components.assistant`
  component by component.
- `attribution` is `null` when `components.total` is `null`. It is present for
  complete and partial observed component totals, including known zero.
- `named` is sorted by descending sum of its four component fields. Ties sort by
  provider, then model, using bytewise ascending order. This removes `HashMap`
  iteration nondeterminism.
- Provider and model strings retain the current 256-byte safe metadata bound and
  control and bidi-control rejection.

### Reason bounds

Every new `reason` is built only from fixed ptop phrases. Join multiple phrases
with `; `, remove duplicates, and cap the final UTF-8 string at 160 bytes without
splitting a code point. Do not copy parser errors or source values into these
fields.

## Exact JSON shape

An attached version-3 session with complete data adds this sibling under the
existing telemetry object:

```json
{
  "telemetry": {
    "attachment": "attached",
    "attachment_confidence": "high",
    "source_health": "healthy",
    "error": null,
    "context": {},
    "usage": {},
    "fleet": {},
    "harness": {
      "assistant_outcomes": {
        "status": "complete",
        "total": 3,
        "stop": 1,
        "length": 0,
        "tool_use": 2,
        "error": 0,
        "aborted": 0,
        "deferred": 0,
        "pending": 0,
        "unknown": 0,
        "reason": null
      },
      "components": {
        "status": "complete",
        "total": {
          "input_tokens": 120,
          "output_tokens": 30,
          "cache_read_tokens": 10,
          "cache_write_tokens": 5
        },
        "assistant": {
          "input_tokens": 100,
          "output_tokens": 30,
          "cache_read_tokens": 10,
          "cache_write_tokens": 5
        },
        "unattributed_tool_or_summary": {
          "input_tokens": 20,
          "output_tokens": 0,
          "cache_read_tokens": 0,
          "cache_write_tokens": 0
        },
        "reason": null
      },
      "reported_cost": {
        "status": "complete",
        "total": 0.0125,
        "reason": null
      },
      "attribution": {
        "named": [
          {
            "provider": "openai",
            "model": "gpt-5",
            "components": {
              "input_tokens": 100,
              "output_tokens": 30,
              "cache_read_tokens": 10,
              "cache_write_tokens": 5
            }
          }
        ],
        "unavailable": {
          "input_tokens": 0,
          "output_tokens": 0,
          "cache_read_tokens": 0,
          "cache_write_tokens": 0
        },
        "overflow": {
          "input_tokens": 0,
          "output_tokens": 0,
          "cache_read_tokens": 0,
          "cache_write_tokens": 0
        }
      }
    }
  }
}
```

The abbreviated existing `context`, `usage`, and `fleet` objects above keep
all current fields. The example omits them only to focus on the new shape.

For process-only rows and attached version-1 or version-2 sessions:

```json
{
  "telemetry": {
    "harness": null
  }
}
```

The containing telemetry object keeps all current fields in real output.

## Internal caller interface

Keep the existing reduction path and extend its private result:

```text
PiSemantic::add(Value)
    -> bounded private PiEntry state

PiSemantic::session_data_for_version(context_complete, rich_supported)
    -> private PiSessionData
       - existing compatibility and context fields
       - harness: Option<PiHarnessTelemetry>

PiCollector::telemetry_for_attachment(...)
    -> resolves the context window from the produced PiSessionData provider/model
    -> SessionTelemetry { harness, ... }
    -> AgentSession.telemetry

TUI reads AgentSession.telemetry.harness
App::to_snapshot clones it into SessionTelemetryView.harness
```

Implementation rules:

1. Add `harness: Option<PiHarnessTelemetry>` to private `PiSessionData`.
2. Build it only in the existing version-3 rich reduction path.
3. Convert `ParentHarnessTelemetry` to `PiHarnessTelemetry` once, after
   `ParentHarnessTelemetry::validates()` succeeds.
4. If `ParentHarnessTelemetry::validates()` fails, set `harness` to `None`, keep
   the process row, and preserve the existing partial or unavailable base
   telemetry behavior. Do not emit a partly trustworthy harness object.
5. Change `resolve_context_window` to accept the already-produced provider and
   model values:

   ```rust
   fn resolve_context_window(
       &mut self,
       attachment: &PiAttachment,
       provider: &str,
       model: &str,
   ) -> Option<u64>
   ```

   Call it with `data.provider` and `data.model`. Remove its current call to
   `PiSemantic::session_data_for_version` so rich reduction and DTO conversion
   happen once per attachment refresh.
6. Versions 1 and 2, unsupported versions, process-only rows, attachment errors,
   and inconsistent private projections set `harness` to `None`.
7. Keep `Collector::collect` returning `(Vec<AgentSession>, Vec<OrphanPort>)`.
8. Do not add an `App` map, collector cache, second parser, or second reduction
   pass.
9. Keep `AssistantObservation` and `SummaryObservation` private. Aggregate from
   their existing bounded projections, then discard them at the module boundary.
10. Do not expose `PiAttributionKey` directly. Convert its map to the sorted DTO
    vector.

This adds one approved consumer interface without weakening the existing deep
module boundary around parsing and validation.

## TUI presentation

### Placement

Use the existing selected-session detail rendered by `src/ui/sessions.rs`.
Do not add a panel or change desktop panel order. Narrow layouts keep the Work,
Usage, and System tabs. The new lines belong to the selected session detail in
the Sessions section.

### Lines

When `harness` is present, render these lines in this order after the essential
attachment, source, context, and token lines:

1. `Outcomes <status> · <total> · <non-zero outcome counts>`
2. `Models <top attribution summaries>`
3. `Usage components <status> · reported cost <status/value>`
4. A reason line only when space remains and a status is not complete.

Outcome counts use this fixed order and these labels:

```text
stop, tool, length, error, aborted, deferred, pending, unknown
```

Omit zero categories. A known-zero session renders:

```text
Outcomes complete · 0
```

A normal example is:

```text
Outcomes complete · 3 · stop 1 · tool 2
```

The Models line shows at most two named buckets, ordered like the JSON array.
Each segment shows its observed four-component sum with `fmt_tokens`:

```text
Models openai/gpt-5 145 · anthropic/claude-opus-4-6 32K · +2 models
```

Rules for this line:

- At width 90 or greater, show `provider/model`.
- Below width 90, show the existing shortened model label without provider.
- `+N models` counts remaining named buckets after the first two.
- Append `unattributed <tokens>` when that bucket is nonzero and space permits.
- Append `other models <tokens>` when the overflow bucket is nonzero and space
  permits.
- Append `partial` after the `Models` label when component status is partial.
- Render `Models unavailable` when `attribution` is `null`.
- Use the existing Pi session and metadata colors. Do not assign colors by
  provider or model.

The reconciliation line uses these forms:

```text
Usage components complete · reported cost $0.0125
Usage components partial · reported cost partial
Usage components unavailable · reported cost unavailable
```

Display cost with the existing four-decimal format. A complete zero is `$0.0000`.
Do not display a currency code because the persisted Pi field does not provide
one.

The optional reason line joins the first component reason and cost reason,
removes a duplicate, and truncates to the content width. It never displays raw
parser errors or source values.

### Height priority

Reserve any Runs line exactly as the current UI does. Fill remaining detail rows
in this order:

1. attachment and source;
2. context and tokens;
3. outcomes;
4. models;
5. reconciliation;
6. identity;
7. context, component, and cost notes;
8. fleet summary or additional Runs rows.

The current renderer combines those base lines only for a small Runs case. Keep
that condition, and also compact a session with harness data when fewer than
eight metadata rows remain after reserving the current Runs state line. This
must combine attachment with source and context with tokens before adding
harness lines. Drop lower-priority lines instead of wrapping or hiding the
selected session rows.

### Interaction

This slice adds no interaction. Existing session selection, scrolling, jump,
kill confirmation, Runs interaction, and click targets remain unchanged.

## Compatibility strategy

### JSON

This is an additive wire change:

- no existing key is removed, renamed, moved, or retyped;
- no existing enum spelling changes;
- `sessions[].telemetry.harness` is the only new key;
- the key is always present when `telemetry` is present, with either a validated
  object or `null`;
- authoritative unavailable values inside the object are `null`;
- existing compatibility numeric fields keep their current values and meaning;
- `active_leaf_id` remains `null`;
- exact key-set guards are updated only for this approved key and its exact
  nested shape.

Do not add a top-level snapshot version in this slice. It would create another
wire change without protecting consumers that already reject additive keys.
Consumers that require exact object shapes must update for the documented
additive key.

### Rust library

Adding a public field to `SessionTelemetry` can break external exhaustive struct
literals even though reading existing fields still works. Therefore:

- release this change in the next minor version, not a patch version;
- call out the added field in release notes;
- keep `App::to_snapshot` as the supported snapshot construction path;
- keep `SessionTelemetry::process_only` working and initialize `harness` to
  `None`; and
- do not change `Collector::collect`, `App::tick`, or `App::to_snapshot`
  signatures.

Do not add a parallel versioned snapshot API for one additive nested field.

### Text and configuration

`ptop --once`, CLI flags, configuration, themes, key bindings, and panel numbers
remain byte-for-byte compatible apart from unrelated elapsed-time values.

## Implementation slices after approval

### Change 2.1: public aggregate model and collector seam

- Add the approved public types and private `PiSessionData.harness` field.
- Convert the validated version-3 private projection once.
- Add reconciliation reasons and deterministic attribution ordering.
- Keep point histories and summary events private.

Suggested commit subject:

```text
feat: expose Pi harness aggregates
```

### Change 2.2: JSON DTO

- Add `SessionTelemetryView.harness`.
- Update exact key-set and nullability tests.
- Prove existing values and nesting stay unchanged.

Suggested commit subject:

```text
feat: add Pi harness snapshot telemetry
```

### Change 2.3: compact selected-session UI

- Add the three approved detail lines and bounded optional reason line.
- Preserve Runs reservation and existing controls.
- Add wide, compact, and narrow rendering tests.

Suggested commit subject:

```text
feat: show Pi harness session details
```

### Change 2.4: documentation and acceptance

- Update `README.md` and `docs/pi-support.md` with shipped behavior.
- Re-run privacy, exact DTO, CLI, layout, and full CI checks.

Suggested commit subject:

```text
docs: document Pi harness telemetry
```

## Required tests

### Model and parser

- Version 3 complete, partial-with-values, unavailable-with-no-values, known
  empty, and inconsistent-private-projection cases.
- Versions 1 and 2 produce `harness: None` without changing base accounting.
- Unsupported versions and failed attachment remain process-only.
- Every outcome maps to its fixed counter. Unknown source text is discarded.
- Outcome totals and status remain correct after parser limits, malformed
  entries, duplicate IDs, truncation, replacement, and deletion.
- Component reconciliation preserves complete zero, partial lower bounds, and
  unavailable nulls.
- Cost covers unavailable, partial, complete zero, complete positive, invalid,
  non-finite, negative, overflow, and incomplete-scan cases.
- Attribution covers response-model precedence, message-model fallback,
  unavailable metadata, 64 named buckets, overflow, deterministic ties, and
  component overflow.
- Component and attribution sum invariants hold.
- Generated reasons are fixed, deduplicated, valid UTF-8, and at most 160 bytes.

### Snapshot and privacy

- Exact key sets for every new DTO.
- `harness` object versus `null` availability matrix.
- Numeric fields serialize as JSON numbers or `null`, never numeric strings.
- Existing DTO keys, enum strings, values, and nesting remain unchanged.
- `active_leaf_id` remains present and null.
- JSON contains no prompt, assistant, tool, summary, retained-tail, ID, parent-ID,
  raw stop-reason, or unsafe metadata sentinel.
- Named attribution order is stable across repeated snapshots.
- `--once` output gains no harness line or private sentinel.

Privacy and exact-shape guards must be proven by temporary mutations before the
change is accepted.

### UI

Use populated complete, partial, unavailable, attribution-overflow, and long
metadata fixtures.

- Wide desktop renders all approved lines.
- Compact desktop renders outcomes and models before identity and notes.
- Narrow Work layout keeps sessions visible and renders the highest-priority
  detail lines that fit.
- Very long safe provider/model values truncate without wrapping into another
  panel.
- Partial and unavailable states never render as known zero.
- Complete zero cost renders `$0.0000`.
- Existing Runs state remains visible at compact desktop heights.
- Session row, narrow tab, narrow section, zoom, and orphan-kill click targets
  remain aligned.
- No new theme field or per-model color appears.

### Full acceptance

Run:

```bash
./scripts/check-rustfmt.sh
cargo clippy --all-targets -- -D warnings -A clippy::uninlined-format-args
cargo test --all-targets
cargo build --release
cargo run -- --once
cargo run -- --json
cargo run -- --demo --once
```

Run the release parser benchmark because the collector projection changes:

```bash
cargo test --release pi_parser_release_benchmark -- --ignored --nocapture
```

## Stop conditions

Stop implementation and return to design if any change requires:

- exposing retained observations or summary events to satisfy the compact UI;
- changing `Collector::collect` or adding a second cross-tick cache;
- publishing IDs, topology, raw source values, or conversation content;
- treating a partial value as complete or an unavailable value as zero;
- merging parent and fleet usage;
- adding another agent, mode, panel, tab, color role, or network request;
- changing existing JSON values rather than adding the approved nested key; or
- adding graphs, compaction markers, topology, history, or sidecar state to this
  delivery slice.

## Approval gate

Approved by the owner on 2026-09-13. The approval covers all of these:

1. the `sessions[].telemetry.harness` location and exact field names;
2. `ReconciliationStatus` and its complete, partial, and unavailable meanings;
3. the rule that a valid version-3 projection gets an object while other cases,
   including projection validation failure, get `null`;
4. component lower-bound and null behavior;
5. the reported-cost status and duplicated compatibility value;
6. attribution bucket meaning, ordering, and 64-name bound;
7. the three compact TUI lines and height priority;
8. additive JSON compatibility and unchanged `--once` output;
9. the next-minor Rust compatibility policy; and
10. the private `PiSessionData` carrier with unchanged collector and App method
    signatures.
