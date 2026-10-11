//! `Journal` — reading the system journal (subsystem 2), and `AuditSink` — the
//! audit delivery seam. Persistence and past-boot facts are capabilities reported
//! by the adapter, never derived (capability-model.md "Independent Capabilities").

use async_trait::async_trait;

use crate::dto::journal::{JournalPage, JournalQuery, JournalRecord, TailRequest};
use crate::errors::ContractError;

/// Journal reading: filtered queries and tail. `boot = Previous` is only offered to
/// the agent while the prev_boot capability is present; the core enforces the
/// gating, the adapter enforces the journal itself.
#[async_trait]
pub trait Journal: Send + Sync {
    /// The adapter report (persistence, prev_boot facts, provider).
    fn report(&self) -> crate::dto::probe::AdapterReport;

    /// `journal_query` — filtered, ordered oldest→newest within the window.
    async fn query(&self, filter: JournalQuery) -> Result<JournalPage, ContractError>;

    /// `journal tail` — the last N records of the current boot.
    async fn tail(&self, request: TailRequest) -> Result<Vec<JournalRecord>, ContractError>;
}

/// Audit delivery (specs/domains/audit.md): the core produces exactly one
/// `AuditEvent` per tool call; sinks deliver it. Sinks never learn tool parameters —
/// only their hash.
#[async_trait]
pub trait AuditSink: Send + Sync {
    /// Sink identity (`journald`, `file-fallback`, ...), surfaced in doctor.
    fn kind(&self) -> &'static str;

    /// Delivers one structured event; a sink failure degrades to the file fallback
    /// in the core pipeline and must never fail the tool call.
    async fn write(&self, event: crate::dto::audit::AuditEvent) -> Result<(), ContractError>;
}
