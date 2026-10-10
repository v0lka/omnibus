# ADR-008: Polling over Push Events for Desktop State

## Status

Accepted

## Context

The original design had an a11y subscription (`subscribe`) with no specified delivery mechanism:
MCP request/response does not define how events reach an agent, push support across MCP clients is
heterogeneous, and a buffered polling channel (`desktop_events`, 1000 events / 1 h, cursors,
dedup) had no consumer - no demo or scenario reads the a11y change stream.

## Decision

No event channel in v1, neither push nor buffered polling. UI observation is polling
`desktop.tree`; watch scenarios use `journal_query` and `svc_status`. The channel returns
additively (new capability + tool) at the first scenario that polling demonstrably cannot serve.

## Consequences

- The tool surface carries only request/response semantics - every MCP client is served equally.
- The trait loses `poll_events`; watch machinery (AXObserver on macOS) leaves the roadmap.
- Watch-style agent patterns are expressed with existing tools.

## Alternatives Considered

- Buffered polling with cursors as the contract - rejected: a buffer, dedup, and TTL policy
  maintaining an instrument nobody calls; honest YAGNI cut.

## Source Docs

- `docs/development/omnibus-spec.md` section 7.
- `docs/development/omnibus-roadmap.md` un-plan section 4.4.
