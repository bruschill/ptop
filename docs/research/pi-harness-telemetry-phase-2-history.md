# Pi harness telemetry Phase 2 history design

## Status

Approved by the owner on 2026-09-13. Implementation may proceed against this
exact Phase 2 history delivery slice.

The first Phase 2 aggregate slice is on `main` and remains authoritative. This
proposal adds bounded persisted history and summary-event markers without
changing its aggregate meanings.

## Approved decisions

The owner approved these defaults before implementation started.

| Decision | Approved behavior |
|---|---|
| Delivery | Add a bounded assistant component-history DTO, per-point Pi attribution, compaction and branch-summary markers, and a selected-session history graph. |
| JSON location | Add `history` under the existing `sessions[].telemetry.harness` object. |
| Availability | A valid version-3 harness always contains the `history` key. It is an object when its history projection validates, including a known-empty history, and `null` when history validation fails. Aggregate harness fields remain available when only history fails. |
| Point bound | Keep the latest 64 accepted persisted assistant entries in file order. Preserve one array slot per entry, including component gaps. |
| Marker bound | Publish at most 64 summary-event markers that fall inside the visible assistant window. Report observed and omitted counts explicitly. |
| Point contents | Publish complete numeric component usage and safe Pi provider/model attribution only. Publish no outcome, cost, timestamp, ID, parent ID, text, or tool data per point. |
| Marker contents | Publish kind and position only. Keep declared `tokensBefore` private until a concrete consumer is approved. Publish no summary, details, retained tail, file list, trigger label, ID, or timestamp. |
| Existing history | Keep `SessionView.token_history` unchanged for compatibility. The new history is authoritative for gaps, components, attribution transitions, and markers. |
| Internal seam | Build and validate history inside `collector::pi` from the existing bounded `PiSemantic` state. Carry it through private `PiSessionData.harness`. Add no cache, parser, or `App` side map. |
| UI location | Render history only in surplus selected-session detail space. Use the existing narrow Sessions zoom for progressive disclosure. Add no panel, tab, key binding, mode, click target, or theme field. |
| Graph | Use four component rows at width 90 or greater and one total row below width 90. Preserve gaps, model uncertainty, model changes, and summary markers as separate glyph rows. |
| Compatibility | Add one nested JSON key and one public Rust field. Keep existing keys, values, methods, text output, configuration, controls, and colors unchanged. Bundle this source change into the planned next minor release. |

## Goals

1. Show how persisted assistant component usage changes across the latest 64
   accepted assistant entries.
2. Preserve missing or invalid usage as a visible gap instead of compressing the
   sequence or drawing a false zero.
3. Mark known Pi attribution changes without presenting providers or models as
   separate monitored agents.
4. Position compaction and branch-summary events without publishing linkage
   identifiers or content.
5. Keep aggregate outcomes, component reconciliation, reported cost, and model
   attribution unchanged.
6. Keep one small caller interface while the collector owns ordering,
   validation, omission accounting, and privacy reduction.

## Non-goals

This slice does not add:

- live activity or active-path history;
- active-leaf, active-depth, or tree-topology claims;
- timestamps, durations, rates, or wall-clock spacing between points;
- per-point stop reasons or reported cost;
- summary text, `details`, `retainedTail`, file lists, entry IDs, or parent IDs;
- compaction trigger labels such as manual, threshold, or overflow;
- tool-result points as graph points;
- a full attribution table or model-specific color;
- historical storage or scanning of inactive sessions;
- sidecar data;
- a new panel, tab, key binding, mode, click target, configuration key, or theme
  field;
- changes to fleet-run accounting; or
- network requests.

## Source meaning

The history covers successfully reduced persisted version-3 JSONL entries in
file order across all parsed branches. It is not an active-path timeline.

File order gives sequence but not elapsed time. Adjacent graph columns mean
adjacent retained persisted assistant entries, not equal time intervals.

A summary marker means only that ptop observed a persisted `compaction` or
`branch_summary` entry at that position. It does not identify why the event
occurred or whether its branch is active.

