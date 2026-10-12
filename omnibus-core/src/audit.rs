//! The audit pipeline (specs/domains/audit.md, P5): exactly one structured event
//! per tool call, reads included, produced at the facade level. Adapters never
//! know about audit. `params_hash` never carries raw parameters and never
//! reversibly restores them — the hash is keyed with a per-install secret, so
//! low-entropy parameters cannot be recovered by enumeration. A failing (or
//! hung, or unwritable-fallback) sink degrades without failing the tool call,
//! and the degradation is visible to doctor. Primary delivery is at-least-once
//! under a hang: an event whose sink write completes after the sink budget
//! elapses may surface in both the primary and the fallback — duplicates are
//! preferred over loss for an audit trail.

use std::sync::Arc;

use omnibus_contracts::dto::Milliseconds;
use omnibus_contracts::dto::audit::{AuditActor, AuditEvent};
use omnibus_contracts::spi::audit_sink::AuditSink;

/// Wall-clock bound on one primary-sink write (audit.md: a failing sink never
/// fails the tool call — a *hung* sink is a failure, not an excuse to wait).
pub const AUDIT_SINK_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

/// The name of the per-install key file kept next to the audit fallback (the
/// state directory).
const PARAMS_KEY_FILE: &str = "params-hash.key";

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        hash ^= hash >> 29;
    }
    hash
}

/// Keyed hash of the serialized arguments: FNV-1a-64 over `key || arguments`,
/// mixed per byte. An unkeyed 64-bit hash of dictionary-shaped parameters
/// (package and unit names, `$HOME`-relative paths) is trivially restorable by
/// enumeration; keyed with the per-install secret, restoration requires the
/// daemon's key (audit.md: parameters are never reversibly restored).
pub fn params_hash_keyed(key: &[u8; 32], arguments: &serde_json::Value) -> String {
    let text = arguments.to_string();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.iter().copied().chain(text.as_bytes().iter().copied()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        hash ^= hash >> 29;
    }
    format!("{hash:016x}")
}

/// The legacy zero-key digest, kept test-side only: it is exactly what events
/// must NOT carry (see the `params_hash_is_keyed_per_install` test below).
/// Shipped code always goes through the keyed [`AuditPipeline::record`].
#[cfg(test)]
fn params_hash(arguments: &serde_json::Value) -> String {
    params_hash_keyed(&[0u8; 32], arguments)
}

/// 32 bytes from the platform entropy source; the audit module's entropy-source
/// access, confined here (layers-and-crates.md "Carve-out": no native types cross the
/// seam, and nothing OS-shaped leaves this function).
fn random_key() -> Option<[u8; 32]> {
    use std::io::Read;
    let mut key = [0u8; 32];
    let mut source = std::fs::File::open("/dev/urandom").ok()?;
    source.read_exact(&mut key).ok()?;
    Some(key)
}

/// Weak degradation: derived from the state path, process id, and clock. The
/// inputs are weak, not merely non-secret: the state path is guessable and
/// the pid is journal-visible in every event's actor, so the only real
/// unknown left is the start millisecond — this key does NOT deliver the
/// anti-enumeration property the persisted per-install key provides (the
/// bounded exception documented in specs/domains/audit.md), and it is used
/// only when neither the key file nor the entropy source is available.
fn weak_key(fallback_path: &std::path::Path) -> [u8; 32] {
    let mut seed = format!(
        "{}|{}|{}",
        fallback_path.display(),
        std::process::id(),
        Milliseconds::now_epoch().0
    );
    let mut key = [0u8; 32];
    for (round, slot) in key.chunks_mut(8).enumerate() {
        seed.push_str(&format!("|round{round}"));
        let hash = fnv1a64(seed.as_bytes()).to_le_bytes();
        slot.copy_from_slice(&hash);
    }
    key
}

