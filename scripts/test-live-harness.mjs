// Deterministic Node 26 tests. All runtime filesystem, clock, and socket work is injected.
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { readFileSync } from "node:fs";
import harness, { Heartbeat, PROTOCOL, Reducer, createRuntime, frame } from "../assets/pi-extension/ptop-live-harness.ts";

const fixture = JSON.parse(readFileSync(new URL("../src/collector/fixtures/pi_live_harness_v1.json", import.meta.url)));
const fixtureLimits = {
  max_frame_bytes: PROTOCOL.maxFrameBytes,
  max_buffer_bytes: PROTOCOL.maxBufferBytes,
  max_session_id_bytes: PROTOCOL.maxSessionIdBytes,
  epoch_hex_bytes: PROTOCOL.epochHexBytes,
  max_tool_ids: PROTOCOL.maxToolIds,
  max_tool_call_id_bytes: PROTOCOL.maxToolCallIdBytes,
  max_nesting: PROTOCOL.maxNesting,
  heartbeat_ms: PROTOCOL.heartbeatMs,
};
const phases = ["idle", "generating", "tool_running", "compacting", "waiting_for_user"];
assert.deepEqual(fixture.wire_keys, [...PROTOCOL.keys]);
assert.deepEqual(fixture.phases, phases);
assert.deepEqual(fixture.limits, fixtureLimits);

for (const test of fixture.reducer_cases) {
  const reducer = new Reducer();
  for (let index = 0; index < test.events.length; index++) {
    const event = test.events[index];
    let state;
    for (let repeat = 0; repeat < (event.repeat ?? 1); repeat++) {
      state = reducer.reduce(
        event.hook,
        event.tool_call_id_prefix ? `${event.tool_call_id_prefix}${repeat}` : event.tool_call_id,
        event.is_idle,
      );
    }
    if (test.closed?.[index]) assert.equal(state, null, test.name);
    else assert.equal(state.phase, test.phases[index], test.name);
  }
}

const epoch = "0123456789abcdef0123456789abcdef";
const session = "attached-session-id";
for (const valid of fixture.valid_frames) {
  const expected = JSON.parse(valid.json);
  const encoded = frame(expected.epoch, BigInt(expected.sequence), expected.session_id, {
    phase: expected.phase,
    pending_messages: expected.pending_messages,
  });
  assert.equal(encoded.readUInt32BE(0), encoded.length - 4, valid.name);
  assert.deepEqual(JSON.parse(encoded.subarray(4)), expected, valid.name);
}
assert.ok(fixture.invalid_frames.length > 0, "collector-only malformed frame fixtures are nonempty");
for (const malformed of fixture.invalid_frames) assert.equal(typeof malformed.name, "string");

const firstBeat = new Heartbeat(epoch, session);
const first = firstBeat.poll(0, { phase: "generating", pending_messages: null }, () => true);
assert.equal(first.kind, "frame");
assert.deepEqual(JSON.parse(first.frame.subarray(4)), {
  magic: "ptop-live",
  version: 1,
  epoch,
  sequence: 0,
  session_id: session,
  phase: null,
  pending_messages: null,
});
assert.equal(firstBeat.poll(999, { phase: "idle", pending_messages: null }, () => false).kind, "not_due");
const later = firstBeat.poll(1000, { phase: "idle", pending_messages: null }, () => false);
assert.equal(later.kind, "frame");
assert.equal(JSON.parse(later.frame.subarray(4)).sequence, 1);
assert.equal(JSON.parse(later.frame.subarray(4)).pending_messages, false);