## Exact public types

Add these serializable types in `src/model/session.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PiPointAttribution {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PiAssistantUsagePoint {
    pub components: Option<TokenComponents>,
    pub attribution: Option<PiPointAttribution>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PiSummaryMarkerKind {
    Compaction,
    BranchSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PiSummaryMarker {
    pub kind: PiSummaryMarkerKind,
    pub position: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PiHarnessHistoryTelemetry {
    pub status: ReconciliationStatus,
    pub assistant_count: u32,
    pub omitted_assistant_points: u32,
    pub points: Vec<PiAssistantUsagePoint>,
    pub summary_event_count: u32,
    pub omitted_summary_events: u32,
    pub markers: Vec<PiSummaryMarker>,
    pub reason: Option<String>,
}
```

Add one field to the existing public aggregate:

```rust
pub struct PiHarnessTelemetry {
    // Existing fields remain unchanged and in their current order.
    pub history: Option<PiHarnessHistoryTelemetry>,
}
```

Do not add a second history field to `SessionTelemetryView`. The existing
`harness` value already crosses the model-to-snapshot seam and serializes this
nested field.

## Field semantics

### `history`

- `Some` means the bounded history projection passed its internal invariants.
- `None` means the aggregate harness is valid but the history projection is not
  trustworthy.
- A complete version-3 session with no assistant or summary entries uses
  `Some` with empty arrays, zero counts, and `status: complete`.
- A history validation failure does not remove aggregate outcomes, component
  reconciliation, reported cost, or aggregate attribution.

### `status`

Use the existing `ReconciliationStatus`:

- `complete`: the attached source scan is complete. Retention omission does not
  make this partial because omission counts are explicit.
- `partial`: the source scan or semantic reduction is incomplete and at least
  one accepted assistant point or summary marker is available.
- `unavailable`: the source scan or semantic reduction is incomplete and no
  accepted assistant point or summary marker is available.

`reason` is `null` when complete. Partial or unavailable history uses fixed ptop
phrases, deduplicated and capped at 160 UTF-8 bytes under the existing reason
rules. It never contains source values or parser error text.

### Assistant counts and points

- `assistant_count` is the saturating count of accepted persisted assistant
  entries observed across parsed branches.
- `points` contains `min(assistant_count, 64)` latest accepted assistant entries
  in file order.
- `omitted_assistant_points` is
  `assistant_count - points.len()` using saturating arithmetic.
- An incomplete scan makes `assistant_count` and its omission count observed
  lower bounds. `status` carries that limitation.
- Array position is the only public point identity. Do not publish an ordinal,
  entry ID, parent ID, or timestamp.

### Point components

- `components` is non-null only when all four usage components are valid,
  non-negative, and fit in `u64`.
- Complete zero usage is a non-null `TokenComponents` object with four zeros.
- Missing, null, malformed, incomplete, negative, or overflowing usage produces
  `components: null` for that assistant point.
- A null point remains in the array. It is a graph gap, not zero.
- Tool-result, compaction, and branch-summary usage remains in aggregate parent
  accounting but does not create an assistant graph point.

### Point attribution

- `attribution` uses the existing Pi rule `(provider, responseModel ?? model)`.
- Absent `responseModel` permits `model` fallback. Present invalid
  `responseModel` does not.
- Missing, null, wrong-typed, empty, oversized, control-bearing, or
  bidi-control-bearing provider/model values produce `attribution: null`.
- Valid values retain the current 256-byte bound.
- Attribution may be non-null when point components are null. This preserves a
  known model transition while leaving usage as a gap.
- Attribution identifies Pi metadata. It does not create another monitored
  agent or claim requested-versus-actual semantics.

### Summary counts and markers

- `summary_event_count` is the saturating count of accepted compaction and
  branch-summary entries observed across parsed branches.
- `markers` contains at most the latest 64 accepted summary events that fall
  within the visible assistant-point window.
