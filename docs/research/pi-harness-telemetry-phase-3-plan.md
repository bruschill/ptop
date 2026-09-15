# Pi harness telemetry Phase 3 implementation plan

**Status: Phase 3 is complete and release-accepted.** The private protocol, extension lifecycle/runtime, bounded registry, credentialed AF_UNIX transport, two-tick staged observations, public JSON DTO, selected-session presentation, and final release validation are complete.

## 1. Scope and security predicate

Deliver an **optional** Pi extension and ptop collector transport on Linux and
macOS. Windows remains process-only. The only live values are a fixed phase and
a sampled `pending_messages` boolean. Unknown is only `phase: null` and
`pending_messages: null`; it is never false, idle, or an `unknown` enum value.

Do not add queue depth, retry state, tool names, payloads, event objects, raw
errors, titles, paths, RPC, Internet/IP, cross-session history, or fleet data.
`--demo` never resolves directories, scans, opens, connects, or loads an
extension. Sidecar failure never removes valid JSONL telemetry or a process row.

Run sidecar work only after `PiCollector::resolve_attachments_with_budget` has
completed existing JSONL ownership, header, CWD, file-identity, ambiguity, and
process-start checks. A socket path and frame identity fields are discovery
hints, not proof. Accept only a kernel-credentialed endpoint whose peer PID is
the selected Pi PID, whose first frame matches the attached session ID, and whose
process-start identity remains valid under the staged checks below.

Linux uses `SO_PEERCRED`. Darwin requires runtime-successful `LOCAL_PEERPID` and
`LOCAL_PEERTOKEN`; `LOCAL_PEEREPID` is a consistency check. The Darwin audit
PID-version need not map to ptop's opaque start identity because ptop instead
rechecks the credentialed PID against its normal process snapshot before
publication. Missing APIs, credentials, snapshots, or identity equality fail
closed. This is endpoint identity under the benign same-user model, **not exact
byte authorship**. Same-UID malicious writers, malicious in-Pi extensions,
root/kernel attackers, and deliberate descriptor transfer are out of scope.

## 2. Effective Pi agent directory and short socket path

Resolve the effective global Pi agent directory exactly as Pi 0.85.1 handles the
leading-tilde form of `PI_CODING_AGENT_DIR`:

1. empty or unset: current home joined with `.pi/agent`;
2. a leading `~` or `~/`: replace that leading part with current home;
3. an absolute resulting path: normalize lexically to an absolute path; or
4. any other form: reject before touching files.

Reject unresolved home, `~name`, `~other/...`, relative paths, embedded tilde
forms, and paths that cannot normalize without preserving an absolute root. The
install command and Pi process must use the same environment; ptop warns rather
than guesses when it cannot establish this.

After expansion, require an absolute path. Open trusted root `/`, then walk **all
existing** components through the effective agent directory and `extensions`
with directory-FD-relative no-follow operations. A root-owned system ancestor is
allowed only when it is not group/world writable; an effective-UID-owned user
component is allowed only when it is not group/world writable. Reject every other
owner, symlink, non-directory, ownership/mode change, or ambiguous traversal.

The creation boundary is the first already-open effective-UID-owned secure
directory. Create missing components only below that boundary, one `0700`
component at a time, then reopen no-follow and `fstat` it before continuing. If
no safe user-owned creation anchor exists, refuse. Thus default `$HOME/.pi/agent`
works when the home and existing user path validate; macOS `/Users` and Linux
`/home` are permitted root-owned secure ancestors; leading-tilde expansion uses
that same home walk; and an absolute override works only if its full walk reaches
a safe user-owned anchor. A `.pi` symlink, or a mutation of any intermediate
component, rejects the operation.

The extension creates one runtime directory with `0700` before advertising. Its
short deterministic root is:

| Platform | Root selection |
|---|---|
| Linux | `/tmp/ptop-<uid>` |
| macOS | `/private/tmp/ptop-<uid>`; do not inherit `TMPDIR` |

`/tmp` and `/private/tmp` are trusted system sticky roots; ptop validates their
sticky-directory semantics where the platform exposes them and never treats
root ownership as an error. **Owner-approved Slice 3.2 amendment:** Linux no
longer uses `XDG_RUNTIME_DIR`; it always uses `/tmp/ptop-<uid>`. macOS always
uses `/private/tmp/ptop-<uid>`. This avoids inheriting an attacker-selected
runtime root. The extension creates only that direct child nonrecursively and
checks its identity before and after bind; same-UID races remain outside the
extension threat model. The per-UID child is effective-UID-owned, nonsymlink,
and exact `0700`. The socket name is `s-<24 lowercase hex>.sock` (31 bytes).
It is discovery entropy, not identity, and is effective-UID-owned, nonsymlink,
and exact `0600`.

