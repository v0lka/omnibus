//! Live binding of the NetworkStatus contract suite against this machine's
//! NetworkManager (roadmap 2.2.3: the suite runs green against the
//! implementation). When NM is absent on the system bus the test skips
//! gracefully — absence affects no other subsystem, including tests.

use omnibus_adapter_nm::NmAdapter;
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::net::NetworkStatus;
use omnibus_contracts::suite;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("test runtime must build")
}

#[test]
fn net_suite_exercises_the_live_nm() {
    let rt = runtime();
    let connect_result = rt.block_on(NmAdapter::connect());
    match connect_result {
        Ok(adapter) => {
            let report = rt.block_on(suite::net::net_suite(move || {
                Box::new(adapter.clone()) as Box<dyn omnibus_contracts::spi::net::NetworkStatus>
            }));
            assert!(
                report.is_clean(),
                "net suite red against live NM: {report:?}"
            );
        }
        // Graceful skip: behind netplan/networkd there is no NM and no suite.
        Err(ContractError::AdapterUnavailable) => {
            eprintln!("skipped: NetworkManager is not live on the system bus");
        }
        Err(other) => panic!("unexpected contract error connecting to NM: {other}"),
    }
}

#[test]
fn status_of_live_nm_carries_devices_and_capability_fact() {
    let rt = runtime();
    let connect_result = rt.block_on(NmAdapter::connect());
    let adapter = match connect_result {
        Ok(adapter) => adapter,
        Err(ContractError::AdapterUnavailable) => {
            eprintln!("skipped: NetworkManager is not live on the system bus");
            return;
        }
        Err(other) => panic!("unexpected contract error connecting to NM: {other}"),
    };
    let report = adapter.report();
    assert_eq!(report.adapter, "nm");
    assert_eq!(report.subsystem, "net");
    assert_eq!(report.capabilities.len(), 1);
    assert_eq!(
        report.capabilities[0].name,
        omnibus_contracts::capabilities::NET_READ
    );
    assert_eq!(report.capabilities[0].status, "present");

    let status = rt
        .block_on(adapter.status())
        .expect("live NM must yield status");
    assert!(
        !status.connectivity.is_empty(),
        "connectivity must be named"
    );
    assert!(
        !status.devices.is_empty(),
        "a live NM always knows at least one device"
    );
    for device in &status.devices {
        assert!(!device.name.is_empty());
        assert!(!device.kind.is_empty());
        assert!(!device.state.is_empty());
    }
}
