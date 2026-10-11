//! Portal DTOs: clipboard (read+write), screenshot, notifications
//! (specs/domains/clipboard.md, screen-capture.md, notifications.md).

crate::dto!(
    pub struct ClipboardContent {
        pub mime: String,
        pub text: String,
        pub size_bytes: u64,
    }
);

crate::dto_enum!(
    pub enum ImageFormat {
        Png,
    }
);

crate::dto!(
    pub struct ScreenshotRequest {
        pub format: ImageFormat,
    }
);

crate::dto!(
    pub struct ScreenshotResult {
        pub mime: String,
        pub image_base64: String,
        pub size_bytes: u64,
    }
);

crate::dto_enum!(
    pub enum Urgency {
        Low,
        Normal,
        Critical,
    }
);

crate::dto!(
    pub struct NotifyRequest {
        pub title: String,
        pub body: String,
        pub urgency: Option<Urgency>,
    }
);

crate::dto!(
    pub struct NotifyResult {
        pub delivered: bool,
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DtoSchema, sample};

    #[test]
    fn portal_dto_roundtrips_with_schema() {
        for node in [
            &ClipboardContent::schema() as &crate::schema::SchemaNode,
            &ScreenshotRequest::schema(),
            &ScreenshotResult::schema(),
            &NotifyRequest::schema(),
            &NotifyResult::schema(),
        ] {
            let value = sample(node, 0);
            let text = serde_json::to_string(&value).unwrap_or_default();
            let back = serde_json::from_str::<serde_json::Value>(&text).unwrap_or_default();
            assert_eq!(back, value);
        }
    }
}
