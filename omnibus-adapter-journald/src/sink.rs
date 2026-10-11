//! The `AuditSink` implementation over the journald ingestion socket
//! (specs/domains/audit.md, roadmap 1.3.4).
//!
//! Each `AuditEvent` becomes one datagram of newline-delimited journal-export
//! field pairs (`MESSAGE` carries the compact JSON of the whole event; the
//! `OMNIBUS_*` fields mirror the core schema; `SYSLOG_IDENTIFIER` marks the
//! daemon) sent to `/run/systemd/journal/socket`. The blocking datagram send
//! runs inside `tokio::task::spawn_blocking`. Socket failures surface as
//! `ContractError::NativeFailure` with the raw OS errno as the code and the
//! original message as the payload — the core then degrades to its file sink.
//! Native types (the socket) never leave this module.

use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use omnibus_contracts::dto::audit::AuditEvent;
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::audit_sink::AuditSink;

/// The journald ingestion socket (datagram, world-writable by design).
const JOURNAL_SOCKET: &str = "/run/systemd/journal/socket";
/// The syslog identifier every audit record carries.
const IDENTIFIER: &str = "omnibusd";
/// Native code when the OS layer produced no errno.
const NO_NATIVE_CODE: i32 = -1;

/// The journald-backed audit sink. Cheap to clone: the socket path and the
/// delivery counter live behind an `Arc`, so contract-suite factories can clone.
#[derive(Debug, Clone)]
pub struct JournaldAuditSink {
    inner: Arc<SinkState>,
}

/// Shared sink state; the counter observes successful deliveries.
#[derive(Debug)]
struct SinkState {
    socket: PathBuf,
    delivered: Arc<AtomicUsize>,
}

impl JournaldAuditSink {
    /// Composition-root constructor: the native socket must exist, otherwise the
    /// core falls back to its file sink.
    pub fn new() -> Result<Self, ContractError> {
        let socket = PathBuf::from(JOURNAL_SOCKET);
        if !socket.exists() {
            return Err(ContractError::NativeFailure {
                code: NO_NATIVE_CODE,
                payload: format!(
                    "journald ingestion socket {} is absent; the audit pipeline will use its file fallback",
                    socket.display()
                ),
            });
        }
        Ok(Self::at(socket, Arc::new(AtomicUsize::new(0))))
    }

    /// Crate-level constructor shared by `new` and the contract-suite factory.
    pub(crate) fn at(socket: PathBuf, delivered: Arc<AtomicUsize>) -> Self {
        Self {
            inner: Arc::new(SinkState { socket, delivered }),
        }
    }
}

impl Default for JournaldAuditSink {
    fn default() -> Self {
        // A sink without the backend is useless but harmless: `write` reports the
        // native failure and the core degrades to the file fallback.
        Self::at(PathBuf::from(JOURNAL_SOCKET), Arc::new(AtomicUsize::new(0)))
    }
}

#[async_trait]
impl AuditSink for JournaldAuditSink {
    fn kind(&self) -> &'static str {
        "journald"
    }

    /// Delivers exactly one datagram per event; any socket error is a native
    /// failure the core pipeline handles (never a panic).
    async fn write(&self, event: AuditEvent) -> Result<(), ContractError> {
        let message = serde_json::to_string(&event).map_err(|e| ContractError::NativeFailure {
            code: NO_NATIVE_CODE,
            payload: format!("cannot encode the audit event: {e}"),
        })?;
        let mut datagram = String::new();
        push_field(&mut datagram, "MESSAGE", &message);
        push_field(&mut datagram, "OMNIBUS_SESSION", &event.session);
        push_field(&mut datagram, "OMNIBUS_TOOL", &event.tool);
        push_field(&mut datagram, "OMNIBUS_ADAPTER", &event.adapter);
        push_field(&mut datagram, "OMNIBUS_OP", &event.op);
        push_field(&mut datagram, "OMNIBUS_RESULT", &event.result);
        push_field(&mut datagram, "OMNIBUS_PARAMS_HASH", &event.params_hash);
        if let Some(scope) = event.scope_ref.as_deref() {
            push_field(&mut datagram, "OMNIBUS_SCOPE_REF", scope);
        }
        push_field(&mut datagram, "SYSLOG_IDENTIFIER", IDENTIFIER);

        let path = self.inner.socket.clone();
        let bytes = datagram.into_bytes();
        let total = bytes.len();
        let joined = tokio::task::spawn_blocking(move || send_datagram(&path, &bytes))
            .await
            .map_err(|e| ContractError::NativeFailure {
                code: NO_NATIVE_CODE,
                payload: format!("audit sink worker failed: {e}"),
            })?;
        let sent = joined?;
        if sent == total {
            self.inner.delivered.fetch_add(1, Ordering::Relaxed);
            Ok(())
        } else {
            Err(ContractError::NativeFailure {
                code: NO_NATIVE_CODE,
                payload: format!("short datagram write: {sent} of {total} bytes"),
            })
        }
    }
}

