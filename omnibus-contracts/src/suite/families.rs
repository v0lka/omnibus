//! `ServiceManager`, `PackageManager`, `ScreenCapture`, `Notify`, `FileScope`,
//! `KnowledgeBase`, `PlatformProbe`, `NetworkStatus`, `AuditSink` contract suites.
//! Kept in one module per family below; each follows the same fixture pattern as
//! the Journal suite (fixture describes configured backend state, harness asserts).

use crate::errors::ContractError;

use super::{SuiteReport, check_report_shape};

/// Common poison/slow sentinels shared by the suites (adapters bind these names to
/// their configured failure injections).
pub const SENTINEL_POISON: &str = "omnibus-suite-poison";
pub const SENTINEL_MISSING: &str = "omnibus-suite-missing";
pub const SENTINEL_SLOW: &str = "omnibus-suite-slow";

pub mod services {
    //! ServiceManager suite.

    use super::*;
    use crate::dto::services::{ServiceAction, ServiceActionRequest, ServiceFilter, ServiceUnit};
    use crate::spi::services::ServiceManager;

    /// The backend state the implementation was configured with. The
    /// fixture-free [`services_suite`] cannot verify a known unit; binders that
    /// configure one must bind [`services_suite_with_fixture`].
    #[derive(Debug, Clone, Default)]
    pub struct ServicesFixture {
        /// A unit the backend reports as present: `status` must answer Ok,
        /// name the unit, and carry an active state.
        pub known_unit: String,
    }

    async fn services_core<F>(make: &F) -> SuiteReport
    where
        F: Fn() -> Box<dyn ServiceManager> + Send + Sync,
    {
        let mut out = SuiteReport::default();
        {
            let s = make();
            check_report_shape(&s.report(), &mut out);
        }
        let s = make();
        let list = s
            .list(ServiceFilter {
                active_only: None,
                name_pattern: None,
                limit: Some(200),
            })
            .await;
        match list {
            Ok(units) => {
                out.check("list respects the limit", units.len() <= 200);
                out.check(
                    "units carry a name and an active state",
                    units
                        .iter()
                        .all(|u: &ServiceUnit| !u.name.is_empty() && !u.active_state.is_empty()),
                );
            }
            Err(e) => out.failed.push(format!("  list failed: {e}")),
        }
        let status = s.status(String::from(SENTINEL_MISSING)).await;
        out.check(
            "unknown unit is a contract error",
            matches!(
                status,
                Err(ContractError::NativeFailure { .. }
                    | ContractError::Unsupported { .. }
                    | ContractError::AdapterUnavailable)
            ),
        );
        let slow = s.status(format!("{SENTINEL_SLOW}.service")).await;
        out.check(
            "slow status maps to TimeoutError",
            matches!(slow, Err(ContractError::TimeoutError)),
        );
        let denied = s
            .apply(ServiceActionRequest {
                unit: String::from(SENTINEL_POISON),
                action: ServiceAction::Restart,
            })
            .await;
        out.check(
            "polkit denial maps to a contract error",
            matches!(
                denied,
                Err(ContractError::NativeFailure { .. } | ContractError::ScopeDenied)
            ),
        );
        drop(s);
        out
    }

    /// Runs the fixture-free `ServiceManager` behavioral set against `make`.
    ///
    /// Fixture-free by signature and by honesty: no unit name exists on every
    /// machine, so this entry point cannot gate a known unit. Binders that
    /// configure the state the fixture prescribes must bind
    /// [`services_suite_with_fixture`], which gates it for real.
    pub async fn services_suite<F>(make: F) -> SuiteReport
    where
        F: Fn() -> Box<dyn ServiceManager> + Send + Sync,
    {
        services_core(&make).await
    }

