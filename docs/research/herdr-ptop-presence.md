# Herdr and ptop presence

## Conclusion

Herdr v0.9.0 can already expose **workspace-level, non-agent metadata**. ptop can refresh a workspace token such as `ptop=running` with a short TTL when it runs inside Herdr. A Space sidebar row can render that token without claiming ptop is an agent.

This is possible today, but it is not visible with Herdr's default sidebar configuration. The default Space rows contain only agent state, workspace, branch, and Git status. The user must add `$ptop` to `ui.sidebar.spaces.rows`, or Herdr must add first-class/default UI support.

A pane metadata report can add a title and tokens, but pane tokens render only on existing Agent sidebar rows. It does not create a non-agent application row. Do not use `pane.report_agent` for ptop because that API changes semantic agent state, waits, notifications, and rollups.

## Documented facts

### 1. How Herdr decides an agent is running

- Herdr defines an agent as a process it recognizes in a pane, and says detection uses foreground processes, screen manifests, and optional integrations. [Concepts](https://github.com/herdrdev/herdr/blob/v0.9.0/docs/next/website/src/content/docs/concepts.mdx); [detector source](https://github.com/herdrdev/herdr/blob/v0.9.0/src/detect/mod.rs#L1-L8).
- The checked-in detector maps only its enumerated coding-agent labels (including `pi`, `claude`, and `codex`) to agents; an unrecognized process yields no agent identity and an unknown state. [agent list and lookup](https://github.com/herdrdev/herdr/blob/v0.9.0/src/detect/mod.rs#L35-L180); [unknown fallback](https://github.com/herdrdev/herdr/blob/v0.9.0/src/detect/mod.rs#L205-L218).
- Herdr injects `HERDR_ENV=1`, socket path, workspace ID, tab ID, and pane ID into processes it launches in managed panes. This lets an in-pane program identify its exact Herdr location without guessing. [Socket API](https://github.com/herdrdev/herdr/blob/v0.9.0/docs/next/website/src/content/docs/socket-api.mdx); [launch environment source](https://github.com/herdrdev/herdr/blob/v0.9.0/src/pane.rs#L137-L158).

### 2. Non-agent presentation metadata

- `pane.report_metadata` accepts a pane ID, source, optional `agent` guard, title, display agent, state labels, tokens, sequence, and TTL. Its schema makes `agent` optional. [schema](https://github.com/herdrdev/herdr/blob/v0.9.0/src/api/schema/panes.rs#L467-L505).
- Herdr documents this metadata as **display-only**: it can override title, displayed agent name, state labels, and tokens, but semantic state still controls waits, notifications, and rollups. Pane tokens can render as `$name` in Agent sidebar rows. [Socket API: Agent state reporting](https://github.com/herdrdev/herdr/blob/v0.9.0/docs/next/website/src/content/docs/socket-api.mdx).
- `workspace.report_metadata` accepts only tokens plus source, sequence, and TTL, and workspace responses expose them. Space sidebar rows can render these values through `$name` tokens. [workspace schema](https://github.com/herdrdev/herdr/blob/v0.9.0/src/api/schema/workspaces.rs#L38-L72); [metadata contract](https://github.com/herdrdev/herdr/blob/v0.9.0/docs/next/website/src/content/docs/socket-api.mdx#L770-L798); [Space row configuration](https://github.com/herdrdev/herdr/blob/v0.9.0/docs/next/website/src/content/docs/configuration.mdx#L353-L397).
- The default Space sidebar rows do not include a custom metadata token. Custom `$name` tokens appear only after the sidebar configuration names them. [default rows and token list](https://github.com/herdrdev/herdr/blob/v0.9.0/docs/next/website/src/content/docs/configuration.mdx#L353-L397); [custom token example](https://github.com/herdrdev/herdr/blob/v0.9.0/docs/next/website/src/content/docs/configuration.mdx#L447-L466).
- Pane and agent responses contain `workspace_id`, `tab_id`, `pane_id`, title/display fields, state labels, and tokens. Thus external clients can inspect the association even if the standard UI does not render a distinct application row. [pane response schema](https://github.com/herdrdev/herdr/blob/v0.9.0/src/api/schema/panes.rs#L528-L565); [agent response schema](https://github.com/herdrdev/herdr/blob/v0.9.0/src/api/schema/agents.rs#L135-L180).

**Inference:** a ptop-owned workspace token is the documented UI path that can surface non-agent presence. A ptop pane token is useful to API clients and possibly to an already-existing Agent row, but the sources do not establish that it creates a visible non-agent row.

### 3. Why `report-agent` is not accurate for ptop

- `pane.report_agent` requires `agent` and semantic `state`. [schema](https://github.com/herdrdev/herdr/blob/v0.9.0/src/api/schema/panes.rs#L440-L458).
- Herdr states that this state affects waits, notifications, and rollups; `agent.wait` observes semantic state. [Socket API: Agent state reporting and Waiting for state](https://github.com/herdrdev/herdr/blob/v0.9.0/docs/next/website/src/content/docs/socket-api.mdx).

**Inference:** reporting `agent: "ptop"` and `state: "working"` would deliberately put ptop into Herdr's agent model and could affect attention/coordination behavior. It is therefore misleading and should not be used merely to show that the monitor process exists. `report_metadata` is semantically safer, but should not set `display_agent` or state labels that make ptop look like an agent.

### 4. Cleanup, TTL, and exit behavior

- A metadata TTL is 1–86,400,000 ms. Omitting it leaves metadata until replacement, clearing, or pane/workspace closure. Token TTL is per updated key; token metadata is not restored after server restart. [Socket API: metadata TTL](https://github.com/herdrdev/herdr/blob/v0.9.0/docs/next/website/src/content/docs/socket-api.mdx); [schema bounds](https://github.com/herdrdev/herdr/blob/v0.9.0/src/api/schema/panes.rs#L491-L504).
- The token implementation removes entries whose deadline has passed; a later report without TTL cancels the old expiry for its updated key. [token implementation](https://github.com/herdrdev/herdr/blob/v0.9.0/src/metadata_tokens.rs#L34-L99).
- Herdr emits `workspace.metadata_updated` for workspace token changes and expiry. It emits `pane.exited` for pane process exit, but the documentation does not state that a process exit automatically removes metadata from an open pane. [Socket API: event subscriptions](https://github.com/herdrdev/herdr/blob/v0.9.0/docs/next/website/src/content/docs/socket-api.mdx).

**Inference:** use a renewable short TTL, for example 15–60 seconds, and refresh it while ptop is alive. A crash, kill, or lost socket then self-clears after the final refresh. Do not claim immediate disappearance on ptop exit; it is bounded by the TTL unless Herdr adds process-bound metadata.

Do not clear the shared `ptop` token on normal exit unless ptop first solves multiple-instance ownership. Herdr stores tokens by key, not by reporting source, so one exiting ptop process could clear a token still refreshed by another ptop process in the same workspace. [token storage and patch behavior](https://github.com/herdrdev/herdr/blob/v0.9.0/src/metadata_tokens.rs#L1-L65).

## Smallest viable integrations, ranked

1. **Existing Herdr API + ptop + one sidebar config change. Recommended near-term.** When `HERDR_ENV=1` and the injected workspace ID/socket are present, the interactive, non-demo ptop TUI refreshes `workspace.report_metadata` with source `ptop:presence`, token `{ "ptop": "running" }`, and a short TTL. Add `$ptop` to `ui.sidebar.spaces.rows`. This neither creates an agent nor changes agent state. Keep the reporter outside `Collector::collect`, preserve `--demo` as collector-free, and bound socket/CLI calls so a missing Herdr server cannot stall the TUI.
2. **ptop-only supplemental pane identification.** ptop emits an OSC terminal title or calls `pane.report_metadata` with a neutral title/token, not `display_agent`, agent state, or state labels. Herdr exposes terminal titles and pane metadata through `pane.get` and `pane.list`. **Limit:** this does not create a non-agent sidebar row.
3. **Herdr operational convention.** Launch or rename the ptop pane as `ptop`. Herdr exposes pane process information, IDs, labels, and workspace/tab membership. **Limit:** this is a label or inspection path, not a live running badge, and a pane label can outlive the process.
4. **Herdr change for an automatic default indicator.** Add a first-class non-agent presence resource or a built-in Space token tied to exact pane/process lifetime, then render it separately from agents. This is required for an out-of-box badge with immediate exit cleanup and no user sidebar configuration. ptop could report it, or Herdr could derive it from the foreground process.

## Verified on the installed build

On Herdr v0.9.0, a one-second `workspace report-metadata` probe added `ptop_probe=running` to this workspace's `workspace get` response. Two seconds later, the token was absent. Herdr's default Space rows do not render custom tokens.

After adding the configuration below, a live build from this branch published `ptop=running` to the exact workspace. After ptop exited, the token expired as designed.

A minimal user configuration is:

```toml
[ui.sidebar.spaces]
rows = [
  ["state_icon", "workspace", "$ptop"],
  ["branch", "git_status"],
]
```

## Open evidence limits
- Herdr v0.9.0 has no atomic guard connecting `workspace.report_metadata` to a pane. A pane move between ptop's validation and report commands can refresh the old workspace once; later validation failures stop refreshes, and the TTL bounds the stale indication.
- No reviewed source establishes automatic removal of pane metadata when only the foreground ptop process exits and the shell/pane remains alive. TTL is the verified fallback.
- No reviewed source provides a generic, non-agent “running application” semantic state or a process-bound metadata lease.

## Primary sources reviewed

- [Herdr v0.9.0 Socket API](https://github.com/herdrdev/herdr/blob/v0.9.0/docs/next/website/src/content/docs/socket-api.mdx): documented API behavior, metadata, TTL, events, and injected environment.
- [Herdr v0.9.0 Concepts](https://github.com/herdrdev/herdr/blob/v0.9.0/docs/next/website/src/content/docs/concepts.mdx): agent and pane model.
- [Herdr v0.9.0 detector](https://github.com/herdrdev/herdr/blob/v0.9.0/src/detect/mod.rs): recognized agent/process logic.
- [Herdr v0.9.0 API schemas](https://github.com/herdrdev/herdr/tree/v0.9.0/src/api/schema): wire-level fields and constraints.
- [Herdr v0.9.0 metadata token implementation](https://github.com/herdrdev/herdr/blob/v0.9.0/src/metadata_tokens.rs): expiry behavior.
