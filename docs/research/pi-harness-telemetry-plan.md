# Pi harness telemetry Phase 0 and Phase 1 implementation plan

## Status

The owner approved the defaults. Phase 0 and its acceptance gate are complete.
Phase 1 Change 1.1 is implemented and verified. Change 1.2 and later changes
have not started.

This plan turns Phase 0 and Phase 1 from
[`pi-harness-telemetry.md`](pi-harness-telemetry.md) into ordered,
compile-green changes. Execution required the approval gate below.

## Goal

Correct existing parent-session accounting, remove an unsupported active-leaf
claim, and retain bounded privacy-safe Pi metadata inside the passive collector.
Phase 1 does not add UI, text-output, or JSON snapshot fields.

## Non-goals

- No topology counts or active-path telemetry.
- No sidecar, RPC attachment, queue, retry, current-tool, or UI-prompt telemetry.
- No cross-session history.
- No prompt, assistant, thinking, error, summary, `details`, tool argument,
  tool result, or `retainedTail` content.
- No new dependency.
- No network request in a monitoring path.
- No Windows session attachment.

## Approved-by-execution defaults

Execution may begin only after the owner accepts these defaults or edits this
section.

| Decision | Default |
|---|---|
| Passive active leaf | Keep the existing `active_leaf_id` JSON key for compatibility, but always serialize it as `null`. Keep the latest persisted entry ID private for inferred context calculation. |
| Internal seam | Keep rich telemetry inside `collector::pi`, behind `PiSemantic::add` and `PiSemantic::session_data`. Do not change public model structs or `Collector::collect`. |
| Internal retention | Enrich the existing bounded `PiSemantic` entries retained by `PiTail`. Build bounded private projections in `session_data`; add no separate cache or caller interface before Phase 2 has an approved consumer. |
| Session versions | Preserve current base accounting for versions 1 through 3. Populate new rich telemetry only for version 3. Unsupported versions remain process-only. |
| Per-entry timestamps | Do not parse or retain them in Phase 0 or Phase 1. File order is sufficient. |
| Entry IDs | Retain only the existing bounded IDs needed for deduplication and inferred context. Publish no new IDs. |
| Metadata bounds | Reuse the existing 256-byte semantic metadata bound. Reject empty, oversized, control-bearing, or bidi-control-bearing attribution values. Do not normalize them into collisions. |
| History bounds | Keep 64 assistant observations, 64 named attribution buckets plus fixed unavailable and overflow buckets, and 64 compaction or branch-summary observations. |
| `totalTokens` | Use only as the existing context baseline, with checked component fallback. Never use it to repair component accounting and do not require equality with the component sum. |
| Invalid numeric values | Reject negative, wrong-typed, non-finite, and overflowing values. Never clamp them or substitute zero. |
| Reported cost | Reconcile independently from token components. Publish the existing `reported_cost` only when the internal cost state is complete. Cost failure does not make otherwise complete component totals partial. |
| Assistant stop reason | Convert immediately to a fixed enum. Retain no unknown raw value. `pending` is a separate unexpected nonterminal state. |
| `responseModel` fallback | Fall back to `model` only when `responseModel` is absent. A present invalid `responseModel` makes attribution unavailable. |
| Duplicate entry IDs | Keep the first entry, count it once, and mark usage and context partial. Do not parse the duplicate payload. |
| Public output | Phase 1 changes no snapshot keys, TUI fields, text output, or README feature claims. Phase 0 may correct existing values and their documentation. |

## Existing seam and invariants

The current deep module is `PiSemantic` in `src/collector/pi.rs`:

```text
PiSemantic::add(Value)
    -> bounded privacy reduction retained by the stateful tailer
PiSemantic::session_data(context_complete)
    -> existing parent usage and inferred-context projection
```

Keep this seam. Do not add an `App` side map, a second parser, a separate
`PiCollector` cache, or a public rich telemetry type. `PiTail.semantic` already
retains bounded reduced entries across ticks. Phase 2 must design a caller
interface only after its DTOs and UI consumer are approved.

