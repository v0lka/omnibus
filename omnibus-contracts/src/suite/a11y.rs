//! `DesktopA11y` contract suite: tree shape, element resolution, stale elements,
//! typed writes, poison/timeout mapping.

use crate::dto::desktop::{InvokeRequest, SetValueRequest, TreeSelector};
use crate::errors::ContractError;
use crate::spi::a11y::DesktopA11y;

use super::{SuiteReport, check_report_shape};

#[derive(Debug, Clone)]
pub struct A11yFixture {
    /// The application the backend is configured with.
    pub app: String,
    /// Pid disambiguating `app` when several live instances share the name;
    /// `None` keeps the name-only selector (fine for fakes and single-instance
    /// backends). The live binder fills this from `discover_apps` so the
    /// suite cannot trip the ambiguity error an honest backend must raise.
    pub pid: Option<u32>,
    /// A node reference that resolves.
    pub live_ref: String,
    /// A node reference that must yield `ElementStale`.
    pub stale_ref: String,
    /// A selector that must yield `NativeFailure`.
    pub poison_app: String,
    /// A selector that must yield `TimeoutError`.
    pub slow_app: String,
}

impl Default for A11yFixture {
    fn default() -> Self {
        Self {
            app: String::from("omnibus-suite-app"),
            pid: None,
            live_ref: String::from("app#1/button#1"),
            stale_ref: String::from("app#gone"),
            poison_app: String::from("omnibus-suite-poison"),
            slow_app: String::from("omnibus-suite-slow"),
        }
    }
}

pub async fn a11y_suite<F>(make: F, fx: A11yFixture) -> SuiteReport
where
    F: Fn() -> Box<dyn DesktopA11y> + Send + Sync,
{
    let mut out = SuiteReport::default();

    {
        let a = make();
        check_report_shape(&a.report(), &mut out);
    }

    let a = make();
    let tree = a
        .tree(TreeSelector {
            app: fx.app.clone(),
            pid: fx.pid,
        })
        .await;
    if let Ok(t) = tree {
        out.check("tree returns the requested app", t.app == fx.app);
        out.check("tree counts nodes", t.node_count >= 1);
        // The label is the contract: the root must actually name the requested
        // app (the atspi binder derives fx.app from the same Name property the
        // tree root reports, so this holds for an honest backend).
        out.check("tree root names the app", t.root.name.contains(&fx.app));
        let element = a.element(t.root.node_ref.clone()).await;
        out.check("element resolves a live ref", element.is_ok());
    } else {
        out.failed
            .push(String::from("  tree failed on the fixture app"));
    }
    drop(a);

    let a = make();
    let stale = a.element(fx.stale_ref.clone()).await;
    out.check(
        "stale ref yields ElementStale",
        matches!(stale, Err(ContractError::ElementStale)),
    );

    let poisoned = a
        .tree(TreeSelector {
            app: fx.poison_app.clone(),
            pid: None,
        })
        .await;
    out.check(
        "poison selector yields NativeFailure",
        matches!(poisoned, Err(ContractError::NativeFailure { .. })),
    );

    let slow = a
        .tree(TreeSelector {
            app: fx.slow_app.clone(),
            pid: None,
        })
        .await;
    out.check(
        "slow selector yields TimeoutError",
        matches!(slow, Err(ContractError::TimeoutError)),
    );

    let write = a
        .set_value(SetValueRequest {
            node_ref: fx.live_ref.clone(),
            value: String::from("suite"),
        })
        .await;
    out.check(
        "set_value applies or reports honestly",
        write.is_ok() || matches!(write, Err(ContractError::ElementStale)),
    );

    let invoked = a
        .invoke(InvokeRequest {
            node_ref: fx.live_ref,
            action: None,
        })
        .await;
    out.check(
        "invoke applies or reports honestly",
        invoked.is_ok() || matches!(invoked, Err(ContractError::ElementStale)),
    );
    drop(a);

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::desktop::{A11yElement, A11yNode, A11yTree, WriteOutcome};
    use crate::dto::probe::AdapterReport;

    struct FakeA11y;

    fn node_tree() -> A11yNode {
        A11yNode {
            node_ref: String::from("app#1/button#1"),
            role: String::from("push button"),
            name: String::from("omnibus-suite-app button"),
            pid: Some(1),
            children: vec![],
        }
    }

    #[async_trait::async_trait]
    impl DesktopA11y for FakeA11y {
        fn report(&self) -> AdapterReport {
            AdapterReport {
                adapter: String::from("fake-a11y"),
                subsystem: String::from("a11y"),
                contract_version: String::from(crate::version::CONTRACT_VERSION),
                capabilities: vec![],
                notes: vec![],
            }
        }
        async fn tree(&self, selector: TreeSelector) -> Result<A11yTree, ContractError> {
            match selector.app.as_str() {
                "omnibus-suite-poison" => Err(ContractError::NativeFailure {
                    code: 2,
                    payload: String::from("poison"),
                }),
                "omnibus-suite-slow" => Err(ContractError::TimeoutError),
                _ => Ok(A11yTree {
                    app: selector.app,
                    pid: Some(1),
                    root: node_tree(),
                    node_count: 1,
                    truncated: false,
                }),
            }
        }
        async fn element(&self, node_ref: String) -> Result<A11yElement, ContractError> {
            if node_ref == "app#gone" {
                return Err(ContractError::ElementStale);
            }
            Ok(A11yElement {
                node_ref,
                role: String::from("push button"),
                name: String::from("omnibus-suite-app button"),
                attributes: vec![],
                states: vec![String::from("enabled")],
                actions: vec![String::from("press")],
                child_refs: vec![],
            })
        }
        async fn read_attr(
            &self,
            _node_ref: String,
            _attribute: String,
        ) -> Result<String, ContractError> {
            Ok(String::from("value"))
        }
        async fn set_value(
            &self,
            _request: SetValueRequest,
        ) -> Result<WriteOutcome, ContractError> {
            Ok(WriteOutcome {
                node_ref: String::from("app#1/button#1"),
                applied: true,
            })
        }
        async fn invoke(&self, _request: InvokeRequest) -> Result<WriteOutcome, ContractError> {
            Ok(WriteOutcome {
                node_ref: String::from("app#1/button#1"),
                applied: true,
            })
        }
    }

    #[tokio::test]
    async fn harness_passes_against_the_reference_fake() {
        let report = a11y_suite(|| Box::new(FakeA11y), A11yFixture::default()).await;
        assert!(report.is_clean(), "{report}");
    }
}
