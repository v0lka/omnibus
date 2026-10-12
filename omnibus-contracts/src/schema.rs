//! Schema generation from DTOs (tool-registry.md Invariants: "JSON schemas are
//! always generated from DTOs; hand-written schemas fail the build").
//!
//! `schemars` is outside the contracts dependency allowlist, so the schema is
//! derived by the [`dto!`] family of macros from the *same declaration* that defines
//! the struct — one source of truth, no second hand-written copy to drift. The
//! round-trip guard in `tests/schema.rs` builds a sample value from every schema and
//! serde-deserializes it back into the DTO: a schema that disagrees with serde fails
//! the test run (the "mismatch = compile/test error" requirement).

/// Scalar leaf types expressible in tool arguments and DTO fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarType {
    Str,
    I64,
    U32,
    U64,
    U8,
    Bool,
    F64,
}

impl ScalarType {
    /// The JSON Schema `type` keyword value.
    pub fn json_type(self) -> &'static str {
        match self {
            Self::Str => "string",
            Self::I64 | Self::U32 | Self::U64 | Self::U8 => "integer",
            Self::Bool => "boolean",
            Self::F64 => "number",
        }
    }
}

/// A schema node: scalar, array, object (struct), closed string enum,
/// optional wrapper, or free-form JSON (serde_json::Value).
#[derive(Debug, Clone, PartialEq)]
pub enum SchemaNode {
    Scalar(ScalarType),
    Array(Box<SchemaNode>),
    Object(Box<StructSchema>),
    Enum(Vec<String>),
    Optional(Box<SchemaNode>),
    Any,
}

/// One struct field: name, schema node, human-readable doc line.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: &'static str,
    pub node: SchemaNode,
    pub doc: &'static str,
}

/// A named flat struct schema.
#[derive(Debug, Clone, PartialEq)]
pub struct StructSchema {
    pub name: &'static str,
    pub fields: Vec<Field>,
}

/// Every DTO and tool-argument type provides its schema.
pub trait DtoSchema {
    fn schema() -> SchemaNode;
}

impl DtoSchema for String {
    fn schema() -> SchemaNode {
        SchemaNode::Scalar(ScalarType::Str)
    }
}

impl DtoSchema for &str {
    fn schema() -> SchemaNode {
        SchemaNode::Scalar(ScalarType::Str)
    }
}

impl DtoSchema for i64 {
    fn schema() -> SchemaNode {
        SchemaNode::Scalar(ScalarType::I64)
    }
}

impl DtoSchema for u8 {
    fn schema() -> SchemaNode {
        SchemaNode::Scalar(ScalarType::U8)
    }
}

impl DtoSchema for u32 {
    fn schema() -> SchemaNode {
        SchemaNode::Scalar(ScalarType::U32)
    }
}

impl DtoSchema for u64 {
    fn schema() -> SchemaNode {
        SchemaNode::Scalar(ScalarType::U64)
    }
}

impl DtoSchema for bool {
    fn schema() -> SchemaNode {
        SchemaNode::Scalar(ScalarType::Bool)
    }
}

impl DtoSchema for f64 {
    fn schema() -> SchemaNode {
        SchemaNode::Scalar(ScalarType::F64)
    }
}

impl<T: DtoSchema> DtoSchema for Vec<T> {
    fn schema() -> SchemaNode {
        SchemaNode::Array(Box::new(T::schema()))
    }
}

impl<T: DtoSchema> DtoSchema for Option<T> {
    fn schema() -> SchemaNode {
        SchemaNode::Optional(Box::new(T::schema()))
    }
}

impl DtoSchema for serde_json::Value {
    fn schema() -> SchemaNode {
        SchemaNode::Any
    }
}

/// Renders a schema node as a JSON-Schema-shaped value (the exact shape MCP
/// `tools/list` carries). Objects are strict: `additionalProperties: false`.
pub fn json_schema(node: &SchemaNode) -> serde_json::Value {
    match node {
        SchemaNode::Scalar(s) => serde_json::json!({ "type": s.json_type() }),
        SchemaNode::Array(inner) => {
            serde_json::json!({ "type": "array", "items": json_schema(inner) })
        }
        SchemaNode::Object(s) => {
            let mut properties = serde_json::Map::new();
            let mut required = Vec::new();
            for field in &s.fields {
                properties.insert(field.name.to_string(), json_schema(&field.node));
                if !matches!(field.node, SchemaNode::Optional(_)) {
                    required.push(serde_json::Value::String(field.name.to_string()));
                }
            }
            serde_json::json!({
                "type": "object",
                "title": s.name,
                "properties": properties,
                "required": required,
                "additionalProperties": false,
            })
        }
        SchemaNode::Enum(variants) => serde_json::json!({ "enum": variants }),
        SchemaNode::Optional(inner) => {
            // serde maps an explicit JSON null on an Option<T> field to None, so
            // the rendered schema must accept null too — otherwise tools/list
            // clients reject calls serde would serve. The inner schema stays
            // intact: when the field IS present with a value, serde still
            // enforces the value's own shape — a nested struct's required
            // fields stay required and unknown properties stay rejected.
            // Stripping them here would render a schema looser than serde,
            // accepting calls the round-trip would reject.
            let base = json_schema(inner);
            serde_json::json!({ "anyOf": [base, { "type": "null" }] })
        }
        SchemaNode::Any => serde_json::json!({}),
    }
}

