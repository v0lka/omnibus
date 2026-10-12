//! `Journal` contract suite: filter semantics, poison and timeout mapping, empty
//! results, report shape. Prev-boot behavior has no fixture-independent check
//! here: whether a previous boot exists is a per-machine fact the trait signature
//! (`Result<_, ContractError>`) and the core's capability gating already type.

use crate::dto::journal::{BootSelector, JournalQuery, TailRequest};
use crate::errors::ContractError;
use crate::spi::journal::Journal;

use super::{SuiteReport, check_report_shape};

/// The backend state the implementation was configured with.
#[derive(Debug, Clone)]
pub struct JournalFixture {
    /// A unit whose query must fail with `NativeFailure` (poison input).
    pub poison_unit: String,
    /// A unit whose query must exceed the budget and yield `TimeoutError`.
    pub slow_unit: String,
    /// A unit with known records present.
    pub known_unit: String,
    /// Whether the backend actually holds records for [`JournalFixture::known_unit`].
    /// Only then can the suite demand a non-empty filtered page: a binder that
    /// seeds no records must leave this `false` (the default — "the unit filter
    /// drops everything" and "the unit has no records" are indistinguishable
    /// through the API), otherwise the filter gate below is declared, not silent.
    pub known_unit_seeded: bool,
    /// A unit with no records: empty page, not an error.
    pub empty_unit: String,
}

impl Default for JournalFixture {
    fn default() -> Self {
        Self {
            poison_unit: String::from("omnibus-suite-poison.service"),
            slow_unit: String::from("omnibus-suite-slow.service"),
            known_unit: String::from("omnibus-suite-known.service"),
            known_unit_seeded: false,
            empty_unit: String::from("omnibus-suite-empty.service"),
        }
    }
}

