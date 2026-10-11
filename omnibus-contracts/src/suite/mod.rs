//! The contract test suite harness (roadmap task 1.1.4, rule R2): the behavioral
//! set every trait implementation must pass before merge. Implementations bind the
//! suite with their own test binder and a [`SuiteFixture`] describing the
//! configured backend state; the harness compares behavior against the fixture and
//! returns a [`SuiteReport`] (never panics — the binder decides merge/red).
//!
//! Usage (in an adapter or a core test):
//!
//! ```ignore
//! let report = omnibus_contracts::suite::journal_suite(
//!     || Box::new(FakeJournal::configured()),
//!     JournalFixture::default(),
//! ).await;
//! assert!(report.is_clean(), "{report:?}");
//! ```
//!
//! Adding a trait implementation = implementing the trait + binding its suite; the
//! core never changes (specs/architecture/spi-registry.md "Anti-Patterns").

use std::fmt;

/// Outcome of one suite run: every check is recorded, failures do not abort.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SuiteReport {
    pub passed: Vec<String>,
    pub failed: Vec<String>,
}

impl SuiteReport {
    pub fn check(&mut self, name: &str, ok: bool) {
        if ok {
            self.passed.push(name.to_string());
        } else {
            self.failed.push(name.to_string());
        }
    }

    pub fn is_clean(&self) -> bool {
        self.failed.is_empty()
    }
}

impl fmt::Display for SuiteReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "suite: {} passed, {} failed",
            self.passed.len(),
            self.failed.len()
        )?;
        for name in &self.failed {
            writeln!(f, "  FAIL {name}")?;
        }
        Ok(())
    }
}

/// Validates that a report only names registry capabilities (CoN rule holds at the
/// seam, not just in the lint run).
pub fn check_report_shape(report: &crate::dto::probe::AdapterReport, out: &mut SuiteReport) {
    out.check("report.adapter non-empty", !report.adapter.is_empty());
    out.check(
        "report.contract_version matches CONTRACT_VERSION",
        report.contract_version == crate::version::CONTRACT_VERSION,
    );
    let all_known = report
        .capabilities
        .iter()
        .all(|c| crate::capabilities::ALL.contains(&c.name.as_str()));
    out.check("report capability names are registry constants", all_known);
}

pub mod a11y;
pub mod families;
pub mod journal;

pub use families::{audit_sink, files, kb, net, packages, portals, probe, services};