The implementation calculates the encoded path length, including the NUL, and
requires it to be no more than the platform's runtime `sockaddr_un.sun_path`
capacity. It uses the smaller documented target capacity of 104 bytes for both
platforms. Thus `root + "/" + name + NUL <= 104`; the root itself is at most 71
bytes. No bind, connect, or discovery proceeds on an over-bound path. Tests cover
ordinary macOS paths, maximum decimal UID text,
exact boundary/over-boundary, and random-name collision retry.

## 3. Reduced v1 embedded extension lifecycle CLI

This owner-approved reduced substep provides only three local commands on macOS and Linux:

```text
ptop extension install
ptop extension status
ptop extension remove
```

The self-contained tracked asset is embedded at `extensions/ptop-live-harness.ts` under the effective global Pi agent directory. The resolver accepts only an absolute path, exact `~`, or `~/relative`; it rejects `~//tmp`, `~user`, embedded tilde, relative/root-escape, NUL, non-UTF-8, and unresolved-home forms. Descriptor-relative no-follow traversal permits secure root-owned ancestors and creates `0700` directories only below an existing secure effective-UID-owned anchor.

`status` is strictly read-only: it creates no directory or lock and reports target `absent`, `current`, `recognized-prior`, `unknown-or-modified`, or `unavailable`, plus any fixed install residue it observes. It reads no unrelated extension files. Recognition is SHA-256 of the embedded asset plus a typed capacity of eight prior official `(version, digest)` records; the current list is empty. Reads are bounded at exactly 64 KiB plus one byte.

`install` takes a validated no-follow `0600`, single-link, effective-UID lock, writes one exclusive fixed same-directory `0600` install temp, fsyncs and rereads it, then hard-links it to an absent target without replacement. It revalidates the directory chain and lock before mutation. It never rolls back a linked target or cleans residue automatically. Install interruption states are: target absent/not published with a partial or unknown fixed temp requiring manual inspection; a target installed/present with a recognized two-link install residue requiring manual inspection; or a target installed/present with durability uncertain after directory-sync failure.

`remove` uses the same modifying lock, removes only a current or recognized-prior exact target after immediate identity/content revalidation, then syncs the directory. It reports either removed or removal durability uncertain after unlink and directory-sync failure. Unknown or modified bytes are never overwritten or removed. Deliberate same-UID races remain outside the threat model.

There is no update, restore, force, backup, rollback, journal, recovery, backup rotation, transaction machinery, or automated temp/runtime cleanup. Upgrade is explicit `remove` then `install`; users run `/reload` or restart Pi only after both succeed. After stopping Pi, users must inspect exact files before manually removing a reported residue. Optional live telemetry can be absent after failure; JSONL and process-only monitoring are unchanged. Windows parses these commands but performs no mutation and reports unsupported.

The approved fixed runtime roots and later collector slices remain unchanged. Slice 3.2 now includes the accepted TypeScript runtime; private collector transport begins in Slice 3.3.

## 4. Complete global discovery generations

`PiCollector` owns one `SidecarRegistry`; it never scans once per session. After
the normal attachment pass, the registry receives every verified attached
`(pid, session_id, start_id)` and normal current process snapshot.

A discovery generation first enumerates one complete sorted set of at most 64
candidate socket identities `(path, device, inode)`. It reads entry 65 only to
prove exhaustion. More than 64 entries, descriptor exhaustion, elapsed-budget
exhaustion, or a directory error makes **new discovery unavailable** for that
generation; existing valid streams continue. It never probes a path owned by an
existing valid or pending stream.

The registry probes that fixed set across ticks under global budgets. No new
stream can publish until every candidate in the set is resolved. It then
re-enumerates the directory and requires the complete sorted identity set to be
unchanged before committing candidates. A changed set discards pending discovery
and starts a new generation. More than one credential/session-matching candidate
is unavailable. Candidate association is first by credentialed peer PID and then
by attached session ID; self-reported identity never selects a process.

