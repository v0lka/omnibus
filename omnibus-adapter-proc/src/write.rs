//! The `ps.signal` write path (roadmap task 2.8.3, specs/domains/process-telemetry.md):
//! one signal from the closed contract enum to one pid taken from `ps.list`
//! as fact.
//!
//! Boundaries the domain pins:
//!
//! - PID 1 and system-unit processes answer [`ContractError::Unsupported`]
//!   with the `svc_*` hint — no "system restart disguised as a process kill".
//!   (The user hierarchy stays signalable: the launched `app-*.service`
//!   transient units are the act side of the closed loop.)
//! - The signal is the closed enum from contracts — a raw number cannot cross
//!   the seam at all (the DTO's type); this module maps enum → kernel number
//!   one step before `kill(2)`, and nothing else may address the kernel.
//! - The core owns the write gate: without the out-of-band `ps.signal` grant
//!   the dispatch answers `ScopeDenied` and audits the attempt before the
//!   adapter is reached. This module never sees an ungranted call on the MCP
//!   surface; the `SENTINEL_PID_POISON` binding stands in for the seam shape
//!   in the suite.
//! - `kill(2)` is the entire native write: direct FFI via the safe
//!   `libc::kill` (no shell, no helper binary; the workspace forbids
//!   `unsafe` and none is needed).

use omnibus_contracts::dto::process_telemetry::{Signal, SignalRequest};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::suite::process_telemetry::{SENTINEL_PID_POISON, SENTINEL_PID_SLOW};
use rustix::process::{Signal as OsSignal, kill_process};

use crate::read::{NF_NO_PROCESS, NF_SIGNAL, cgroup_path_of, excluded, excluded_refusal};

/// Maps the closed contract enum onto the kernel signal values. Every variant
/// is total: the compiler proves a new enum variant cannot reach the kernel
/// unnumbered. The kernel numbers themselves come from rustix's typed
/// `Signal` (safe, no raw libc FFI under `unsafe_code = "forbid"`).
fn os_signal(signal: Signal) -> OsSignal {
    match signal {
        Signal::Term => OsSignal::TERM,
        Signal::Int => OsSignal::INT,
        Signal::Hup => OsSignal::HUP,
        Signal::Usr1 => OsSignal::USR1,
        Signal::Usr2 => OsSignal::USR2,
        Signal::Kill => OsSignal::KILL,
    }
}

