//! The Manjaro overlay: a directory of `.md` articles the adapter prepends to
//! search results (the Manjaro half of roadmap 2.5.2 — kb-archwiki serves
//! ArchWiki plus this overlay on Manjaro machines). Matching is lexical: a
//! query term set matches when every term appears in the file name or body.

use std::path::Path;

use omnibus_contracts::dto::kb::{KbArticle, KbArticleRef, KbPageRequest};
use omnibus_contracts::errors::ContractError;

/// The source label of overlay entries (distinct from the wiki source label).
pub const OVERLAY_SOURCE: &str = "manjaro-overlay";
/// URL scheme that routes a page request back into the overlay directory.
pub const OVERLAY_SCHEME: &str = "overlay:";
/// Maximum snippet length carried in a reference.
const SNIPPET_MAX_CHARS: usize = 200;

/// Searches the overlay directory (no network); a missing or unreadable
/// directory simply contributes no entries — the overlay is an additive source.
pub fn search(overlay_dir: Option<&Path>, query_text: &str) -> Vec<KbArticleRef> {
    let Some(dir) = overlay_dir else {
        return Vec::new();
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let terms: Vec<String> = query_text
        .split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect();
    if terms.is_empty() {
        return Vec::new();
    }
    let mut refs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_md = path.extension().is_some_and(|ext| ext == "md");
        if !is_md {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let haystack = format!("{}\n{}", stem, body).to_ascii_lowercase();
        if !terms.iter().all(|term| haystack.contains(term.as_str())) {
            continue;
        }
        refs.push(KbArticleRef {
            title: stem.replace('-', " "),
            url: format!("{}{}", OVERLAY_SCHEME, stem),
            snippet: snippet_of(&body),
            source: String::from(OVERLAY_SOURCE),
        });
        let _ = &mut refs; // keep the iterator simple and ordered by file name
    }
    refs.sort_by(|a, b| a.title.cmp(&b.title));
    refs
}

/// Reads one overlay article body for a page request routed to the overlay.
pub fn page(
    overlay_dir: Option<&Path>,
    request: &KbPageRequest,
) -> Result<KbArticle, ContractError> {
    let Some(dir) = overlay_dir else {
        return Err(ContractError::AdapterUnavailable);
    };
    let Some(stem) = request.url.strip_prefix(OVERLAY_SCHEME) else {
        return Err(ContractError::NativeFailure {
            code: 1,
            payload: format!(
                "overlay page request has a foreign url shape: {}",
                request.url
            ),
        });
    };
    // The stem comes from our own search results; still, refuse traversal.
    if stem.is_empty() || stem.contains('/') || stem.contains('\\') || stem.contains("..") {
        return Err(ContractError::NativeFailure {
            code: 22,
            payload: format!("overlay article name is not a safe stem: {stem}"),
        });
    }
    let path = dir.join(format!("{stem}.md"));
    let body = std::fs::read_to_string(&path).map_err(|e| ContractError::NativeFailure {
        code: 2,
        payload: format!("overlay article {}: {e}", path.display()),
    })?;
    Ok(KbArticle {
        title: stem.replace('-', " "),
        url: request.url.clone(),
        source: String::from(OVERLAY_SOURCE),
        text: body,
        cached_at_epoch_ms: None,
        stale: false,
    })
}

/// First non-empty lines of the body, capped to the snippet budget.
fn snippet_of(body: &str) -> String {
    let flattened = body.split_whitespace().collect::<Vec<&str>>().join(" ");
    flattened.chars().take(SNIPPET_MAX_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_overlay(dir: &Path) {
        std::fs::write(
            dir.join("suspend-and-hibernate.md"),
            "# Suspend\nManjaro notes: use systemctl suspend; check the swap size.",
        )
        .unwrap_or_else(|e| panic!("fixture write: {e}"));
        std::fs::write(
            dir.join("mhwd-graphics.md"),
            "# mhwd\nManjaro graphics driver management notes.",
        )
        .unwrap_or_else(|e| panic!("fixture write: {e}"));
        std::fs::write(dir.join("notes.txt"), "not an article")
            .unwrap_or_else(|e| panic!("fixture write: {e}"));
    }

    #[test]
    fn overlay_search_matches_all_terms_and_prepends_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        make_overlay(dir.path());
        let refs = search(Some(dir.path()), "suspend swap");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].title, "suspend and hibernate");
        assert_eq!(refs[0].url, "overlay:suspend-and-hibernate");
        assert_eq!(refs[0].source, OVERLAY_SOURCE);
        assert!(refs[0].snippet.contains("systemctl suspend"));
        // A term outside the corpus matches nothing.
        assert!(search(Some(dir.path()), "suspend x11-driver-mystery").is_empty());
    }

    #[test]
    fn overlay_page_reads_the_body_and_refuses_foreign_urls() {
        let dir = tempfile::tempdir().expect("tempdir");
        make_overlay(dir.path());
        let article = page(
            Some(dir.path()),
            &KbPageRequest {
                url: String::from("overlay:suspend-and-hibernate"),
            },
        )
        .expect("overlay page");
        assert_eq!(article.source, OVERLAY_SOURCE);
        assert!(!article.stale);
        assert!(article.text.contains("systemctl suspend"));
        assert!(article.cached_at_epoch_ms.is_none());
        let foreign = page(
            Some(dir.path()),
            &KbPageRequest {
                url: String::from("https://wiki.archlinux.org/title/Suspend"),
            },
        );
        assert!(matches!(foreign, Err(ContractError::NativeFailure { .. })));
    }

    #[test]
    fn overlay_traversal_and_missing_files_are_native_failures() {
        let dir = tempfile::tempdir().expect("tempdir");
        let traversal = page(
            Some(dir.path()),
            &KbPageRequest {
                url: String::from("overlay:../escape"),
            },
        );
        assert!(matches!(
            traversal,
            Err(ContractError::NativeFailure { code: 22, .. })
        ));
        let missing = page(
            Some(dir.path()),
            &KbPageRequest {
                url: String::from("overlay:not-there"),
            },
        );
        assert!(matches!(
            missing,
            Err(ContractError::NativeFailure { code: 2, .. })
        ));
        let no_dir = page(
            None,
            &KbPageRequest {
                url: String::from("overlay:suspend"),
            },
        );
        assert!(matches!(no_dir, Err(ContractError::AdapterUnavailable)));
    }
}