- `omitted_summary_events` is
  `summary_event_count - markers.len()` using saturating arithmetic. It includes
  older retained events, events outside the visible assistant window, and events
  beyond the marker bound.
- Markers remain in file order. Multiple markers may have the same position.

### Marker position

`position` is the number of visible assistant points that occur before the
summary event. It is always in `0..=points.len()` and therefore fits in `u8`.

Examples:

- `0`: before the first visible assistant point;
- `1`: after point 0 and before point 1; and
- `points.len()`: after the last visible assistant point.

Let:

```text
visible_start = assistant_count - points.len()
position = assistants_before_event - visible_start
```

Include a marker only when `assistants_before_event` is between
`visible_start` and `assistant_count`, inclusive. Omit older markers and count
them in `omitted_summary_events`.

Map a public position to a rendered graph column with this exact rule. Interior
boundaries belong to the following point bucket. The final boundary belongs to
the last point bucket.

```text
if point_count == 0:
    column_count = 1
    column = 0
else:
    point_index = min(position, point_count - 1)
    column = (((point_index + 1) * column_count) - 1) / point_count
```

Use widened integer arithmetic for the multiplication. `column_count` is at
least one and at most `point_count` when points exist. A zero-point history uses
one synthetic graph column: component and model rows render a gap, and every
position-0 summary marker maps to that column.

This position is relative to persisted file order across branches. It is not an
active-path location.

### Marker kind

- `kind` serializes as `compaction` or `branch_summary`.
- Keep declared compaction `tokensBefore` in the existing private
  `SummaryObservation`. This slice has no consumer that needs it.
- Publishing `tokensBefore` later requires a separate compatibility and consumer
  review.

## Exact JSON shape

The existing harness object gains one key:

```json
{
  "harness": {
    "assistant_outcomes": {},
    "components": {},
    "reported_cost": {},
    "attribution": {},
    "history": {
      "status": "complete",
      "assistant_count": 3,
      "omitted_assistant_points": 0,
      "points": [
        {
          "components": {
            "input_tokens": 100,
            "output_tokens": 20,
            "cache_read_tokens": 0,
            "cache_write_tokens": 5
          },
          "attribution": {
            "provider": "openai",
            "model": "gpt-5"
          }
        },
        {
          "components": null,
          "attribution": {
            "provider": "openai",
            "model": "gpt-5"
          }
        },
        {
          "components": {
            "input_tokens": 80,
            "output_tokens": 10,
            "cache_read_tokens": 40,
            "cache_write_tokens": 0
          },
          "attribution": {
            "provider": "anthropic",
            "model": "claude-opus-4-6"
          }
        }
      ],
      "summary_event_count": 2,
      "omitted_summary_events": 0,
      "markers": [
        {
          "kind": "compaction",
          "position": 2
        },
        {
          "kind": "branch_summary",
          "position": 3
        }
      ],
      "reason": null
    }
  }
}
```

The abbreviated existing harness objects above keep all current fields. The
example omits their contents only to focus on `history`.

When aggregate harness data is valid but history validation fails:

```json
{
  "harness": {
    "history": null
  }
}
```

All existing aggregate harness keys remain present in real output.

## Internal module and seam

Keep `PiSemantic` as the deep parser module. Callers continue to receive one
`PiSessionData` result.

```text
PiSemantic::add(Value)
    -> existing bounded private PiEntry state

PiSemantic::session_data_for_version(context_complete, rich_supported)
    -> ParentHarnessTelemetry
    -> private bounded history projection
    -> PiSessionData.harness.history

PiCollector::telemetry_for_attachment(...)
    -> SessionTelemetry.harness.history

App::to_snapshot
    -> existing SessionTelemetryView.harness serialization

src/ui/sessions.rs
    -> session_history::history_lines(...)
```

Implementation rules:

1. Build history only in the existing version-3 rich reduction path.
2. Traverse `PiSemantic.order` in file order without reading source JSON again.
3. Keep at most 64 assistant points and 64 candidate summary observations while
   traversing. Use the existing saturating all-entry counts.
