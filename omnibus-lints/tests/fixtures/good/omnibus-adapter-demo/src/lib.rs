//! Fixed CoN counterpart: the adapter references the constant, not a literal.
pub fn demo_capability_name() -> &'static str {
    omnibus_contracts::DEMO_READ
}
