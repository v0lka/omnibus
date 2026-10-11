//! The article cache: every fetched wiki article is stored under the
//! adapter's cache directory keyed by slug, and served stale (flagged) when
//! the network is gone — honest offline degradation (roadmap 1.5.6).

use std::path::{Path, PathBuf};

use omnibus_contracts::dto::kb::KbArticle;

/// File extension of cached articles.
const CACHE_SUFFIX: &str = "json";

/// Cache location of one article slug.
pub fn article_path(cache_dir: &Path, slug: &str) -> PathBuf {
    cache_dir.join(format!("{slug}.{CACHE_SUFFIX}"))
}

/// Persists one fetched article; a failing write must never fail the read that
/// produced it, so the outcome is reported but not propagated.
pub fn store(cache_dir: &Path, article: &KbArticle) -> Result<(), String> {
    let slug = super::wiki::slugify(&article.title);
    if slug.is_empty() {
        return Err(String::from("article title has no slug"));
    }
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| format!("cache dir {}: {e}", cache_dir.display()))?;
    let path = article_path(cache_dir, &slug);
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

    fn sample_article(title: &str) -> KbArticle {
        KbArticle {
            title: String::from(title),
            url: format!(
                "https://wiki.archlinux.org/title/{}",
                title.replace(' ', "_")
            ),
            source: String::from("archwiki"),
            text: String::from("# Sample\nSome wikitext body."),
            cached_at_epoch_ms: Some(1730000000000),
            stale: false,
        }
    }

    #[test]
    fn cache_write_read_roundtrip_preserves_the_article() {
        let dir = tempfile::tempdir().expect("tempdir");
        let article = sample_article("Suspend and Hibernate");
        store(dir.path(), &article).expect("cache store");
        let slug = super::super::wiki::slugify(&article.title);
        let loaded = load(dir.path(), &slug).expect("cached article must load");
        assert_eq!(loaded, article);
    }

    #[test]
    fn load_of_missing_slug_is_none_and_write_failures_are_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(load(dir.path(), "nope").is_none());
        // A title with no slug-able characters cannot be stored; the error is
        // a string payload, never a panic.
        assert!(store(dir.path(), &sample_article("!!!")).is_err());
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