4. Derive point attribution from the already reduced `AssistantMetadata` on
   `PiEntry`. Do not retain another metadata copy across ticks.
5. Compute marker positions from assistant counts, not IDs or parent linkage.
6. Validate history before converting it to the public DTO.
7. On history validation failure, set only `harness.history` to `None`.
8. Do not call `PiSemantic::session_data_for_version` or
   `parent_harness_telemetry` a second time from a caller.
9. Do not add an `App` map, collector cache, second parser, or history store.
10. Keep the release parser benchmark within its existing budget.

### Private validation

The private history projection must prove:

- assistant points and markers are each bounded to 64;
- point count equals `min(assistant_count, 64)`;
- omitted assistant count reconciles to observed assistant count;
- every marker position is at most `points.len()`;
- markers remain in source order after filtering;
- omitted summary count reconciles to observed summary count;
- no public marker contains private `tokensBefore` data;
- safe attribution values still satisfy existing metadata bounds; and
- no public object contains IDs, timestamps, stop reasons, costs, summary data,
  tool data, or source text.

The existing aggregate `ParentHarnessTelemetry::validates()` remains unchanged.
History validation is separate so a history-only defect cannot suppress trusted
aggregates.

## UI presentation

### Placement and disclosure

Keep the existing aggregate harness lines first. Add history after outcomes,
Models, and reconciliation, but before identity and lower-priority notes.

Render no history when the selected detail lacks surplus space:

- At width 90 or greater, require at least 12 metadata rows after the selected
  session title and any reserved Runs row.
- Below width 90, require at least 9 metadata rows after the selected session
  title and any reserved Runs row.
- In compact layouts, the existing Sessions `+` zoom supplies the normal path to
  enough space. Do not add a history toggle.
- Keep at least the current minimum session-table rows and the current Runs state
  reservation. If both cannot fit, omit history.

When history is rendered, combine attachment with source and context with
Tokens, as the current compact detail does. These two base rows, the three
aggregate harness rows, and the seven full-history rows fit the 12-row full
threshold. The compact graph uses four history rows and fits its 9-row threshold.
Optional reasons, identity, context notes, and fleet detail follow only if space
remains.

When a selected session has validated history but the graph is omitted for
space, the existing aggregate lines remain unchanged. Do not add a clipped
history heading.

### Full graph

At width 90 or greater, render:

```text
History complete · assistants 64 shown, 56 omitted
 I  ▁▂·▄▇█...  max 12K
 O  ▁▁·▃▄▅...  max 2K
 R  ▁▄·█▂▁...  max 48K
 W  ▁▁·▂▁▁...  max 4K
 M  ···│··?...  │ change  ? uncertain  ! both
 E  ··C··B*...  summaries 12 shown, 3 omitted
```

Use labels:

- `I`: input tokens;
- `O`: output tokens;
- `R`: cache-read tokens;
- `W`: cache-write tokens;
- `M`: attribution state; and
- `E`: summary events.

Each component row scales independently to its maximum visible numeric value.
The trailing `max` label makes that scale explicit. A complete numeric zero uses
`▁`. A bucket containing only null points uses `·`. A bucket containing both
null and numeric points uses `◌`, which means mixed availability and does not
encode magnitude.

### Compact graph

Below width 90, render one total row plus marker rows:

```text
History partial · assistants 64 shown, 4 omitted
 T  ▁▂◌▄▇!※...
 M  ···│··?!...
 E  ··C··B*...  summaries 5 shown, 2 omitted
```

The heading uses `points.len()` for `assistants <n> shown` and
`omitted_assistant_points` for `<n> omitted`. Downsampling does not change either
number because every retained point remains represented by a bucket.

For each non-null component object, compute `T` with checked addition across its
four fields. Use these compact total glyphs before numeric scaling:

- `·`: every point in the bucket has null components;
- `◌`: the bucket mixes null components and valid checked totals, with no
  overflow;
- `!`: at least one checked total overflows and the bucket has no null component
  point; and
