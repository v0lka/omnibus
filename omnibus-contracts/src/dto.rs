//! DTO discipline seeds: newtype wrappers for units
//! (specs/architecture/spi-registry.md "DTO Discipline").
//!
//! CoM (meaning of values) is converted into CoT (types): units travel as newtypes,
//! never as bare integers. Flat named structs only; native types never cross the seam.

use core::fmt;

/// A duration in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Milliseconds(pub i64);

/// A size in bytes (never KiB, never sectors).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Bytes(pub u64);

/// Severity level of a journal record. Concrete level mapping arrives with the
/// `Journal` contract (roadmap 1.2.1); the unit travels as this newtype already.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Severity(pub u8);

impl fmt::Display for Milliseconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}B", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_display_with_their_dimension() {
        assert_eq!(Milliseconds(1500).to_string(), "1500ms");
        assert_eq!(Bytes(42).to_string(), "42B");
    }

    #[test]
    fn newtypes_order_within_their_unit() {
        let timeout = Milliseconds(500);
        let size = Bytes(500);
        assert!(timeout > Milliseconds(499));
        assert!(size < Bytes(501));
        // Distinct newtype types cannot be confused: `timeout == size` does not compile.
        assert_eq!(timeout, Milliseconds(size.0 as i64)); // only via explicit conversion
    }
}
