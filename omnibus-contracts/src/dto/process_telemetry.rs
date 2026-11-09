//! Process-telemetry DTOs (specs/domains/process-telemetry.md). Limits and
//! filters travel inside the DTO (the `journal_query` discipline); addressing
//! is always by `pid` taken from `ps.list` as fact, never by a guessed name;
//! the signal is a closed contract enum — a raw signal number cannot cross
//! the seam (roadmap task 2.8.1).

use crate::dto::{Bytes, Milliseconds, Percent, Seconds};

crate::dto_enum!(
    /// The closed signal set `ps.signal` accepts: the signal is an enum in the
    /// contract, never a number (roadmap task 2.8.1; domain Invariants). Wire
    /// forms are lowercase strings; a signal outside this set is
    /// unrepresentable at the seam, which is the "raw number always fails" rule
    /// enforced by types.
    pub enum Signal {
        Term,
        Int,
        Hup,
        Usr1,
        Usr2,
        Kill,
    }
);

crate::dto!(
    /// `ps.list` limits and filters — they travel in the DTO, the same
    /// discipline as `journal_query` (specs/domains/process-telemetry.md
    /// Behavior); never free parameters on the trait.
    pub struct ProcessListQuery {
        pub name_contains: Option<String>,
        pub limit: Option<u32>,
    }
);

crate::dto!(
    /// One process as `ps.list` reports it: the `/proc` facts visible to the
    /// daemon's own uid, without escalations. PID 1 and systemd-unit processes
    /// never appear — services are addressed only through the services
    /// subsystem (`svc_*`).
    pub struct ProcessEntry {
        pub pid: u32,
        pub name: String,
        pub state: String,
        pub cpu_percent: Percent,
        pub rss_bytes: Bytes,
        pub cgroup: Option<String>,
    }
);

crate::dto!(
    /// A consistent per-PID snapshot (`ps.info`): every field is observed at
    /// one instant, never stitched from separate samples. A nonexistent pid
    /// yields a contract error, never a panic.
    pub struct ProcessSnapshot {
        pub pid: u32,
        pub name: String,
        pub state: String,
        pub cpu_percent: Percent,
        pub rss_bytes: Bytes,
        pub cgroup: Option<String>,
        pub command_line: Option<String>,
        pub start_time_epoch_ms: Milliseconds,
    }
);

crate::dto!(
    /// `sys.stats`: CPU load, load average, memory and swap facts read from
    /// `/proc` and `/sys`. Load averages are the kernel's own numbers and may
    /// exceed the core count — they are honest floats, not percents.
    pub struct SysStats {
        pub cpu_percent: Percent,
        pub load_avg_1m: f64,
        pub load_avg_5m: f64,
        pub load_avg_15m: f64,
        pub mem_total_bytes: Bytes,
        pub mem_used_bytes: Bytes,
        pub swap_total_bytes: Bytes,
        pub swap_used_bytes: Bytes,
        pub uptime_seconds: Seconds,
    }
);

crate::dto!(
    /// `power.status`: battery and supply facts through UPower, "when present"
    /// (probe auto-detect). On a machine without UPower/battery the tool is
    /// hidden (R3/R7) — a direct call answers `CapabilityRequired`, never a
    /// hollow report.
    pub struct PowerStatus {
        pub on_battery: bool,
        pub battery_percent: Option<Percent>,
        pub seconds_until_empty: Option<Seconds>,
        pub seconds_until_full: Option<Seconds>,
    }
);

crate::dto!(
    /// `ps.info` request: the pid is taken from `ps.list` as fact — a named
    /// struct, never a bare number on the wire (the DTO discipline).
    pub struct PidRequest {
        pub pid: u32,
    }
);

crate::dto!(
    /// `ps.signal` (write, granted) request: one closed-enum signal to one pid
    /// taken from `ps.list` as fact — a named struct, never a tuple.
    pub struct SignalRequest {
        pub pid: u32,
        pub signal: Signal,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DtoSchema, sample};

    #[test]
    fn process_telemetry_dto_roundtrips_with_schema() {
        for node in [
            &Signal::schema() as &crate::schema::SchemaNode,
            &ProcessListQuery::schema(),
            &PidRequest::schema(),
            &ProcessEntry::schema(),
            &ProcessSnapshot::schema(),
            &SysStats::schema(),
            &PowerStatus::schema(),
            &SignalRequest::schema(),
        ] {
            let value = sample(node, 0);
            let text = serde_json::to_string(&value).unwrap_or_default();
            let back = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            assert_eq!(back, value);
        }
    }

    #[test]
    fn signal_wire_forms_are_the_closed_kebab_set() {
        // The closed enum, exactly the six roadmap names; a raw number or an
        // unknown string is unrepresentable (the parse direction is the proof).
        assert_eq!(
            serde_json::from_value::<Signal>(serde_json::json!("term")).unwrap_or(Signal::Kill),
            Signal::Term
        );
        for (wire, signal) in [
            ("term", Signal::Term),
            ("int", Signal::Int),
            ("hup", Signal::Hup),
            ("usr1", Signal::Usr1),
            ("usr2", Signal::Usr2),
            ("kill", Signal::Kill),
        ] {
            assert_eq!(
                serde_json::from_value::<Signal>(serde_json::json!(wire)).unwrap_or(Signal::Kill),
                signal,
                "wire form {wire} parses to the typed variant"
            );
        }
        assert!(
            serde_json::from_value::<Signal>(serde_json::json!(9)).is_err(),
            "a raw signal number must not parse"
        );
        assert!(
            serde_json::from_value::<Signal>(serde_json::json!("sigkill")).is_err(),
            "an off-registry name must not parse"
        );
    }
}
