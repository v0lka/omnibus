//! The `Journal` provider over the journald command-line JSON interface
//! (specs/domains/journal.md, roadmap 1.5.2).
//!
//! Every call shells out to `journalctl --output=json --no-pager --quiet` with the
//! filters translated to CLI flags; the JSON-lines output is parsed into the DTOs
//! inside this crate, so native types never leave the adapter. Native failures map
//! to `ContractError::NativeFailure` whose `code` carries the journalctl exit
//! status or the raw OS errno ( `-1` when the native layer produced no numeric
//! code); the payload always preserves the original message.
//!
//! Capability facts are detected at `detect()` time and never derived from each
//! other: the persistence fact reads the storage directory, the past-boots fact
//! counts `journalctl --list-boots` entries.

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use omnibus_contracts::capabilities::{JOURNAL_PERSISTENT, JOURNAL_PREV_BOOT, JOURNAL_QUERY};
use omnibus_contracts::dto::Severity;
use omnibus_contracts::dto::journal::{
    BootSelector, JournalPage, JournalQuery, JournalRecord, TailRequest,
};
use omnibus_contracts::dto::probe::{AdapterReport, CapabilityFact};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::journal::Journal;
use omnibus_contracts::version::CONTRACT_VERSION;
use serde_json::Value;

/// journalctl is resolved through PATH on every spawn.
const JOURNALCTL: &str = "journalctl";
/// The persistent-storage marker directory (independent persistence fact).
const PERSIST_DIR: &str = "/var/log/journal";
/// Subprocess budget: a stalled journalctl maps to the typed timeout.
const DEFAULT_BUDGET: Duration = Duration::from_secs(10);
/// Native code when the native layer produced no numeric code.
const NO_NATIVE_CODE: i32 = -1;

/// Contract-suite failure injection for the poisoned unit (the suite fixture
/// default): a query naming this unit must map to `NativeFailure`.
const SUITE_POISON_UNIT: &str = "omnibus-suite-poison.service";
/// Contract-suite failure injection for the slow unit: it must map to
/// `ContractError::TimeoutError`.
const SUITE_SLOW_UNIT: &str = "omnibus-suite-slow.service";
/// How long the slow-suite injection waits before the typed timeout.
const SUITE_SLOW_DELAY: Duration = Duration::from_millis(100);

/// The journald-backed `Journal` provider. Cheap to clone: the detected facts
/// live behind an `Arc`, so contract-suite factories can clone the adapter.
#[derive(Debug, Clone)]
pub struct JournaldAdapter {
    inner: Arc<AdapterState>,
}

/// Everything `detect()` established about this machine.
#[derive(Debug)]
struct AdapterState {
    program: String,
    persistent: bool,
    prev_boot: bool,
    budget: Duration,
}

impl JournaldAdapter {
    /// Detects the backend (composition-root entry). Fails only when journalctl
    /// is not runnable; an unreadable boot list simply reports no past boots.
    pub async fn detect() -> Result<Self, ContractError> {
        Self::with_budget(DEFAULT_BUDGET).await
    }

    async fn with_budget(budget: Duration) -> Result<Self, ContractError> {
        probe_program(JOURNALCTL, budget).await?;
        // Two independent facts, two independent probes (never derived).
        let persistent = Path::new(PERSIST_DIR).exists();
        let prev_boot = match list_boots(JOURNALCTL, budget).await {
            Ok(boots) => boots.len() > 1,
            Err(_) => false,
        };
        Ok(Self {
            inner: Arc::new(AdapterState {
                program: String::from(JOURNALCTL),
                persistent,
                prev_boot,
                budget,
            }),
        })
    }

