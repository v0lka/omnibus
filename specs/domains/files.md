# Files

## Purpose

The files domain gives the agent scoped access to the file system: `fs_list`/`fs_read`/`fs_write` work strictly inside granted scopes, with `resolve` mapping paths into them. Reads inside the default scope are always available; everything beyond needs an explicit human-issued entry.

## Key Files

- `omnibus-contracts/src/spi/files.rs` - the `FileScope` trait; scope DTOs in `omnibus-contracts/src/dto/files.rs`, capability constants in `omnibus-contracts/src/capabilities.rs`
- `omnibus-adapter-fs-xdg/src/lib.rs` - Linux provider over XDG dirs: list/read and the write path; its end-to-end audit test lives in `omnibus-core/tests/scopes_grants.rs`
- `omnibus-adapter-fs-tcc/src/` (planned) - macOS provider (MS3) over TCC-granted folders

## Capability and Tools

| MCP tool | Capability | Scope |
| --- | --- | --- |
| `fs_list` / `fs_read` | `files.scope.read` | default read scope (Linux: all of `$HOME`) |
| `fs_write` | `files.scope.write` | [+scope] write grant required |

The probe additionally reports `files.scope.tcc` on macOS - the TCC grant state that gates reads at the OS level.

## Behavior

Trait `FileScope` operations: `list/read/write` in granted scopes, `resolve`.

```
fs_* tool --> FileScope trait --> provider
  Linux  fs-xdg: default read scope = the entire $HOME, dotfiles
                 included (decision of 2026-10-10); beyond $HOME -
                 an explicit scope entry only
  macOS  fs-tcc: TCC-granted folders (~/Documents and similar);
                 beyond them - Full Disk Access; a missing grant is
                 opt-in-required with the doctor wizard
```

- Grant entries live in the scope manifests — system templates `/etc/omnibus/scopes.d/*.toml` and the user store `omnibusd grant` writes (`…/scopes.d/90-grants.toml`); the list is a whitelist - anything not listed is denied.
- Reads outside the granted set return `ScopeDenied`; a denied write also emits an audit event.
- For writes, the scope model pins the path prohibitions (never-writable prefixes `/etc`, `/usr`, `/boot`, `/proc`, `/sys`, `/dev`, `/var`, plus the grant store and the daemon state dir; ordinary dotfiles are writable) and the size/time limits; tests lock them.
- On macOS, TCC is necessary but never sufficient: the core scope model applies on top, so a TCC-granted folder without a TOML entry still yields `ScopeDenied`.
- A missing TCC grant has status `opt-in-required`; doctor walks the human through the wizard (the human grants via GUI or MDM/PPPC).

## Degradation

- Degraded capability: not applicable - file access is granted or absent, with no partial values.
- Missing capability: `fs_*` tools are absent from `tools/list` when the probe reports no file provider; a direct call returns `CapabilityRequired` with a doctor hint.
- Below version floor: no version floor is defined for this subsystem.
- Grant not issued: on macOS, reads outside the TCC grants stay `opt-in-required` until the human grants access; until then the behavior equals the missing-capability state and the doctor hint explains the wizard.

## Invariants

- The default Linux read scope always covers exactly `$HOME` (dotfiles included), except the daemon's own directories - the grant store (`$XDG_CONFIG_HOME/omnibus/scopes.d` or `$HOME/.config/omnibus/scopes.d`) and the audit state directory (`$XDG_STATE_HOME/omnibus` or `$HOME/.local/state/omnibus`, or the root-anchored `/.local/state/omnibus` fallback when neither resolves) are never readable and never writable through the file tools; access beyond `$HOME` always requires an explicit scope entry.
- A write always requires `files.scope.write` plus a write grant; TCC presence alone never enables a write.
- Every file operation is always audited, reads included.
- TCC grants are always issued by a human (GUI or MDM/PPPC); no adapter or doctor flow ever attempts programmatic TCC.
- Outside a granted scope the agent always receives `ScopeDenied` with a working remedy command, never a raw OS error.

## Related Specs

- [Security Model](../architecture/security-model.md) - the grant store and the TCC-on-top rule.
- [Contract: Config and Grants](../contracts/config-and-grants.md) - manifest format.
- [Capability Model](../architecture/capability-model.md) - the `files.scope.*` constants and statuses.
- [SPI Registry](../architecture/spi-registry.md) - subsystem #6: `FileScope`.
- [Error Taxonomy](../architecture/error-taxonomy.md) - `ScopeDenied` semantics.