The following existing limits remain authoritative:

| Limit | Value | Source |
|---|---:|---|
| Tail work per collection tick | 2 MiB | `MAX_TAIL_WORK_BYTES` |
| JSONL line | 1 MiB | `MAX_TAIL_LINE_BYTES` |
| Semantic entries | 8,192 | `MAX_SEMANTIC_ENTRIES` |
| Existing token history | 64 | `MAX_TOKEN_HISTORY_POINTS` |
| IDs, parent IDs, metadata | 256 bytes | `MAX_SEMANTIC_*_BYTES` |
| Attachment candidates and Pi processes | 128 each | existing collector constants |

Process-first discovery, ownership checks, file-identity checks, stateful tailing,
process-only fallback, null-for-unknown snapshots, and fleet separation do not
change.

## Concrete internal model

All types remain private to `src/collector/pi.rs` unless the file becomes too
large during implementation. Do not create a new module before the behavior is
green.

### Usage observations

Replace the implicit `Option<PiUsage>` convention with explicit observations:

```rust
enum Observation<T> {
    Absent,
    Invalid,
    Value(T),
}

struct ComponentUsage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

struct PiUsage {
    components: Observation<ComponentUsage>,
    total_tokens: Observation<u64>,
    reported_cost: Observation<f64>,
}

enum UsageObservation {
    Absent,
    Invalid,
    Value(PiUsage),
}
```

`UsageObservation::Absent` means the `usage` key is missing.
`UsageObservation::Invalid` means the key exists but is null or not an object.
A `PiUsage` can still contain incomplete components or cost through its nested
observations.

Record-kind policy:

| Record | Missing usage | Present invalid or incomplete usage |
|---|---|---|
| Assistant | Component totals partial; turn still counted | Component totals partial; turn still counted |
| Tool result | No contribution; not partial | Component totals partial |
| Compaction | No contribution; not partial | Component totals partial |
| Branch summary | No contribution; not partial | Component totals partial |

### Cost state

Use a separate accumulator:

```rust
enum ReportedCostState {
    Unavailable,
    Partial,
    Complete(f64),
}
```

Rules:

1. Every assistant record creates one cost expectation, including an assistant
   whose `usage` key is absent or invalid.
2. A tool-result, compaction, or branch-summary record creates one cost
   expectation only when its `usage` key is present. A null or non-object value
   cannot supply cost and makes that expectation partial.
3. For every present usage object, evaluate `cost.total` independently from the
   component fields. Valid reported cost remains eligible when components are
   missing or invalid. Missing or invalid `cost.total` makes cost partial.
4. A missing optional `usage` key creates no expectation.
5. No expectations produces `Unavailable`, not complete zero.
6. All expected costs must be present, finite, non-negative, and add to a finite
   value. A complete set of zero costs is `Complete(0.0)`.
7. Any missing, invalid, or overflowing expected cost produces `Partial`.
8. An incomplete tail, parser limit, semantic loss, duplicate-ID ambiguity, or
   unsupported rich schema prevents `Complete` and publishes no cost.
9. Existing `UsageTelemetryDetails.reported_cost` receives `Some(value)` only
   from `Complete(value)`; both other states map to `None` until Phase 2 adds an
   explicit public cost status.
10. Cost state does not change component completeness.

### Assistant metadata

```rust
enum AssistantStopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
    Deferred,
    Pending,
    Unknown,
}

struct AssistantMetadata {
    provider: Observation<String>,
    message_model: Observation<String>,
    response_model: Observation<String>,
    stop_reason: AssistantStopReason,
}
```

Missing, null, wrong-typed, empty, oversized, or unsafe strings become absent or
invalid observations without discarding otherwise valid numeric usage. Unknown
stop-reason text maps to `Unknown` and is discarded.

A valid context baseline requires complete positive baseline usage and a stop
reason other than `Error`, `Aborted`, `Pending`, or `Unknown`.

