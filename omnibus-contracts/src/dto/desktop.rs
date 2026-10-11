//! Desktop a11y DTOs (specs/domains/desktop-a11y.md). Addressing is role+name based,
//! never pixel/coordinate based (P2/P3). `node_ref` is the adapter-owned address
//! (CoA: tree traversal and address resolution belong to the adapter); the core swaps
//! it for a core-minted opaque element token on the MCP surface.

use crate::dto::NamedValue;

crate::dto!(
    /// Selects an application by executable basename (exact, case-insensitive); an
    /// optional pid disambiguates multiple matches (spi-registry.md).
    pub struct TreeSelector {
        pub app: String,
        pub pid: Option<u32>,
    }
);

/// One node of an a11y tree snapshot; children are nested, the depth budget lives
/// with the adapter (a full desktop tree is never required by a single call).
///
/// Declared by hand instead of the `dto!` macro: the struct is recursive
/// (`children`), and an eager recursive schema construction would not terminate.
/// The schema breaks the cycle by describing nested children as shape-open — the
/// wire format is unchanged (serde nests fully).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct A11yNode {
    pub node_ref: String,
    pub role: String,
    pub name: String,
    pub pid: Option<u32>,
    pub children: Vec<A11yNode>,
}

impl crate::schema::DtoSchema for A11yNode {
    fn schema() -> crate::schema::SchemaNode {
        use crate::schema::{Field, ScalarType, SchemaNode, StructSchema};
        fn scalar(name: &'static str, ty: ScalarType) -> Field {
            Field {
                name,
                node: SchemaNode::Scalar(ty),
                doc: "",
            }
        }
        SchemaNode::Object(Box::new(StructSchema {
            name: "A11yNode",
            fields: vec![
                scalar("node_ref", ScalarType::Str),
                scalar("role", ScalarType::Str),
                scalar("name", ScalarType::Str),
                Field {
                    name: "pid",
                    node: SchemaNode::Optional(Box::new(SchemaNode::Scalar(ScalarType::U32))),
                    doc: "",
                },
                Field {
                    // Recursion break: nested children stay shape-open in the schema.
                    name: "children",
                    node: SchemaNode::Array(Box::new(SchemaNode::Object(Box::new(StructSchema {
                        name: "A11yNode",
                        fields: vec![],
                    })))),
                    doc: "",
                },
            ],
        }))
    }
}

crate::dto!(
    /// `desktop.tree` result: the root of the requested application tree.
    pub struct A11yTree {
        pub app: String,
        pub pid: Option<u32>,
        pub root: A11yNode,
        pub node_count: u32,
        pub truncated: bool,
    }
);

crate::dto!(
    /// `desktop.element` argument: the core-minted element token from a tree.
    pub struct ElementRequest {
        pub element_id: String,
    }
);

crate::dto!(
    /// `desktop.element` result: flat node detail with attributes, states, actions.
    pub struct A11yElement {
        pub node_ref: String,
        pub role: String,
        pub name: String,
        pub attributes: Vec<NamedValue>,
        pub states: Vec<String>,
        pub actions: Vec<String>,
        pub child_refs: Vec<String>,
    }
);

crate::dto!(
    /// Typed write: set a value on an editable element (no keyboard emulation, ADR-009).
    pub struct SetValueRequest {
        pub node_ref: String,
        pub value: String,
    }
);

crate::dto!(
    /// Typed write: invoke a named action (button press etc.); the default action is
    /// resolved by the adapter when the field is `None`.
    pub struct InvokeRequest {
        pub node_ref: String,
        pub action: Option<String>,
    }
);

crate::dto!(
    /// Outcome of a typed write: applied flag plus the element written.
    pub struct WriteOutcome {
        pub node_ref: String,
        pub applied: bool,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DtoSchema, sample};

    #[test]
    fn tree_roundtrips_with_schema() {
        for round in [
            &A11yTree::schema() as &crate::schema::SchemaNode,
            &A11yNode::schema(),
            &A11yElement::schema(),
            &TreeSelector::schema(),
            &SetValueRequest::schema(),
            &InvokeRequest::schema(),
            &WriteOutcome::schema(),
        ] {
            let value = sample(round, 0);
            let text = serde_json::to_string(&value).unwrap_or_default();
            let back = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            assert_eq!(back, value, "schema/serde drift for {round:?}");
        }
    }
}
