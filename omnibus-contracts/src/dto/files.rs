//! Scoped file access DTOs (specs/domains/files.md). The default read scope is the
//! whole $HOME including dotfiles (decision 2026-10-10); outside $HOME only via an
//! explicit parameterized grant; writes are grant-only in every case.

use crate::dto::{Bytes, OffsetBytes};

crate::dto!(
    pub struct PathQuery {
        pub path: String,
        pub depth: Option<u8>,
        pub limit: Option<u32>,
    }
);

crate::dto_enum!(
    pub enum EntryKind {
        File,
        Dir,
        Symlink,
    }
);

crate::dto!(
    pub struct DirEntry {
        pub name: String,
        pub kind: EntryKind,
        pub size_bytes: Option<u64>,
    }
);

crate::dto!(
    pub struct DirListing {
        pub path: String,
        pub entries: Vec<DirEntry>,
        pub truncated: bool,
    }
);

crate::dto!(
    pub struct FileReadRequest {
        pub path: String,
        pub offset_bytes: Option<OffsetBytes>,
        pub max_bytes: Option<Bytes>,
    }
);

crate::dto!(
    pub struct FileContent {
        pub path: String,
        pub size_bytes: u64,
        pub data_base64: String,
        pub text: Option<String>,
        pub truncated: bool,
    }
);

crate::dto!(
    pub struct FileWriteRequest {
        pub path: String,
        pub data_base64: Option<String>,
        pub text: Option<String>,
        pub create_parents: Option<bool>,
    }
);

crate::dto!(
    pub struct FileWriteResult {
        pub path: String,
        pub bytes_written: u64,
    }
);

crate::dto!(
    pub struct PathResolveRequest {
        pub path: String,
    }
);

crate::dto!(
    pub struct ResolvedPath {
        pub path: String,
        pub canonical: String,
        pub inside_home: bool,
        pub read_allowed: bool,
        pub write_allowed: bool,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DtoSchema, sample};

    #[test]
    fn file_dto_roundtrips_with_schema() {
        for node in [
            &PathQuery::schema() as &crate::schema::SchemaNode,
            &DirEntry::schema(),
            &DirListing::schema(),
            &FileReadRequest::schema(),
            &FileContent::schema(),
            &FileWriteRequest::schema(),
            &FileWriteResult::schema(),
            &ResolvedPath::schema(),
        ] {
            let value = sample(node, 0);
            let text = serde_json::to_string(&value).unwrap_or_default();
            let back = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            assert_eq!(back, value);
        }
    }
}
