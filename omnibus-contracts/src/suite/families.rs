//! `ServiceManager`, `PackageManager`, `ScreenCapture`, `Notify`, `FileScope`,
//! `KnowledgeBase`, `PlatformProbe`, `NetworkStatus`, `AppLauncher`,
//! `ProcessTelemetry`, `AuditSink` contract suites.
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
            let undo_typed = undo_fact
                .as_ref()
                .map(crate::capabilities::PkgUndoValue::from_json);
            out.check(
                "pkg.undo value present when undo expected",
                !fx.undo_expected || undo_fact.is_some(),
            );
            // The value is typed: every reported pkg.undo value parses into the
            // closed PkgUndoValue set (roadmap 1.2.3 AC2) — a raw string
            // convention outside the enum is a suite failure.
            out.check(
                "pkg.undo value parses into the typed PkgUndoValue",
                !undo_typed.as_ref().is_some_and(|parsed| parsed.is_none()),
            );
            if fx.undo_expected {
                out.check(
                    "pkg.undo is typed and not none when undo is expected",
                    matches!(
                        undo_typed,
                        Some(Some(parsed)) if parsed != crate::capabilities::PkgUndoValue::None
                    ),
                );
            }
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

pub mod app_launcher {
    //! AppLauncher suite: inventory read, write actions behind the grant
    //! ladder (launch/open), the launch → observe unit handle.

    use super::*;
    use crate::dto::app_launcher::{AppSpec, OpenRequest};
    use crate::spi::app_launcher::AppLauncher;

    /// The backend state the implementation was configured with. The
    /// fixture-free [`app_launcher_suite`] cannot verify a known application;
    /// binders that configure one must bind [`app_launcher_suite_with_fixture`].
    #[derive(Debug, Clone, Default)]
    pub struct AppLauncherFixture {
        /// An application the backend reports in `list` and can `launch`
        /// (the closed-loop stand of task 2.7.2 binds a test `.desktop` app).
        pub known_app: String,
        /// A target (URI or path) the backend can `open`; when empty the
        /// open gate is skipped honestly (a stand without an opener).
        pub openable_target: String,
    }

    async fn app_launcher_core<F>(make: &F) -> SuiteReport
    where
        F: Fn() -> Box<dyn AppLauncher> + Send + Sync,
    {
        let mut out = SuiteReport::default();
        {
            let a = make();
            check_report_shape(&a.report(), &mut out);
        }
        let a = make();
        let list = a.list().await;
        match list {
            Ok(apps) => out.check(
                "entries carry an app_id and a name",
                apps.iter()
                    .all(|app| !app.app_id.is_empty() && !app.name.is_empty()),
            ),
            Err(e) => out.failed.push(format!("  list failed: {e}")),
        }
        let missing = a
            .launch(AppSpec {
                app_id: String::from(SENTINEL_MISSING),
                // Metacharacters are typed data crossing the SPI — never shell
                // syntax; the sentinel must still answer the configured error.
                args: vec![String::from("--flag;|`")],
            })
            .await;
        out.check(
            "unknown app is a contract error",
            matches!(
                missing,
                Err(ContractError::NativeFailure { .. }
                    | ContractError::Unsupported { .. }
                    | ContractError::AdapterUnavailable)
            ),
        );
        let slow = a
            .launch(AppSpec {
                app_id: String::from(SENTINEL_SLOW),
                args: vec![],
            })
            .await;
        out.check(
            "slow launch maps to TimeoutError",
            matches!(slow, Err(ContractError::TimeoutError)),
        );
        let denied = a
            .launch(AppSpec {
                app_id: String::from(SENTINEL_POISON),
                args: vec![],
            })
            .await;
        out.check(
            "launch without the grant is a contract error",
            matches!(
                denied,
                Err(ContractError::NativeFailure { .. } | ContractError::ScopeDenied)
            ),
        );
        let open_slow = a
            .open(OpenRequest {
                target: String::from(SENTINEL_SLOW),
            })
            .await;
        out.check(
            "slow open maps to TimeoutError",
            matches!(open_slow, Err(ContractError::TimeoutError)),
        );
        let open_denied = a
            .open(OpenRequest {
                target: String::from(SENTINEL_POISON),
            })
            .await;
        out.check(
            "open without the grant is a contract error",
            matches!(
                open_denied,
                Err(ContractError::NativeFailure { .. } | ContractError::ScopeDenied)
            ),
        );
        drop(a);
        out
    }