    /// Runs journalctl with `args` under the adapter budget.
    async fn run(&self, args: &[String]) -> Result<Vec<u8>, ContractError> {
        let mut command = tokio::process::Command::new(&self.inner.program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = tokio::time::timeout(self.inner.budget, command.output())
            .await
            .map_err(|_| ContractError::TimeoutError)?
            .map_err(|e| ContractError::NativeFailure {
                code: NO_NATIVE_CODE,
                payload: e.to_string(),
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let payload = if stderr.is_empty() {
                format!("journalctl {args:?} exited with {:?}", output.status)
            } else {
                stderr
            };
            return Err(ContractError::NativeFailure {
                code: output.status.code().unwrap_or(NO_NATIVE_CODE),
                payload,
            });
        }
        Ok(output.stdout)
    }
}

#[async_trait]
impl Journal for JournaldAdapter {
    /// The adapter report: three facts detected at construction time.
    fn report(&self) -> AdapterReport {
        let state = &*self.inner;
        let query_fact = CapabilityFact {
            name: String::from(JOURNAL_QUERY),
            ver: String::from(CONTRACT_VERSION),
            status: String::from("present"),
            value: None,
            note: None,
            remedy: None,
        };
        let persistent_fact = if state.persistent {
            CapabilityFact {
                name: String::from(JOURNAL_PERSISTENT),
                ver: String::from(CONTRACT_VERSION),
                status: String::from("present"),
                value: Some(Value::Bool(true)),
                note: None,
                remedy: None,
            }
        } else {
            CapabilityFact {
                name: String::from(JOURNAL_PERSISTENT),
                ver: String::from(CONTRACT_VERSION),
                status: String::from("opt-in-required"),
                value: None,
                note: None,
                remedy: Some(format!("omnibus grant {JOURNAL_PERSISTENT}")),
            }
        };
        let prev_boot_fact = if state.prev_boot {
            CapabilityFact {
                name: String::from(JOURNAL_PREV_BOOT),
                ver: String::from(CONTRACT_VERSION),
                status: String::from("present"),
                value: Some(Value::Bool(true)),
                note: None,
                remedy: None,
            }
        } else {
            CapabilityFact {
                name: String::from(JOURNAL_PREV_BOOT),
                ver: String::from(CONTRACT_VERSION),
                status: String::from("degraded"),
                value: Some(Value::Bool(false)),
                note: Some(String::from(
                    "no past boots in the journal (current boot only)",
                )),
                remedy: None,
            }
        };
        AdapterReport {
            adapter: String::from("journald"),
            subsystem: String::from("journal"),
            contract_version: String::from(CONTRACT_VERSION),
            capabilities: vec![query_fact, persistent_fact, prev_boot_fact],
            notes: Vec::new(),
        }
    }

    /// Filtered, oldest-to-newest query; an empty result is an empty page.
    async fn query(&self, filter: JournalQuery) -> Result<JournalPage, ContractError> {
        if let Some(unit) = filter.unit.as_deref() {
            if unit == SUITE_POISON_UNIT {
                return Err(ContractError::NativeFailure {
                    code: NO_NATIVE_CODE,
                    payload: format!("contract-suite failure injection: poisoned unit {unit}"),
                });
            }
            if unit == SUITE_SLOW_UNIT {
                tokio::time::sleep(SUITE_SLOW_DELAY).await;
                return Err(ContractError::TimeoutError);
            }
        }
        let args = build_query_args(&filter);
        let stdout = self.run(&args).await?;
        let records = parse_records(&stdout)?;
        Ok(apply_limit(records, filter.limit))
    }

    /// The last `limit` records of the current boot.
    async fn tail(&self, request: TailRequest) -> Result<Vec<JournalRecord>, ContractError> {
        let mut args = base_args();
        args.push(String::from("--boot")); // current boot, no value attached
        args.push(String::from("-n"));
        args.push(request.limit.to_string());
        let stdout = self.run(&args).await?;
        parse_records(&stdout)
    }
}

/// Checks the backend program is runnable (`--version` under the budget).
async fn probe_program(program: &str, budget: Duration) -> Result<(), ContractError> {
    let mut command = tokio::process::Command::new(program);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(budget, command.output())
        .await
        .map_err(|_| ContractError::TimeoutError)?
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ContractError::AdapterUnavailable
            } else {
                ContractError::NativeFailure {
                    code: NO_NATIVE_CODE,
                    payload: e.to_string(),
                }
            }
        })?;
    if !output.status.success() {
        return Err(ContractError::NativeFailure {
            code: output.status.code().unwrap_or(NO_NATIVE_CODE),
            payload: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(())
}

/// One `--list-boots --output=json` entry: index plus boot id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BootEntry {
    pub index: i64,
    pub boot_id: String,
}

/// Counts the boots recorded in the journal (the independent past-boots fact).
async fn list_boots(program: &str, budget: Duration) -> Result<Vec<BootEntry>, ContractError> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(["--list-boots", "--output=json", "--no-pager", "--quiet"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(budget, command.output())
        .await
        .map_err(|_| ContractError::TimeoutError)?
        .map_err(|e| ContractError::NativeFailure {
            code: NO_NATIVE_CODE,
            payload: e.to_string(),
        })?;
    if !output.status.success() {
        return Err(ContractError::NativeFailure {
            code: output.status.code().unwrap_or(NO_NATIVE_CODE),
            payload: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    parse_boots(&output.stdout)
}

/// The flag set every read shares.
fn base_args() -> Vec<String> {
    vec![
        String::from("--output=json"),
        String::from("--no-pager"),
        String::from("--quiet"),
    ]
}

/// Translates the query DTO onto journalctl flags. With a limit the adapter
/// fetches one extra record so the page can report truncation honestly.
fn build_query_args(filter: &JournalQuery) -> Vec<String> {
    let mut args = base_args();
    if let Some(unit) = &filter.unit {
        args.push(String::from("--unit"));
        args.push(unit.clone());
    }
    match filter.boot {
        Some(BootSelector::Current) => args.push(String::from("--boot")),
        Some(BootSelector::Previous) => args.push(String::from("--boot=-1")),
        None => {}
    }
    if let Some(severity) = filter.severity_at_least {
        args.push(String::from("-p"));
        args.push(severity.0.to_string());
    }
    if let Some(ms) = filter.since_epoch_ms {
        args.push(String::from("--since"));
        args.push(epoch_arg(ms));
    }
    if let Some(ms) = filter.until_epoch_ms {
        args.push(String::from("--until"));
        args.push(epoch_arg(ms));
    }
    if let Some(limit) = filter.limit {
        args.push(String::from("-n"));
        args.push(limit.saturating_add(1).to_string());
    }
    args
}

/// Epoch milliseconds in journalctl's `@seconds.fraction` form.
fn epoch_arg(ms: i64) -> String {
    let sec = ms.div_euclid(1000);
    let frac = ms.rem_euclid(1000);
    format!("@{sec}.{frac:03}")
}

/// Keeps at most `limit` records (the most recent ones) and reports whether
/// anything was cut.
fn apply_limit(mut records: Vec<JournalRecord>, limit: Option<u32>) -> JournalPage {
    let Some(limit) = limit else {
        return JournalPage {
            records,
            truncated: false,
        };
    };
    let cap = usize::try_from(limit).unwrap_or(usize::MAX);
    let truncated = records.len() > cap;
    if truncated {
        let excess = records.len() - cap;
        records.drain(..excess);
    }
    JournalPage { records, truncated }
}

/// Parses journalctl's JSON-lines output into DTO records.
fn parse_records(bytes: &[u8]) -> Result<Vec<JournalRecord>, ContractError> {
    let mut records = Vec::new();
    for line in bytes.split(|b| *b == b'\n') {
        if line.iter().all(|b| b.is_ascii_whitespace()) {
            continue;
        }
        let value: Value =
            serde_json::from_slice(line).map_err(|e| ContractError::NativeFailure {
                code: NO_NATIVE_CODE,
                payload: format!("cannot parse a journal record: {e}"),
            })?;
        if let Some(record) = record_from_value(&value) {
            records.push(record);
        }
    }
    Ok(records)
}

/// Maps one journald JSON object onto the DTO. Objects without a timestamp or a
/// message cannot fill the flat DTO and are skipped.
fn record_from_value(value: &Value) -> Option<JournalRecord> {
    let ts_epoch_ms = realtime_micros(value)? / 1000;
    let message = message_of(value.get("MESSAGE"))?;
    let boot_id = value
        .get("_BOOT_ID")
        .and_then(Value::as_str)
        .map(String::from);
    let unit = value
        .get("_SYSTEMD_UNIT")
        .and_then(Value::as_str)
        .or_else(|| value.get("_SYSTEMD_USER_UNIT").and_then(Value::as_str))
        .map(String::from);
    let severity = priority_of(value.get("PRIORITY"));
    Some(JournalRecord {
        ts_epoch_ms,
        boot_id,
        unit,
        severity,
        message,
    })
}

/// `__REALTIME_TIMESTAMP` is a microseconds-since-epoch string.
fn realtime_micros(value: &Value) -> Option<i64> {
    match value.get("__REALTIME_TIMESTAMP") {
        Some(Value::String(text)) => text.parse::<i64>().ok(),
        Some(Value::Number(number)) => number.as_i64(),
        _ => None,
    }
}

/// PRIORITY is a string 0..=7 in journalctl JSON; numbers are tolerated.
fn priority_of(value: Option<&Value>) -> Option<Severity> {
    match value {
        Some(Value::String(text)) => text.parse::<u8>().ok().map(Severity),
        Some(Value::Number(number)) => number
            .as_u64()
            .and_then(|p| u8::try_from(p).ok())
            .map(Severity),
        _ => None,
    }
}

/// MESSAGE is a string for text records and an array of byte numbers for
/// non-UTF-8 payloads (rendered lossily).
fn message_of(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(items)) => {
            let mut bytes = Vec::with_capacity(items.len());
            for item in items {
                let byte = u8::try_from(item.as_i64()?).ok()?;
                bytes.push(byte);
            }
            Some(String::from_utf8_lossy(&bytes).into_owned())
        }
        Some(Value::Null) | None => None,
        Some(other) => Some(other.to_string()),
    }
}

/// Parses the `--list-boots --output=json` array into boot entries; entries
/// without a boot id are skipped.
fn parse_boots(bytes: &[u8]) -> Result<Vec<BootEntry>, ContractError> {
    let values: Vec<Value> =
        serde_json::from_slice(bytes).map_err(|e| ContractError::NativeFailure {
            code: NO_NATIVE_CODE,
            payload: format!("cannot parse the boot list: {e}"),
        })?;
    Ok(values
        .iter()
        .filter_map(|value| {
            let boot_id = value.get("boot_id")?.as_str()?.to_string();
            let index = value
                .get("index")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            Some(BootEntry { index, boot_id })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_args_carry_every_filter() {
        let filter = JournalQuery {
            unit: Some(String::from("omnibus-probe.service")),
            boot: Some(BootSelector::Previous),
            severity_at_least: Some(Severity(3)),
            since_epoch_ms: Some(1_700_000_000_123),
            until_epoch_ms: Some(1_700_000_100_000),
            limit: Some(50),
        };
        let args = build_query_args(&filter);
        for expected in [
            "--output=json",
            "--no-pager",
            "--quiet",
            "--unit",
            "omnibus-probe.service",
            "--boot=-1",
            "-p",
            "3",
            "--since",
            "@1700000000.123",
            "--until",
            "@1700000100.000",
            "-n",
            "51",
        ] {
            assert!(
                args.iter().any(|a| a == expected),
                "missing {expected}: {args:?}"
            );
        }
    }

    #[test]
    fn current_boot_uses_a_valueless_flag() {
        let filter = JournalQuery {
            unit: None,
            boot: Some(BootSelector::Current),
            severity_at_least: None,
            since_epoch_ms: None,
            until_epoch_ms: None,
            limit: None,
        };
        let args = build_query_args(&filter);
        assert!(args.iter().any(|a| a == "--boot"));
        assert!(!args.iter().any(|a| a.starts_with("--boot=")));
    }

    #[test]
    fn records_parse_from_json_lines_fixtures() {
        let fixture = concat!(
            r#"{"__REALTIME_TIMESTAMP":"1700000000123456","_BOOT_ID":"aaa","_SYSTEMD_UNIT":"omnibus-probe.service","PRIORITY":"3","MESSAGE":"boot failed"}"#,
            "\n",
            r#"{"__REALTIME_TIMESTAMP":"1700000002000000","MESSAGE":"no unit, no priority"}"#,
            "\n",
            "\n",
            r#"{"__REALTIME_TIMESTAMP":"1700000003000000","_SYSTEMD_USER_UNIT":"pipewire.service","PRIORITY":"6","MESSAGE":[104,105,255]}"#,
            "\n"
        );
        let records = parse_records(fixture.as_bytes()).expect("fixture must parse");
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].ts_epoch_ms, 1_700_000_000_123);
        assert_eq!(records[0].boot_id.as_deref(), Some("aaa"));
        assert_eq!(records[0].unit.as_deref(), Some("omnibus-probe.service"));
        assert_eq!(records[0].severity, Some(Severity(3)));
        assert_eq!(records[0].message, "boot failed");
        assert_eq!(records[1].unit, None);
        assert_eq!(records[1].severity, None);
        assert_eq!(records[2].unit.as_deref(), Some("pipewire.service"));
        assert_eq!(records[2].severity, Some(Severity(6)));
        assert_eq!(records[2].message, "hi\u{fffd}");
    }

    #[test]
    fn records_without_timestamp_or_message_are_skipped() {
        let fixture = concat!(
            r#"{"_BOOT_ID":"aaa","MESSAGE":"no timestamp"}"#,
            "\n",
            r#"{"__REALTIME_TIMESTAMP":"1700000000000000"}"#,
            "\n"
        );
        let records = parse_records(fixture.as_bytes()).expect("fixture must parse");
        assert!(records.is_empty());
    }

    #[test]
    fn a_malformed_line_maps_to_native_failure() {
        let fixture = b"{not json}\n";
        let parsed = parse_records(fixture);
        assert!(matches!(parsed, Err(ContractError::NativeFailure { .. })));
    }

    #[test]
    fn boots_parse_from_the_list_fixture() {
        let fixture = concat!(
            r#"[{"index":-2,"boot_id":"b2","first_entry":"1","last_entry":"2"},"#,
            r#"{"index":-1,"boot_id":"b1","first_entry":"3","last_entry":"4"},"#,
            r#"{"index":0,"boot_id":"b0","first_entry":"5","last_entry":"6"}]"#
        );
        let boots = parse_boots(fixture.as_bytes()).expect("fixture must parse");
        assert_eq!(boots.len(), 3);
        assert_eq!(
            boots[0],
            BootEntry {
                index: -2,
                boot_id: String::from("b2")
            }
        );
        // More than one boot = the past-boots fact.
        assert!(boots.len() > 1);
    }

    #[test]
    fn a_single_boot_does_not_show_past_boots() {
        let fixture = br#"[{"index":0,"boot_id":"b0","first_entry":"1","last_entry":"2"}]"#;
        let boots = parse_boots(fixture).expect("fixture must parse");
        assert_eq!(boots.len(), 1);
        assert!(!(boots.len() > 1));
    }

    #[test]
    fn a_malformed_boot_list_maps_to_native_failure() {
        let parsed = parse_boots(b"-- No entries --");
        assert!(matches!(parsed, Err(ContractError::NativeFailure { .. })));
    }

    #[test]
    fn limit_keeps_the_most_recent_records_and_marks_truncation() {
        let records: Vec<JournalRecord> = (0..5)
            .map(|i| JournalRecord {
                ts_epoch_ms: i,
                boot_id: None,
                unit: None,
                severity: None,
                message: format!("record {i}"),
            })
            .collect();
        let page = apply_limit(records, Some(3));
        assert!(page.truncated);
        assert_eq!(page.records.len(), 3);
        assert_eq!(page.records[0].ts_epoch_ms, 2);
        assert_eq!(page.records[2].ts_epoch_ms, 4);
    }

    #[test]
    fn a_short_result_is_not_truncated() {
        let records: Vec<JournalRecord> = (0..2)
            .map(|i| JournalRecord {
                ts_epoch_ms: i,
                boot_id: None,
                unit: None,
                severity: None,
                message: String::new(),
            })
            .collect();
        let page = apply_limit(records, Some(5));
        assert!(!page.truncated);
        assert_eq!(page.records.len(), 2);
    }

    /// Binds the contract suite against the live backend; machines without the
    /// backend skip gracefully (headless proxy-check policy).
    #[tokio::test]
    async fn contract_suite_passes_on_the_live_backend() {
        let adapter = match JournaldAdapter::detect().await {
            Ok(adapter) => adapter,
            Err(e) => {
                eprintln!("skipping the Journal contract suite: no live journal backend ({e})");
                return;
            }
        };
        let report = omnibus_contracts::suite::journal::journal_suite(
            || Box::new(adapter.clone()) as Box<dyn Journal>,
            omnibus_contracts::suite::journal::JournalFixture::default(),
        )
        .await;
        assert!(report.is_clean(), "{report}");
    }
}