/// Builds a deterministic sample value for a schema node — the round-trip guard's
/// data source (schema → sample → serde → DTO).
pub fn sample(node: &SchemaNode, depth: usize) -> serde_json::Value {
    match node {
        SchemaNode::Scalar(ScalarType::Str) => serde_json::Value::String(String::from("sample")),
        SchemaNode::Scalar(ScalarType::I64) => serde_json::json!(1),
        SchemaNode::Scalar(ScalarType::U32) => serde_json::json!(2),
        SchemaNode::Scalar(ScalarType::U64) => serde_json::json!(3),
        SchemaNode::Scalar(ScalarType::U8) => serde_json::json!(4),
        SchemaNode::Scalar(ScalarType::Bool) => serde_json::Value::Bool(true),
        SchemaNode::Scalar(ScalarType::F64) => serde_json::json!(1.5),
        SchemaNode::Array(inner) => {
            // Depth cap keeps recursive DTOs (a11y trees) finite.
            if depth >= 3 {
                serde_json::json!([])
            } else {
                serde_json::json!([sample(inner, depth + 1)])
            }
        }
        SchemaNode::Object(s) => {
            let mut map = serde_json::Map::new();
            if depth < 6 {
                for field in &s.fields {
                    map.insert(field.name.to_string(), sample(&field.node, depth + 1));
                }
            }
            serde_json::Value::Object(map)
        }
        SchemaNode::Enum(variants) => variants
            .first()
            .map(|v| serde_json::Value::String(v.clone()))
            .unwrap_or(serde_json::Value::Null),
        SchemaNode::Optional(inner) => {
            if depth >= 2 {
                serde_json::Value::Null
            } else {
                sample(inner, depth + 1)
            }
        }
        SchemaNode::Any => serde_json::json!({ "free": "form" }),
    }
}

/// Defines a flat DTO struct together with its generated schema.
#[macro_export]
macro_rules! dto {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$fmeta:meta])*
                $fvis:vis $field:ident : $fty:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        pub struct $name {
            $(
                $(#[$fmeta])*
                pub $field: $fty,
            )*
        }

        impl $crate::schema::DtoSchema for $name {
            fn schema() -> $crate::schema::SchemaNode {
                $crate::schema::SchemaNode::Object(Box::new($crate::schema::StructSchema {
                    name: stringify!($name),
                    fields: vec![
                        $(
                            $crate::schema::Field {
                                name: stringify!($field),
                                node: <$fty as $crate::schema::DtoSchema>::schema(),
                                doc: "",
                            },
                        )*
                    ],
                }))
            }
        }
    };
}

/// Converts a Rust variant identifier (`PkgUndoCache`) to its serde snake_case wire
/// form (`pkg_undo_cache`), matching `#[serde(rename_all = "snake_case")]`.
pub fn snake_case(name: &'static str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Defines a closed unit-variant enum serialized as snake_case strings, with its
/// generated enum schema.
#[macro_export]
macro_rules! dto_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($variant:ident),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant,)*
        }

        impl $crate::schema::DtoSchema for $name {
            fn schema() -> $crate::schema::SchemaNode {
                $crate::schema::SchemaNode::Enum(vec![
                    $( $crate::schema::snake_case(stringify!($variant)) ),*
                ])
            }
        }
    };
}

/// Defines a unit newtype over a scalar (transparent serde, schema of the inner
/// scalar) — the DTO-discipline wrapper for units.
#[macro_export]
macro_rules! dto_newtype {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident($fvis:vis $inner:ty);
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord,
                 serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub $inner);

        impl $crate::schema::DtoSchema for $name {
            fn schema() -> $crate::schema::SchemaNode {
                <$inner as $crate::schema::DtoSchema>::schema()
            }
        }
    };
}