/// Loads — or creates — the per-install `params_hash` key persisted in the
/// state directory next to the audit fallback, so hashes stay stable across
/// sessions of one install while remaining unguessable from the journal. An
/// existing key file that cannot be used degrades honestly: one stderr
/// warning names the path and the consequence, a fresh key is minted for this
/// start, and the pipeline keeps running (never fails on key churn).
fn load_or_create_params_key(fallback_path: &std::path::Path) -> [u8; 32] {
    let key_file = fallback_path.parent().map(|dir| dir.join(PARAMS_KEY_FILE));
    if let Some(key_file) = &key_file {
        match std::fs::read(key_file) {
            Ok(bytes) => {
                if let Ok(key) = <[u8; 32]>::try_from(bytes.as_slice()) {
                    return key;
                }
                warn_unusable_key_file(key_file, String::from("content is not a 32-byte key"));
            }
            // A missing key file is the normal first start; an existing one
            // that cannot be read must not silently mint a fresh key every
            // start — that churns every per-install hash, so it surfaces like
            // the entropy degradation in the composition root.
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                warn_unusable_key_file(key_file, e.to_string());
            }
            Err(_) => {}
        }
    }
    let key = random_key().unwrap_or_else(|| weak_key(fallback_path));
    if let Some(key_file) = &key_file {
        // The state directory may not exist yet (fresh install with a healthy
        // journald primary: nothing else creates it) — best-effort mkdir so
        // the key can actually persist; a real failure still warns through
        // the persist path below.
        if let Some(dir) = key_file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        write_key_file(key_file, &key);
    }
    key
}

/// The single stderr warning for an unusable per-install key file — mirrors
/// the entropy-degradation warning in `omnibusd` wiring: honest degradation
/// (R3), never a pipeline failure.
fn warn_unusable_key_file(key_file: &std::path::Path, why: String) {
    eprintln!(
        "omnibusd: DEGRADED: the audit params_hash key file {} cannot be used \
         ({why}) — a fresh key is minted for this start, so params_hash values \
         written by earlier sessions of this install will not match \
         re-hashed arguments",
        key_file.display()
    );
}

fn write_key_file(key_file: &std::path::Path, key: &[u8; 32]) {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let written = std::fs::OpenOptions::new()
            .mode(0o600)
            .create(true)
            .truncate(true)
            .write(true)
            .open(key_file)
            .and_then(|mut file| file.write_all(key));
        if let Err(e) = written {
            // R3: never silent — an unwritable key file churns every future
            // params_hash exactly like an unusable one, so it warns through
            // the same channel.
            warn_unusable_key_file(key_file, format!("cannot persist: {e}"));
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(e) = std::fs::write(key_file, key) {
            warn_unusable_key_file(key_file, format!("cannot persist: {e}"));
        }
    }
}

/// The tri-state health of the audit fallback sink: the doctor's
/// completeness assurance must distinguish "proven writable" from "never
/// attempted" — a pristine-true flag would let the doctor vouch for a sink
/// that no audit event has ever exercised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackHealth {
    /// No append has been attempted yet this session: writability unproven.
    Unproven,
    /// The most recent append wrote.
    Writing,
    /// The most recent append failed: the audit trail is being lost.
    Failing,
}

/// The file fallback sink (tests and sink-failure degradation): JSONL events.
/// The sink knows the outcome of its append stream — a silently voiding
/// fallback is the one failure mode doctor must be able to name, and an
/// untried one is a state doctor must not confuse with proven health.
pub struct FileSink {
    path: std::path::PathBuf,
    /// Guards the {append, record} pair, so the state names the outcome of
    /// the append that completed LAST — concurrent tool calls share one
    /// pipeline, and unordered atomic stores could mislabel it.
    last_append: std::sync::Mutex<FallbackHealth>,
}

impl FileSink {
    pub fn new(path: std::path::PathBuf) -> Self {
        Self {
            path,
            last_append: std::sync::Mutex::new(FallbackHealth::Unproven),
        }
    }

    /// Appends one event; never panics — audit must not break the tool call.
    /// The outcome is recorded for [`FileSink::health`] under the state lock,
    /// so "most recent append" means the one that finished last. A poisoned
    /// lock is recovered through, not propagated: the guarded body cannot
    /// panic today, but if it ever did, the sink must degrade, not take the
    /// tool call down with it (the idiom `session.rs`/`write_gate.rs` use).
    pub fn append(&self, event: &AuditEvent) {
        let mut state = self
            .last_append
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let written = self.try_append(event);
        *state = if written.is_err() {
            FallbackHealth::Failing
        } else {
            FallbackHealth::Writing
        };
    }

