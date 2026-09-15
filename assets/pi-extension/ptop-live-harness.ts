// ptop live harness Protocol v1. It retains only reduced, non-content state.
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import * as fs from "node:fs";
import { randomBytes } from "node:crypto";
import { createServer, type Server, type Socket } from "node:net";
import { performance } from "node:perf_hooks";

export const PROTOCOL = { magic: "ptop-live", version: 1, keys: ["magic", "version", "epoch", "sequence", "session_id", "phase", "pending_messages"], maxFrameBytes: 512, maxBufferBytes: 1024, maxSessionIdBytes: 256, epochHexBytes: 32, maxToolIds: 32, maxToolCallIdBytes: 256, maxNesting: 4, heartbeatMs: 1000 } as const;
export type Phase = "idle" | "generating" | "tool_running" | "compacting" | "waiting_for_user";
export type State = { phase: Phase | null; pending_messages: boolean | null };
const PHASES: readonly Phase[] = ["idle", "generating", "tool_running", "compacting", "waiting_for_user"];
type Identity = { dev: number; ino: number };
type Stat = { dev: number; ino: number; uid: number; mode: number; isDirectory(): boolean; isSymbolicLink(): boolean; isSocket(): boolean };
export type RuntimeDependencies = {
  platform: string; uid(): number | undefined; lstat(path: string): Stat; fstat(fd: number): Stat; mkdir(path: string, mode: number): void; openDirectory(path: string): number; closeFd(fd: number): void; unlink(path: string): void; umask(mask: number): number;
  random(bytes: number): string; now(): number; setInterval(fn: () => void, ms: number): unknown; clearInterval(timer: unknown): void; createServer(listener: (socket: Socket) => void): Server;
};
const MAX_SEQUENCE = 9_007_199_254_740_990n; // JSON numbers must remain exact in JavaScript readers.

