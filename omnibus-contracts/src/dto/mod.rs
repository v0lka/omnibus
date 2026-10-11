//! DTO discipline (specs/architecture/spi-registry.md, roadmap 1.2.2): flat named
//! structs only, newtype wrappers for units, opaque tokens minted only by the core,
//! native types never cross the seam. Every struct below is declared together with
//! its generated schema via the [`crate::dto!`] macro family.

pub mod audit;
pub mod caps;
pub mod desktop;
pub mod files;
pub mod journal;
pub mod kb;
pub mod net;
pub mod packages;
pub mod portals;
pub mod probe;
pub mod services;

use core::fmt;

crate::dto_newtype!(
    /// A duration in milliseconds.
    pub struct Milliseconds(pub i64);
);

crate::dto_newtype!(
    /// A size in bytes (never KiB, never sectors).
    pub struct Bytes(pub u64);
);

crate::dto_newtype!(
    /// Journal severity level (0=emerg .. 7=debug, syslog ordering).
    pub struct Severity(pub u8);
);

crate::dto_newtype!(
    /// A byte offset into a file or stream.
    pub struct OffsetBytes(pub u64);
);

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

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sev{}", self.0)
    }
}

crate::dto!(
    /// A flat name→string pair (attributes, extra fields) — never a tuple.
    pub struct NamedValue {
        pub name: String,
        pub value: String,
    }
);

impl Milliseconds {
    /// Epoch milliseconds "now" for adapters and tests with a real clock.
    pub fn now_epoch() -> Self {
        Self(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
                .unwrap_or(0),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DtoSchema, sample};

    #[test]
    fn named_value_roundtrips_with_schema() {
        let node = NamedValue::schema();
        let value = sample(&node, 0);
        let parsed: NamedValue = serde_json::from_value(value.clone()).unwrap_or(NamedValue {
            name: String::new(),
            value: String::new(),
        });
        let back = serde_json::to_value(&parsed).unwrap_or_default();
        assert_eq!(back, value);
    }

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
        assert_eq!(timeout, Milliseconds(size.0 as i64));
    }

    #[test]
    fn severity_schema_is_a_byte_scalar() {
        assert_eq!(
            Severity::schema(),
            crate::schema::SchemaNode::Scalar(crate::schema::ScalarType::U8)
        );
    }
}
