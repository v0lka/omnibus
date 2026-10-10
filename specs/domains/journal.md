# Journal

## Purpose

The journal domain gives the agent read access to system logs: current boot, past boots, and live tailing. The same backends host the audit sink (`AuditSink` shares journald/unifiedlog), while the audit event schema always belongs to the core.

## Key Files

- `omnibus-contracts/` (planned) - `Journal` and `AuditSink` traits, the `Filter` DTO, capability constants
- `omnibus-adapter-journald/src/journal.rs` (planned) - Linux provider over the journald JSON interface
- `omnibus-adapter-unifiedlog/src/` (planned) - macOS provider (MS3) over `log show --predicate`
- `omnibus-core/src/tools/doctor.rs` (planned) - the opt-in persistence walkthrough
- `omnibus-probe/` (planned) - re-reads persistence and past-boot facts at every instance start

## Capability and Tools

| Capability | Semantics | MCP tool |
| --- | --- | --- |
| `journal.query` | present when a journal backend is active | `journal_query` |
| `journal.prev_boot` | the fact that past boots exist in the journal | `journal_query` (past-boot reads) |
| `journal.persistent` | storage setting surviving reboots; on volatile machines status `opt-in-required`, remedy `omnibus grant journal.persistent` | none (storage setting) |

Past-boot reads require `journal.prev_boot` in addition to `journal.query`.

## Behavior

Trait `Journal` operations: `query(Filter{unit, boot, severity, window})`, `tail`.

```
journal_query(Filter) --> Journal trait --> provider
  journald (Linux): JSON interface; all four filter fields supported
  unifiedlog (macOS, MS3): `log show --predicate`
    severity/window -> mapped onto predicate syntax
    Filter{unit}    -> typed Unsupported{reason} with a hint toward
                       severity/window/process
```

| Platform | Provider | Persistence |
| --- | --- | --- |
| Arch | journald | persistent out of the box (confirmed) |
| Manjaro | journald | persistent, subject to the M0 check (task 1.1.1) |
| Debian trixie / bookworm / bullseye | journald | default, M0 check; doctor enables where volatile |
| Ubuntu LTS / Mint | journald | volatile by default (confirmed); opt-in via doctor, then reboot |
| Fedora | journald | default, M0 check; doctor enables where volatile |
| macOS (MS3) | unifiedlog | persistent within retention (retention limits verified in phase F3.1) |

The Ubuntu/Mint opt-in path: doctor proposes `mkdir /var/log/journal` plus a journald restart; the human executes the mutation in the doctor CLI or by running the printed command. The next instance started after the doctor-CLI run sees `journal.persistent=true`; `journal.prev_boot` appears only after the first actual reboot that follows.

The audit pipeline reuses the same backends: `AuditSink` writes core-schema events to journald on Linux and to the unified log (subsystem/category `omnibus`) on macOS.

## Degradation

- Degraded capability: the capability document marks a volatile machine honestly - the example entry reports `journal.prev_boot` with `value: false` and the note "volatile: current boot only".
- Missing capability: past-boot reads stay unavailable until past boots exist in the journal; a direct call returns `CapabilityRequired` with a doctor hint.
- Below version floor: no version floor is defined for this subsystem (the floor list covers systemd, AT-SPI2, macOS, and portal versions).
- Grant not issued: on Ubuntu/Mint `journal.persistent` is `opt-in-required` until the human runs the doctor command; a declined opt-in keeps the read-only status quo without repeated nagging (frequency policy is tested).

## Invariants

- `journal.persistent` and `journal.prev_boot` are always reported as independent facts; neither is ever derived from the other.
- `journal.prev_boot` becomes true only after an actual reboot follows persistence enablement; a freshly persistent machine without reboots reports no past boots.
- Doctor always executes system mutations outside the agent's reach; the agent receives diagnosis plus a copy-paste command only.
- Every `journal_query` call always emits one audit event.
- Native backend failures always surface as `NativeFailure{code}` with the preserved payload.
- The audit event schema always belongs to the core; the sink stays swappable.

## Related Specs

- [SPI Registry](../architecture/spi-registry.md) - subsystem #2: `Journal` + `AuditSink`.
- [Capability Model](../architecture/capability-model.md) - the independent-capabilities rule and the document format.
- [Platform Matrix](../architecture/platform-matrix.md) - per-distro persistence facts.
- [Error Taxonomy](../architecture/error-taxonomy.md) - `Unsupported{reason}` for `Filter{unit}` on unifiedlog.
- [Security Model](../architecture/security-model.md) - opt-in grants and the `[optin]` manifest section.