    /// [`services_suite`] plus the fixture gate: `status` of the declared known
    /// unit answers Ok, names the unit, and carries an active state. A fixture
    /// without a known unit fails loudly instead of silently skipping the gate.
    pub async fn services_suite_with_fixture<F>(make: F, fx: ServicesFixture) -> SuiteReport
    where
        F: Fn() -> Box<dyn ServiceManager> + Send + Sync,
    {
        let mut out = services_core(&make).await;
        if fx.known_unit.is_empty() {
            out.check("fixture declares a known unit", false);
            return out;
        }
        let s = make();
        let status = s.status(fx.known_unit.clone()).await;
        match status {
            Ok(st) => {
                out.check(
                    "known unit status names the unit",
                    st.unit.name == fx.known_unit,
                );
                out.check(
                    "known unit carries an active state",
                    !st.unit.active_state.is_empty(),
                );
            }
            Err(e) => out.failed.push(format!("  known unit status failed: {e}")),
        }
        drop(s);
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::dto::probe::AdapterReport;
        use crate::dto::services::{ServiceActionResult, ServiceStatus};

        struct FakeServices;

        fn unit(name: &str) -> ServiceUnit {
            ServiceUnit {
                name: String::from(name),
                unit_type: String::from("service"),
                description: String::from("suite fake"),
                active_state: String::from("active"),
                sub_state: String::from("running"),
                enabled: Some(true),
            }
        }

        #[async_trait::async_trait]
        impl ServiceManager for FakeServices {
            fn report(&self) -> AdapterReport {
                AdapterReport {
                    adapter: String::from("fake-services"),
                    subsystem: String::from("services"),
                    contract_version: String::from(crate::version::CONTRACT_VERSION),
                    capabilities: vec![],
                    notes: vec![],
                }
            }
            async fn list(
                &self,
                _filter: ServiceFilter,
            ) -> Result<Vec<ServiceUnit>, ContractError> {
                Ok(vec![unit("suite-known.service")])
            }
            async fn status(&self, name: String) -> Result<ServiceStatus, ContractError> {
                if name == "suite-known.service" {
                    Ok(ServiceStatus {
                        unit: unit(&name),
                        main_pid: Some(1),
                        since_epoch_ms: None,
                        docs: vec![],
                    })
                } else if name == format!("{SENTINEL_SLOW}.service") {
                    Err(ContractError::TimeoutError)
                } else {
                    Err(ContractError::NativeFailure {
                        code: 1,
                        payload: String::from("no such unit"),
                    })
                }
            }
            async fn apply(
                &self,
                _request: ServiceActionRequest,
            ) -> Result<ServiceActionResult, ContractError> {
                Err(ContractError::ScopeDenied)
            }
        }

        #[tokio::test]
        async fn fixture_suite_gates_the_known_unit() {
            let fx = ServicesFixture {
                known_unit: String::from("suite-known.service"),
            };
            let report = services_suite_with_fixture(
                || Box::new(FakeServices) as Box<dyn ServiceManager>,
                fx,
            )
            .await;
            assert!(report.is_clean(), "{report}");
        }

        #[tokio::test]
        async fn fixture_suite_fails_when_the_known_unit_is_missing() {
            struct NoKnownUnit;
            #[async_trait::async_trait]
            impl ServiceManager for NoKnownUnit {
                fn report(&self) -> AdapterReport {
                    AdapterReport {
                        adapter: String::from("fake-services-broken"),
                        subsystem: String::from("services"),
                        contract_version: String::from(crate::version::CONTRACT_VERSION),
                        capabilities: vec![],
                        notes: vec![],
                    }
                }
                async fn list(
                    &self,
                    _filter: ServiceFilter,
                ) -> Result<Vec<ServiceUnit>, ContractError> {
                    Ok(vec![])
                }
                async fn status(&self, _name: String) -> Result<ServiceStatus, ContractError> {
                    Err(ContractError::NativeFailure {
                        code: 1,
                        payload: String::from("no such unit"),
                    })
                }
                async fn apply(
                    &self,
                    _request: ServiceActionRequest,
                ) -> Result<ServiceActionResult, ContractError> {
                    Err(ContractError::ScopeDenied)
                }
            }
            let fx = ServicesFixture {
                known_unit: String::from("suite-known.service"),
            };
            let report = services_suite_with_fixture(
                || Box::new(NoKnownUnit) as Box<dyn ServiceManager>,
                fx,
            )
            .await;
            assert!(!report.is_clean(), "{report}");
        }
    }
}