### Attribution

Do not create a slash-joined string as the internal key:

```rust
struct PiAttributionKey {
    provider: String,
    model: String,
}

enum PiAttribution {
    Named(PiAttributionKey),
    Unavailable,
}
```

For valid fields, the attribution model is `responseModel ?? model`. Absent
`responseModel` permits fallback. Invalid `responseModel` does not.

Keep at most 64 named keys. Additional valid keys contribute to a fixed overflow
bucket. Missing or invalid metadata contributes to a fixed unavailable bucket.
Neither case loses numeric usage or changes parent completeness.

### Private semantic projection

`PiEntry` is the retained private record. Add the safe assistant and compaction
observations to that type. `PiTail.semantic` remains the only cross-tick store.

`PiSemantic::session_data` builds a bounded projection on demand:

```rust
struct ParentHarnessTelemetry {
    component_total: ComponentUsage,
    component_completeness: TelemetryCompleteness,
    reported_cost: ReportedCostState,
    assistant_total: ComponentUsage,
    unattributed_tool_or_summary_total: ComponentUsage,
    assistant_outcomes: AssistantOutcomeCounts,
    attribution: BoundedAttributionBreakdown,
    assistant_points: VecDeque<AssistantObservation>,
    summary_events: VecDeque<SummaryObservation>,
}
```

The projection is private and is not cached or carried into `App`. Its component
total, cost, turn count, and complete-point values become the source for the
existing `PiSessionData` fields. Its remaining fields are checked by
`ParentHarnessTelemetry::validate()` and exercised through parser tests.
Validation failure marks the affected existing telemetry partial or unavailable.
It never removes the process row.

Build the rich projection only when the attached header version is 3. Versions 1
and 2 continue through the existing base projection. Phase 2 must define any
cross-module interface together with the approved DTO and UI consumer. Do not
add temporary dead-code allowances in Phase 1.

### Compaction observations

```rust
enum SummaryKind {
    Compaction,
    BranchSummary,
}

struct SummaryObservation {
    kind: SummaryKind,
    tokens_before: Observation<u64>,
    usage: UsageObservation,
}
```

Retain only the latest 64 observations in file order and a saturating all-entry
count if needed for reconciliation. `tokens_before` applies only to compaction.
Do not read `summary`, `details`, `retainedTail`, `firstKeptEntryId`, or `fromId`
for telemetry. Existing parent linkage parsing remains separate.

Do not overwrite `AgentSession.compaction_count`; it currently describes the
inferred context branch, not all persisted summary events.

## Phase 0 changes

### Change 0.1: type and reconcile parent usage

**Commit subject:** `fix: reconcile persisted Pi usage`

**Files and symbols**

- `src/collector/pi.rs`
  - `PiEntry`
  - `PiUsage`
  - `parse_usage`
  - `parse_pi_entry`
  - `PiSemantic::add`
  - `PiSemantic::session_data`
  - `valid_baseline`
  - `context_baseline`
  - `PiSessionData.cost`
  - `telemetry_for_attachment`
- `docs/pi-support.md`
- `README.md`

**Behavior**

- Introduce the component, usage, and cost observations together so every
  intermediate build has one coherent private model.
- Preserve absent versus present-invalid usage.
- Count every accepted assistant entry as a turn.
- Apply the record-kind component and cost rules above.
- Add no fabricated components or cost.
- Keep component and cost completeness independent.
- Map only `ReportedCostState::Complete` to the existing `reported_cost` field.
- Prevent complete cost after incomplete tailing, parser limits, semantic loss,
  or duplicate-ID ambiguity.
- Keep the latest 64 complete compatibility `token_history` values exactly as
  documented today.
- Keep the first duplicate ID, mark usage and context partial, and do not inspect
  or add the duplicate payload.
- Use checked component, combined, and cost additions.
- Document the implemented component, cost, `totalTokens`, all-branch, and fleet
  separation rules in `docs/pi-support.md` and README in this commit.