const maxSafeSequence = BigInt(Number.MAX_SAFE_INTEGER) - 1n;
const terminalBeat = new Heartbeat(epoch, session, maxSafeSequence);
const terminalFrame = terminalBeat.poll(0, { phase: "idle", pending_messages: null }, () => false);
assert.equal(terminalFrame.kind, "frame");
assert.equal(JSON.parse(terminalFrame.frame.subarray(4)).sequence, Number(maxSafeSequence));
assert.equal(terminalBeat.poll(1000, { phase: "idle", pending_messages: null }, () => false).kind, "exhausted");
assert.equal(terminalBeat.poll(2000, { phase: "idle", pending_messages: null }, () => false).kind, "closed");
assert.throws(() => frame(epoch, maxSafeSequence + 1n, session, { phase: "idle", pending_messages: false }));
assert.throws(() => frame(epoch, 0n, session, { phase: "invalid", pending_messages: false }));
assert.throws(() => frame(epoch, 0n, session, { phase: "idle", pending_messages: 1 }));

// The default factory registers only real Pi hooks. Repeated session_start models
// startup, new/resume, switch/fork, and extension reload through Pi's one hook.
const hooks = new Map();
harness({ on(name, callback) { hooks.set(name, callback); } });
assert.deepEqual([...hooks.keys()], [
  "session_start", "session_shutdown", "agent_start", "agent_end", "agent_settled",
  "tool_execution_start", "tool_execution_end", "session_before_compact", "session_compact",
  "session_compact_failed", "ui_prompt_start", "ui_prompt_end",
]);
let fileCalls = 0;
const noSessionContext = {
  sessionManager: {
    getSessionFile() { fileCalls++; return undefined; },
    getSessionId() { throw new Error("ID must not be read without a persisted session"); },
  },
};
for (const _transition of ["startup", "new-or-resume", "switch-or-fork", "reload"]) {
  await hooks.get("session_start")({}, noSessionContext);
}
await hooks.get("session_shutdown")();
await hooks.get("session_shutdown")();
assert.equal(fileCalls, 4);

function dependencyProbe(id, sessionFile = "attached") {
  let effects = 0;
  const runtime = createRuntime(new Proxy({}, {
    get() { effects++; throw new Error("dependency accessed"); },
  }));
  runtime({ sessionManager: { getSessionFile: () => sessionFile, getSessionId: () => id } }, new Reducer());
  return effects;
}
for (const id of ["", "x".repeat(257), `bad${String.fromCharCode(0)}id`, `bad${String.fromCharCode(0x202e)}id`, "é".repeat(129)]) {
  assert.equal(dependencyProbe(id), 0, `invalid ID has zero side effects: ${JSON.stringify(id.slice(0, 8))}`);
}
assert.equal(dependencyProbe("x".repeat(256)) > 0, true, "exact 256-byte ID is accepted");
assert.equal(dependencyProbe("é".repeat(128)) > 0, true, "exact 256-byte multibyte ID is accepted");
assert.equal(dependencyProbe('"'.repeat(256)), 0, "an identifier whose canonical frame exceeds 512 bytes fails before runtime side effects");

function stat(kind, dev, ino, mode, uid) {
  return {
    dev, ino, mode, uid,
    isDirectory: () => kind === "dir",
    isSymbolicLink: () => kind === "link",
    isSocket: () => kind === "socket",
  };
}

class FakeSocket extends EventEmitter {
  destroyed = false;
  writes = [];
  writeResults = [];
  throwOnWrite = false;
  constructor(operations) { super(); this.operations = operations; }
  write(value) {
    this.operations.push("client-write");
    if (this.throwOnWrite) throw new Error("write failed");
    this.writes.push(Buffer.from(value));
    return this.writeResults.length ? this.writeResults.shift() : true;
  }
  destroy() { this.destroyed = true; this.operations.push("client-destroy"); return this; }
}