pub mod packages {
    //! PackageManager suite.

    use super::*;
    use crate::dto::packages::{
        PackageHistoryQuery, PackageListRequest, PackageNameRequest, PackageSearch,
    };
    use crate::spi::packages::PackageManager;

    #[derive(Debug, Clone, Default)]
    pub struct PackagesFixture {
        /// Whether the backend implements undo at all (apt: false).
        pub undo_expected: bool,
    }

    pub async fn packages_suite<F>(make: F, fx: PackagesFixture) -> SuiteReport
    where
        F: Fn() -> Box<dyn PackageManager> + Send + Sync,
    {
        let mut out = SuiteReport::default();
        {
            let p = make();
            check_report_shape(&p.report(), &mut out);
            let undo_fact = p
                .report()
                .capabilities
                .iter()
                .find(|c| c.name == crate::capabilities::PKG_UNDO)
                .and_then(|c| c.value.clone());
            out.check(
                "pkg.undo value present when undo expected",
                !fx.undo_expected || undo_fact.is_some(),
            );
        }
        let p = make();
        let list = p.list(PackageListRequest { limit: Some(5000) }).await;
        match list {
            Ok(pkgs) => out.check("list respects the limit", pkgs.len() <= 5000),
            Err(e) => out.failed.push(format!("  list failed: {e}")),
        }
        let missing = p.info(String::from(SENTINEL_MISSING)).await;
        out.check(
            "unknown package is a contract error",
            matches!(
                missing,
                Err(ContractError::NativeFailure { .. }
                    | ContractError::Unsupported { .. }
                    | ContractError::AdapterUnavailable)
            ),
        );
        let search = p
            .search(PackageSearch {
                query: String::from("suite"),
                limit: Some(10),
            })
            .await;
        match search {
            Ok(found) => out.check("search respects the limit", found.len() <= 10),
            Err(e) => out.failed.push(format!("  search failed: {e}")),
        }
        let history = p
            .history(PackageHistoryQuery {
                name: None,
                limit: Some(10),
            })
            .await;
        out.check("history yields Ok", history.is_ok());
        let undo = p
            .undo(PackageNameRequest {
                name: String::from(SENTINEL_MISSING),
            })
            .await;
        if fx.undo_expected {
            out.check(
                "undo of unknown package is an honest contract error",
                undo.is_err(),
            );
        }
        drop(p);
        out
    }
}

pub mod portals {
    //! ScreenCapture + Notify suites.

    use super::*;
    use crate::dto::portals::{ClipboardContent, NotifyRequest, ScreenshotRequest};
    use crate::spi::portals::{Notify, ScreenCapture};

    #[derive(Debug, Clone, Default)]
    pub struct PortalsFixture {
        pub clipboard_backend: bool,
        pub screenshot_backend: bool,
        pub notify_backend: bool,
        /// Marker text the backend was preloaded with.
        pub marker_text: String,
    }

