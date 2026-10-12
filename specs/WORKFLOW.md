# Specification Workflow

A reference guide for developers and AI agents on using this project's specification system.
Formats, templates, and rules live in [META.md](META.md); navigation in [INDEX.md](INDEX.md).

---

## 1. General Philosophy

Specs are the **source of truth** about intended behavior, derived from the design docs in
`docs/development/` (hierarchy in [META.md](META.md)). Key implications:

- A discrepancy between a spec and future code is a bug - in the code or in the spec; determine
  by context and fix the wrong side.
- A spec may describe code that does not exist yet: future crate paths are marked `(planned)`
  (or sit inside a fenced planned-layout block) and describe the target layout the roadmap
  builds toward; every unmarked path must already exist.
- Specs never introduce new product or architecture decisions; when a decision is needed, the
  source design doc changes first, then the specs.
- Organized by domains (functional areas), not by file layout; contracts are first-class.

## 2. Getting Started: Navigation

1. Open [INDEX.md](INDEX.md) - the task table maps your task to spec files.
2. Read the relevant `architecture/` spec for system rules that constrain your change.
3. Read the domain spec under `domains/` for the subsystem's behavior, degradation, invariants.
4. Cross-boundary work: read the `contracts/` file for the boundary you touch.
5. "Why is it like this?" - check `decisions/` for an ADR before concluding nothing documents it.

## 3. Document Formats

Five formats, all defined in [META.md](META.md):

1. Architecture spec - `architecture/*.md` (Context / main content / Invariants / Anti-Patterns)
2. Domain spec - `domains/*.md` (Purpose / Key Files / Capability and Tools / Behavior /
   Degradation / Invariants / Related Specs)
3. Contract - `contracts/*.md` (Boundary Rule / Interfaces / Initialization / Data Flow /
   Error Propagation / Breaking Change Checklist)
4. ADR - `decisions/NNN-kebab-case-slug.md` (Status / Context / Decision / Consequences /
   Alternatives Considered)
5. Root guides - META / INDEX / WORKFLOW

Every spec ends with `## Source Docs` and/or `## Related Specs`.

## 4. Cross-References

```markdown
<!-- to another spec, relative from specs/ -->
[Tool Registry](architecture/tool-registry.md)

<!-- to a section -->
[Degradation Ladder](architecture/capability-model.md#degradation-ladder-by-increasing-severity)

<!-- to source code or docs: backticked repo-root path -->
`omnibus-contracts/src/capability.rs` (planned)
```

Rules: links are relative from `specs/`; section anchors are lowercase, hyphenated; paths that do
not exist yet are either inside a fenced planned-layout block or marked `(planned)` inline.

## 5. Update Protocol

### When to update

- A source design doc changed -> update affected specs in the same change.
- An interface on a boundary changed -> update the contract and both sides' specs.
- A capability, tool, or trait was added/removed -> update the registries
  ([spi-registry](architecture/spi-registry.md), [tool-registry](architecture/tool-registry.md),
  [capability-model](architecture/capability-model.md)) plus the domain spec.
- A decision was made -> create an ADR.

### How to update

1. Read the current spec fully before modifying.
2. Preserve the template (sections and order) from [META.md](META.md).
3. Update cross-references when paths change.
4. Add/remove the file in [INDEX.md](INDEX.md); add a coverage row in
   [tools/coverage.toml](tools/coverage.toml) if it covers a source-doc heading.
5. Run `python3 specs/tools/validate_specs.py` - it must exit 0.

### ADR lifecycle

- Determine the next number in `decisions/`; numbers are never reused.
- `Proposed` marks a decision gated on a verification fact; the closing roadmap task converts it
  to `Accepted` (see [ADR-002](decisions/002-apt-backend.md), [ADR-004](decisions/004-unifiedlog-backend.md)).
- `Accepted` ADRs are immutable; superseding creates a new ADR and flips the old status to
  `Superseded by`.

## 6. Workflow for Typical Tasks

### "Implement a roadmap task"

1. Find the task in `docs/development/omnibus-roadmap.md`; read its What/How/AC.
2. Read the domain spec + contracts the task touches; honor the Invariants.
3. Implement; the AC section of the task is the acceptance bar.
4. Update specs if the task changed documented behavior; close `Proposed` ADRs the task gates.

### "Add a new MCP tool"

1. Read [tool-registry](architecture/tool-registry.md) (the registry is closed; adding a tool
   requires a capability constant and a trait first - roadmap rule R1).
2. Add the capability constant story to [capability-model](architecture/capability-model.md),
   the tool row to the registry, the behavior to the owning domain spec.
3. Update the source docs first if this is a product change (spec hierarchy in META).

### "Add an adapter for a new platform"

1. Read [spi-registry](architecture/spi-registry.md) (one adapter per subsystem; probe selects)
   and [platform-matrix](architecture/platform-matrix.md) (tier assignment, floors).
2. Implement against [core-adapters](contracts/core-adapters.md); extend the contract suite.
3. Update the platform matrix row and the domain spec's provider table.

### "Add or change a grant"

1. Read [security-model](architecture/security-model.md) and
   [config-and-grants](contracts/config-and-grants.md): one mechanism, whitelist default,
   no in-band consent ever ([ADR-006](decisions/006-consent-out-of-agent-band.md)).
2. Any new consent-adjacent idea starts by re-reading ADR-006 - if it relays consent through the
   agent, it is rejected by design.

### "Close an M0 open question"

1. The question lives in [risks-and-mitigations](architecture/risks-and-mitigations.md) with its
   closing roadmap task.
2. Record the fact, flip the affected `Proposed` ADR(s) to `Accepted`, adjust the platform matrix
   and domain degradation notes.

### "Change the spec system itself"

1. Edit [META.md](META.md) first; templates and rules propagate from it.
2. Keep [INDEX.md](INDEX.md) listing every file; keep the validator green.

## 7. Content Principles

- Invariants are affirmative ("The core always calls the trait"), never negative-only.
- Tables for catalogs (tools, capabilities, platforms, providers); ASCII for diagrams.
- English only; identifiers (tool/capability/crate/error names) are quoted verbatim from sources.
- No filler; every sentence carries information an agent can act on.

## 8. Anti-Patterns

| Anti-pattern | Why it is bad | Do instead |
|---|---|---|
| Inventing behavior absent from source docs | Specs must stay derivable and auditable | Omit it, or change the source doc first |
| Unmarked planned paths | A reader cannot tell existing from future files | Mark `(planned)` or use fenced layout blocks |
| Editing an Accepted ADR | Destroys decision history | New ADR + `Superseded by` |
| Skipping INDEX/coverage updates | Agents cannot find the spec; coverage matrix lies | Update both, run the validator |
| Negative-only invariants | Hard to verify compliance | Affirmative phrasing |
| Generating specs from code | Hides discrepancies | Specs first; code follows specs |

## 9. Quick Start (TL;DR)

1. Need context -> [INDEX.md](INDEX.md) -> task table -> spec.
2. Implementing -> domain spec + contract -> implement -> update specs -> validator green.
3. New decision -> source doc first -> ADR -> INDEX.
4. New spec file -> META template -> INDEX row -> coverage row -> validator green.
