//! `omnibus-core` — the OS-agnostic core of `omnibusd`
//! (specs/architecture/layers-and-crates.md crate map).
//!
//! The core calls traits through a probe-assembled registry and never links adapter
//! crates. This skeleton seeds the two policy nuclei that the quality gates test
//! already today: the out-of-band grant check (R5/R6) and the degradation ladder (R3).

pub mod grants;
pub mod ladder;