    pub async fn screen_capture_suite<F>(make: F, fx: PortalsFixture) -> SuiteReport
    where
        F: Fn() -> Box<dyn ScreenCapture> + Send + Sync,
    {
        let mut out = SuiteReport::default();
        {
            let s = make();
            check_report_shape(&s.report(), &mut out);
        }
        let s = make();
        if fx.clipboard_backend {
            let round = async {
                s.clip_write(ClipboardContent {
                    mime: String::from("text/plain"),
                    text: fx.marker_text.clone(),
                    size_bytes: u64::try_from(fx.marker_text.len()).unwrap_or(0),
                })
                .await?;
                let read = s.clip_read().await?;
                Ok::<bool, ContractError>(read.text == fx.marker_text)
            }
            .await;
            match round {
                Ok(matches) => out.check("clipboard round-trip preserves content", matches),
                Err(e) => out
                    .failed
                    .push(format!("  clipboard round-trip failed: {e}")),
            }
        } else {
            let absent = s.clip_read().await;
            out.check(
                "absent backend reports AdapterUnavailable",
                matches!(absent, Err(ContractError::AdapterUnavailable)),
            );
        }
        if fx.screenshot_backend {
            let shot = s
                .screenshot(ScreenshotRequest {
                    format: crate::dto::portals::ImageFormat::Png,
                })
                .await;
            match shot {
                Ok(img) => out.check(
                    "screenshot carries image bytes",
                    !img.image_base64.is_empty(),
                ),
                Err(e) => out.failed.push(format!("  screenshot failed: {e}")),
            }
        } else {
            // The fixture's `false` covers two honest degradations: no portal
            // screenshot interface (AdapterUnavailable) and an interface present
            // but refusing unattended capture (interactive consent). Both must
            // answer with a typed contract error — a fake Ok with empty bytes
            // must fail here. (AdapterUnavailable cannot be demanded: the same
            // fixture flag also covers the consent case.)
            let shot = s
                .screenshot(ScreenshotRequest {
                    format: crate::dto::portals::ImageFormat::Png,
                })
                .await;
            out.check(
                "absent screenshot backend answers with a contract error, not a fake capture",
                shot.is_err(),
            );
        }
        drop(s);
        out
    }

    pub async fn notify_suite<F>(make: F, fx: PortalsFixture) -> SuiteReport
    where
        F: Fn() -> Box<dyn Notify> + Send + Sync,
    {
        let mut out = SuiteReport::default();
        {
            let n = make();
            check_report_shape(&n.report(), &mut out);
        }
        let n = make();
        if fx.notify_backend {
            let sent = n
                .send(NotifyRequest {
                    title: String::from("omnibus-suite"),
                    body: String::from("contract suite"),
                    urgency: Some(crate::dto::portals::Urgency::Normal),
                })
                .await;
            match sent {
                Ok(r) => out.check("notify reports delivery", r.delivered),
                Err(e) => out.failed.push(format!("  notify failed: {e}")),
            }
        } else {
            let absent = n
                .send(NotifyRequest {
                    title: String::from("omnibus-suite"),
                    body: String::from("contract suite"),
                    urgency: None,
                })
                .await;
            out.check(
                "absent notification backend reports AdapterUnavailable",
                matches!(absent, Err(ContractError::AdapterUnavailable)),
            );
        }
        drop(n);
        out
    }
}

pub mod files {
    //! FileScope suite.

    use super::*;
    use crate::dto::files::{FileReadRequest, FileWriteRequest, PathQuery, PathResolveRequest};
    use crate::spi::files::FileScope;

    #[derive(Debug, Clone, Default)]
    pub struct FilesFixture {
        /// A path inside the granted scopes with known content.
        pub readable_path: String,
        pub expected_snippet: String,
        /// A path outside every scope.
        pub forbidden_path: String,
    }

    pub async fn files_suite<F>(make: F, fx: FilesFixture) -> SuiteReport
    where
        F: Fn() -> Box<dyn FileScope> + Send + Sync,
    {
        let mut out = SuiteReport::default();
        {
            let f = make();
            check_report_shape(&f.report(), &mut out);
        }
        let f = make();
        let resolved = f
            .resolve(PathResolveRequest {
                path: fx.readable_path.clone(),
            })
            .await;
        match resolved {
            Ok(r) => out.check("resolve marks granted path readable", r.read_allowed),
            Err(e) => out.failed.push(format!("  resolve failed: {e}")),
        }
        let forbidden = f
            .resolve(PathResolveRequest {
                path: fx.forbidden_path.clone(),
            })
            .await;
        match forbidden {
            Ok(r) => out.check("resolve marks forbidden path unreadable", !r.read_allowed),
            Err(e) => out.failed.push(format!("  forbidden resolve failed: {e}")),
        }
        let read = f
            .read(FileReadRequest {
                path: fx.readable_path.clone(),
                offset_bytes: None,
                max_bytes: Some(crate::dto::Bytes(4096)),
            })
            .await;
        match read {
            Ok(content) => out.check(
                "granted read returns expected content",
                content
                    .text
                    .as_deref()
                    .unwrap_or_default()
                    .contains(&fx.expected_snippet),
            ),
            Err(e) => out.failed.push(format!("  granted read failed: {e}")),
        }
        let denied = f
            .read(FileReadRequest {
                path: fx.forbidden_path,
                offset_bytes: None,
                max_bytes: None,
            })
            .await;
        out.check(
            "out-of-scope read yields ScopeDenied",
            matches!(denied, Err(ContractError::ScopeDenied)),
        );
        let listed = f
            .list(PathQuery {
                path: String::from("."),
                depth: Some(1),
                limit: Some(10),
            })
            .await;
        match listed {
            Ok(l) => out.check("list respects the limit", l.entries.len() <= 10),
            Err(e) => out.failed.push(format!("  list failed: {e}")),
        }
        drop(f);
        out
    }