**Tests in `src/collector/pi.rs`**

Add table-driven tests for:

- assistant usage omitted, null, non-object, incomplete, negative, wrong-typed,
  component-overflowing, and complete;
- optional usage absent, null, non-object, incomplete, and complete for tool
  results, compactions, and branch summaries;
- all expected costs valid, including complete zero;
- one missing cost among otherwise valid records and all costs absent;
- negative, wrong-typed, non-finite or unrepresentable, and accumulated-overflow
  cost;
- valid cost with invalid components and invalid cost with valid components;
- incomplete tail, parser limit, semantic loss, and duplicate-ID ambiguity with
  otherwise valid cost;
- a duplicate ID with different usage;
- header-only known-zero components and unavailable cost;
- `totalTokens` disagreement with valid components;
- `totalTokens` missing, invalid, and overflowing.

Update the existing tests near:

- `semantic_usage_deduplicates_branches_and_context_uses_active_leaf`
- `semantic_token_history_keeps_the_latest_64_assistant_turns`
- `semantic_context_rejects_missing_parent_and_requires_post_compaction_baseline`
- `malformed_usage_never_becomes_a_baseline_or_complete_total`
- `malformed_usage_keeps_the_assistant_turn_without_a_history_sample`
- `overflow_or_invalid_cost_makes_usage_partial`

**Narrow gate**

```bash
cargo test collector::pi::tests
cargo test reported_cost
cargo test snapshot::tests::pi_snapshot_uses_structured_unknowns_instead_of_numeric_placeholders
```

### Change 0.2: retire the passive active-leaf claim

**Commit subject:** `fix: stop publishing inferred Pi leaf identity`

**Files and symbols**

- `src/collector/pi.rs`
  - `PiSessionData.active_leaf_id`
  - `PiSemantic::session_data`
  - `PiCollector::collect_sessions`
  - `telemetry_for_attachment`
- `src/demo.rs`
- `src/snapshot.rs` tests
- `README.md`
- `docs/pi-support.md`

**Behavior**

- Rename the private context cursor to `latest_persisted_entry_id` or keep it as
  a local variable.
- Continue inferring context from the latest persisted entry, with precision and
  reason stating that limitation.
- Set `ContextTelemetryDetails.active_leaf_id` to `None` for attached sessions.
- Keep the JSON key present and serialized as `null`.
- Decouple `current_tasks` from leaf-ID presence. Use successful attachment and
  telemetry availability instead.
- Set demo `active_leaf_id` to `None`.

**Tests**

- navigation to an earlier Pi leaf without append cannot change passive state;
- attached telemetry still gets the attached task label with a null leaf ID;
- context reason says latest persisted entry, not active leaf;
- snapshot key set is unchanged and `active_leaf_id` is null;
- demo remains collector-free and contains no fabricated leaf ID.

**Narrow gate**

```bash
cargo test leaf
cargo test snapshot::tests
cargo test demo::tests
```

### Change 0.3: guard compaction payload privacy

**Commit subject:** `test: guard Pi compaction payload privacy`

Add a fixture containing sentinel text in `summary`, `details`, and
`retainedTail`, first without and then with a valid post-compaction assistant
baseline.

Assert:

- no sentinel appears in retained safe fields, debug output, text output, or
  serialized snapshot;
- the payload does not change component accounting;
- context is unknown before a valid post-compaction assistant baseline;
- context becomes available after that baseline;
- `retainedTail` does not become a context or usage source.

**Narrow gate**

```bash
cargo test compaction
cargo test demo::tests::pi_demo_snapshot_uses_pi_privacy_suppression
```

## Phase 0 acceptance gate

Phase 0 is complete only when:

