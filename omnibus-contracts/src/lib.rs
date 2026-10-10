//! `omnibus-contracts` — the explicit seam of Omnibus.
//!
//! This crate is the ONLY shared dependency of the system (see
//! `specs/architecture/layers-and-crates.md` in the repo): traits, DTOs, capability
//! constants, and the error taxonomy live here and nowhere else.
//! It has zero OS dependencies and zero dependencies on the core.
//!
//! SPI traits are declared just-in-time together with their first implementation
//! (specs/architecture/spi-registry.md); no speculative trait stubs live here yet.

pub mod capabilities;
pub mod dto;
pub mod errors;