    /// Write-path suite (bound by write-capable fixtures; grant checks stay in the
    /// core, the adapter enforces its activated scopes).
    pub async fn files_write_suite<F>(make: F, target: String, payload: String) -> SuiteReport
    where
        F: Fn() -> Box<dyn FileScope> + Send + Sync,
    {
        let mut out = SuiteReport::default();
        let f = make();
        let expected_bytes = u64::try_from(payload.len()).unwrap_or(u64::MAX);
        let written = f
            .write(FileWriteRequest {
                path: target.clone(),
                data_base64: None,
                text: Some(payload.clone()),
                create_parents: Some(false),
            })
            .await;
        match written {
            Ok(r) => {
                out.check(
                    "write reports the exact byte count",
                    r.bytes_written == expected_bytes,
                );
                let read = f
                    .read(FileReadRequest {
                        path: target,
                        offset_bytes: None,
                        max_bytes: None,
                    })
                    .await;
                match read {
                    Ok(content) => out.check(
                        "written content reads back byte-for-byte",
                        content.text.as_deref() == Some(payload.as_str()),
                    ),
                    Err(e) => out
                        .failed
                        .push(format!("  written content did not read back: {e}")),
                }
            }
            Err(ContractError::ScopeDenied) => {
                out.check("write outside scope denied", true);
            }
            Err(e) => out.failed.push(format!("  write failed unexpectedly: {e}")),
        }
        drop(f);
        out
    }
}

pub mod kb {
    //! KnowledgeBase suite.

    use super::*;
    use crate::dto::kb::{KbPageRequest, KbQuery};
    use crate::spi::kb::KnowledgeBase;

    #[derive(Debug, Clone, Default)]
    pub struct KbFixture {
        pub query_text: String,
        pub expected_title_fragment: String,
        pub known_url: String,
        pub offline: bool,
    }

    pub async fn kb_suite<F>(make: F, fx: KbFixture) -> SuiteReport
    where
        F: Fn() -> Box<dyn KnowledgeBase> + Send + Sync,
    {
        let mut out = SuiteReport::default();
        {
            let k = make();
            check_report_shape(&k.report(), &mut out);
        }
        let k = make();
        let found = k
            .search(KbQuery {
                text: fx.query_text.clone(),
                limit: Some(5),
            })
            .await;
        match found {
            Ok(refs) => {
                out.check("search finds the known fix", !refs.is_empty());
                // The fixture prescribes a state (binders populate
                // expected_title_fragment); the gate must verify it. Match
                // case-insensitively: a live title may capitalize the fragment.
                if fx.expected_title_fragment.is_empty() {
                    out.check("fixture declares an expected title fragment", false);
                } else {
                    out.check(
                        "search returns the fixture's expected article",
                        refs.iter().any(|r| {
                            r.title
                                .to_lowercase()
                                .contains(&fx.expected_title_fragment.to_lowercase())
                        }),
                    );
                }
                out.check(
                    "refs name their source",
                    refs.iter().all(|r| !r.source.is_empty()),
                );
            }
            Err(e) => out.failed.push(format!("  search failed: {e}")),
        }
        let page = k
            .page(KbPageRequest {
                url: fx.known_url.clone(),
            })
            .await;
        match page {
            Ok(article) => {
                out.check(
                    "offline mode marks stale cache",
                    !fx.offline || article.stale,
                );
                out.check("page carries text", !article.text.is_empty());
            }
            Err(e) => out.failed.push(format!("  page failed: {e}")),
        }
        drop(k);
        out
    }
}

