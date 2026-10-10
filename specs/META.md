# META — Spec System Rules

This document is the foundation of the Omnibus spec system. Every other spec follows the
templates, naming rules, and update rules defined here. When a spec conflicts with META.md,
META.md wins; when a spec conflicts with the source design docs (`docs/development/omnibus-spec.md`,
`docs/development/omnibus-architecture.md`, `docs/development/omnibus-roadmap.md`), the source docs win
and the spec is a bug.

## Source of Truth Hierarchy

1. `docs/development/omnibus-spec.md` — product layer: decisions, tool surface, platform tiers, roadmap summary.
2. `docs/development/omnibus-architecture.md` — SPI architecture: crates, traits, capabilities, degradation, coupling.
3. `docs/development/omnibus-roadmap.md` — work plan: milestones MS1–MS3, phases, atomic tasks, acceptance criteria.
4. `specs/` — formal English specs derived from (1)–(3). Specs never introduce new product or architecture decisions.

## Language

All spec files are written in English. Tool names, capability names, crate names, and error names are
quoted verbatim from the source docs (they are ASCII identifiers). Russian prose from the sources is translated,
never copied.

## File Organization

```
specs/
├── META.md              # this file: rules, templates
├── INDEX.md             # navigation: task -> spec table, dependency graph, full listing
├── WORKFLOW.md          # how to change code/docs/specs; typical task workflows
├── architecture/        # system-level concerns (one file per concern)
├── domains/             # functional areas: one file per capability domain
├── contracts/           # boundaries between major components
├── decisions/           # ADRs: NNN-kebab-case-slug.md + _template.md
└── tools/               # validation: coverage matrix + validator script
```

## Templates

### Architecture Spec

```markdown
# [Topic]

## Context

Why this architectural aspect matters.

## [Main Content]

Diagrams, rules, descriptions. ASCII only (no Mermaid).

## Invariants

Rules that always hold. Affirmative phrasing.

## Anti-Patterns

What not to do, and why.
```

### Domain Spec

```markdown
# [Domain Name]

## Purpose

1-3 sentences: what this domain does in the system.

## Key Files

Planned crate paths (no code exists yet; every path is explicitly marked planned).

## Capability and Tools

Capabilities this domain declares and MCP tools built on it.

## Behavior

Contract operations, platform providers, semantics.

## Degradation

Behavior across the four degradation ladder states.

## Invariants

Properties that always hold. Affirmative phrasing.

## Related Specs

- [Spec Name](relative/path.md) - relationship context
```

### Contract Spec

```markdown
# Contract: [Side A] <-> [Side B]

## Boundary Rule

One sentence: dependency direction and what is forbidden.

## Interfaces

| Interface | Crate | Consumed By | Purpose |
| --------- | ----- | ----------- | ------- |

## Initialization

How components are wired at startup.

## Data Flow Across Boundary

What crosses, in what form, in which direction.

## Error Propagation

Wrapping/transform rules.

## Breaking Change Checklist

If you change X, you MUST also update Y.
```

### ADR

See [decisions/_template.md](decisions/_template.md). Statuses used: `Accepted`, `Proposed`
(Proposed = gated on an M0 verification fact; see the roadmap task that closes it).

## Conventions

- Files: `kebab-case.md`. ADRs: `NNN-kebab-case-slug.md` (three digits, never reused).
- Cross-references: relative markdown links (a square-bracket label, a parenthesised relative
  target); section anchors lowercase-hyphenated.
- Repo-root paths in prose: backticked. A path that does not exist yet is either inside a fenced
  "planned layout" code block or marked `(planned)` inline. Unmarked paths must exist.
- Invariants are affirmative ("The scheduler always X"), never negative-only phrasing.
- Tables for catalogs (tools, capabilities, platforms); ASCII for diagrams.
- Every spec ends with a `## Related Specs` or `## Source Docs` section linking the source material.

## Update Rules

- Changing product/architecture behavior: update the source design doc first, then the affected specs,
  then run `python3 specs/tools/validate_specs.py` (must exit 0).
- Adding a spec: follow vibespec-create; register it in `INDEX.md` and, if it covers a source-doc heading,
  add it to `specs/tools/coverage.toml`.
- ADRs: once `Accepted`, immutable; superseding creates a new ADR (see template).

## Validation

`python3 specs/tools/validate_specs.py` checks: required structure, INDEX completeness, link resolution,
coverage of every heading of the three source docs (matrix in `specs/tools/coverage.toml`), English-only
content, and path references (existing, or explicitly marked planned).