    fn try_append(&self, event: &AuditEvent) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Created 0600 like the sibling params key: the trail is human-only
        // (audit.md) and carries per-tool activity, so group/other have no
        // business reading it. (An already existing file keeps its mode —
        // this is creation-time hardening.)
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path)?;
        let line = serde_json::to_string(event)
            .map_err(|_| std::io::Error::other("audit event does not serialize"))?;
        writeln!(file, "{line}")
    }

    /// The append-stream health, tri-state (see [`FallbackHealth`]): the
    /// doctor's completeness claim reads this, so an untried sink is never
    /// presented as proven.
    pub fn health(&self) -> FallbackHealth {
        *self
            .last_append
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// False when an append was attempted and failed: the audit trail is
    /// being lost. An `Unproven` sink stays healthy — nothing has been lost
    /// (and nothing is proven yet; the completeness claim uses
    /// [`FileSink::health`]).
    pub fn healthy(&self) -> bool {
        self.health() != FallbackHealth::Failing
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
    params_key: [u8; 32],
    /// The MCP client identity captured at initialize (audit `actor`
    /// correlation); beats the construction-time placeholder once set.
    client_name: std::sync::OnceLock<String>,
}

impl AuditPipeline {
    pub fn new(
        session: String,
        actor: AuditActor,
        primary: Option<Arc<dyn AuditSink>>,
        fallback_path: std::path::PathBuf,
    ) -> Self {
        let params_key = load_or_create_params_key(&fallback_path);
        Self {
            session,
            actor,
            primary,
            fallback: Arc::new(FileSink::new(fallback_path)),
            params_key,
            client_name: std::sync::OnceLock::new(),
        }
    }

    /// Installs the MCP client name captured at initialize; every subsequent
    /// event's `actor.client_name` carries it (audit.md actor model).
    pub fn set_client_name(&self, name: String) {
        let _ = self.client_name.set(name);
    }

    /// The keyed `params_hash` this pipeline stamps into events (tests and
    /// correlation tooling recompute it without touching raw parameters).
    pub fn hash_params(&self, arguments: &serde_json::Value) -> String {
        params_hash_keyed(&self.params_key, arguments)
    }

    /// Records exactly one event for a completed tool call. A primary-sink
    /// failure — an error *or* a hang past [`AUDIT_SINK_BUDGET`] — routes the
    /// event to the fallback and returns `false`; doctor surfaces that
    /// degradation (`fallback_healthy`), never the startup capabilities
    /// document; the tool result is unaffected.
    pub async fn record(
        &self,
        tool: &str,
        adapter: &str,
        op: &str,
        arguments: &serde_json::Value,
        result: &str,
        scope_ref: Option<String>,
    ) -> bool {
        let actor = AuditActor {
            uid: self.actor.uid,
            pid: self.actor.pid,
            client_name: self
                .client_name
                .get()
                .cloned()
                .or_else(|| self.actor.client_name.clone()),
        };
        let event = AuditEvent {
            ts_epoch_ms: Milliseconds::now_epoch().0,
            session: self.session.clone(),
            actor,
            tool: tool.to_string(),
            adapter: adapter.to_string(),
            op: op.to_string(),
            params_hash: params_hash_keyed(&self.params_key, arguments),
            result: result.to_string(),
            scope_ref,
        };
        if let Some(sink) = &self.primary {
            // Elapse is a sink failure like any other: degrade to the fallback.
            if let Ok(Ok(())) =
                tokio::time::timeout(AUDIT_SINK_BUDGET, sink.write(event.clone())).await
            {
                return true;
            }
        }
        // The fallback is synchronous file I/O; run inline it would stall the
        // async worker — and with it every other session task — for the
        // duration of the write, so it rides the blocking pool (the changeset
        // offload doctrine, same as the journald capture). The health
        // bookkeeping lives inside `append`, under its state lock, so the
        // sink-state semantics — "the outcome of the append that finished
        // last" — are unchanged; only the thread they are recorded on moves.
        // A join failure cannot happen today (`append` never panics) and must
        // not break the tool call even then: the outcome is ignored, any
        // degradation stays visible through `fallback_healthy`.
        let fallback = std::sync::Arc::clone(&self.fallback);
        let _ = tokio::task::spawn_blocking(move || fallback.append(&event)).await;
        false
    }

    /// True when the primary sink is absent (degradation visible in doctor).
    pub fn primary_present(&self) -> bool {
        self.primary.is_some()
    }

    /// False when the last fallback append failed — the trail is being lost
    /// with no primary to catch it (visible in doctor).
    pub fn fallback_healthy(&self) -> bool {
        self.fallback.healthy()
    }

    /// The tri-state fallback health, for the doctor's completeness wording:
    /// `Unproven` (no event written yet) must not read as proven health.
    pub fn fallback_health(&self) -> FallbackHealth {
        self.fallback.health()
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

    struct HangingSink;

    #[async_trait::async_trait]
    impl AuditSink for HangingSink {
        fn kind(&self) -> &'static str {
            "hanging"
        }
        async fn write(
            &self,
            _event: AuditEvent,
        ) -> Result<(), omnibus_contracts::errors::ContractError> {
            // Wedged socket: never resolves on its own.
            std::future::pending::<()>().await;
            Ok(())
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
        assert!(pipeline.fallback_healthy(), "the fallback wrote the event");
        let events = pipeline.fallback().read_all();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].tool, "journal_query");
        assert_eq!(events[0].session, "sess-test");
        assert_eq!(events[0].result, "ok");
    }

    #[tokio::test(start_paused = true)]
    async fn hung_primary_sink_degrades_within_the_budget() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let path = dir.path().join("fallback.jsonl");
        let pipeline = AuditPipeline::new(
            String::from("sess-test"),
            actor(),
            Some(Arc::new(HangingSink)),
            path,
        );
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
        assert!(!delivered, "a wedged sink is a failing sink");
        assert_eq!(pipeline.fallback().read_all().len(), 1);
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

    #[tokio::test]
    async fn client_name_captured_at_initialize_reaches_events() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let path = dir.path().join("fallback.jsonl");
        let pipeline = AuditPipeline::new(String::from("sess-test"), actor(), None, path);
        pipeline.set_client_name(String::from("claude-code"));
        let _ = pipeline
            .record(
                "doctor",
                "core",
                "doctor",
                &serde_json::json!({}),
                "ok",
                None,
            )
            .await;
        let events = pipeline.fallback().read_all();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].actor.client_name.as_deref(), Some("claude-code"));
    }

    #[test]
    fn unwritable_fallback_sink_is_visible_not_silent() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"not a directory").unwrap_or_else(|e| panic!("write: {e}"));
        // The fallback "directory" is a regular file: every append must fail,
        // deterministically even for root.
        let sink = FileSink::new(blocker.join("audit.jsonl"));
        let event = AuditEvent {
            ts_epoch_ms: 1,
            session: String::from("s"),
            actor: actor(),
            tool: String::from("t"),
            adapter: String::from("a"),
            op: String::from("op"),
            params_hash: String::from("h"),
            result: String::from("ok"),
            scope_ref: None,
        };
        sink.append(&event);
        assert!(!sink.healthy(), "a broken fallback reports itself");
        assert_eq!(sink.health(), FallbackHealth::Failing);
        let ok_sink = FileSink::new(dir.path().join("audit.jsonl"));
        ok_sink.append(&event);
        assert!(ok_sink.healthy());
        assert_eq!(ok_sink.health(), FallbackHealth::Writing);
    }

    /// A fresh sink has attempted nothing: it must not present as proven —
    /// the doctor's completeness claim distinguishes `Unproven` from
    /// `Writing` (a pristine-true flag would vouch for an untested sink).
    #[test]
    fn fresh_fallback_is_unproven_not_proven() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let sink = FileSink::new(dir.path().join("fresh.jsonl"));
        assert_eq!(sink.health(), FallbackHealth::Unproven);
        assert!(
            sink.healthy(),
            "nothing has been lost — but nothing is proven"
        );
        let pipeline = AuditPipeline::new(
            String::from("sess-test"),
            actor(),
            None,
            dir.path().join("pipeline.jsonl"),
        );
        assert_eq!(pipeline.fallback_health(), FallbackHealth::Unproven);
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

    /// audit.md: parameters are never reversibly restorable. Low-entropy
    /// arguments (package names, unit names, paths) must not be recoverable by
    /// hashing a candidate list: the per-install key stands between the journal
    /// and the parameters.
    #[test]
    fn params_hash_is_keyed_per_install() {
        let args = serde_json::json!({ "name": "ripgrep" });
        let key_a = [1u8; 32];
        let key_b = [2u8; 32];
        assert_ne!(
            params_hash_keyed(&key_a, &args),
            params_hash(&args),
            "the unkeyed form is not what events carry"
        );
        assert_ne!(
            params_hash_keyed(&key_a, &args),
            params_hash_keyed(&key_b, &args),
            "a different install key yields a different digest"
        );
        assert_eq!(
            params_hash_keyed(&key_a, &args),
            params_hash_keyed(&key_a, &args),
            "the same key stays stable"
        );
    }

    #[test]
    fn params_key_persists_in_the_state_directory() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let fallback = dir.path().join("audit.jsonl");
        let first = load_or_create_params_key(&fallback);
        assert!(
            dir.path().join(PARAMS_KEY_FILE).exists(),
            "the key materializes next to the fallback"
        );
        let second = load_or_create_params_key(&fallback);
        assert_eq!(first, second, "the same install reloads the same key");
        let other_dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let third = load_or_create_params_key(&other_dir.path().join("audit.jsonl"));
        assert_ne!(first, third, "distinct installs get distinct keys");
    }
}