pub mod net {
    //! NetworkStatus suite (read-only by contract).

    use super::*;
    use crate::spi::net::NetworkStatus;

    pub async fn net_suite<F>(make: F) -> SuiteReport
    where
        F: Fn() -> Box<dyn NetworkStatus> + Send + Sync,
    {
        let mut out = SuiteReport::default();
        {
            let n = make();
            check_report_shape(&n.report(), &mut out);
        }
        let n = make();
        let status = n.status().await;
        match status {
            Ok(s) => out.check("connectivity named", !s.connectivity.is_empty()),
            Err(e) => out.failed.push(format!("  status failed: {e}")),
        }
        drop(n);
        out
    }
}

pub mod probe {
    //! PlatformProbe suite.

    use super::*;
    use crate::spi::probe::PlatformProbe;

    pub async fn probe_suite<F>(make: F) -> SuiteReport
    where
        F: Fn() -> Box<dyn PlatformProbe> + Send + Sync,
    {
        let mut out = SuiteReport::default();
        {
            let p = make();
            check_report_shape(&p.report(), &mut out);
        }
        let p = make();
        let report = p.detect().await;
        match report {
            Ok(r) => {
                out.check("distro id present", !r.distro.id.is_empty());
                out.check(
                    "floors evaluated with reasons when unmet",
                    r.floors.iter().all(|f| f.met || f.reason.is_some()),
                );
            }
            Err(e) => out.failed.push(format!("  detect failed: {e}")),
        }
        drop(p);
        out
    }
}

pub mod audit_sink {
    //! AuditSink suite: one event in, exactly one delivered event out.

    use super::*;
    use crate::dto::audit::{AuditActor, AuditEvent};
    use crate::spi::audit_sink::AuditSink;

    /// How the binder observes delivery (sinks are write-only by contract).
    pub trait SuiteProbe: Send + Sync {
        fn delivered(&self) -> usize;
    }

    /// Factory returns the sink binding: the sink plus its probe handle (named
    /// struct — CoP holds in the harness too).
    pub struct SinkBinding<P: SuiteProbe> {
        pub sink: Box<dyn AuditSink>,
        pub probe: std::sync::Arc<P>,
    }

    pub trait SinkFactory: Send + Sync {
        type Probe: SuiteProbe;
        fn make(&self) -> SinkBinding<Self::Probe>;
    }

    pub fn audit_event_template() -> AuditEvent {
        AuditEvent {
            ts_epoch_ms: crate::dto::Milliseconds::now_epoch().0,
            session: String::from("sess-suite"),
            actor: AuditActor {
                uid: 1000,
                pid: 42,
                client_name: Some(String::from("contract-suite")),
            },
            tool: String::from("suite.tool"),
            adapter: String::from("suite"),
            op: String::from("op"),
            params_hash: String::from("0"),
            result: String::from("ok"),
            scope_ref: None,
        }
    }

    pub async fn audit_sink_suite<F>(factory: F) -> SuiteReport
    where
        F: SinkFactory,
    {
        let mut out = SuiteReport::default();
        let binding = factory.make();
        let sink = binding.sink;
        let probe = binding.probe;
        let before = probe.delivered();
        let delivered = sink.write(audit_event_template()).await;
        match delivered {
            Ok(()) => {
                out.check(
                    "one write delivers exactly one event",
                    probe.delivered() == before + 1,
                );
            }
            Err(e) => out.failed.push(format!("  sink write failed: {e}")),
        }
        out
    }
}