- `※`: the bucket contains both a null component point and a checked-total
  overflow.

Never saturate an overflowing total or turn it into zero. Omit trailing maximum
labels when they do not fit.

### Numeric scaling

Use the ordered eight-glyph scale `▁▂▃▄▅▆▇█`. Let `value` be a bucket maximum and
`row_max` be the maximum valid numeric value in the rendered row.

```text
if row_max == 0:
    glyph_index = 0
else:
    glyph_index = ceil(value * 7 / row_max)
```

Calculate the ceiling with widened integer arithmetic:

```text
glyph_index = ((value * 7) + row_max - 1) / row_max
```

The index is in `0..=7`. An all-zero numeric row therefore renders `▁` for every
numeric bucket. Special gap, mixed, and overflow glyphs bypass numeric scaling.

### Model markers

Compute one model state for every original visible point:

- The first point is `·` when attribution is known and `?` when unavailable.
- For later points, `│` means the previous and current attributions are both
  known and their provider/model pairs differ.
- `·` means both are known and equal.
- `?` means either the previous or current attribution is unavailable.

Therefore `known -> unavailable -> known` renders `??`; it does not claim a
change across the unavailable point.

When downsampling, a model bucket renders:

- `!` when it contains both a definite `│` change and a `?` uncertain state;
- `│` when it contains a definite change and no uncertainty;
- `?` when it contains uncertainty and no definite change; and
- `·` otherwise.

Do not assign colors by provider or model. The existing aggregate Models line
continues to show names and usage totals.

### Summary marker row

Map each marker position with the exact boundary formula in **Marker position**:

- `C` for one or more compactions;
- `B` for one or more branch summaries;
- `*` when both kinds share a column; and
- `·` when no marker maps to the column.

The row legend reports `markers.len()` as summaries shown and
`omitted_summary_events` as summaries omitted. Do not show trigger labels,
summary text, or private `tokensBefore` data.

### Shared graph width

Every component, total, model, and event row uses one shared `column_count` so
all glyphs align. The fixed row prefix is four display columns: one leading
space, the one-character row label, and two spaces.

```text
prefix_width = 4
available = width - prefix_width
legend_target = 44 when width >= 90, otherwise 32
legend_reserve = min(legend_target, available - 1)
graph_capacity = available - legend_reserve
desired_columns = max(points.len(), 1)
column_count = min(desired_columns, graph_capacity, 64)
```

Use saturating subtraction in the implementation. If `width < 5`, return no
graph rows. At supported render widths, `column_count` is at least one. A
zero-point history uses its one synthetic column.

After rendering the fixed prefix and `column_count` glyphs, a row may add two
spaces and a legend only from the remaining display columns. Truncate the legend
to that remainder. Legends never reduce or shift graph columns, and all rows use
the same count.

### Downsampling

When points exceed the shared graph-column count, partition the point array into
contiguous buckets using integer boundaries:

```text
start = column * point_count / column_count
end   = (column + 1) * point_count / column_count
```

For each full component bucket:

- render `·` when every point is null;
- render `◌` when null and numeric points are both present; and
- otherwise scale the maximum numeric value with the exact eight-level formula.

For compact totals, apply the gap and checked-overflow rules above. For summary
markers, use the separate boundary-to-column formula from **Marker position**.
Preserve both marker kinds with `*` when they collapse into one column.

For model rows, compute original per-point states before bucketing, then apply
the four-state bucket priority above.

Do not interpolate across gaps.

### Color and width

- Use `theme.pi_agent` for graph values and marker glyphs.
- Use existing metadata and inactive colors for labels, gaps, legends, and
  status text.
- Add no theme field or provider/model color.
- Every produced line must fit the supplied display width. Truncate legends
  before graph columns, never after rendering into another panel.

### UI module interface

Move graph formatting and downsampling behind one private module interface:

```rust
pub(crate) fn history_lines(
    history: &PiHarnessHistoryTelemetry,
    width: u16,
    max_lines: usize,
    theme: &Theme,
) -> Vec<Line<'static>>;
```

