//! Knowledge base DTOs (specs/domains/knowledge.md). Applicability of an article is
//! judged by the agent (P7) — the adapter never ranks for correctness.

crate::dto!(
    pub struct KbQuery {
        pub text: String,
        pub limit: Option<u32>,
    }
);

crate::dto!(
    pub struct KbArticleRef {
        pub title: String,
        pub url: String,
        pub snippet: String,
        pub source: String,
    }
);

crate::dto!(
    pub struct KbPageRequest {
        pub url: String,
    }
);

crate::dto!(
    pub struct KbArticle {
        pub title: String,
        pub url: String,
        pub source: String,
        pub text: String,
        pub cached_at_epoch_ms: Option<i64>,
        pub stale: bool,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DtoSchema, sample};

    #[test]
    fn kb_dto_roundtrips_with_schema() {
        for node in [
            &KbQuery::schema() as &crate::schema::SchemaNode,
            &KbArticleRef::schema(),
            &KbPageRequest::schema() as &crate::schema::SchemaNode,
            &KbArticle::schema() as &crate::schema::SchemaNode,
        ] {
            let value = sample(node, 0);
            let text = serde_json::to_string(&value).unwrap_or_default();
            let back = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            assert_eq!(back, value);
        }
    }
}