    /// Runs the fixture-free `AppLauncher` behavioral set against `make`.
    ///
    /// Fixture-free by signature and by honesty: no application exists on
    /// every machine, so this entry point cannot gate a known app. Binders
    /// that configure the state the fixture prescribes must bind
    /// [`app_launcher_suite_with_fixture`], which gates it for real.
    pub async fn app_launcher_suite<F>(make: F) -> SuiteReport
    where
        F: Fn() -> Box<dyn AppLauncher> + Send + Sync,
    {
        app_launcher_core(&make).await
    }

    /// [`app_launcher_suite`] plus the fixture gates: the declared known app
    /// is listed, `launch` of it succeeds and names the transient user unit
    /// (the observation join key), and — when `openable_target` is declared —
    /// `open` of it succeeds. A fixture without a known app fails loudly
    /// instead of silently skipping the gates.
    pub async fn app_launcher_suite_with_fixture<F>(make: F, fx: AppLauncherFixture) -> SuiteReport
    where
        F: Fn() -> Box<dyn AppLauncher> + Send + Sync,
    {
        let mut out = app_launcher_core(&make).await;
        if fx.known_app.is_empty() {
            out.check("fixture declares a known app", false);
            return out;
        }
        let a = make();
        let list = a.list().await;
        match list {
            Ok(apps) => out.check(
                "fixture known app is listed",
                apps.iter().any(|app| app.app_id == fx.known_app),
            ),
            Err(e) => out.failed.push(format!("  fixture list failed: {e}")),
        }
        let launched = a
            .launch(AppSpec {
                app_id: fx.known_app.clone(),
                args: vec![],
            })
            .await;
        match launched {
            Ok(unit) => out.check(
                "launch names the transient unit",
                !unit.unit_name.is_empty(),
            ),
            Err(e) => out.failed.push(format!("  fixture launch failed: {e}")),
        }
        if !fx.openable_target.is_empty() {
            let opened = a
                .open(OpenRequest {
                    target: fx.openable_target.clone(),
                })
                .await;
            match opened {
                Ok(()) => out.check("open succeeds on the declared target", true),
                Err(e) => out.failed.push(format!("  fixture open failed: {e}")),
            }
        }
        drop(a);
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::dto::app_launcher::{AppEntry, LaunchOutcome};
        use crate::dto::probe::AdapterReport;

        struct FakeLauncher;

        #[async_trait::async_trait]
        impl AppLauncher for FakeLauncher {
            fn report(&self) -> AdapterReport {
                AdapterReport {
                    adapter: String::from("fake-launch"),
                    subsystem: String::from("app-launch"),
                    contract_version: String::from(crate::version::CONTRACT_VERSION),
                    capabilities: vec![],
                    notes: vec![],
                }
            }
            async fn list(&self) -> Result<Vec<AppEntry>, ContractError> {
                Ok(vec![AppEntry {
                    app_id: String::from("suite-app.desktop"),
                    name: String::from("Suite App"),
                    exec: String::from("suite-app"),
                    icon: None,
                    no_display: false,
                }])
            }
            async fn launch(&self, spec: AppSpec) -> Result<LaunchOutcome, ContractError> {
                match spec.app_id.as_str() {
                    // The typed arg list is data; the sentinel mapping does
                    // not depend on it (no shell anywhere to interpret it).
                    s if s == SENTINEL_MISSING => Err(ContractError::NativeFailure {
                        code: 1,
                        payload: String::from("no such application"),
                    }),
                    s if s == SENTINEL_SLOW => Err(ContractError::TimeoutError),
                    s if s == SENTINEL_POISON => Err(ContractError::ScopeDenied),
                    app_id => Ok(LaunchOutcome {
                        unit_name: format!("app-{app_id}.service"),
                    }),
                }
            }
            async fn open(&self, request: OpenRequest) -> Result<(), ContractError> {
                match request.target.as_str() {
                    t if t == SENTINEL_SLOW => Err(ContractError::TimeoutError),
                    t if t == SENTINEL_POISON => Err(ContractError::ScopeDenied),
                    _ => Ok(()),
                }
            }
        }

        #[tokio::test]
        async fn fixture_suite_runs_green_against_the_fake() {
            let fx = AppLauncherFixture {
                known_app: String::from("suite-app.desktop"),
                openable_target: String::from("https://suite.invalid/"),
            };
            let report = app_launcher_suite_with_fixture(
                || Box::new(FakeLauncher) as Box<dyn AppLauncher>,
                fx,
            )
            .await;
            assert!(report.is_clean(), "{report}");
        }

        #[tokio::test]
        async fn fixture_suite_fails_when_the_known_app_is_missing() {
            struct NoKnownApp;
            #[async_trait::async_trait]
            impl AppLauncher for NoKnownApp {
                fn report(&self) -> AdapterReport {
                    AdapterReport {
                        adapter: String::from("fake-launch-broken"),
                        subsystem: String::from("app-launch"),
                        contract_version: String::from(crate::version::CONTRACT_VERSION),
                        capabilities: vec![],
                        notes: vec![],
                    }
                }
                async fn list(&self) -> Result<Vec<AppEntry>, ContractError> {
                    Ok(vec![])
                }
                async fn launch(&self, _spec: AppSpec) -> Result<LaunchOutcome, ContractError> {
                    Err(ContractError::NativeFailure {
                        code: 1,
                        payload: String::from("no such application"),
                    })
                }
                async fn open(&self, _request: OpenRequest) -> Result<(), ContractError> {
                    Err(ContractError::ScopeDenied)
                }
            }
            let fx = AppLauncherFixture {
                known_app: String::from("suite-app.desktop"),
                openable_target: String::new(),
            };
            let report = app_launcher_suite_with_fixture(
                || Box::new(NoKnownApp) as Box<dyn AppLauncher>,
                fx,
            )
            .await;
            assert!(!report.is_clean(), "{report}");
        }
    }
}