Registry work is nonblocking and persists over normal ticks. Limits per tick:
64 entries plus entry 65 during enumeration, 4 new connects, 4 credential
checks, 16 frames, 8 KiB input, 8 descriptors globally, and 5 ms elapsed work.
A connect or partial frame gets a private `Instant` deadline of one second; the
first tick after deadline invalidates it. There are no synchronous scan/frame
waits and no per-session thread, timer, watcher, DNS, subprocess, or network.
Backoff is 1, 2, 4, then 10 seconds maximum and resets only on attachment change.

A registry entry is keyed by `(pid, start_id, session_id, device, inode)`. It
keeps one pending and one accepted descriptor per PID. Attachment change,
process disappearance, start mismatch, session mismatch, or identity-set
mutation closes only the affected pending/new entry. A required two-live-Pi test
proves discovery for B cannot close, replace, or delay valid A.

## 5. Protocol v1 and heartbeat

One listener permits one ptop client. A credential/session-checked connection
has a random 32-lowercase-hex **connection-scoped epoch**. Its immediate first
complete frame has sequence `0`, `phase: null`, and `pending_messages: null`.
Every later complete frame increments sequence by exactly one. A frame is the
heartbeat; no heartbeat field exists. Before `u64` exhaustion, extension closes;
a later authenticated connection creates a new epoch and repeats the first frame.

```json
{
  "magic":"ptop-live",
  "version":1,
  "epoch":"0123456789abcdef0123456789abcdef",
  "sequence":0,
  "session_id":"attached-session-id",
  "phase":null,
  "pending_messages":null
}
```

The object has no missing, extra, duplicate, or future keys. `magic` is exact;
version is integer 1; epoch is exact lowercase hex; session ID is 1..=256 valid
UTF-8 bytes without control/bidi characters and equals the attachment; sequence
is JSON `u64`; phase is null or `idle`, `generating`, `tool_running`,
`compacting`, or `waiting_for_user`; pending is null or boolean.

Frames are a four-byte big-endian length and strict UTF-8 JSON. Length is
1..=512; buffer is at most 1,024 bytes; validate before allocating. Because JSON
escaping can expand an otherwise valid 256-byte session ID, the extension first
constructs the largest canonical v1 frame shape in memory. If it cannot fit in
512 bytes, startup returns before filesystem, random, timer, or socket work and
ptop retains JSONL or process-only monitoring. Any later frame-construction or
sampling failure closes the optional listener without escaping into Pi. A partial
frame stays pending to its one-second deadline. Extra bytes parse only as the
next frame. Invalid framing/UTF-8/schema, unknown key/version/enum, first-frame
failure, sequence gap/rollback, epoch change, buffer excess, EOF, credential
mismatch, or process/start mismatch closes and invalidates immediately.
Reconnect is a fresh epoch after old close and full checks.

While connected, extension emits one complete frame every one-second interval,
including unchanged state. It coalesces changes to the latest reduced state and
emits no more than one frame per second after the immediate first frame. If
backpressure prevents a complete frame by the next interval, it closes the
stream. It stores no event/payload queue and with no client stores no write queue.
A second accepted client closes both and makes the listener unavailable until a
new session-scoped listener starts.

## 6. Reducer

Each listed Pi 0.85.1 extension hook is documented and globally delivered to the
extension instance. The handler immediately reduces then discards the event
object. It never logs or serializes content, titles, names, arguments, results,
or transient IDs.

| Hook | Reduction | Observed contradiction/recovery |
|---|---|---|
| `session_start` | new epoch state null/null | no session file: no listener |
| `session_shutdown` | close/unlink; retain no state | reload/switch is new runtime |
| `agent_start` | increment agent span; `generating` unless higher phase | duplicate active start -> null/null |
| `agent_end` | decrement agent span | unmatched end -> null/null |
| `agent_settled` | call `ctx.isIdle()`; if true clear spans and emit `idle`, if false null/null | later authoritative event is recovery |
| `tool_execution_start` | add private `toolCallId`; `tool_running` | max 32 IDs; duplicate/overflow clears set and nulls |
| `tool_execution_end` | remove matching private ID | unmatched end clears set and nulls |
| `session_before_compact` | increment compaction span; `compacting` | max 4 spans; overflow nulls |
| `session_compact` / `session_compact_failed` | decrement compaction span | unmatched completion/failure nulls |
| `ui_prompt_start` | increment prompt span; `waiting_for_user` | max 4 spans; overflow nulls |
| `ui_prompt_end` | decrement prompt span | unmatched end nulls |

