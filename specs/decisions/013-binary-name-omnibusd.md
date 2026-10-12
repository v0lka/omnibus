# ADR-013: Shipped Binary Name Is `omnibusd`

## Status

Accepted

## Context

The daemon and its human CLI ship as one static binary named `omnibusd` (MS1 packaging installs
`/usr/bin/omnibusd`; `omnibusd grant` and `omnibusd doctor` are its subcommands). Several earlier
accepted records predate the name being pinned and write the grant/doctor commands as
`omnibus grant` / `omnibus doctor` (ADR-005, ADR-006, ADR-010). Accepted records are immutable
(specs/META.md), so their bodies keep the older spelling while the shipped system — and every
mutable spec — uses the new one.

## Decision

The shipped binary name is `omnibusd`. Wherever an earlier ADR or design text writes
`omnibus grant`, `omnibus optin`, or `omnibus doctor`, read `omnibusd grant`, `omnibusd optin`,
and `omnibusd doctor`: same command, same mechanism, corrected spelling. The decisions of ADR-005,
ADR-006, and ADR-010 stand unchanged; this record corrects only the command spelling in their
prose. As a runtime belt-and-braces measure, the `omnibusd doctor` CLI accepts the historical
`omnibus grant ...` spelling in stored ready-made commands and rewrites it to `omnibusd grant`
for display, `--apply` matching, and execution (`display_command` in
`omnibusd/src/doctor_cli.rs`); `omnibusd grant` itself parses only the capability name.

## Consequences

- One canonical spelling across the shipped system (`omnibusd`), with the erratum documented
  instead of silently diverging from the immutable records.
- A reader applying an earlier ADR verbatim gets a working command by following this record's
  rule.

## Alternatives Considered

- Superseding ADR-005/006/010 wholesale: rejected — their decisions (stdio-only topology,
  out-of-band consent, exactly one grant mechanism) remain in force; a supersession would falsely
  mark them replaced.
- Editing the accepted bodies in place: rejected — violates the immutability rule in
  specs/META.md (the only permitted edit is the supersession flip).
