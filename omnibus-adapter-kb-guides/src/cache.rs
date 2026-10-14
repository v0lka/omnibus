//! The article cache (the Ф1.5.6 pattern): every fetched doc article is
//! stored under the adapter's cache directory keyed by its request-derived
//! slug, and served stale (flagged) when the doc source is unreachable —
//! honest offline degradation.

use std::path::{Path, PathBuf};

use omnibus_contracts::dto::kb::KbArticle;

/// File extension of cached articles.
const CACHE_SUFFIX: &str = "json";

/// Cache location of one article slug.
pub fn article_path(cache_dir: &Path, slug: &str) -> PathBuf {
    cache_dir.join(format!("{slug}.{CACHE_SUFFIX}"))
}

/// Persists one fetched article under the given slug key. The key is the
/// request-derived slug the offline lookup uses — deliberately not derived
/// from `article.title`, so store and lookup always agree on the same key. A
/// failing write must never fail the read that produced it, so the outcome is
/// reported but not propagated.
pub fn store(cache_dir: &Path, slug: &str, article: &KbArticle) -> Result<(), String> {
    if slug.is_empty() {
        return Err(String::from("article slug is empty"));
    }
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| format!("cache dir {}: {e}", cache_dir.display()))?;
    let path = article_path(cache_dir, slug);
    let payload =
        serde_json::to_string_pretty(article).map_err(|e| format!("cache serialize: {e}"))?;
    std::fs::write(&path, payload).map_err(|e| format!("cache write {}: {e}", path.display()))
}

/// Loads the cached copy of an article, if one exists and parses.
pub fn load(cache_dir: &Path, slug: &str) -> Option<KbArticle> {
    if slug.is_empty() {
        return None;
    }
    let path = article_path(cache_dir, slug);
    let payload = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&payload).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_article(source: &str, title: &str) -> KbArticle {
        KbArticle {
            title: String::from(title),
            url: format!("https://example.org/{title}"),
            source: String::from(source),
            text: String::from("# Sample\nSome doc body."),
            cached_at_epoch_ms: Some(1730000000000),
            stale: false,
        }
    }

    #[test]
    fn cache_write_read_roundtrip_preserves_the_article() {
        let dir = crate::testutil::temp_dir("cache-roundtrip");
        let article = sample_article("debian-wiki", "Suspend");
        let slug = "suspend";
        store(dir.path(), slug, &article).expect("cache store");
        let loaded = load(dir.path(), slug).expect("cached article must load");
        assert_eq!(loaded, article);
        drop(dir);
    }

    #[test]
    fn load_of_missing_slug_is_none_and_write_failures_are_reported() {
        let dir = crate::testutil::temp_dir("cache-missing");
        assert!(load(dir.path(), "nope").is_none());
        // An empty slug cannot be stored; the error is a string payload,
        // never a panic.
        assert!(store(dir.path(), "", &sample_article("ubuntu-docs", "Suspend")).is_err());
        drop(dir);
    }

    #[test]
    fn store_keys_by_the_given_slug_not_the_article_title() {
        // The request-derived key is the only key: an article whose title
        // would slug differently is still stored under the request slug, and
        // no second file appears under the title slug.
        let dir = crate::testutil::temp_dir("cache-slug-keyed");
        let article = sample_article("fedora-docs", "Adding Users to sudoers");
        store(
            dir.path(),
            "en-us-quick-docs-adding-users-to-sudoers",
            &article,
        )
        .expect("store under the request slug");
        assert!(
            load(dir.path(), "en-us-quick-docs-adding-users-to-sudoers").is_some(),
            "request-slug key hits"
        );
        assert!(load(dir.path(), "adding-users-to-sudoers").is_none());
        drop(dir);
    }

    #[test]
    fn article_path_is_slug_keyed() {
        let dir = Path::new("/tmp/cache");
        assert_eq!(
            article_path(dir, "suspend-and-hibernate"),
            PathBuf::from("/tmp/cache/suspend-and-hibernate.json")
        );
    }
}
