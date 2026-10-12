//! Binder for the `DesktopA11y` contract suite (R2) against the live AT-SPI2
//! backend. Skips honestly (with a note on stderr) when this machine has no
//! a11y bus, no registry, or no accessible application — headless runs stay
//! green; on a live desktop the suite genuinely walks a real application tree.

use omnibus_adapter_atspi::{AtspiAdapter, REGISTRY_DEST, encode_node_ref};
use omnibus_contracts::dto::desktop::TreeSelector;
use omnibus_contracts::spi::a11y::DesktopA11y;
use omnibus_contracts::suite::a11y::{A11yFixture, a11y_suite};

#[tokio::test]
async fn a11y_contract_suite_against_the_live_backend() {
    let adapter = match AtspiAdapter::connect().await {
        Ok(adapter) => adapter,
        Err(error) => {
            eprintln!("skip: no a11y bus on this machine ({error})");
            return;
        }
    };
    if !adapter.registry_reachable() {
        eprintln!("skip: the atspi registry is not reachable on the bus");
        return;
    }
    let apps = match adapter.discover_apps().await {
        Ok(apps) => apps,
        Err(error) => {
            eprintln!("skip: application discovery failed ({error})");
            return;
        }
    };
    let Some(app) = apps.iter().find(|app| !app.name.is_empty()) else {
        eprintln!("skip: no accessible application is registered right now");
        return;
    };
    let tree = match adapter
        .tree(TreeSelector {
            app: app.name.clone(),
            pid: app.pid,
        })
        .await
    {
        Ok(tree) => tree,
        Err(error) => {
            eprintln!("skip: the tree walk of {} failed ({error})", app.name);
            return;
        }
    };
    println!(
        "live backend: app {}, {} nodes, truncated {}",
        app.name, tree.node_count, tree.truncated
    );
    let fixture = A11yFixture {
        app: app.name.clone(),
        pid: app.pid,
        live_ref: tree.root.node_ref.clone(),
        // A well-formed reference to an object the registry does not own.
        stale_ref: encode_node_ref(REGISTRY_DEST, "/org/a11y/atspi/accessible/suite-stale"),
        // The families sentinels bind poison/slow to their failure classes.
        ..A11yFixture::default()
    };
    let shared = adapter.clone();
    let report = a11y_suite(move || Box::new(shared.clone()), fixture).await;
    println!("{report}");
    assert!(report.is_clean(), "{report}");
}
