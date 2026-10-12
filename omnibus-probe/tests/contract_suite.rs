//! Contract suite binding for the platform probe (R2): `probe_suite` runs
//! against the real `Probe` over `SystemRunner`, mirroring the other adapters'
//! binding tests. The probe never fails a session — every observation degrades
//! to `None`/absent — so the suite runs on any Linux host and still gates the
//! report shape (registry-constant capability names, non-empty distro id,
//! floor rows carrying reasons when unmet).

use omnibus_contracts::spi::probe::PlatformProbe;
use omnibus_contracts::suite;
use omnibus_probe::Probe;
use omnibus_probe::runner::SystemRunner;
use std::sync::Arc;

#[tokio::test]
async fn probe_suite_green_on_the_real_probe() {
    let report = suite::probe::probe_suite(|| -> Box<dyn PlatformProbe> {
        Box::new(Probe::system(Arc::new(SystemRunner)))
    })
    .await;
    assert!(report.is_clean(), "probe suite red: {report}");
    // The suite must actually run: assert each normative gate is among the
    // passed checks, not silently skipped.
    for gate in [
        "report.adapter non-empty",
        "distro id present",
        "floors evaluated with reasons when unmet",
    ] {
        assert!(
            report.passed.iter().any(|name| name == gate),
            "probe suite gate '{gate}' did not run: {report}"
        );
    }
}
