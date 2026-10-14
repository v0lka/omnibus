# Knowledge

## Purpose

The knowledge domain gives the agent fix-it documentation per platform: search and page retrieval over distro docs and vendor guides. Diagnosis is universal, treatment is distro-aware (P7): the same failure leads to different articles on different distributions.

## Key Files

- `omnibus-contracts/src/spi/kb.rs` - the `KnowledgeBase` trait; article DTOs in `omnibus-contracts/src/dto/kb.rs`, capability constants in `omnibus-contracts/src/capabilities.rs`
- `omnibus-adapter-kb-archwiki/src/` - ArchWiki + Manjaro overlay provider (Arch, Manjaro)
- `omnibus-adapter-kb-guides/src/` - distro docs (Debian wiki, Ubuntu docs, Mint docs/forums, Fedora docs); Apple/SS64 guides join the same crate on macOS (MS3)
- `omnibus-core/src/doctor.rs` - consumes KB facts (adapter report/registry presence) among the subsystem facts doctor composes

## Capability and Tools

| MCP tool | Capability | Notes |
| --- | --- | --- |
| `wiki_search` | `kb.present` | hidden without an active KB provider |
| `wiki_page` | `kb.present` | returns the article content |

## Behavior

Trait `KnowledgeBase` operations: `search`, `page`.

| Provider | Sources | Platforms |
| --- | --- | --- |
| `kb-archwiki` | ArchWiki + Manjaro overlay | Arch (T1), Manjaro (T1) |
| `kb-guides` | Debian wiki, Ubuntu docs, Mint docs/forums, Fedora docs | other Linux targets (MS2) |
| `kb-guides` | Apple Support, SS64, community | macOS (MS3) |

- Adapters cache articles with rolling freshness; an offline run degrades honestly - stale cache content is marked as stale.
- The contract is identical across providers: the same suite passes against kb-archwiki and kb-guides without changes.
- The same diagnosis (a suspend failure) on Arch and Ubuntu leads to different articles - locked by an integration test on a machine pair.
- On macOS there is no canonical ArchWiki equivalent; community moderation makes content quality the weakest link of the macOS contract - documented as a known risk.
- v1 ships raw adapters plus caching only: no playbook engine and no applicability matrix. The playbook machinery is deliberately on the un-plan, with the revisit condition "the first real content pipeline that the agent-judges model cannot serve".

## Degradation

- Degraded capability: offline mode keeps the tools visible; stale articles are marked as stale in responses.
- Missing capability: `wiki_search`/`wiki_page` are absent from `tools/list` when the probe reports no KB provider; a direct call returns `CapabilityRequired` with a doctor hint.
- Below version floor: no version floor is defined for this subsystem.
- Grant not issued: not applicable - reading documentation needs no grant, and `capabilities()` always names the active provider.

## Invariants

- Article applicability is always judged by the agent (it has `capabilities()` and task context), never by runtime playbook machinery (P7).
- The `KnowledgeBase` contract is always identical across providers.
- The capability document always names the active provider.
- Stale cache content is always marked as stale.

## Related Specs

- [Principles](../architecture/principles.md) - P7: agent-judged applicability.
- [Non-goals](../architecture/non-goals.md) - the KB playbook engine on the un-plan.
- [SPI Registry](../architecture/spi-registry.md) - subsystem #7: `KnowledgeBase`.
- [Tool Registry](../architecture/tool-registry.md) - the `wiki_*` binding and the hiding rule.
- [Domain: Packages](packages.md) - the undo failure payload names the ALA/AUR archive; the actionable undo hints (which article, which URL) live in KB articles.