`toolCallId` tracking is private, limited to 32 IDs of at most 256 UTF-8 bytes
each, and never serialized/logged. Slice 3.2 must use the same byte bound. The
precedence is `waiting_for_user > compacting > tool_running > generating > idle`.
`ctx.hasPendingMessages()` is sampled only on frame emission. The reducer does
not infer UI prompts. It never claims unobservable handler loss is detected;
crash or unobserved loss becomes unavailable through EOF/expiry. Only observed
duplicate, overflow, unmatched, or impossible transitions null values. The next
`session_start`, a valid authoritative event, or `agent_settled` with
`ctx.isIdle() == true` recovers. Tests include a competing extension emitting an
extra agent start to prove this contradiction nulls rather than fabricates idle.

## 7. Two-phase acceptance, clocks, and public state

No frame is published when read. The caller order and private state are:

```text
PiCollector::collect_sessions
  -> existing attachments + normal fresh process snapshot
  -> SidecarRegistry::poll_generation(...)
  -> UnixLiveHarnessStream::read_frame_into(PendingLiveFrame) at tick N
  -> tick N+1: SidecarRegistry::commit_pending(... fresh snapshot, attachment)
  -> LiveHarnessObservation -> SessionTelemetry.live_harness
```

`PendingLiveFrame` contains only validated reduced values, connection epoch,
sequence, receipt `Instant`, public Unix-epoch receipt milliseconds, attachment
key, peer PID, socket identity, and a credential-valid marker. At tick N+1,
commit requires the normal freshly collected snapshot to contain the same PID and
start ID, the JSONL attachment/session mapping to be unchanged, and a fresh
credential query on the connected descriptor to still name that PID. It also
requires socket and protocol checks to remain valid. Otherwise discard it. First
frames/new connections use the same staging. This re-queries only the descriptor
credential API; there is no extra macOS `ps` per frame. Tests cover process
exit/PID reuse/session replacement, credential failure, socket replacement, and
attachment change between read and commit.

Private receipt/deadline time uses `Instant`; public `observed_at_ms` is captured
separately as Unix epoch milliseconds. Clocks are never compared across
processes. The first tick after a three-second private receipt deadline expires
the observation:

| Condition | health / stale | public phase and pending | TUI |
|---|---|---|---|
| committed receipt before deadline | `healthy` / false | latest committed values, possibly null/null | `Live <phase> · pending <yes/no>`; null reads `unavailable` |
| deadline reached, before failed/absent stream handling | `stale` / true | null/null | `Live stale · values unavailable` |
| EOF, reject, absent stream, or stale cleanup | `unavailable` / false | null/null | suppress; use `Live unavailable` only in a reserved row |

The stale state is one collector observation only: its first expiry tick clears
current values; the following tick or stream close reports unavailable. Old phase
or pending is never shown as current.

Private validation precedes public work. The public slice adds `PiLivePhase`,
`PiLiveHarnessProvenance::ExtensionAfUnixV1`, and
`PiLiveHarnessTelemetry { phase: Option<PiLivePhase>, pending_messages:
Option<bool>, source_health: SourceHealth, provenance: PiLiveHarnessProvenance,
observed_at_ms: Option<u64>, stale: bool, reason: Option<String> }` under
`SessionTelemetry.live_harness` and
`SessionTelemetryView.live_harness`, serialized as
`sessions[].telemetry.live_harness`. It is null for process-only, no extension,
unsupported platform/API, ambiguity, and rejected data. Reasons are fixed ptop
text only. `--once` remains unchanged.

Adding fields to public `SessionTelemetry` and `SessionTelemetryView` breaks
external exhaustive struct literals. This is additive JSON compatibility but a
**Rust source breaking change**. Slice 3.5 is an explicit pre-1.0 release to
0.7.0: update `Cargo.toml` and `Cargo.lock`, changelog/README if repository
conventions require, exact-key tests, and source API compile tests. Do not call
it source-additive.

`src/ui/sessions.rs` adds at most one selected-session line after attachment and
source and before identity. Wide, compact, and narrow layouts retain session rows
and existing Runs priority. No panel, tab, key, click target, or theme field is
added. Fleet stays separate. README and `docs/pi-support.md` change only with
the public slice.

## 8. Seams, compatibility, and bounds