Place it in `src/ui/session_history.rs`. `src/ui/sessions.rs` decides whether
surplus space exists and how many lines are available. The history module owns
sampling, scaling, glyph selection, marker alignment, model-transition logic,
width bounds, and focused rendering tests.

This seam keeps the selected-session renderer from learning graph internals.

## Compatibility strategy

### JSON

- Add only `sessions[].telemetry.harness.history`.
- Always serialize the key when `harness` is non-null, with an object or `null`.
- Keep every existing key, value, enum spelling, and nesting unchanged.
- Keep `SessionView.token_history` unchanged.
- Use JSON `null` for unavailable point components, attribution, and failed
  history projection.
- Update exact key-set guards for the one approved key and every new nested DTO.
- Add no top-level snapshot version.

### Rust library

Adding `history` to public `PiHarnessTelemetry` can break external exhaustive
struct literals. PR #33 has not yet been released as the planned next minor
version, so bundle this field into that same minor release rather than creating
an intermediate release shape.

Keep `Collector::collect`, `App::tick`, `App::to_snapshot`, and existing model
methods unchanged.

### Text, configuration, and controls

Keep `ptop --once`, CLI flags, configuration keys, panel numbers, key bindings,
click targets, themes, and terminal-jump behavior unchanged.

## Implementation slices after approval

### Change 2.5: bounded private history projection

- Build assistant points and positioned summary markers from `PiSemantic.order`.
- Preserve component and attribution gaps.
- Add independent history validation and omission accounting.

Suggested commit subject:

```text
feat: project bounded Pi harness history
```

### Change 2.6: public history DTO

- Add the approved public types and `PiHarnessTelemetry.history`.
- Serialize the exact nested JSON shape.
- Keep legacy `token_history` unchanged.

Suggested commit subject:

```text
feat: expose Pi harness history
```

### Change 2.7: selected-session history graph

- Add `src/ui/session_history.rs` behind the approved one-function interface.
- Render full and compact graphs only with surplus detail space.
- Preserve aggregate-line and Runs priority.

Suggested commit subject:

```text
feat: graph Pi harness history
```

### Change 2.8: documentation and acceptance

- Update `README.md` and `docs/pi-support.md` after the behavior is green but in
  the same merged implementation PR. The current privacy contract prohibits
  public histories and summary events, so code must not merge without its
  matching contract update.
- Re-run parser, privacy, compatibility, layout, and full release checks.

Suggested commit subject:

```text
docs: document Pi harness history
```

## Required tests

### Projection and DTO

- Valid known-empty, one-point, exactly-64, 65-point, and saturated-count cases.
- Complete, partial-with-data, and unavailable-without-data status.
- Parser limits, duplicate IDs, incomplete tails, truncation, replacement, and
  deletion produce the documented history status without inventing points.
- Complete zero components versus null gap components.
- Missing, malformed, negative, and overflowing component values.
- Valid point attribution, response-model precedence, model fallback, invalid
  response model, unavailable metadata, 256-byte values, oversized values,
  control characters, and bidi controls.
- Attribution remains available when point components are null.
- File order across abandoned branches.
- Marker positions before the first point, between points, and after the last
  point.
- Zero-point position 0, every exact bucket boundary, and `position ==
  points.len()` before and after downsampling.
- Multiple markers at one position.
- Marker filtering outside the visible assistant window.
- Exactly-64, 65, malformed, and saturated summary-event cases.
- Compaction and branch-summary markers never publish private `tokensBefore`,
  including valid, absent, null, malformed, negative, and overflowing values.
- History-validation failure yields `history: null` without suppressing aggregate
  harness data.
- Existing aggregate sums, outcomes, cost, attribution, and `token_history` do
  not change.
- The rich projection still runs once per attachment refresh and stays within
  the release benchmark budget.

### Snapshot and privacy

- Exact key sets for every new DTO and the added harness key.
- Numeric fields serialize as numbers or null, never strings.
- Known-empty history serializes as empty arrays and zero counts.
- Every point component or attribution gap and failed history projection
  serializes as null.