pub mod process_telemetry {
    //! ProcessTelemetry suite: the read ladder (`list`, `info`, `sys_stats`,
    //! `power_status`) and the granted write (`signal`), with the negative
    //! cases the domain mandates: a nonexistent pid yields a contract error,
    //! never a panic, and PID 1 / systemd-unit processes stay outside every
    //! operation (specs/domains/process-telemetry.md Invariants).

    use super::*;
    use crate::dto::process_telemetry::{ProcessListQuery, Signal, SignalRequest};
    use crate::spi::process_telemetry::ProcessTelemetry;

    /// Sentinel pids the binder maps to configured failure injections. They
    /// sit beyond every Linux `pid_max` (≤ 2^22), so real backends answer
    /// them as nonexistent; the shared [`super::SENTINEL_*`] strings cannot
    /// address processes — pids are numbers, not names.
    pub const SENTINEL_PID_MISSING: u32 = 2_000_000_000;
    pub const SENTINEL_PID_SLOW: u32 = 2_000_000_001;
    /// The write without its grant (the core denies scope before the adapter
    /// runs; a stand binds this pid to its configured denial injection).
    pub const SENTINEL_PID_POISON: u32 = 2_000_000_002;

    /// The backend state the implementation was configured with. The
    /// fixture-free [`process_telemetry_suite`] cannot verify a known
    /// process; binders that configure one must bind
    /// [`process_telemetry_suite_with_fixture`].
    #[derive(Debug, Clone, Default)]
    pub struct ProcessTelemetryFixture {
        /// A pid the backend lists, snapshots in `info`, and signals
        /// successfully (the integration stand binds a test process with the
        /// `ps.signal` write grant wired; task 2.8.3).
        pub known_pid: u32,
    }

