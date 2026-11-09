#![cfg_attr(test, allow(clippy::expect_used))] // unit tests may expect; the seam must not
//! `omnibus-contracts` — the explicit seam of Omnibus.
//!
//! This crate is the ONLY shared dependency of the system (see
//! `specs/architecture/layers-and-crates.md` in the repo): traits, DTOs, capability
//! constants, and the error taxonomy live here and nowhere else.
//! It has zero OS dependencies and zero dependencies on the core.
//!
//! SPI traits are declared just-in-time together with their first implementation
//! (specs/architecture/spi-registry.md, ADR-011). MS1 declares the eight subsystems
//! with a first implementation each; `NetworkStatus` is declared together with its
//! first implementation, `omnibus-adapter-nm` (roadmap task 2.2.3, just-in-time).
//! `AppIntentsBridge` stays undeclared until its macOS implementation exists (MS3).

pub mod capabilities;
pub mod dto;
pub mod errors;
pub mod floors;
pub mod schema;
pub mod spi;
pub mod suite;
pub mod tokens;
pub mod version;
