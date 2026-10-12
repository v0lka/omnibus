# omnibus-contracts

The explicit seam of Omnibus (specs/architecture/layers-and-crates.md): SPI traits,
DTOs, capability constants, version floors, the error taxonomy — and the contract
test suite harness (roadmap rule R2). Zero OS dependencies; external dependencies
are restricted to the serde/async allowlist (`serde`, `serde_json`, `async-trait`,
`tokio`, `futures-core`, `thiserror`) and enforced by `omnibus-lints` (R4-NATIVE).

## Layout

- `src/spi/` — the traits. Declared just-in-time together with their first
  implementation (ADR-011). MS1: `DesktopA11y`, `Journal` + `AuditSink`,
  `ServiceManager`, `PackageManager`, `ScreenCapture` + `Notify`, `FileScope`,
  `KnowledgeBase`, `PlatformProbe`; MS2 adds `NetworkStatus` (with
  `omnibus-adapter-nm`). `AppIntentsBridge` waits for its macOS implementation.
- `src/dto/` — flat named DTOs, each declared together with its generated JSON
  schema (`dto!` / `dto_enum!` / `dto_newtype!` macros — one declaration, no
  hand-written schema copy to drift). Newtypes for units (`Milliseconds`, `Bytes`,
  `Severity`); opaque tokens in `src/tokens.rs`.
- `src/capabilities.rs` — the closed registry of capability name constants. Name
  literals anywhere else are an R4-CON lint violation.
- `src/errors.rs` — the closed eight-code taxonomy.
- `src/floors.rs` — version floor constants (systemd ≥ 252, AT-SPI2 per M0).
- `src/version.rs` — the contract semver, independent of code versions.
- `src/suite/` — the contract test suite (R2).

## Binding the suite to an implementation (how to add an adapter)

1. Implement the trait(s) in your adapter crate (adapters depend on
   `omnibus-contracts` only — R4-EDGES).
2. In the adapter's `tests/`, add a binder that constructs your implementation in
   the state the suite fixture prescribes (sentinel units/paths/apps for the
   poison/empty/slow injections; see `suite/families.rs` for the fixture fields).
3. Run the suite and require a clean report:

```rust
#[tokio::test]
async fn contract_suite() {
    let report = omnibus_contracts::suite::journal_suite(
        || Box::new(MyJournal::configured_for_suite()),
        omnibus_contracts::suite::journal::JournalFixture::default(),
    ).await;
    assert!(report.is_clean(), "{report}");
}
```

The suite never panics; it records every check and returns a `SuiteReport` so a
binder decides merge/red. CI runs every binder on every PR — a failing binder is a
merge block (R2, quality-gates.md).

## Adding a trait or a DTO

- A trait enters only together with its first implementation (just-in-time, no
  speculative stubs). Announce it in `specs/architecture/spi-registry.md` first —
  specs are the source of truth.
- Declare DTOs with the `dto!` family so the schema is derived from the same
  declaration serde uses; the typed round-trip guard in `tests/schema.rs` (sample
  from schema → serde → compare) fails the build on drift, while the in-file
  `dto/` tests cover Value-level round-trips.
- Bump `CONTRACT_VERSION` per semver: additive trait/capability/DTO = minor;
  removal or reshaping of a field, an error code, or the capability registry =
  major.

## Token minting model (honest enforcement statement)

`SessionId` / `ElementId` have no public value constructor — by review discipline
only, with no R4 fixture pinning it; the enforcement is runtime on both halves.
Minting requires a `MintKey` that the core builds from fresh per-process entropy
and never shares, and the derived `Deserialize` accepts arbitrary strings: a
foreign or stale token is not a constructor escape — it simply fails to resolve,
and the core answers `ElementStale`. See `src/tokens.rs`.
