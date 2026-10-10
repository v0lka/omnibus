# ADR-012: No Playbook Engine in the Knowledge Domain

## Status

Accepted

## Context

The MS2 draft specified a TOML playbook format with applicability matrices, versioning, and
deprecation rules for distro guides - a mini content-management platform before any content or
contributor pipeline exists. Meanwhile the agent, by construction, can do what a playbook runtime
would do: it reads `wiki_page`, knows the platform from `capabilities()`, and judges applicability
in its own reasoning.

## Decision

v1 ships raw knowledge adapters only: `search` / `page` over the same `KnowledgeBase` contract
(kb-archwiki, kb-guides) plus caching with honest staleness marking. No playbook format, no
applicability machinery, no `kb.playbooks.*` capabilities. The engine may return additively when a
real content pipeline and contributors exist.

## Consequences

- MS2 knowledge work is an adapter plus a cache, not a content platform.
- Applicability quality is the agent's responsibility - consistent with P7 (distro-aware
  treatment lives in the agent + adapter pair, not in core runtime).
- The treatment path for demos runs on plain wiki articles.

## Alternatives Considered

- Ship the TOML playbook engine for structured distro guidance - rejected: content-platform
  machinery before content; the agent layer already performs the judgment.

## Source Docs

- `docs/development/omnibus-architecture.md` section 4 (knowledge base row).
- `docs/development/omnibus-roadmap.md` tasks 2.5.2, 3.6.1, un-plan section 4.4.