- missing required assistant usage is partial;
- missing optional usage is no contribution and not partial;
- present malformed optional usage is partial;
- component and cost completeness are independent;
- incomplete cost is not published as a total;
- duplicate IDs cannot silently produce an apparently complete total;
- parent and fleet totals remain separate;
- the active-leaf JSON key remains present but null;
- compaction payloads remain unretained;
- README and `docs/pi-support.md` match behavior; and
- all Phase 0 narrow and full gates pass.

Do not start Phase 1 until this gate passes.

## Phase 1 changes

### Change 1.1: retain and enforce the session version

**Commit subject:** `refactor: gate rich Pi telemetry by session version`

**Files and symbols**

- `src/collector/pi.rs`
  - `PiHeader`
  - `PiAttachment`
  - `PiTail`
  - `parse_header`
  - `resolve_attachments_with_budget`
  - `tail_session_with_expected_header`
  - `telemetry_for_attachment`
- `docs/pi-support.md`

**Behavior**

- Add `version: u64` to `PiHeader` and carry it through attachment and tail
  identity.
- Include version in header revalidation and tail-reset decisions.
- Keep versions 1 through 3 accepted for existing base telemetry.
- Enable rich Phase 1 reduction only for version 3.
- Keep unsupported versions process-only.
- Do not infer a newer schema from entry shape.
- Document the rich version gate and unchanged version 1 through 3 base support
  in `docs/pi-support.md` in this commit.

**Tests**

Extend
`header_defaults_missing_version_to_v1_and_rejects_unsupported_versions` and
header-replacement tests for:

- missing version maps to version 1;
- versions 1, 2, and 3 preserve base telemetry;
- the retained version and private rich-parsing predicate identify only version 3;
- invalid or unsupported versions fail closed;
- in-place version change resets or rejects the tail instead of reusing state.

### Change 1.2: collect assistant outcomes and observations

**Commit subject:** `feat: collect bounded Pi assistant outcomes`

**Files and symbols**

- `src/collector/pi.rs`
  - `PiEntry`
  - `parse_pi_entry`
  - `AssistantStopReason`
  - `AssistantOutcomeCounts`
  - assistant observation ring
- `docs/pi-support.md`

**Behavior**

- Parse directly into the fixed enum.
- Count each accepted assistant entry once across all persisted branches.
- Keep the latest 64 assistant observations in file order, including explicit
  component or cost gaps.
- Preserve the existing public token-history contract separately.
- Do not retain raw unknown stop reasons or adjacent error text.
- Document the fixed outcome vocabulary, unknown reduction, privacy rule, and
  64-observation bound in `docs/pi-support.md` in this commit.

**Tests**

Cover `stop`, `length`, `toolUse`, `error`, `aborted`, `deferred`, persisted
`pending`, future strings, missing, null, wrong type, and private adjacent
`errorMessage`. The fixed counters must sum to the accepted assistant count.

### Change 1.3: collect Pi attribution and usage breakdown

**Commit subject:** `feat: collect bounded Pi model attribution`

**Files and symbols**

- `src/collector/pi.rs`
  - `AssistantMetadata`
  - `PiAttributionKey`
  - `PiAttribution`
  - `BoundedAttributionBreakdown`
  - `ParentHarnessTelemetry`
- `docs/pi-support.md`

**Behavior**

- Parse bounded provider, message model, and response-model observations without
  letting malformed metadata discard valid usage.
- Use the response-model value when valid, otherwise fall back only when absent.
- Keep assistant and unattributed tool-or-summary usage separate.
- Compute the parent total once. Breakdowns are views of that total.
- Preserve all numeric contributions in named, unavailable, or overflow buckets.
- Keep fleet usage entirely outside the calculation.
- Count every accepted all-branch entry once.
- Document attribution semantics, named/unavailable/overflow buckets,
  all-branch accounting, and bounds in `docs/pi-support.md` in this commit.

**Tests**

- valid fields with and without `responseModel`;
- present invalid response-model value does not fall back;
- missing, empty, wrong-typed, oversized, control-bearing, and bidi-bearing
  values;