export class Reducer {
  #known = false; #idle = false; #agents = 0; #compactions = 0; #prompts = 0; #tools = new Set<string>();
  reduce(hook: string, id?: string, isIdle?: boolean): State | null {
    if (hook === "session_start") { this.#clear(); this.#known = this.#idle = false; return this.state(); }
    if (hook === "session_shutdown") { this.#clear(); return null; }
    if (hook === "agent_start") { if (this.#agents) this.#invalid(); else { this.#activity(); this.#agents = 1; } }
    else if (hook === "agent_end") { if (!this.#known || !this.#agents) this.#invalid(); else this.#agents--; }
    else if (hook === "agent_settled") { if (isIdle) { this.#clear(); this.#known = this.#idle = true; } else this.#invalid(); }
    else if (hook === "tool_execution_start") { this.#activity(); if (!id || Buffer.byteLength(id) > PROTOCOL.maxToolCallIdBytes || this.#tools.size === PROTOCOL.maxToolIds || this.#tools.has(id)) this.#invalid(); else this.#tools.add(id); }
    else if (hook === "tool_execution_end") { if (!this.#known || !id || !this.#tools.delete(id)) this.#invalid(); }
    else if (hook === "session_before_compact") { this.#activity(); if (this.#compactions === PROTOCOL.maxNesting) this.#invalid(); else this.#compactions++; }
    else if (hook === "session_compact" || hook === "session_compact_failed") { if (!this.#known || !this.#compactions) this.#invalid(); else this.#compactions--; }
    else if (hook === "ui_prompt_start") { this.#activity(); if (this.#prompts === PROTOCOL.maxNesting) this.#invalid(); else this.#prompts++; }
    else if (hook === "ui_prompt_end") { if (!this.#known || !this.#prompts) this.#invalid(); else this.#prompts--; }
    return this.state();
  }
  state(): State { let phase: Phase | null = null; if (this.#known) { if (this.#prompts) phase = "waiting_for_user"; else if (this.#compactions) phase = "compacting"; else if (this.#tools.size) phase = "tool_running"; else if (this.#agents) phase = "generating"; else if (this.#idle) phase = "idle"; } return { phase, pending_messages: null }; }
  #activity() { if (!this.#known) { this.#clear(); this.#known = true; } this.#idle = false; }
  #invalid() { this.#clear(); this.#known = this.#idle = false; }
  #clear() { this.#agents = this.#compactions = this.#prompts = 0; this.#tools.clear(); }
}
function validSessionId(value: string): boolean { const bytes = Buffer.byteLength(value); return bytes >= 1 && bytes <= PROTOCOL.maxSessionIdBytes && !/[\u0000-\u001f\u007f-\u009f\u200e\u200f\u202a-\u202e\u2066-\u2069]/u.test(value); }
export function frame(epoch: string, sequence: bigint, sessionId: string, state: State): Buffer {
  if (!/^[0-9a-f]{32}$/.test(epoch) || !validSessionId(sessionId) || sequence < 0n || sequence > MAX_SEQUENCE) throw new Error("invalid frame identity");
  if ((state.phase !== null && !PHASES.includes(state.phase)) || (state.pending_messages !== null && typeof state.pending_messages !== "boolean")) throw new Error("invalid frame state");
  const body = Buffer.from(`{"magic":"ptop-live","version":1,"epoch":${JSON.stringify(epoch)},"sequence":${sequence},"session_id":${JSON.stringify(sessionId)},"phase":${state.phase === null ? "null" : JSON.stringify(state.phase)},"pending_messages":${state.pending_messages === null ? "null" : state.pending_messages}}`);
  if (!body.length || body.length > PROTOCOL.maxFrameBytes) throw new Error("frame bound"); const prefix = Buffer.alloc(4); prefix.writeUInt32BE(body.length); return Buffer.concat([prefix, body]);
}
export class Heartbeat {
  #sequence: bigint; #next = 0; #closed = false;
  readonly epoch: string; readonly sessionId: string;
  constructor(epoch: string, sessionId: string, sequence = 0n) { this.epoch = epoch; this.sessionId = sessionId; this.#sequence = sequence; }
  isDue(now: number): boolean { return !this.#closed && now >= this.#next; }
  poll(now: number, state: State, pending: () => boolean): { kind: "not_due" | "closed" | "exhausted" } | { kind: "frame"; frame: Buffer } { if (this.#closed) return { kind: "closed" }; if (!this.isDue(now)) return { kind: "not_due" }; if (this.#sequence > MAX_SEQUENCE) { this.close(); return { kind: "exhausted" }; } const first = this.#sequence === 0n; const output = frame(this.epoch, this.#sequence++, this.sessionId, first ? { phase: null, pending_messages: null } : { ...state, pending_messages: pending() }); this.#next = now + PROTOCOL.heartbeatMs; return { kind: "frame", frame: output }; }
  close() { this.#closed = true; }
}
function same(a: Stat, b: Stat): boolean { return a.dev === b.dev && a.ino === b.ino; }
function checkedDirectory(d: RuntimeDependencies, path: string, uid: number, root: boolean): Stat {
  const stat = d.lstat(path); const mode = stat.mode & 0o7777;
  if (!stat.isDirectory() || stat.isSymbolicLink() || stat.uid !== (root ? 0 : uid) || (root ? mode !== 0o1777 : mode !== 0o700)) throw new Error("unsafe runtime directory");
  return stat;
}
/** Create one owner-approved child and retain its descriptor until shutdown. */
function runtimeDirectory(d: RuntimeDependencies): { path: string; fd: number; identity: Identity } {
  const uid = d.uid(); if (uid === undefined || (d.platform !== "linux" && d.platform !== "darwin")) throw new Error("unsupported runtime");
  const base = d.platform === "darwin" ? "/private/tmp" : "/tmp"; checkedDirectory(d, base, uid, true);
  const path = `${base}/ptop-${uid}`; if (Buffer.byteLength(path) > 71) throw new Error("runtime path bound");
  try { d.mkdir(path, 0o700); } catch (error) { if ((error as { code?: string }).code !== "EEXIST") throw error; }
  const before = checkedDirectory(d, path, uid, false); const fd = d.openDirectory(path);
  try { const opened = d.fstat(fd); const after = checkedDirectory(d, path, uid, false); if (!same(before, opened) || !same(before, after)) throw new Error("runtime directory replaced"); return { path, fd, identity: { dev: before.dev, ino: before.ino } }; } catch (error) { d.closeFd(fd); throw error; }
}
function defaultDependencies(): RuntimeDependencies { return { platform: process.platform, uid: () => process.getuid?.(), lstat: fs.lstatSync as (path: string) => Stat, fstat: fs.fstatSync as (fd: number) => Stat, mkdir: fs.mkdirSync, openDirectory: path => fs.openSync(path, fs.constants.O_RDONLY | fs.constants.O_DIRECTORY | fs.constants.O_NOFOLLOW), closeFd: fs.closeSync, unlink: fs.unlinkSync, umask: process.umask, random: bytes => randomBytes(bytes).toString("hex"), now: () => performance.now(), setInterval, clearInterval, createServer }; }
export function createRuntime(deps: RuntimeDependencies = defaultDependencies()) {
  return function start(ctx: ExtensionContext, reducer: Reducer): { close(): Promise<void> } {
    // This must precede every filesystem, random, clock, timer, or socket operation.
    const sessionFile = ctx.sessionManager.getSessionFile(); if (!sessionFile) return { async close() {} };
    const sessionId = ctx.sessionManager.getSessionId(); if (!sessionId || !validSessionId(sessionId)) return { async close() {} };
    // A syntactically valid identifier can expand when JSON-escaped. Refuse it
    // before runtime side effects unless every v1 frame shape remains bounded.
    try { frame("0".repeat(PROTOCOL.epochHexBytes), MAX_SEQUENCE, sessionId, { phase: "waiting_for_user", pending_messages: false }); } catch { return { async close() {} }; }
    let directory: { path: string; fd: number; identity: Identity }; try { directory = runtimeDirectory(deps); } catch { return { async close() {} }; }
    let server: Server | undefined; let client: Socket | undefined; let timer: unknown; let pending = false; let disabled = false; let closed = false; let released = false; let socketIdentity: Identity | undefined; let heartbeat: Heartbeat | undefined; let path = ""; let attempts = 0; let shutdownPromise: Promise<void> | undefined;
    const releaseDirectory = () => { if (!released) { released = true; deps.closeFd(directory.fd); } };
    const clearClient = (socket?: Socket) => { if (socket && client !== socket) return; if (timer !== undefined) deps.clearInterval(timer); timer = undefined; if (!socket || client === socket) { client = undefined; pending = false; heartbeat?.close(); heartbeat = undefined; } };
    const closeServer = (current: Server | undefined) => new Promise<void>(resolve => { if (!current) { resolve(); return; } try { current.close(() => resolve()); } catch { resolve(); } });
    const unlinkIfOwned = () => { if (!socketIdentity) return; try { const stat = deps.lstat(path); if (stat.isSocket() && stat.uid === deps.uid() && (stat.mode & 0o7777) === 0o600 && stat.dev === socketIdentity.dev && stat.ino === socketIdentity.ino) deps.unlink(path); } catch {} };
    const shutdown = (): Promise<void> => { if (shutdownPromise) return shutdownPromise; closed = true; shutdownPromise = (async () => { const currentClient = client; if (currentClient) currentClient.destroy(); clearClient(currentClient); const current = server; server = undefined; await closeServer(current); unlinkIfOwned(); releaseDirectory(); })(); return shutdownPromise; };
    const emit = (socket: Socket) => { if (client !== socket || socket.destroyed || pending || !heartbeat) return; let result: ReturnType<Heartbeat["poll"]>; try { result = heartbeat.poll(deps.now(), reducer.state(), () => ctx.hasPendingMessages()); } catch { void shutdown(); return; } if (result.kind === "not_due") return; if (result.kind !== "frame") { void shutdown(); return; } try { pending = !socket.write(result.frame); if (pending) socket.once("drain", () => { if (client === socket) pending = false; }); } catch { socket.destroy(); clearClient(socket); } };
    const startListener = () => {
      if (closed || attempts === 4) { void shutdown(); return; }
      const hex = deps.random(12); if (!/^[0-9a-f]{24}$/.test(hex)) { attempts++; startListener(); return; }
      const candidate = `${directory.path}/s-${hex}.sock`; if (Buffer.byteLength(candidate) + 1 > 104) { void shutdown(); return; }
      attempts++; path = candidate; socketIdentity = undefined;
      const current = deps.createServer(socket => {
        if (disabled || client) { socket.destroy(); if (client) client.destroy(); disabled = true; void shutdown(); return; }
        client = socket; heartbeat = new Heartbeat(deps.random(16), sessionId);
        socket.once("close", () => clearClient(socket));
        socket.once("error", () => { if (client === socket) { socket.destroy(); clearClient(socket); } });
        emit(socket);
        if (client === socket && !closed) timer = deps.setInterval(() => { if (client !== socket || !heartbeat) return; const now = deps.now(); if (pending) { if (heartbeat.isDue(now)) void shutdown(); } else emit(socket); }, PROTOCOL.heartbeatMs);
      });
      server = current;
      current.once("error", (error: { code?: string }) => { if (server !== current || closed) return; if (error.code === "EADDRINUSE" && attempts < 4) { server = undefined; void closeServer(current).then(startListener); } else void shutdown(); });
      try {
        // bind creates the socket with 0600 under the private directory. Restore
        // the process-wide umask immediately, even if listen throws.
        const oldMask = deps.umask(0o177);
        try { current.listen(candidate, () => { if (server !== current || closed) return; try { const before = deps.lstat(candidate); if (!before.isSocket() || before.uid !== deps.uid() || (before.mode & 0o7777) !== 0o600) throw new Error("unsafe socket"); const opened = deps.fstat(directory.fd); const afterDirectory = deps.lstat(directory.path); const after = deps.lstat(candidate); if (!same(opened, afterDirectory) || opened.dev !== directory.identity.dev || opened.ino !== directory.identity.ino || !same(before, after)) throw new Error("runtime identity replaced"); socketIdentity = { dev: before.dev, ino: before.ino }; } catch { void shutdown(); } }); } finally { deps.umask(oldMask); }
      } catch { void shutdown(); }
    };
    startListener();
    return { close: shutdown };
  };
}
export default function (pi: ExtensionAPI) {
  const start = createRuntime(); let reducer: Reducer | undefined; let resource: { close(): Promise<void> } | undefined;
  pi.on("session_start", async (_event, ctx) => { const old = resource; resource = undefined; reducer = undefined; await old?.close(); const next = new Reducer(); next.reduce("session_start"); reducer = next; resource = start(ctx, next); });
  pi.on("session_shutdown", async () => { reducer?.reduce("session_shutdown"); const old = resource; reducer = undefined; resource = undefined; await old?.close(); });
  pi.on("agent_start", () => reducer?.reduce("agent_start")); pi.on("agent_end", () => reducer?.reduce("agent_end")); pi.on("agent_settled", (_event, ctx) => reducer?.reduce("agent_settled", undefined, ctx.isIdle()));
  pi.on("tool_execution_start", event => reducer?.reduce("tool_execution_start", event.toolCallId)); pi.on("tool_execution_end", event => reducer?.reduce("tool_execution_end", event.toolCallId)); pi.on("session_before_compact", () => reducer?.reduce("session_before_compact")); pi.on("session_compact", () => reducer?.reduce("session_compact")); pi.on("session_compact_failed", () => reducer?.reduce("session_compact_failed")); pi.on("ui_prompt_start", () => reducer?.reduce("ui_prompt_start")); pi.on("ui_prompt_end", () => reducer?.reduce("ui_prompt_end"));
}
