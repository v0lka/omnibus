# Architecture decision records (pointer)

The authoritative home of omnibus ADRs is `specs/decisions/` (see
`specs/META.md`: ADRs are numbered `NNN-` + kebab-case slug, `Accepted`
records are immutable, and supersession happens by a new numbered ADR — never
by editing the old one).

- **ADR-001 — Dynamic Trait SPI for the adapter boundary**: normative record
  at [specs/decisions/001-dyn-trait-spi.md](../specs/decisions/001-dyn-trait-spi.md).
  In short: SPI traits are trait objects (`dyn Trait`) held in the
  probe-assembled registry; adapters are compile-time workspace members
  instantiated exactly once.

This `docs/` page used to carry a full copy of ADR-001; the copy is gone so
the two records cannot drift. Two facts the copy used to state, corrected
here because they are load-bearing for the crate map:

- The composition root is **the `omnibusd` binary crate** (`omnibusd/`,
  assembled in `omnibusd/src/wiring.rs`): it links the adapter crates and
  builds the registry once, from the probe report. It is *not* "the
  `omnibusd` binary target in `omnibus-core`" — an earlier revision of the
  copy here said exactly that, which the R4-EDGES lint now forbids
  (`omnibus-core` links no adapter crates; the thin `omnibusd` crate exists
  so the composition root has somewhere to live).
- Everything else in ADR-001 (context, decision, consequences, alternatives)
  is normative in `specs/decisions/001-dyn-trait-spi.md`; where wording
  differs, that file wins.