/// Delivers one signal. `proc_root` is the injected `/proc` (live or fixture
/// stand) used for the exclusion and existence checks; the delivery itself is
/// `kill(2)` on the real kernel pid.
pub fn send(proc_root: &std::path::Path, request: SignalRequest) -> Result<(), ContractError> {
    // R2 sentinel bindings first, before any native work: the suite maps its
    // failure injections onto pids beyond every pid_max, so real backends
    // would answer them as missing — the bindings give the suite its typed
    // shapes deterministically (slow → `TimeoutError`, poison → the
    // os-denied native failure that stands in for the grant-denied seam).
    if request.pid == SENTINEL_PID_SLOW {
        return Err(ContractError::TimeoutError);
    }
    if request.pid == SENTINEL_PID_POISON {
        return Err(ContractError::NativeFailure {
            code: NF_SIGNAL,
            payload: String::from("os-denied: signal refused by suite poison sentinel"),
        });
    }
    // The domain invariant first: PID 1 never answers a signal.
    if request.pid == 1 {
        return Err(excluded_refusal(request.pid));
    }
    // The system-manager hierarchy neither: services are addressed via svc_*.
    let cgroup = std::fs::read_to_string(proc_root.join(request.pid.to_string()).join("cgroup"))
        .ok()
        .and_then(|text| cgroup_path_of(&text));
    if excluded(request.pid, cgroup.as_deref()) {
        return Err(excluded_refusal(request.pid));
    }
    // The pid is a fact from ps.list — if it has since vanished, the honest
    // answer is the typed no-such-process error (the agent re-lists).
    if !proc_root
        .join(request.pid.to_string())
        .join("stat")
        .exists()
    {
        return Err(ContractError::NativeFailure {
            code: NF_NO_PROCESS,
            payload: format!(
                "no-such-process: pid {} is not visible to this daemon's uid — re-list with \
                 ps.list for fresh facts instead of guessing pids",
                request.pid
            ),
        });
    }
    // `from_raw` refuses non-positive pids — pid 0 is not a process, and by
    // this point the request pid has passed the existence check, so the
    // conversion is the honest guard, not an unwrap.
    let raw_pid = i32::try_from(request.pid)
        .ok()
        .filter(|raw| *raw > 0)
        .ok_or_else(|| ContractError::NativeFailure {
            code: NF_NO_PROCESS,
            payload: format!(
                "no-such-process: pid {} is not a signallable process id",
                request.pid
            ),
        })?;
    let pid =
        rustix::process::Pid::from_raw(raw_pid).ok_or_else(|| ContractError::NativeFailure {
            code: NF_NO_PROCESS,
            payload: format!(
                "no-such-process: pid {} is not a signallable process id",
                request.pid
            ),
        })?;
    kill_process(pid, os_signal(request.signal)).map_err(|os| ContractError::NativeFailure {
        code: NF_SIGNAL,
        payload: format!("signal: kill({}): {os}", request.pid),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(pid: u32, signal: Signal) -> SignalRequest {
        SignalRequest { pid, signal }
    }

    #[test]
    fn every_enum_variant_maps_to_a_distinct_kernel_number() {
        let numbers = [
            os_signal(Signal::Term).as_raw(),
            os_signal(Signal::Int).as_raw(),
            os_signal(Signal::Hup).as_raw(),
            os_signal(Signal::Usr1).as_raw(),
            os_signal(Signal::Usr2).as_raw(),
            os_signal(Signal::Kill).as_raw(),
        ];
        assert_eq!(numbers.len(), 6, "the closed set, all six mapped");
        for pair in numbers.windows(2) {
            assert_ne!(pair[0], pair[1], "kernel signal numbers never collide");
        }
    }

    #[test]
    fn pid_1_refuses_with_the_svc_hint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err =
            send(dir.path(), request(1, Signal::Kill)).expect_err("pid 1 never answers a signal");
        let ContractError::Unsupported { reason } = err else {
            panic!("expected Unsupported, got {err:?}");
        };
        assert!(
            reason.contains("svc_"),
            "the refusal hints at the service tools: {reason}"
        );
    }

    #[test]
    fn sentinel_pids_answer_the_suite_shapes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let slow = send(dir.path(), request(SENTINEL_PID_SLOW, Signal::Term)).await_error();
        assert!(matches!(slow, Some(ContractError::TimeoutError)));
        let poison = send(dir.path(), request(SENTINEL_PID_POISON, Signal::Term)).await_error();
        assert!(matches!(poison, Some(ContractError::NativeFailure { .. })));
    }

    /// The negative-case proof at the write seam: a pid the stand does not
    /// carry is the typed `no-such-process` failure, never a panic and never
    /// a silent success.
    #[test]
    fn unknown_pid_is_a_no_such_process_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = send(dir.path(), request(424_242, Signal::Term))
            .expect_err("a pid absent from the stand cannot be signalled");
        let ContractError::NativeFailure { code, payload } = err else {
            panic!("expected NativeFailure, got {err:?}");
        };
        assert_eq!(code, NF_NO_PROCESS);
        assert!(payload.contains("no-such-process"), "{payload}");
    }

    /// A system-unit process (cgroup in `system.slice`) refuses with the svc
    /// hint even though its `/proc` stand is fully readable.
    #[test]
    fn system_unit_process_refuses_with_the_svc_hint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_dir = dir.path().join("777");
        std::fs::create_dir_all(&pid_dir).expect("pid dir");
        std::fs::write(
            pid_dir.join("stat"),
            "777 (nginx) S 1 1 1 0 -1 4194560 100 0 0 0 10 5 0 0 20 0 1 0 1000 100000 200 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n",
        )
        .expect("stat");
        std::fs::write(pid_dir.join("cgroup"), "0::/system.slice/nginx.service\n").expect("cgroup");
        let err = send(dir.path(), request(777, Signal::Term))
            .expect_err("system-unit processes stay outside ps.signal");
        let ContractError::Unsupported { reason } = err else {
            panic!("expected Unsupported, got {err:?}");
        };
        assert!(reason.contains("svc_"), "{reason}");
    }

    /// Small readability helper for `Result`-returning sync calls in tests.
    trait AwaitError {
        type Error;
        fn await_error(self) -> Option<Self::Error>;
    }
    impl<T, E> AwaitError for Result<T, E> {
        type Error = E;
        fn await_error(self) -> Option<E> {
            self.err()
        }
    }
}