/// Runs the full `Journal` behavioral set against `make`.
pub async fn journal_suite<F>(make: F, fx: JournalFixture) -> SuiteReport
where
    F: Fn() -> Box<dyn Journal> + Send + Sync,
{
    let mut out = SuiteReport::default();

    {
        let j = make();
        check_report_shape(&j.report(), &mut out);
    }

    // Happy path: current-boot query returns a well-formed page.
    let j = make();
    let page = j
        .query(JournalQuery {
            unit: None,
            boot: Some(BootSelector::Current),
            severity_at_least: None,
            since_epoch_ms: None,
            until_epoch_ms: None,
            limit: Some(50),
        })
        .await;
    match page {
        Ok(p) => {
            out.check("query happy path is Ok", true);
            out.check(
                "records sorted by ts",
                p.records
                    .windows(2)
                    .all(|w| w[0].ts_epoch_ms <= w[1].ts_epoch_ms),
            );
            out.check("limit respected", p.records.len() <= 50);
        }
        Err(e) => {
            out.check("query happy path is Ok", false);
            out.failed.push(format!("  detail: {e}"));
        }
    }
    drop(j);

    // Known unit: filter returns only that unit.
    let j = make();
    let page = j
        .query(JournalQuery {
            unit: Some(fx.known_unit.clone()),
            boot: None,
            severity_at_least: None,
            since_epoch_ms: None,
            until_epoch_ms: None,
            limit: None,
        })
        .await;
    match page {
        Ok(p) => {
            let only_known = p
                .records
                .iter()
                .all(|r| r.unit.as_deref() == Some(fx.known_unit.as_str()));
            out.check("unit filter returns only the requested unit", only_known);
            // Real gate against the dropped-filter bug class: when the fixture
            // declares the unit seeded, an empty page must fail the suite.
            if fx.known_unit_seeded {
                out.check(
                    "unit filter returns a non-empty page for a seeded unit",
                    !p.records.is_empty(),
                );
            }
        }
        Err(e) => out.failed.push(format!("  known unit query failed: {e}")),
    }
    drop(j);

    // Empty unit: Ok with zero records, not an error.
    let j = make();
    let empty = j
        .query(JournalQuery {
            unit: Some(fx.empty_unit.clone()),
            boot: None,
            severity_at_least: None,
            since_epoch_ms: None,
            until_epoch_ms: None,
            limit: None,
        })
        .await;
    match empty {
        Ok(p) => out.check(
            "empty unit yields Ok page with no records",
            p.records.is_empty(),
        ),
        Err(e) => out.failed.push(format!("  empty unit must not error: {e}")),
    }
    drop(j);

    // Poison input maps to NativeFailure.
    let j = make();
    let poisoned = j
        .query(JournalQuery {
            unit: Some(fx.poison_unit.clone()),
            boot: None,
            severity_at_least: None,
            since_epoch_ms: None,
            until_epoch_ms: None,
            limit: None,
        })
        .await;
    out.check(
        "poison input maps to NativeFailure",
        matches!(poisoned, Err(ContractError::NativeFailure { .. })),
    );
    drop(j);

    // Slow input maps to the typed timeout.
    let j = make();
    let slow = j
        .query(JournalQuery {
            unit: Some(fx.slow_unit.clone()),
            boot: None,
            severity_at_least: None,
            since_epoch_ms: None,
            until_epoch_ms: None,
            limit: None,
        })
        .await;
    out.check(
        "slow input maps to TimeoutError",
        matches!(slow, Err(ContractError::TimeoutError)),
    );
    drop(j);

    // Tail is bounded.
    let j = make();
    let tail = j.tail(TailRequest { limit: 2 }).await;
    match tail {
        Ok(records) => out.check("tail respects the limit", records.len() <= 2),
        Err(e) => out.failed.push(format!("  tail failed: {e}")),
    }
    drop(j);

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::journal::{JournalPage, JournalRecord};
    use crate::dto::probe::AdapterReport;
    use std::sync::Arc;
    use std::sync::Mutex;

    /// In-memory reference implementation used by the harness's own tests.
    /// `mangled_unit_filter` models the dropped-filter empty-page bug class
    /// (issue #61): every unit-filtered query comes back as an empty page
    /// while records for the unit exist.
    struct FakeJournal {
        records: Arc<Mutex<Vec<JournalRecord>>>,
        mangled_unit_filter: bool,
    }

    impl FakeJournal {
        fn seeded_records() -> Arc<Mutex<Vec<JournalRecord>>> {
            let fx = JournalFixture::default();
            let mut records = Vec::new();
            for i in 0..3 {
                records.push(JournalRecord {
                    ts_epoch_ms: i64::from(i),
                    boot_id: Some(String::from("b0")),
                    unit: Some(fx.known_unit.clone()),
                    severity: Some(crate::dto::Severity(6)),
                    message: format!("record {i}"),
                });
            }
            Arc::new(Mutex::new(records))
        }

        fn configured() -> Box<dyn Journal> {
            Box::new(Self {
                records: Self::seeded_records(),
                mangled_unit_filter: false,
            })
        }
    }

    #[async_trait::async_trait]
    impl Journal for FakeJournal {
        fn report(&self) -> AdapterReport {
            AdapterReport {
                adapter: String::from("fake-journal"),
                subsystem: String::from("journal"),
                contract_version: String::from(crate::version::CONTRACT_VERSION),
                capabilities: vec![],
                notes: vec![],
            }
        }

        async fn query(&self, filter: JournalQuery) -> Result<JournalPage, ContractError> {
            if self.mangled_unit_filter && filter.unit.is_some() {
                return Ok(JournalPage {
                    truncated: false,
                    records: Vec::new(),
                });
            }
            let unit = filter.unit.unwrap_or_default();
            if unit == "omnibus-suite-poison.service" {
                return Err(ContractError::NativeFailure {
                    code: 1,
                    payload: String::from("poisoned"),
                });
            }
            if unit == "omnibus-suite-slow.service" {
                return Err(ContractError::TimeoutError);
            }
            let guard = self
                .records
                .lock()
                .map_err(|_| ContractError::AdapterUnavailable)?;
            let records: Vec<JournalRecord> = guard
                .iter()
                .filter(|r| unit.is_empty() || r.unit.as_deref() == Some(unit.as_str()))
                .take(usize::try_from(filter.limit.unwrap_or(u32::MAX)).unwrap_or(usize::MAX))
                .cloned()
                .collect();
            Ok(JournalPage {
                truncated: false,
                records,
            })
        }

        async fn tail(&self, request: TailRequest) -> Result<Vec<JournalRecord>, ContractError> {
            let guard = self
                .records
                .lock()
                .map_err(|_| ContractError::AdapterUnavailable)?;
            let take = usize::try_from(request.limit).unwrap_or(0);
            Ok(guard.iter().rev().take(take).cloned().collect())
        }
    }

    #[tokio::test]
    async fn harness_passes_against_the_reference_fake() {
        // The fake seeds records for known_unit, so the fixture declares them and
        // the non-empty filter gate below actually runs.
        let fx = JournalFixture {
            known_unit_seeded: true,
            ..JournalFixture::default()
        };
        let report = journal_suite(FakeJournal::configured, fx).await;
        assert!(report.is_clean(), "{report}");
    }

    #[tokio::test]
    async fn harness_rejects_an_empty_filtered_page_for_a_seeded_unit() {
        // The dropped-filter bug class the seeded gate exists for: the unit
        // filter mangles into a no-match, the filtered page comes back empty
        // while records for the unit exist — the vacuous all()-over-empty
        // (issue #61) stayed green here, the non-empty gate must not.
        let fx = JournalFixture {
            known_unit_seeded: true,
            ..JournalFixture::default()
        };
        let report = journal_suite(
            || {
                Box::new(FakeJournal {
                    records: FakeJournal::seeded_records(),
                    mangled_unit_filter: true,
                })
            },
            fx,
        )
        .await;
        assert!(
            report
                .failed
                .iter()
                .any(|f| f.contains("non-empty page for a seeded unit")),
            "the seeded non-empty gate must catch the empty filtered page: {report}"
        );
    }

    #[tokio::test]
    async fn harness_detects_a_broken_implementation() {
        struct Broken;
        #[async_trait::async_trait]
        impl Journal for Broken {
            fn report(&self) -> AdapterReport {
                AdapterReport {
                    adapter: String::new(),
                    subsystem: String::from("journal"),
                    contract_version: String::from("0.0.0"),
                    capabilities: vec![],
                    notes: vec![],
                }
            }
            async fn query(&self, _filter: JournalQuery) -> Result<JournalPage, ContractError> {
                Err(ContractError::AdapterUnavailable)
            }
            async fn tail(
                &self,
                _request: TailRequest,
            ) -> Result<Vec<JournalRecord>, ContractError> {
                Err(ContractError::AdapterUnavailable)
            }
        }
        let report = journal_suite(|| Box::new(Broken), JournalFixture::default()).await;
        assert!(!report.is_clean());
    }
}