- more than 64 distinct valid attribution keys;
- attribution changes across abandoned branches;
- unavailable and overflow buckets retain numeric totals;
- assistant plus unattributed breakdown equals the parent total when complete;
- no fleet usage enters the result.

### Change 1.4: collect bounded compaction metrics

**Commit subject:** `feat: collect bounded Pi compaction metrics`

**Files and symbols**

- `src/collector/pi.rs`
  - `SummaryKind`
  - `SummaryObservation`
  - `PiEntry`
  - `parse_pi_entry`
  - `ParentHarnessTelemetry`
- `docs/pi-support.md`

**Behavior**

- Retain event kind, valid `tokensBefore` for compaction, and optional numeric
  usage observation.
- Keep the latest 64 events in file order.
- Keep all-entry totals separate from the existing inferred-branch
  `AgentSession.compaction_count`.
- Never inspect or retain content-bearing fields.
- Document retained numeric fields, content suppression, all-entry versus
  inferred-branch meaning, and the 64-event bound in `docs/pi-support.md` in
  this commit.

**Tests**

- compaction and branch-summary entries;
- valid, missing, wrong-typed, negative, and overflowing `tokensBefore`;
- absent, valid, and malformed optional usage;
- more than 64 events;
- unknown fields;
- sentinel suppression for `summary`, `details`, `retainedTail`,
  `firstKeptEntryId`, and `fromId`.

### Change 1.5: prove no public Phase 1 output

**Commit subject:** `test: keep rich Pi telemetry private`

Add exact serialized key-set tests for:

- `Snapshot`;
- `SessionView`;
- `SessionTelemetryView`;
- `ContextTelemetryView`; and
- `UsageTelemetryView`.

Assert that private type and field names, attribution buckets, outcomes,
compaction metrics, and fixture sentinels do not occur in JSON or text output.
Confirm no Phase 1 commit changes `src/ui/` or adds output in
`lib.rs::print_snapshot`.

## Phase 1 acceptance gate

Phase 1 is complete only when:

- rich reduction is version-3-only;
- base versions 1 through 3 retain existing supported behavior;
- private records are exactly bound to an owned live parent identity;
- every ring, bucket, field, reason, and file operation is bounded;
- malformed metadata cannot discard valid numeric usage;
- assistant outcomes reconcile to accepted assistant entries;
- parent, assistant, unattributed, and fleet accounting cannot double count;
- abandoned persisted branches contribute once;
- no active-path or topology claim appears;
- no forbidden payload is retained or published;
- no public Rust model or JSON snapshot field is added; and
- all narrow and full gates pass.

## Verification sequence

Run the narrow test after each change. Before completing each phase, run:

```bash
./scripts/check-rustfmt.sh
cargo clippy --all-targets -- -D warnings -A clippy::uninlined-format-args
cargo test --all-targets
cargo build --release
```

Because this changes the parent parser, also run:

```bash
cargo test --release pi_parser_release_benchmark -- --ignored --nocapture
```

Final smoke checks:

```bash
cargo run -- --demo --once
cargo run -- --once
cargo run -- --json
```

For the JSON smoke check, compare the key sets with a baseline captured before
Phase 1. Values corrected by Phase 0 may differ, but Phase 1 must add no key.

## Stop conditions

Stop execution and ask the owner if any of these occurs:

- a safe default above is rejected;
- a public Rust model change appears necessary;
- a new snapshot key appears necessary before Phase 2;
- rich telemetry cannot be bound to exact current parent identity;
- an installed Pi schema contradicts the pinned `v0.85.1` evidence;
- a fixture requires retaining payload content;
- a parser bound must increase;
- component and cost completeness cannot remain independent;
- a phase cannot stay compile-green without temporary dead code allowances; or
- Linux, macOS, or Windows process-only compilation regresses.

## Approval gate

The owner confirmed the defaults in **Approved-by-execution defaults** before
implementation began. Phase 0 and Phase 1 Change 1.1 are complete. The next
implementation step is Phase 1 Change 1.2.
