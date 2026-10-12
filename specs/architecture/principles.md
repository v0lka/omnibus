# Principles (P1-P7)

## Context

Seven immutable principles from `docs/development/omnibus-architecture.md` (§1) govern every design
decision in Omnibus. Each spec and each roadmap task traces back to at least one of them. The table
below is the normative restatement; consequences bind the architecture.

## The Principles

| # | Principle | Architectural consequence |
|---|-----------|---------------------------|
| P1 | Contract-first, OS-second | MCP tools are defined by the core contract, not by what a specific OS offers; the OS supplies implementations behind the explicit SPI boundary. |
| P2 | Semantics, not pixels | Reading is semantic (a11y trees, logs, unit status); writing is typed (`desktop.set_value`, `desktop.invoke`). Input emulation (ydotool/uinput, CGEvent) is out of v1; a "last resort" opt-in returns only on proven demand. |
| P3 | Read/write asymmetry | Reading is always allowed (read is audited); writing happens only through typed methods and only under grants issued by a human out of band (TOML config, `omnibusd grant`, doctor CLI). |
| P4 | `capabilities()` is the single source of truth | Neither core nor agent "knows the distro"; all behavior derives from the machine-readable capability report. Tools with missing capabilities are hidden, not runtime-erroring. |
| P5 | Audit is total | Every agent action emits one structured event; the event schema belongs to the core, the sink is a swappable adapter (`AuditSink`). |
| P6 | Adapters never conspire | An adapter never calls another adapter; tool composition (doctor) is core responsibility. |
| P7 | Diagnosis is universal, treatment is distro-aware | Fix-it knowledge lives in knowledge bases (distro docs); article applicability is judged by the agent (it has `capabilities()` and task context), never by core playbook machinery. |

## Zero Inference (derived invariant of P1/P4)

`omnibusd` performs zero model calls - local or cloud. Triage, summarization, and all semantic work
happen in the user's agent, where the user's policies can observe them. The daemon creates no shadow
control plane around agent policies and adds no inference latency. Embeddings-for-search are the only
theoretical remainder and would be an optional adapter, not core behavior.

## Read/Write Asymmetry in Practice

- Read tools: no grant, always audited. Default read scope on Linux is all of `$HOME` (including
  dotfiles); outside `$HOME` only by explicit scope. On macOS reads are gated by TCC grants.
- Write tools: capability + write grant, both required. The grant list is a whitelist; anything not
  listed is denied. `screenshot` is the one accepted read risk on Linux (X11 path has no system gate)
  - see [security-model.md](security-model.md).
- Grants live only in config; there is no runtime consent API (see [security-model.md](security-model.md)).

## Tier vs Capabilities (derived rule)

A tier (T1/T2/T3) is a promise-and-testing category; `capabilities()` is machine truth about one
machine. They diverge in the wild (a machine with a volatile-configured journald reports `journal.persistent`
as `opt-in-required` and keeps `journal.prev_boot` under the ladder until the doctor-led opt-in
completes — the grant plus the manual persistent-storage creation and a reboot; see journal.md —
although its tier promises full reading) and `capabilities()` always wins in a dispute.

## Invariants

- Every roadmap task always maps to at least one principle (see
  [quality-gates.md](quality-gates.md) for R1-R7 enforcement).
- The never-list is always identical across source docs and specs (one source:
  `docs/development/omnibus-architecture.md` §7.4, restated in [non-goals.md](non-goals.md)).

## Anti-Patterns

- "Just ask the user" flows through the agent - in-band consent is forgeable and therefore forbidden.
- Distro-specific `if` chains in the core - distro knowledge belongs in adapters (P1) and knowledge
  bases (P7).
- Silent feature assumptions - an agent must never need to assume; it must always be able to read the
  answer from `capabilities()`.

## Source Docs

- `docs/development/omnibus-spec.md` (decision list, §2 architecture).
- `docs/development/omnibus-architecture.md` §1 (principles), §7.5 (tier vs capabilities).

## Related Specs

- [Non-goals](non-goals.md) - the never-list and the un-plan.
- [Quality Gates](quality-gates.md) - how the principles are mechanically enforced.