    async fn process_telemetry_core<F>(make: &F) -> SuiteReport
    where
        F: Fn() -> Box<dyn ProcessTelemetry> + Send + Sync,
    {
        let mut out = SuiteReport::default();
        {
            let a = make();
            check_report_shape(&a.report(), &mut out);
        }
        let a = make();
        let list = a
            .list(ProcessListQuery {
                name_contains: None,
                limit: None,
            })
            .await;
        match list {
            Ok(entries) => out.check(
                "entries carry a pid and a name, never pid 0/1",
                entries.iter().all(|e| e.pid > 1 && !e.name.is_empty()),
            ),
            Err(e) => out.failed.push(format!("  list failed: {e}")),
        }
        // The mandated negative case: a nonexistent pid is a contract error,
        // never a panic (roadmap 2.8.1 AC).
        let missing_info = a.info(SENTINEL_PID_MISSING).await;
        out.check(
            "nonexistent pid is a contract error (info)",
            matches!(
                missing_info,
                Err(ContractError::NativeFailure { .. }
                    | ContractError::ElementStale
                    | ContractError::Unsupported { .. })
            ),
        );
        // PID 1 never answers info: services are addressed through svc_*.
        let init_info = a.info(1).await;
        out.check("pid 1 stays outside info", init_info.is_err());
        let stats = a.sys_stats().await;
        match stats {
            Ok(stats) => out.check(
                "sys_stats carries finite load averages and sane memory",
                stats.load_avg_1m.is_finite()
                    && stats.load_avg_5m.is_finite()
                    && stats.load_avg_15m.is_finite()
                    && stats.mem_used_bytes.0 <= stats.mem_total_bytes.0,
            ),
            Err(e) => out.failed.push(format!("  sys_stats failed: {e}")),
        }
        // The R3 ladder: without UPower/battery the tool is hidden, so the
        // honest answers are a report or CapabilityRequired — nothing else.
        let power = a.power_status().await;
        out.check(
            "power_status answers a report or CapabilityRequired",
            matches!(power, Ok(_) | Err(ContractError::CapabilityRequired)),
        );
        let missing_signal = a
            .signal(SignalRequest {
                pid: SENTINEL_PID_MISSING,
                signal: Signal::Term,
            })
            .await;
        out.check(
            "nonexistent pid is a contract error (signal)",
            matches!(
                missing_signal,
                Err(ContractError::NativeFailure { .. }
                    | ContractError::ElementStale
                    | ContractError::Unsupported { .. })
            ),
        );
        // The domain invariant: a signal aimed at PID 1 answers Unsupported
        // (with the hint to the service tools), never is delivered.
        let init_signal = a
            .signal(SignalRequest {
                pid: 1,
                signal: Signal::Term,
            })
            .await;
        out.check(
            "pid 1 signal answers Unsupported (svc_* hint)",
            matches!(init_signal, Err(ContractError::Unsupported { .. })),
        );
        let slow = a
            .signal(SignalRequest {
                pid: SENTINEL_PID_SLOW,
                signal: Signal::Term,
            })
            .await;
        out.check(
            "slow signal maps to TimeoutError",
            matches!(slow, Err(ContractError::TimeoutError)),
        );
        let denied = a
            .signal(SignalRequest {
                pid: SENTINEL_PID_POISON,
                signal: Signal::Term,
            })
            .await;
        out.check(
            "signal without the grant is a contract error",
            matches!(
                denied,
                Err(ContractError::NativeFailure { .. } | ContractError::ScopeDenied)
            ),
        );
        drop(a);
        out
    }

    /// Runs the fixture-free `ProcessTelemetry` behavioral set against `make`.
    ///
    /// Fixture-free by signature and by honesty: no process exists on every
    /// machine, so this entry point cannot gate a known pid. Binders that
    /// configure the state the fixture prescribes must bind
    /// [`process_telemetry_suite_with_fixture`], which gates it for real.
    pub async fn process_telemetry_suite<F>(make: F) -> SuiteReport
    where
        F: Fn() -> Box<dyn ProcessTelemetry> + Send + Sync,
    {
        process_telemetry_core(&make).await
    }

