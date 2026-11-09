#![cfg_attr(test, allow(clippy::expect_used))] // unit tests may expect; the seam must not
//! journald adapter: journal query/tail plus the audit sink (P5).
//!
//! Native backend: sd-journal (systemd journal), reached through the `journalctl`
//! JSON interface for reads and the journald ingestion socket for the audit sink.
//! Native types never leave this crate — mapping to DTOs happens inside the
//! adapter (specs/architecture/spi-registry.md "DTO Discipline").
//! The trait implementation arrives just-in-time with roadmap 1.2.1; compile-time
//! feature flags for detection levels land with it (roadmap 1.1.2 "How").
//!
//! - [`JournaldAdapter`] implements `Journal` (query/tail/report, roadmap 1.5.2).
//! - [`JournaldAuditSink`] implements `AuditSink` (roadmap 1.3.4); a socket
//!   failure degrades to the core's file sink.
//!
//! Both constructors fail fast on a missing backend, so machines without the
//! journal simply leave the subsystem unactivated.

mod journal;
mod sink;

pub use journal::JournaldAdapter;
pub use sink::JournaldAuditSink;
