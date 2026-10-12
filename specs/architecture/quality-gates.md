# Quality Gates

## Context

The roadmap enforces seven cross-cutting rules (R1-R7, `docs/development/omnibus-roadmap.md` §0.2)
that turn the architecture's promises into mechanical checks. This spec is the normative index of
those gates: what each rule demands, where it is enforced, and which CI jobs carry it. A task or PR
that bypasses a gate is rejected in review, and the automated subset fails the build.

## The Seven Rules

| # | Rule | Demand | Enforcement |
|---|---|---|---|
| R1 | Contract before code | A tool appears in the tool list only after its capability constants and trait are declared in `omnibus-contracts`, and the contract test suite can check an implementation | Registry closed over the roadmap task list; CI lint: capability name literals outside `omnibus-contracts` fail the build |
| R2 | Contract test suite is the gate | A trait implementation is accepted only with a green behavioral suite run against it (`omnibus-architecture.md` §8.8.1) - the mechanical guarantee of "new adapter, zero core changes" | Suite runs in CI per adapter; red suite = merge blocked |
| R3 | Degradation follows the ladder | Every write tool and platform-dependent feature defines behavior in all four states: degraded / hidden (`CapabilityRequired`) / below-floor (`FloorUnsupported`) / opt-in-required | Ladder integration test (roadmap task 1.3.5) runs in CI on a simulated environment |
| R4 | Negative checks in CI | Boundary prohibitions are enforced automatically: adapter -> adapter imports, adapter -> `omnibus_core` internals, capability name literals, tuple trait signatures, native types in `omnibus-contracts` (roadmap 1.1.3; `omnibus-architecture.md` §8.8.2) | Dependency-rules CI job; a violating PR does not build |
| R5 | Audit is total (P5) | No write path is considered done without a structured AuditSink event: `{ts, session, actor, tool, adapter, op, params_hash, result, scope_ref}` | Audit-event assertions in the write-path test suites; read paths assert one event per call |
| R6 | Never-list (`omnibus-architecture.md` §7.4, single source) | Write actions outside human-issued out-of-band grants; model calls from `omnibusd`; a consent broker with UI and any in-band confirmations; switching the network backend; systemd backports; changing system security policies (programmatic TCC issuance) | Review rejects any task that plans one; the doctor consent-denial test (security.rs) asserts every doctor proposal is human-executed and scans proposals against the §7.4 list |
| R7 | Product honesty | Acceptance criteria never promise more than the platform tier: apt has no undo; a volatile journal yields `prev_boot` only after opt-in; a non-working adapter means the tool is hidden | Release notes and tier claims checked against [platform-matrix.md](platform-matrix.md) |

## Security CI Tests (derived from R5/R6)

These negative tests are part of the core suite and must stay red on violation:

- An agent that sends a "user confirmed" token/field gets the action denied as grant-less - no MCP
  method accepts consent (see [security-model.md](security-model.md)).
- `doctor` over MCP never mutates the system regardless of the agent's answers - a "consenting"
  agent still receives diagnosis plus a ready command only.
- Every grant-related audit event carries provenance `config|cli|doctor`.
- An `element_id` issued by one instance never resolves in another instance (typed error across two
  sequential instances).
- Two parallel write instances apply in lock-acquisition order while the cross-process gate is
  healthy; if the lock file cannot be created the gate degrades to process-local only (doctor
  reports the degradation), and a write that outlives its budget releases the gate while the
  native operation may still complete. Writes run under exactly one timeout layer — the
  adapter's own, at or below the core's read budget — and the elapse answer is adapter-defined:
  a payload-bearing indeterminate `NativeFailure` where the adapter can name the
  possibly-still-running native write (ALPM code 6 `timeout-indeterminate`, fs-xdg code -2, the
  systemd1 indeterminate-job payload), a plain `TimeoutError` elsewhere; the audit records
  whichever honest outcome the serving adapter produced, never a completion order.

## Suite Tiers per Platform

- T1 platforms (Arch, Manjaro): full suite on every core and adapter release.
- T2 platforms: smoke suite on reference versions (see [platform-matrix.md](platform-matrix.md)).
- T3 platforms: build only; `capabilities()` reports honestly, no guarantees.

The onboarding autotest (TTFM under 5 minutes) runs per milestone release (roadmap 1.8, 2.6, 3.7).

A milestone ships only when its target platforms are green at their tier level, the milestone demo
smoke set passes (MS1: demos 1-3; from MS2: demos 1-4), and the onboarding autotest succeeds on a
fresh VM installed from the native package - the milestone-is-shippable gate (roadmap §0.1; DoD
1.8.3, 2.6.3, 3.7.3).

## Invariants

- Every roadmap task always maps to at least one principle and respects all seven rules.
- The contract suite always gates every adapter, including fallback implementations.
- The negative CI set always includes the boundary prohibitions (R4) and the security tests above.

## Anti-Patterns

- Treating a green build as sufficient without the suite - R2 is behavioral, not compilation.
- "Temporary" manual merges with a red dependency-rules job - the gate has no override.
- Writing an AC that a tier cannot satisfy (R7) - fix the claim, not the test.

## Source Docs

- `docs/development/omnibus-roadmap.md` §0.2 (R1-R7), tasks 1.3.2, 1.3.3, 1.7.1.
- `docs/development/omnibus-architecture.md` §8.8.1-§8.8.2 (suite gate, lint list).

## Related Specs

- [Principles](principles.md) - what the rules enforce.
- [Non-goals](non-goals.md) - the never-list behind R6.
- [Evolution and Compat](evolution-and-compat.md) - additive-only growth the gates protect.
