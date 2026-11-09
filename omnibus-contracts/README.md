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
  `omnibus-adapter-nm`); v1.5 adds `AppLauncher` (with `omnibus-adapter-launch`)
  and `ProcessTelemetry` (with `omnibus-adapter-proc`).
  `AppIntentsBridge` waits for its macOS implementation.
- `src/dto/` — flat named DTOs, each declared together with its generated JSON
  schema (`dto!` / `dto_enum!` / `dto_newtype!` macros — one declaration, no
  hand-written schema copy to drift). Newtypes for units (`Milliseconds`, `Bytes`,
  `Severity`, `Percent`, `Seconds`); opaque tokens in `src/tokens.rs`.
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
    let report = omnibus_contracts::suite::journal::journal_suite(
        || Box::new(MyJournal::configured_for_suite()),
        omnibus_contracts::suite::journal::JournalFixture::default(),
    ).await;
    assert!(report.is_clean(), "{report}");
}
```

The `AppLauncher` suite follows the same pattern (fixture: the known app your
backend lists and can launch; the optional `openable_target` gates `open` when
your stand declares one):

```rust
#[tokio::test]
async fn contract_suite_app_launcher() {
    let report = omnibus_contracts::suite::app_launcher::app_launcher_suite_with_fixture(
        || Box::new(MyLauncher::configured_for_suite()),
        omnibus_contracts::suite::app_launcher::AppLauncherFixture {
            known_app: String::from("suite-app.desktop"),
            openable_target: String::from("https://suite.invalid/"),
        },
    ).await;
    assert!(report.is_clean(), "{report}");
}
```

The `ProcessTelemetry` suite follows the same pattern (fixture: the known pid
your backend lists, snapshots, and signals successfully — the stand binds a
test process with the `ps.signal` grant wired; the sentinel pids
`SENTINEL_PID_MISSING` / `SENTINEL_PID_SLOW` / `SENTINEL_PID_POISON` in
`omnibus_contracts::suite::process_telemetry` carry the negative injections):

```rust
#[tokio::test]
async fn contract_suite_process_telemetry() {
    use omnibus_contracts::suite::process_telemetry::{ProcessTelemetryFixture, process_telemetry_suite_with_fixture};
    let report = process_telemetry_suite_with_fixture(
        || Box::new(MyProc::configured_for_suite()),
        ProcessTelemetryFixture {
            known_pid: 4242,
        },
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

`SessionId` / `ElementId` have no public value constructor — pinned by a
negative compile test (the `compile_fail` doctest in `src/tokens.rs`), so a
forged string cannot become a token value in any crate. Minting requires a
`MintKey`, whose only constructor is the checked `try_from_entropy` (the old
unchecked escape hatch is removed); the composition root is its only legitimate
caller. Rust cannot type-distinguish the composition root from an adapter, so
that half stays runtime: the core never shares its key, and the derived
`Deserialize` accepts arbitrary strings — a foreign, stale, or forged token
mints nothing, it simply fails to resolve, and the core answers `ElementStale`
(pinned end-to-end by the `forged_token_is_rejected` test in
`omnibus-core/tests/session_lifecycle.rs`). See `src/tokens.rs`.