Keep all sidecar work in `collector::pi`; add no second process scan, App map,
collector, or fleet path. Private AF_UNIX helpers belong in
`src/collector/pi_live_harness.rs` behind
`cfg(any(target_os = "linux", target_vendor = "apple"))`; `PiCollector` owns
`SidecarRegistry`. Windows does not reference it. Keep rust-version 1.88 and
Linux/macOS/Windows compilation.

| Bound | Value |
|---|---:|
| path including NUL / socket name | 104 / 31 bytes |
| root path | 71 bytes maximum |
| generation | 64 entries plus entry 65 |
| work/tick | 4 connects, 4 credentials, 16 frames, 8 KiB, 5 ms |
| descriptors | 8 global; 1 pending + 1 accepted/PID |
| connect/partial deadline | 1 second / 1 second |
| frame/buffer | 512 / 1,024 bytes |
| heartbeat/expiry | 1 second / first tick after 3 seconds |
| tool IDs / toolCallId bytes / compaction and prompt spans | 32 / 256 / 4 |
| target extension read | 64 KiB |
| CLI lock acquisition | 1 second |
| retry backoff | 1, 2, 4, then 10 seconds |

## 9. Reviewable slices

Every slice is reversible and preserves JSONL/process-only fallback.

### 3.1 Contract, protocol, reducer, and private fixtures
- **Files:** private collector fixtures and these documents.
- **Behavior/tests:** complete. Private strict null/sequence-zero protocol,
  every real hook, `ctx.isIdle()` settling, heartbeat/backpressure, and mutation
  guards are covered without runtime collection or public output.
- **Docs/rollback:** no public output; remove fixtures without runtime change.
- **Gate:** security reviewer confirms endpoint-not-byte-authorship semantics.

### 3.2 Embedded asset and reduced lifecycle CLI
- **Files:** asset, Cargo package inputs/dependency/lock, `src/extension_lifecycle.rs`, `src/lib.rs`, CLI tests, and lifecycle documentation.
- **Behavior/tests:** complete. The reduced Rust CLI provides install/status/remove, no-follow traversal, lock validation, bounded current-plus-eight recognition, hard-link first-install race, read-only status, and manual-residue guidance. The embedded TypeScript runtime validates session identity before resources, uses owner-approved fixed runtime roots, emits bounded Protocol v1 frames, and closes session-scoped resources safely. Its deterministic Node suite uses injected runtime dependencies and shared protocol/reducer fixtures.
- **Status:** complete and security-accepted. Public live telemetry remains unimplemented.
- **Gate:** packaging/security reviewer verifies asset inclusion, runtime safety, and reduced no-overwrite/no-remove safety.

### 3.3 Private global generations and credential transport
- **Files:** `src/collector/pi.rs`, `pi_live_harness.rs`, private tests.
- **Behavior/tests:** fixed generations, cap+1, re-enumeration mutation, global
  budgets, short paths, socket modes, two Pi sessions, credentials, cfg compile.
- **Docs/rollback:** no public output; remove registry/module to return to JSONL.
- **Status:** complete and security-accepted. The registry publishes no live values.
- **Gate:** platform reviewer validates Linux/Darwin fail-closed behavior.

### 3.4 Private staged integration
- **Files:** collector/extension integration fixtures.
- **Behavior/tests:** cross-tick pending/commit races, reload/switch/crash,
  competing clients/extensions, EOF, expiry, no extension, `--no-session`, demo
  zero access, fleet exclusion.
- **Docs/rollback:** no public docs; remove private integration if any gate fails.
- **Status:** complete and security-accepted. Staged observations remain private.
- **Gate:** owner reviews private evidence before public output.

### 3.5 Public DTO, JSON, and 0.7.0 break
- **Files:** model, snapshot, `Cargo.toml`, `Cargo.lock`, changelog/README if
  required, exact-key and source API tests.
- **Behavior/tests:** nullable health/freshness, no stale old values, privacy,
  `--once` unchanged, 0.7.0 source-break release evidence.
- **Docs/rollback:** update README and `docs/pi-support.md`; revert DTO/docs/version
  as one release change without changing fallback.
- **Status:** complete and API/privacy-accepted at version 0.7.0. The TUI is unchanged.
- **Gate:** API/privacy reviewer accepts JSON compatibility and Rust break.

### 3.6 Selected-session presentation
- **Files:** `src/ui/sessions.rs` and layout tests.
- **Behavior/tests:** exact healthy/stale/unavailable text, wide/compact/narrow,
  Runs/session priority and click alignment.