function runtimeFake(options = {}) {
  const platform = options.platform ?? "linux";
  const uid = options.uid ?? 501;
  const root = platform === "darwin" ? "/private/tmp" : "/tmp";
  const child = `${root}/ptop-${uid}`;
  const stats = new Map();
  const operations = [];
  const servers = [];
  const timers = [];
  const unlinks = [];
  const masks = [];
  const closeCallbacks = [];
  let now = 0;
  let mask = 0o022;
  let fdStat;
  let randomCounter = 20;
  let collisions = options.collisions ?? 0;
  let candidateLstats = 0;
  stats.set(root, options.rootStat ?? stat("dir", 1, 1, 0o1777, 0));
  if (options.childStat) stats.set(child, options.childStat);

  class FakeServer extends EventEmitter {
    closed = false;
    listening = false;
    path = undefined;
    constructor(listener) { super(); this.listener = listener; }
    listen(path, callback) {
      this.path = path;
      operations.push(`listen:${path}`);
      if (collisions > 0) {
        collisions--;
        queueMicrotask(() => this.emit("error", Object.assign(new Error("in use"), { code: "EADDRINUSE" })));
        return this;
      }
      if (options.listenError) {
        queueMicrotask(() => this.emit("error", Object.assign(new Error("listen failed"), { code: options.listenError })));
        return this;
      }
      this.listening = true;
      stats.set(path, stat("socket", 1, randomCounter++, options.socketMode ?? 0o600, uid));
      if (options.replaceDirectoryOnBind) stats.set(child, stat("dir", 1, 999, 0o700, uid));
      callback();
      return this;
    }
    close(callback) {
      operations.push("server-close");
      this.closed = true;
      this.listening = false;
      if (options.deferClose) closeCallbacks.push(callback);
      else callback?.();
      return this;
    }
    connect(socket = new FakeSocket(operations)) {
      if (this.closed || !this.listening) return false;
      this.listener(socket);
      return socket;
    }
  }

  const deps = {
    platform,
    uid: () => uid,
    lstat(path) {
      if (path.includes("/s-") && stats.has(path)) {
        candidateLstats++;
        if (options.replaceSocketDuringValidation && candidateLstats === 2) {
          stats.set(path, stat("socket", 1, 998, 0o600, uid));
        }
      }
      const value = stats.get(path);
      if (!value) throw Object.assign(new Error(`missing ${path}`), { code: "ENOENT" });
      return value;
    },
    fstat: () => fdStat,
    mkdir(path, mode) {
      operations.push(`mkdir:${mode.toString(8)}`);
      if (stats.has(path)) throw Object.assign(new Error("exists"), { code: "EEXIST" });
      stats.set(path, stat("dir", 1, 2, mode, uid));
    },
    openDirectory(path) { operations.push("open-directory"); fdStat = stats.get(path); return 7; },
    closeFd() { operations.push("close-directory"); },
    unlink(path) { operations.push("unlink"); unlinks.push(path); stats.delete(path); },
    umask(value) { const old = mask; mask = value; masks.push(value); operations.push(`umask:${value.toString(8)}`); return old; },
    random(bytes) {
      randomCounter++;
      if (bytes === 12) return randomCounter.toString(16).padStart(24, "0");
      return randomCounter.toString(16).padStart(32, "0");
    },
    now: () => now,
    setInterval(fn, ms) { const timer = { fn, ms, active: true }; timers.push(timer); operations.push("timer-start"); return timer; },
    clearInterval(timer) { timer.active = false; operations.push("timer-clear"); },
    createServer(listener) { const server = new FakeServer(listener); servers.push(server); return server; },
  };

  return {
    deps, root, child, stats, operations, servers, timers, unlinks, masks,
    context: {
      sessionManager: { getSessionFile: () => "attached", getSessionId: () => "session" },
      hasPendingMessages: () => typeof options.pendingMessages === "function" ? options.pendingMessages() : (options.pendingMessages ?? false),
    },
    createSocket: () => new FakeSocket(operations),
    tick(value) { now = value; for (const timer of [...timers]) if (timer.active) timer.fn(); },
    finishCloses() { for (const callback of closeCallbacks.splice(0)) callback?.(); },
    socketPath() { return servers.at(-1)?.path; },
  };
}

