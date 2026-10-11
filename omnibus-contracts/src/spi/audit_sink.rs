//! `AuditSink` delivery variants live in `spi/journal.rs` next to `Journal`
//! (subsystem 2 owns both traits per spi-registry.md). This module is intentionally
//! a re-export so imports read naturally at the core boundary.

pub use crate::spi::journal::AuditSink;