- JSON and `--once` contain no prompt, assistant, tool, summary, retained-tail,
  ID, parent-ID, timestamp, stop-reason, per-point cost, or unsafe metadata
  sentinel.
- `--once` contains no History, component-row, model-marker, or summary-marker
  output.
- Existing JSON keys and `SessionView.token_history` remain unchanged.

Privacy and exact-shape guards must be proven by temporary mutations before the
change is accepted. Also mutation-prove marker boundary accounting by shifting
positions at 0, an interior bucket boundary, and `points.len()`, and prove
history-failure isolation by forcing validation failure while aggregate harness
assertions remain green.

### UI

- Full four-component graph at width 90 and greater.
- Compact total graph below width 90.
- Known zero renders as `▁`; unavailable renders as `·`.
- Independent component scales and maximum labels.
- Known model transition, unavailable attribution, and unchanged model glyphs.
- Compaction, branch-summary, and combined marker glyphs.
- Deterministic downsampling at widths smaller than point count.
- Every graph row uses the same column count at widths 5, 60, 89, and 90;
  legends use only remaining columns and never shift marker alignment.
- Widths below 5 return no graph rows.
- Mixed null/value component buckets render `◌` rather than hiding the gap.
- Compact checked-total overflow renders `!`, and overflow plus a gap renders
  `※`, without saturation or zero substitution.
- The exact eight-level scale covers an all-zero row and every glyph boundary.
- `known -> unavailable -> known` attribution renders uncertainty without a
  claimed transition; a downsampled bucket containing change and uncertainty
  renders `!`.
- Zero-point, interior-boundary, and final-boundary markers map to the specified
  columns before and after downsampling.
- Long provider/model values never appear in the graph and cannot change width.
- Complete, partial, and unavailable headings use `points.len()` for assistant
  points shown and `omitted_assistant_points` for assistant points omitted.
- Event-row legends use `markers.len()` for summaries shown and
  `omitted_summary_events` for summaries omitted.
- History omitted when detail height is insufficient.
- History visible in maximized narrow Sessions view when enough rows exist.
- Existing aggregate lines remain ahead of history.
- Identity and notes remain below history when space exists.
- Existing session rows and at least one Runs state line remain visible.
- Existing session, narrow-tab, narrow-section, zoom, and orphan-kill click
  targets remain aligned.
- No new theme key or per-model color appears.

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
cargo test --release pi_parser_release_benchmark -- --ignored --nocapture
```

## Stop conditions

Stop implementation and return to design if any change requires:

- timestamps, IDs, linkage, summary content, source text, or per-point costs;
- claiming active-path order or equal time spacing;
- treating a null component point as zero or removing its array slot;
- exposing more than 64 assistant points or 64 markers;
- suppressing valid aggregate harness data because history fails;
- changing aggregate reconciliation or fleet accounting;
- a second parser, cache, history store, or `App` side map;
- changing `Collector::collect`, `App::tick`, or `App::to_snapshot`;
- adding a panel, tab, mode, key binding, click target, config key, theme field,
  model color, or network request;
- changing `--once` output or existing JSON values; or
- adding a full table, persisted topology, cross-session history, or sidecar data.

## Approval gate

Approved by the owner on 2026-09-13. The approval covers all of these:

1. the exact history DTO field names and `harness.history` location;
2. section-level null behavior that preserves valid aggregate harness data;
3. complete, partial, unavailable, and bounded-omission meanings;
4. one point per accepted persisted assistant entry, including null gaps;
5. per-point attribution without timestamps, IDs, outcomes, or cost;
6. marker kinds, position meaning, filtering, bounds, and private
   `tokensBefore` behavior;
7. full and compact graph rows, glyphs, scaling, and downsampling;
8. surplus-space and existing narrow-zoom disclosure;
9. one Pi color and no new interaction or configuration;
10. additive JSON and next-minor Rust compatibility; and
11. the private collector projection plus one-function UI module interface.