async function flush() { await new Promise(resolve => setImmediate(resolve)); }
function startFake(fake, reducer = new Reducer()) {
  reducer.reduce("session_start");
  return { reducer, resource: createRuntime(fake.deps)(fake.context, reducer) };
}

let rootPathCases = 0;
for (const platform of ["linux", "darwin"]) {
  const fake = runtimeFake({ platform });
  const { resource } = startFake(fake);
  assert.equal(fake.servers.length, 1);
  assert.equal(fake.servers[0].path.startsWith(platform === "darwin" ? "/private/tmp/ptop-501/" : "/tmp/ptop-501/"), true);
  assert.deepEqual(fake.masks, [0o177, 0o022], "umask is narrowly restored");
  const closing = resource.close();
  await closing;
  assert.ok(fake.operations.indexOf("server-close") < fake.operations.indexOf("unlink"));
  rootPathCases++;
}
for (const rootStat of [
  stat("dir", 1, 1, 0o1777, 501),
  stat("dir", 1, 1, 0o0777, 0),
  stat("file", 1, 1, 0o1777, 0),
  stat("link", 1, 1, 0o1777, 0),
]) {
  const fake = runtimeFake({ rootStat });
  await startFake(fake).resource.close();
  assert.equal(fake.servers.length, 0, "unsafe fixed root fails closed");
  rootPathCases++;
}
for (const childStat of [
  stat("dir", 1, 2, 0o755, 501),
  stat("dir", 1, 2, 0o700, 502),
  stat("file", 1, 2, 0o700, 501),
  stat("link", 1, 2, 0o700, 501),
]) {
  const fake = runtimeFake({ childStat });
  const victim = fake.stats.get(fake.child);
  await startFake(fake).resource.close();
  assert.equal(fake.servers.length, 0, "unsafe child fails closed");
  assert.equal(fake.stats.get(fake.child), victim, "existing child or symlink is unchanged");
  assert.equal(fake.masks.length, 0);
  rootPathCases++;
}
{
  const fake = runtimeFake({ replaceDirectoryOnBind: true });
  const { resource } = startFake(fake);
  assert.equal(fake.servers[0].closed, true, "directory replacement disables the listener before explicit close");
  assert.deepEqual(fake.unlinks, [], "replaced directory prevents socket unlink");
  await resource.close();
  rootPathCases++;
}
{
  const fake = runtimeFake({ replaceSocketDuringValidation: true });
  const { resource } = startFake(fake);
  assert.equal(fake.servers[0].closed, true, "replacement during validation is not accepted");
  assert.deepEqual(fake.unlinks, []);
  await resource.close();
  rootPathCases++;
}
{
  const fake = runtimeFake();
  const { resource } = startFake(fake);
  const path = fake.socketPath();
  fake.stats.set(path, stat("socket", 1, 777, 0o600, 501));
  await resource.close();
  assert.deepEqual(fake.unlinks, [], "replacement before shutdown is not unlinked");
  rootPathCases++;
}
{
  const fake = runtimeFake({ collisions: 2 });
  const { resource } = startFake(fake);
  await flush();
  assert.equal(fake.servers.length, 3, "EADDRINUSE retries with fresh candidates");
  assert.equal(fake.unlinks.length, 0, "collision paths are never unlinked");
  await resource.close();
  rootPathCases++;
}
{
  const fake = runtimeFake({ collisions: 4 });
  const { resource } = startFake(fake);
  await flush();
  assert.equal(fake.servers.length, 4, "collision retry is bounded");
  assert.equal(fake.servers.every(server => server.closed), true);
  assert.equal(fake.timers.length, 0);
  assert.equal(fake.unlinks.length, 0);
  await resource.close();
  rootPathCases++;
}
{
  const fake = runtimeFake({ uid: 4_294_967_295 });
  const { resource } = startFake(fake);
  assert.ok(Buffer.byteLength(fake.socketPath()) + 1 <= 104, "maximum platform UID fits conservative Darwin bound");
  await resource.close();
  rootPathCases++;
}
for (const socketMode of [0o666, 0o4600]) {
  const fake = runtimeFake({ socketMode });
  await startFake(fake).resource.close();
  assert.equal(fake.servers[0].closed, true);
  assert.deepEqual(fake.unlinks, []);
  rootPathCases++;
}
{
  const fake = runtimeFake({ listenError: "EACCES" });
  const { resource } = startFake(fake);
  await flush();
  assert.equal(fake.servers[0].closed, true, "non-collision listener error closes server");
  assert.ok(fake.operations.includes("close-directory"));
  await resource.close();
  rootPathCases++;
}
{
  const fake = runtimeFake({ deferClose: true });
  const { resource } = startFake(fake);
  const firstClose = resource.close();
  const secondClose = resource.close();
  assert.equal(firstClose, secondClose, "idempotent close returns the in-flight shutdown");
  assert.equal(fake.unlinks.length, 0, "unlink waits for server close callback");
  fake.finishCloses();
  await firstClose;
  assert.equal(fake.unlinks.length, 1);
  rootPathCases++;
}

