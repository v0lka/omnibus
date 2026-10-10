# Error Taxonomy

## Context

One closed vocabulary of typed errors shared by every trait method. Native errors never leak across
the SPI: each adapter maps them into `NativeFailure{code}` while keeping the original message in the
payload for diagnosis.

## The Eight Codes

| Error | Meaning | Typical producer | Agent-visible remedy |
|---|---|---|---|
| `CapabilityRequired` | The tool exists but its capability is absent on this machine | hidden-tool direct call | doctor hint; the capability document explains |
| `ScopeDenied` | Capability present, write grant missing or the path is outside granted scopes | any [+scope] tool without a grant | one-line copy-paste grant command |
| `FloorUnsupported` | The platform is below the version floor | probe/enforcement (adapter not activated) | reason string, e.g. "systemd 247 < 252" |
| `AdapterUnavailable` | The subsystem has no active adapter | core registry | `capabilities()` names the gap |
| `NativeFailure{code}` | The OS backend failed; code + preserved payload | any adapter call | payload message for diagnosis |
| `ElementStale` | The referenced UI element no longer exists (UI changed) | `DesktopA11y` | re-read `desktop.tree` |
| `TimeoutError` | Budget exhausted (after the single read retry; writes are never retried) | core policy | adjust or abandon; limits visible via `capabilities()` |
| `Unsupported{reason}` | Operation is structurally impossible on this backend | example: `Filter{unit}` on unifiedlog | reason names alternatives (severity/window/process) |

## Mapping Rules

- Adapter methods always return contract errors; `anyhow`/`Box<dyn Error>` never appear at the
  boundary.
- D-Bus, ALPM, AX, and shell error codes become `NativeFailure{code}` inside the adapter; the payload
  preserves the original human-readable message.
- Authorization failures of the OS (polkit, launchd domains) surface as `NativeFailure` with an honest
  human-readable hint - Omnibus never intercepts system authorization.
- A hidden tool called by name returns `CapabilityRequired` (never "method not found"), so the agent
  can learn why.

## Invariants

- Every trait method's error type is always one of the eight codes (or a payload-carrying
  `NativeFailure`).
- A permission problem always yields `ScopeDenied` with a working remedy command, never a raw OS error.
- The taxonomy is always closed: adding a code is an additive (minor) contract change with a new
  constant in `omnibus-contracts`.

## Anti-Patterns

- String-matching error text to branch on failure mode - branch on the typed code.
- Swallowing a native error into a generic "operation failed" without the payload.

## Source Docs

- `docs/development/omnibus-architecture.md` §3.7 (taxonomy), §4 (mapping rules).
- `docs/development/omnibus-roadmap.md` tasks 1.2.3, 3.3.1 (`Unsupported` example).

## Related Specs

- [Capability Model](capability-model.md) - states that produce `CapabilityRequired`/`FloorUnsupported`.
- [Security Model](security-model.md) - the `ScopeDenied` remedy principle.
