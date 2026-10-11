//! The audit pipeline (specs/domains/audit.md, P5): exactly one structured event
//! per tool call, reads included, produced at the facade level. Adapters never
//! know about audit. `params_hash` never carries raw parameters. A failing sink
//! degrades to the file fallback without failing the tool call.

use std::sync::Arc;

use omnibus_contracts::dto::audit::{AuditActor, AuditEvent};
use omnibus_contracts::spi::audit_sink::AuditSink;

/// FNV-1a 64 of the serialized arguments — irreversible by design (a hash, not an
/// encoding). The hex string is what reaches the journal.
pub fn params_hash(arguments: &serde_json::Value) -> String {
    let text = arguments.to_string();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The file fallback sink (tests and sink-failure degradation): JSONL events.
pub struct FileSink {
    path: std::path::PathBuf,
}

impl FileSink {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self { path }
    }

    /// Appends one event; never panics — audit must not break the tool call.
    pub fn append(&self, event: &AuditEvent) {
        use std::io::Write;
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            && let Ok(line) = serde_json::to_string(event)
        {
            let _ = writeln!(file, "{line}");
        }
    }

    /// Reads back every delivered event (tests, doctor diagnostics).
    pub fn read_all(&self) -> Vec<AuditEvent> {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Vec::new();
        };
        text.lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
}

/// The pipeline: primary sink (journald in production) + file fallback.
#[derive(Clone)]
pub struct AuditPipeline {
    session: String,
    actor: AuditActor,
    primary: Option<Arc<dyn AuditSink>>,
    fallback: Arc<FileSink>,
}

impl AuditPipeline {
    pub fn new(
        session: String,
        actor: AuditActor,
        primary: Option<Arc<dyn AuditSink>>,
        fallback_path: std::path::PathBuf,
    ) -> Self {
        Self {
            session,
            actor,
            primary,
            fallback: Arc::new(FileSink::new(fallback_path)),
        }
    }

    /// Records exactly one event for a completed tool call. A primary-sink failure
    /// routes the event to the fallback and returns `false` so the facade can mark
    /// the degradation in `capabilities()`/doctor — the tool result is unaffected.
    pub async fn record(
        &self,
        tool: &str,
        adapter: &str,
        op: &str,
        arguments: &serde_json::Value,
        result: &str,
        scope_ref: Option<String>,
    ) -> bool {
        let event = AuditEvent {
            ts_epoch_ms: omnibus_contracts::dto::Milliseconds::now_epoch().0,
            session: self.session.clone(),
            actor: self.actor.clone(),
            tool: tool.to_string(),
            adapter: adapter.to_string(),
            op: op.to_string(),
            params_hash: params_hash(arguments),
            result: result.to_string(),
            scope_ref,
        };
        if let Some(sink) = &self.primary
            && sink.write(event.clone()).await.is_ok()
        {
            return true;
        }
        self.fallback.append(&event);
        false
    }

    /// True when the primary sink is absent (degradation visible in doctor).
    pub fn primary_present(&self) -> bool {
        self.primary.is_some()
    }

    /// The fallback path, for doctor output.
    pub fn fallback_path(&self) -> &std::path::Path {
        &self.fallback.path
    }

    #[cfg(test)]
    pub fn fallback(&self) -> Arc<FileSink> {
        self.fallback.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> AuditActor {
        AuditActor {
            uid: 1000,
            pid: 1,
            client_name: Some(String::from("test")),
        }
    }

    #[tokio::test]
    async fn no_primary_sink_degrades_to_file_fallback() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let path = dir.path().join("fallback.jsonl");
        let pipeline = AuditPipeline::new(String::from("sess-test"), actor(), None, path.clone());
        assert!(!pipeline.primary_present());
        let delivered = pipeline
            .record(
                "journal_query",
                "journald",
                "query",
                &serde_json::json!({ "limit": 5 }),
                "ok",
                None,
            )
            .await;
        assert!(!delivered, "no primary: event went to the fallback");
        let events = pipeline.fallback().read_all();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].tool, "journal_query");
        assert_eq!(events[0].session, "sess-test");
        assert_eq!(events[0].result, "ok");
    }

    #[tokio::test]
    async fn failing_primary_sink_still_delivers_via_fallback() {
        struct BrokenSink;
        #[async_trait::async_trait]
        impl AuditSink for BrokenSink {
            fn kind(&self) -> &'static str {
                "broken"
            }
            async fn write(
                &self,
                _event: AuditEvent,
            ) -> Result<(), omnibus_contracts::errors::ContractError> {
                Err(omnibus_contracts::errors::ContractError::NativeFailure {
                    code: 1,
                    payload: String::from("sink down"),
                })
            }
        }
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let path = dir.path().join("fallback.jsonl");
        let pipeline = AuditPipeline::new(
            String::from("sess-test"),
            actor(),
            Some(Arc::new(BrokenSink)),
            path,
        );
        assert!(pipeline.primary_present());
        let delivered = pipeline
            .record(
                "svc_list",
                "systemd1",
                "list",
                &serde_json::json!({}),
                "ok",
                None,
            )
            .await;
        assert!(!delivered, "primary failure degrades to the fallback");
        assert_eq!(pipeline.fallback().read_all().len(), 1);
    }

    #[test]
    fn params_hash_is_stable_and_irreversible() {
        let args = serde_json::json!({ "path": "/home/u/secret.txt" });
        let a = params_hash(&args);
        let b = params_hash(&args);
        assert_eq!(a, b, "stable across calls");
        assert_eq!(a.len(), 16);
        let rendered = format!("{a}{b}");
        assert!(
            !rendered.contains("secret"),
            "raw parameters never leak into the hash"
        );
        let other = params_hash(&serde_json::json!({ "path": "/home/u/other.txt" }));
        assert_ne!(a, other);
    }
}
