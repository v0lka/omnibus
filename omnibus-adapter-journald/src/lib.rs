//! journald adapter: journal query/tail plus the audit sink (P5).
//!
//! Native backend: sd-journal (systemd journal). Native types never leave this crate — mapping to DTOs
//! happens inside the adapter (specs/architecture/spi-registry.md "DTO Discipline").
//! The trait implementation arrives just-in-time with roadmap 1.2.1; compile-time
//! feature flags for detection levels land with it (roadmap 1.1.2 "How").