    /// [`process_telemetry_suite`] plus the fixture gates: the declared known
    /// pid is listed, `info` snapshots it consistently, and `signal` with the
    /// closed-enum TERM succeeds (the stand has the `ps.signal` grant wired).
    /// A fixture without a signalable pid fails loudly instead of silently
    /// skipping the gates.
    pub async fn process_telemetry_suite_with_fixture<F>(
        make: F,
        fx: ProcessTelemetryFixture,
    ) -> SuiteReport
    where
        F: Fn() -> Box<dyn ProcessTelemetry> + Send + Sync,
    {
        let mut out = process_telemetry_core(&make).await;
        if fx.known_pid <= 1 {
            out.check("fixture declares a known pid (pid > 1)", false);
            return out;
        }
        let a = make();
        let list = a
            .list(ProcessListQuery {
                name_contains: None,
                limit: None,
            })
            .await;
        match list {
            Ok(entries) => out.check(
                "fixture known pid is listed",
                entries.iter().any(|e| e.pid == fx.known_pid),
            ),
            Err(e) => out.failed.push(format!("  fixture list failed: {e}")),
        }
        let info = a.info(fx.known_pid).await;
        match info {
            Ok(snapshot) => out.check(
                "info snapshots the known pid consistently",
                snapshot.pid == fx.known_pid && !snapshot.name.is_empty(),
            ),
            Err(e) => out.failed.push(format!("  fixture info failed: {e}")),
        }
        let signalled = a
            .signal(SignalRequest {
                pid: fx.known_pid,
                signal: Signal::Term,
            })
            .await;
        match signalled {
            Ok(()) => out.check("signal delivers the closed-enum signal", true),
            Err(e) => out.failed.push(format!("  fixture signal failed: {e}")),
        }
        drop(a);
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::dto::probe::AdapterReport;
        use crate::dto::process_telemetry::{PowerStatus, ProcessEntry, ProcessSnapshot, SysStats};
        use crate::dto::{Bytes, Milliseconds, Percent, Seconds};

        fn entry(pid: u32, name: &str) -> ProcessEntry {
            ProcessEntry {
                pid,
                name: String::from(name),
                state: String::from("S"),
                cpu_percent: Percent(1),
                rss_bytes: Bytes(4096),
                cgroup: None,
            }
        }

        fn snapshot(pid: u32, name: &str) -> ProcessSnapshot {
            ProcessSnapshot {
                pid,
                name: String::from(name),
                state: String::from("S"),
                cpu_percent: Percent(1),
                rss_bytes: Bytes(4096),
                cgroup: None,
                command_line: Some(String::from("suite-proc --idle")),
                start_time_epoch_ms: Milliseconds(0),
            }
        }

        struct FakeProc;

        #[async_trait::async_trait]
        impl ProcessTelemetry for FakeProc {
            fn report(&self) -> AdapterReport {
                AdapterReport {
                    adapter: String::from("fake-proc"),
                    subsystem: String::from("process-telemetry"),
                    contract_version: String::from(crate::version::CONTRACT_VERSION),
                    capabilities: vec![],
                    notes: vec![],
                }
            }
            async fn list(
                &self,
                _query: ProcessListQuery,
            ) -> Result<Vec<ProcessEntry>, ContractError> {
                Ok(vec![entry(4242, "suite-proc")])
            }
            async fn info(&self, pid: u32) -> Result<ProcessSnapshot, ContractError> {
                match pid {
                    p if p == SENTINEL_PID_MISSING => Err(ContractError::NativeFailure {
                        code: 2,
                        payload: String::from("no such process"),
                    }),
                    p if p == SENTINEL_PID_SLOW => Err(ContractError::TimeoutError),
                    // PID 1 stays outside: services go through svc_*.
                    1 => Err(ContractError::Unsupported {
                        reason: String::from("systemd-unit processes are addressed via svc_*"),
                    }),
                    pid => Ok(snapshot(pid, "suite-proc")),
                }
            }
            async fn sys_stats(&self) -> Result<SysStats, ContractError> {
                Ok(SysStats {
                    cpu_percent: Percent(12),
                    load_avg_1m: 0.42,
                    load_avg_5m: 0.4,
                    load_avg_15m: 0.36,
                    mem_total_bytes: Bytes(8_000_000_000),
                    mem_used_bytes: Bytes(4_000_000_000),
                    swap_total_bytes: Bytes(2_000_000_000),
                    swap_used_bytes: Bytes(0),
                    uptime_seconds: Seconds(1_000),
                })
            }
            async fn power_status(&self) -> Result<PowerStatus, ContractError> {
                Ok(PowerStatus {
                    on_battery: false,
                    battery_percent: Some(Percent(88)),
                    seconds_until_empty: None,
                    seconds_until_full: Some(Seconds(1_800)),
                })
            }
            async fn signal(&self, request: SignalRequest) -> Result<(), ContractError> {
                match request.pid {
                    p if p == SENTINEL_PID_MISSING => Err(ContractError::NativeFailure {
                        code: 3,
                        payload: String::from("no such process"),
                    }),
                    p if p == SENTINEL_PID_SLOW => Err(ContractError::TimeoutError),
                    p if p == SENTINEL_PID_POISON => Err(ContractError::ScopeDenied),
                    // PID 1 stays outside: services go through svc_*.
                    1 => Err(ContractError::Unsupported {
                        reason: String::from("systemd-unit processes are addressed via svc_*"),
                    }),
                    _ => Ok(()),
                }
            }
        }

        #[tokio::test]
        async fn fixture_suite_runs_green_against_the_fake() {
            let fx = ProcessTelemetryFixture { known_pid: 4242 };
            let report = process_telemetry_suite_with_fixture(
                || Box::new(FakeProc) as Box<dyn ProcessTelemetry>,
                fx,
            )
            .await;
            assert!(report.is_clean(), "{report}");
        }

        /// The fixture-free negative control: the mandated case (a
        /// nonexistent pid) is answered with contract errors and the report
        /// stays clean — the suite must not need a live process to gate.
        #[tokio::test]
        async fn fixture_free_suite_gates_the_negative_pid_cases() {
            let report =
                process_telemetry_suite(|| Box::new(FakeProc) as Box<dyn ProcessTelemetry>).await;
            assert!(report.is_clean(), "{report}");
            assert!(
                report
                    .passed
                    .iter()
                    .any(|name| name.contains("nonexistent pid is a contract error")),
                "the negative-pid gates ran: {report:?}"
            );
        }

        #[tokio::test]
        async fn fixture_suite_fails_when_the_known_pid_is_missing() {
            struct NoKnownPid;
            #[async_trait::async_trait]
            impl ProcessTelemetry for NoKnownPid {
                fn report(&self) -> AdapterReport {
                    AdapterReport {
                        adapter: String::from("fake-proc-broken"),
                        subsystem: String::from("process-telemetry"),
                        contract_version: String::from(crate::version::CONTRACT_VERSION),
                        capabilities: vec![],
                        notes: vec![],
                    }
                }
                async fn list(
                    &self,
                    _query: ProcessListQuery,
                ) -> Result<Vec<ProcessEntry>, ContractError> {
                    Ok(vec![])
                }
                async fn info(&self, pid: u32) -> Result<ProcessSnapshot, ContractError> {
                    match pid {
                        p if p == SENTINEL_PID_MISSING => Err(ContractError::NativeFailure {
                            code: 2,
                            payload: String::from("no such process"),
                        }),
                        p if p == SENTINEL_PID_SLOW => Err(ContractError::TimeoutError),
                        1 => Err(ContractError::Unsupported {
                            reason: String::from("systemd-unit processes are addressed via svc_*"),
                        }),
                        pid => Ok(snapshot(pid, "suite-proc")),
                    }
                }
                async fn sys_stats(&self) -> Result<SysStats, ContractError> {
                    FakeProc.sys_stats().await
                }
                async fn power_status(&self) -> Result<PowerStatus, ContractError> {
                    FakeProc.power_status().await
                }
                async fn signal(&self, request: SignalRequest) -> Result<(), ContractError> {
                    FakeProc.signal(request).await
                }
            }
            let fx = ProcessTelemetryFixture { known_pid: 4242 };
            let report = process_telemetry_suite_with_fixture(
                || Box::new(NoKnownPid) as Box<dyn ProcessTelemetry>,
                fx,
            )
            .await;
            assert!(!report.is_clean(), "{report}");
            assert!(
                report
                    .failed
                    .iter()
                    .any(|name| name.contains("fixture known pid is listed")),
                "the fixture gate, not the core gates, goes red: {report:?}"
            );
        }

        #[tokio::test]
        async fn empty_fixture_fails_loudly_instead_of_skipping_gates() {
            let report = process_telemetry_suite_with_fixture(
                || Box::new(FakeProc) as Box<dyn ProcessTelemetry>,
                ProcessTelemetryFixture::default(),
            )
            .await;
            assert!(!report.is_clean(), "{report}");
            assert!(
                report
                    .failed
                    .iter()
                    .any(|name| name.contains("fixture declares a known pid")),
                "the empty-fixture guard is the failure: {report:?}"
            );
        }
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