/// Sends the whole payload as one datagram to the journal socket. Blocking by
/// nature, therefore always called through `spawn_blocking`.
fn send_datagram(path: &Path, payload: &[u8]) -> Result<usize, ContractError> {
    let socket = UnixDatagram::unbound().map_err(native_io)?;
    socket.connect(path).map_err(native_io)?;
    socket.send(payload).map_err(native_io)
}

/// Maps an OS error onto the taxonomy: raw errno as the code, original message
/// in the payload.
fn native_io(error: std::io::Error) -> ContractError {
    ContractError::NativeFailure {
        code: error.raw_os_error().unwrap_or(NO_NATIVE_CODE),
        payload: error.to_string(),
    }
}

/// Appends one journal-export field pair. Newlines and backslashes inside values
/// are escaped so every field stays on its own line (journal export format).
fn push_field(out: &mut String, name: &str, value: &str) {
    out.push_str(name);
    out.push('=');
    for ch in value.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\\' => out.push_str("\\\\"),
            other => out.push(other),
        }
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_contracts::dto::audit::AuditActor;
    use omnibus_contracts::suite::families::audit_sink::{
        SinkBinding, SinkFactory, SuiteProbe, audit_sink_suite,
    };

    #[test]
    fn fields_escape_newlines_and_backslashes() {
        let mut out = String::new();
        push_field(&mut out, "OMNIBUS_RESULT", "line1\nline2\\path");
        assert_eq!(out, "OMNIBUS_RESULT=line1\\nline2\\\\path\n");
    }

    #[test]
    fn one_event_frames_into_exactly_the_contracted_fields() {
        let event = AuditEvent {
            ts_epoch_ms: 1_700_000_000_000,
            session: String::from("sess-x"),
            actor: AuditActor {
                uid: 1000,
                pid: 42,
                client_name: Some(String::from("suite")),
            },
            tool: String::from("suite.tool"),
            adapter: String::from("suite"),
            op: String::from("op"),
            params_hash: String::from("0"),
            result: String::from("ok"),
            scope_ref: None,
        };
        let message = serde_json::to_string(&event).expect("event must encode");
        let mut datagram = String::new();
        push_field(&mut datagram, "MESSAGE", &message);
        push_field(&mut datagram, "OMNIBUS_SESSION", &event.session);
        push_field(&mut datagram, "OMNIBUS_TOOL", &event.tool);
        push_field(&mut datagram, "OMNIBUS_ADAPTER", &event.adapter);
        push_field(&mut datagram, "OMNIBUS_OP", &event.op);
        push_field(&mut datagram, "OMNIBUS_RESULT", &event.result);
        push_field(&mut datagram, "OMNIBUS_PARAMS_HASH", &event.params_hash);
        push_field(&mut datagram, "SYSLOG_IDENTIFIER", IDENTIFIER);
        let lines: Vec<&str> = datagram.lines().collect();
        assert_eq!(lines.len(), 8);
        assert!(lines[0].starts_with("MESSAGE="));
        assert_eq!(lines[0]["MESSAGE=".len()..], message);
        assert_eq!(lines[1], "OMNIBUS_SESSION=sess-x");
        assert_eq!(lines[2], "OMNIBUS_TOOL=suite.tool");
        assert_eq!(lines[3], "OMNIBUS_ADAPTER=suite");
        assert_eq!(lines[4], "OMNIBUS_OP=op");
        assert_eq!(lines[5], "OMNIBUS_RESULT=ok");
        assert_eq!(lines[6], "OMNIBUS_PARAMS_HASH=0");
        assert_eq!(lines[7], "SYSLOG_IDENTIFIER=omnibusd");
    }

    #[test]
    fn kind_names_the_backend() {
        let sink = JournaldAuditSink::default();
        assert_eq!(sink.kind(), "journald");
    }

    /// The suite probe reads the sink's delivery counter.
    struct DeliveryProbe {
        counter: Arc<AtomicUsize>,
    }

    impl SuiteProbe for DeliveryProbe {
        fn delivered(&self) -> usize {
            self.counter.load(Ordering::Relaxed)
        }
    }

    /// Builds sink-plus-probe pairs sharing one delivery counter.
    struct SuiteFactory {
        socket: PathBuf,
        counter: Arc<AtomicUsize>,
    }

    impl SinkFactory for SuiteFactory {
        type Probe = DeliveryProbe;

        fn make(&self) -> SinkBinding<Self::Probe> {
            SinkBinding {
                sink: Box::new(JournaldAuditSink::at(
                    self.socket.clone(),
                    Arc::clone(&self.counter),
                )),
                probe: Arc::new(DeliveryProbe {
                    counter: Arc::clone(&self.counter),
                }),
            }
        }
    }

    /// Binds the contract suite against the live ingestion socket; machines
    /// without the socket skip gracefully (headless proxy-check policy).
    #[tokio::test]
    async fn contract_suite_passes_on_the_live_socket() {
        let socket = PathBuf::from(JOURNAL_SOCKET);
        if !socket.exists() {
            eprintln!(
                "skipping the AuditSink contract suite: no journald ingestion socket at {}",
                socket.display()
            );
            return;
        }
        let report = audit_sink_suite(SuiteFactory {
            socket,
            counter: Arc::new(AtomicUsize::new(0)),
        })
        .await;
        assert!(report.is_clean(), "{report}");
    }
}