let connectionCases = 0;
{
  const fake = runtimeFake();
  const { resource } = startFake(fake);
  const firstSocket = fake.createSocket();
  assert.notEqual(fake.servers[0].connect(firstSocket), false);
  const firstBody = JSON.parse(firstSocket.writes[0].subarray(4));
  assert.equal(firstBody.sequence, 0);
  assert.equal(firstBody.phase, null);
  assert.equal(firstBody.pending_messages, null);
  firstSocket.emit("close");
  const secondSocket = fake.createSocket();
  fake.servers[0].connect(secondSocket);
  const secondBody = JSON.parse(secondSocket.writes[0].subarray(4));
  assert.equal(secondBody.sequence, 0);
  assert.notEqual(secondBody.epoch, firstBody.epoch, "reconnect gets a fresh epoch");
  await resource.close();
  connectionCases++;
}
{
  const fake = runtimeFake();
  const { resource } = startFake(fake);
  const oldSocket = fake.createSocket();
  oldSocket.writeResults.push(false);
  fake.servers[0].connect(oldSocket);
  oldSocket.emit("error", new Error("old error"));
  const currentSocket = fake.createSocket();
  fake.servers[0].connect(currentSocket);
  oldSocket.emit("drain");
  oldSocket.emit("close");
  fake.tick(1000);
  assert.equal(currentSocket.writes.length, 2, "stale old events do not clear reconnect state");
  assert.equal(fake.timers.filter(timer => timer.active).length, 1);
  await resource.close();
  connectionCases++;
}
{
  const fake = runtimeFake();
  const { resource } = startFake(fake);
  const firstSocket = fake.createSocket();
  const secondSocket = fake.createSocket();
  fake.servers[0].connect(firstSocket);
  fake.servers[0].connect(secondSocket);
  assert.equal(firstSocket.destroyed, true);
  assert.equal(secondSocket.destroyed, true);
  assert.equal(fake.servers[0].closed, true, "second client automatically disables and closes the listener");
  assert.equal(fake.timers.every(timer => !timer.active), true, "second-client shutdown clears timers before explicit close");
  assert.equal(fake.servers[0].connect(fake.createSocket()), false, "third connection cannot reach disabled listener");
  await resource.close();
  connectionCases++;
}
{
  const fake = runtimeFake({ pendingMessages: true });
  const { reducer, resource } = startFake(fake);
  const socket = fake.createSocket();
  socket.writeResults.push(false, true, true);
  fake.servers[0].connect(socket);
  reducer.reduce("agent_start");
  reducer.reduce("tool_execution_start", "PRIVATE_TOOL_ID");
  fake.tick(999);
  assert.equal(fake.servers[0].closed, false, "early timer does not close a backpressured client");
  socket.emit("drain");
  fake.tick(1000);
  const coalesced = JSON.parse(socket.writes[1].subarray(4));
  assert.equal(coalesced.phase, "tool_running");
  assert.equal(coalesced.pending_messages, true);
  fake.tick(2000);
  assert.equal(socket.writes.length, 3, "unchanged state still emits each interval");
  await resource.close();
  connectionCases++;
}
{
  const fake = runtimeFake();
  const { resource } = startFake(fake);
  const socket = fake.createSocket();
  socket.writeResults.push(false);
  fake.servers[0].connect(socket);
  fake.tick(999);
  assert.equal(fake.servers[0].closed, false);
  fake.tick(1000);
  assert.equal(fake.servers[0].closed, true, "undrained write automatically closes at the monotonic boundary");
  assert.equal(fake.timers.every(timer => !timer.active), true, "backpressure shutdown clears timers before explicit close");
  await resource.close();
  connectionCases++;
}
{
  const fake = runtimeFake({ pendingMessages: () => { throw new Error("sampling failed"); } });
  const { resource } = startFake(fake);
  const socket = fake.createSocket();
  fake.servers[0].connect(socket);
  fake.tick(1000);
  assert.equal(fake.servers[0].closed, true, "frame or sampling failure fails closed inside the extension");
  assert.equal(fake.timers.every(timer => !timer.active), true);
  await resource.close();
  connectionCases++;
}
{
  const fake = runtimeFake();
  const { resource } = startFake(fake);
  const socket = fake.createSocket();
  socket.throwOnWrite = true;
  fake.servers[0].connect(socket);
  assert.equal(socket.destroyed, true);
  assert.equal(fake.timers.length, 0, "synchronous write failure creates no timer leak");
  await resource.close();
  connectionCases++;
}

