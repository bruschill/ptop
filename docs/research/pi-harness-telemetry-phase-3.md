# Pi harness telemetry Phase 3: approved AF_UNIX transport contract

**Status:** Phase 3 is complete and release-accepted. The protocol, extension lifecycle/runtime, credentialed AF_UNIX collector, staged validation, public JSON DTO, selected-session presentation, and final release validation are complete. This contract replaces the previous feasibility verdict. It is based on Pi coding-agent 0.85.1, the implementation branch, and the disposable probe results recorded below.

## Verdict

**FEASIBLE UNDER THE APPROVED THREAT MODEL, SUBJECT TO IMPLEMENTATION GATES.**

An optional, local AF_UNIX extension may provide generic live phase and a sampled
pending-message boolean on Linux and macOS. AF_UNIX is local IPC permitted for
this optional extension. Internet, IP, HTTP, RPC, and all other network
monitoring remain prohibited.

The accepted predicate is **kernel-authenticated endpoint identity**, not exact
byte authorship. After existing JSONL attachment succeeds, ptop may accept a
stream only when the kernel credentials for its peer identify the exact currently
verified Pi PID and ptop revalidates its existing process-start identity before
and after connection and frame handling. A transferred connected descriptor can
be written by another process. Therefore ptop must never claim that every byte
was authored by Pi.

The benign same-user threat model excludes malicious same-UID processes,
malicious extensions loaded inside Pi, root or kernel attackers, and deliberate
descriptor transfer. This matches the existing local attachment model. Ambiguous,
missing, stale, malformed, unsupported, or failed sidecar data preserves valid
JSONL telemetry; otherwise it preserves the process-only row.

## Scope and exclusions

This is an optional extension plus collector support on Linux and macOS only.
Windows stays process-only. It exposes only a fixed live phase and sampled
pending-message boolean. It does not expose queue depth, retry state, tool names,
payloads, event objects, raw errors, titles, paths, RPC, Internet/IP,
cross-session history, or `pi-subagents` fleet data. `--demo` performs no
sidecar discovery, open, connection, or extension activity.

Pi extension handlers reduce events immediately. They must not retain, write, or
log event objects, prompts, assistant text, tool input/output, error text, UI
titles, or session paths.

## Evidence retained from the spike

The regular-file candidate remains rejected: inode/FD matching can show that Pi
has an inode open, not who wrote the bytes. Sequence, permissions, locks, and
rename do not repair that gap.

**Date and scope:** 2026-09-14; Darwin 27.0.0 arm64 and a local,
network-isolated Linux Docker container (kernel `7.0.12-linuxkit`, aarch64,
Debian GCC 14.2.0). The pre-existing `rust:latest` image supplied the compiler;
no image or package was downloaded. Commands were:

```sh
./prototypes/pi-sidecar-af-unix-probe/run.sh
docker run --rm --network none -v "$PWD:/work:ro" -w /work rust:latest sh -c \
  './prototypes/pi-sidecar-af-unix-probe/run.sh'
```

Both macOS runs and the Linux run passed without a retained build or socket
artifact. On Darwin, client-to-server and server-to-client queries before data
succeeded for `LOCAL_PEERPID`, `LOCAL_PEEREPID`, `LOCAL_PEERTOKEN`, and
`getpeereid`; reported PID, effective UID, and GID matched the expected live
peer. Server-side queries still matched after one fixed frame, and each
`getsockopt` returned the requested result length. The token exposed a matching
PID and nonzero PID version. After the original peer closed and was reaped,
Darwin 27 returned `ENOTCONN` (57) for the PID/token options while `getpeereid`
still returned UID/GID.

On Linux, `SO_PEERCRED` succeeded at both endpoints before data, after a fixed
frame, after a transferred-writer frame, and after reconnect. It kept reporting
the original connection peer PID after that peer exited; a new connection
reported the new client PID. The probe held two clients and two accepted
descriptors concurrently and detected that neither was a valid sole stream.
Reconnect used a distinct peer PID.

For the transfer test, the original client passed its connected data descriptor
through `SCM_RIGHTS` to another writer, closed its copy, and exited before the
new writer sent a later fixed frame. The receiver therefore saw the original
endpoint identity while it remained available, or explicit Darwin post-exit
unavailability, rather than the later writer PID. This is a direct counterexample
to strict per-byte authorship, not a timing inference. It also proves that
reconnect and competing accepted connections are detectable. These measurements
do **not** establish any OS release floor or a supported mapping from Darwin PID
version to ptop's process-start identity.

## Remaining conditions before implementation

1. Implement runtime feature detection and fail closed. Linux requires
   `SO_PEERCRED`; Darwin requires the selected peer-PID/token API to succeed at
   runtime. No fallback to path, UID, self-reported PID, or self-reported session
   identity is allowed.
2. Validate supported Linux and macOS environments in the implementation test
   matrix. Do not generalize the probe into a release-floor statement.
3. On macOS, revalidate ptop's independent existing process-start identity for
   the credentialed PID before and after every accepted operation. A Darwin audit
   token's PID version need not be mapped to ptop's opaque start identity: the
   independent before/after revalidation is sufficient to prevent accepting a
   reused PID during the observation. Fail closed if either check is unavailable
   or changes.
4. Phase 3 implements the Protocol v1 contract, reducer, embedded extension
   lifecycle/runtime, bounded registry, credentialed AF_UNIX transport,
   two-tick staged observations, public JSON telemetry, and the selected-session
   live line. Final release validation passes.

## Pi lifecycle basis

Pi 0.85.1 requires factories not to start session resources. The extension must
bind only after `session_start`, tear down on `session_shutdown`, and treat
reload, session switch, new, resume, and fork as a new runtime. Pi reloads and
rebinds extensions around those transitions. `agent_settled` and
`ctx.hasPendingMessages()` support only the coarse reduction described here;
handlers for agent, tool, compaction, and UI-prompt events can carry forbidden
objects and must discard them. `agent_settled` may establish idle only after
`ctx.isIdle()` returns true. The wire representation of unknown is null for both
values, never an `unknown` phase string. A connection-scoped epoch begins with
sequence zero and a null/null frame; a frame is its own heartbeat. Connected
clients receive one complete frame per second; ptop stages every frame until the
next normal process snapshot confirms its peer and attachment identity.

Primary sources: installed Pi 0.85.1 `docs/extensions.md` (sections
“Long-lived resources and shutdown”, “Session Events”, “Agent Events”, and
“ctx.isIdle() / ctx.abort() / ctx.hasPendingMessages()”), its linked
`packages.md` and `session-format.md`, and purpose-built lifecycle tests planned
in [the Phase 3 plan](pi-harness-telemetry-phase-3-plan.md). The pinned public
counterparts are listed in [the research basis](pi-harness-telemetry.md#sources).

See [the implementation-ready Phase 3 plan](pi-harness-telemetry-phase-3-plan.md)
for the required design and owner gates.