- **Docs/rollback:** document visible behavior; remove one line to revert.
- **Status:** complete and UI-accepted. Live uses only genuine surplus detail rows.
- **Gate:** UI reviewer accepts Runs priority.

### 3.7 Final validation and release
- **Files:** targeted docs/tests only.
- **Behavior/tests:** privacy/platform/demo/release checks, Linux/macOS runtime
  evidence without release-floor claim, Windows process-only compile, package and
  publish dry runs.
- **Docs/rollback:** final consistency; reverse slices if gate fails.
- **Status:** complete and release-accepted.
- **Gate:** owner implementation acceptance and release sign-off.

## 10. Adversarial fixture matrix

| Area | Required proof |
|---|---|
| directory | default macOS/Linux home, override, leading tilde, relative/unsupported, unresolved home, root-owned ancestor, `.pi` symlink, and mutation of every intermediate component |
| reduced lifecycle CLI | install/status/remove only; status creates nothing; modifying-lock contention and replacement; hard-link initial-install target-creation race; bounded reads; current plus exactly-eight prior digest capacity; unknown/modified/type/mode/hardlink refusal; install and remove interruption messages; exact asset inclusion; no automatic cleanup or recovery |
| ownership | current plus latest-eight SHA allowlist, modified/unknown/oversized target or install residue, no-follow/type/mode/owner, and metadata that cannot authorize content |
| path | ordinary macOS, maximum UID, exact/over path bound, collision, sticky roots, user dir `0700`, socket `0600` |
| runtime cleanup | orderly extension close plus exact created-identity unlink only; crash-stale sockets remain; monitoring never unlinks; manual recovery instructions only |
| generation | sorted set, entry 65, mutation/re-enumeration restart, time/count/descriptor exhaustion, never probe existing stream |
| isolation | two live Pi sessions: B discovery cannot disconnect/delay A |
| credentials | wrong PID, PID reuse, exit, Darwin API/snapshot failure, Linux/Darwin differences: reject |
| protocol | malformed/partial/extra/oversized frame, schema/key/version, bidi/control, epoch/sequence, reconnect, heartbeat/backpressure |
| reducer | every table row, parallel IDs, duplicate/overflow/unmatched, `isIdle` false, competing-extension start, compaction/UI nesting |
| staging | first/new/established frames; exit/reuse/session/attachment/socket/credential race from tick N to N+1 |
| public | healthy/stale/unavailable invariant, nulls, exact TUI text, JSON keys, `--once`, privacy sentinels, fleet exclusion |
| boundaries | no extension, `--no-session`, Windows process-only, `--demo` zero access, no Internet/IP/HTTP/RPC/TCP/UDP, no cross-session history |

## 11. Approval gate

All technical decisions above are resolved. The **only** owner decision is to
approve this complete plan, including the extension CLI, `live_harness` JSON/UI,
and the 0.7.0 Rust source break.

### Ready-to-implement checklist

- [x] Owner approved implementation of this plan; Phase 3 is complete.
- [x] Security, packaging, platform, lifecycle, API/privacy, and UI slice gates pass.
- [x] Protocol/reducer, extension lifecycle/runtime, generation, credential,
  budget, isolation, staging, expiry, and mutation evidence pass.
- [x] Package list, publish dry run, demo, Linux/macOS validation, and Windows
  process-only compilation pass.

**Implementation status: complete and release-accepted. Validated live values
are available in JSON and in one surplus selected-session detail row. All slice
and final validation gates pass.**

## Sources

- Pi 0.85.1 [Extensions guide](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/docs/extensions.md): leading lifecycle rules, long-lived-resource shutdown, documented agent/tool/compaction/UI-prompt hooks, and `hasPendingMessages()`/`isIdle()`.
- Pi 0.85.1 [packages guide](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/docs/packages.md) and [session format](https://github.com/earendil-works/pi/blob/v0.85.1/packages/coding-agent/docs/session-format.md).
- Linux [`unix(7)`](https://man7.org/linux/man-pages/man7/unix.7.html) (`SO_PEERCRED`) and Apple XNU [`sys/un.h`](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/un.h) (`LOCAL_PEERPID`, `LOCAL_PEEREPID`, `LOCAL_PEERTOKEN`).
- Measured endpoint evidence: [Phase 3 contract](pi-harness-telemetry-phase-3.md#evidence-retained-from-the-spike).