let reducerFrameCases = 0;
for (const [id, expected] of [["a".repeat(256), "tool_running"], ["a".repeat(257), null]]) {
  const reducer = new Reducer();
  reducer.reduce("session_start");
  assert.equal(reducer.reduce("tool_execution_start", id).phase, expected);
  reducerFrameCases++;
}
for (const [start, end] of [["ui_prompt_start", "ui_prompt_end"], ["session_before_compact", "session_compact"]]) {
  const reducer = new Reducer();
  reducer.reduce("session_start");
  for (let index = 0; index < 4; index++) assert.notEqual(reducer.reduce(start).phase, null);
  assert.equal(reducer.reduce(start).phase, null, `${start} nesting five fails closed`);
  assert.notEqual(reducer.reduce(start).phase, null, `${start} recovers on authoritative activity`);
  assert.equal(reducer.reduce(end).phase, null);
  reducerFrameCases++;
}
{
  const reducer = new Reducer();
  reducer.reduce("session_start");
  reducer.reduce("tool_execution_start", "PRIVATE_TOOL_ID");
  const output = frame(epoch, 1n, session, { ...reducer.state(), pending_messages: false });
  const body = JSON.parse(output.subarray(4));
  assert.deepEqual(Object.keys(body), [...PROTOCOL.keys]);
  for (const secret of ["PROMPT_PRIVATE", "TOOL_ARGUMENT_PRIVATE", "ASSISTANT_PRIVATE", "ERROR_PRIVATE", "TITLE_PRIVATE", "PATH_PRIVATE", "PRIVATE_TOOL_ID"]) {
    assert.equal(output.includes(secret), false, `${secret} is not emitted`);
  }
  reducerFrameCases++;
}

const source = readFileSync(new URL("../assets/pi-extension/ptop-live-harness.ts", import.meta.url), "utf8");
for (const forbidden of ["fetch(", "http:", "https:", "child_process", "spawn(", "exec(", "console.", "toolName", "event.args", "event.result"]) {
  assert.equal(source.includes(forbidden), false, forbidden);
}

console.log("live-harness TypeScript/Rust fixture, protocol, lifecycle, heartbeat, backpressure, and privacy tests passed");
console.log(`live-harness deterministic cases: roots/path=${rootPathCases}, connections/time=${connectionCases}, reducer/frame=${reducerFrameCases}`);
