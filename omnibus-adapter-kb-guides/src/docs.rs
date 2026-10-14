//! The distro-docs content layer: search and page reads over the per-source
//! mirror directories (`<guides root>/<source id>/*.md`, the kb-archwiki
//! Manjaro-overlay layout one level down). Matching is lexical — a query term
//! set matches when every term appears in the file name or body; article
//! applicability stays with the agent (P7). Failure classification mirrors the
//! kb-archwiki doctrine: a source that cannot be reached at all is the
//! transport class (degradable to the stale cache), while an existing source
//! that answers "no such article" is the article's true state and propagates.

use std::path::{Path, PathBuf};

use omnibus_contracts::dto::kb::KbArticleRef;
use omnibus_contracts::errors::ContractError;

use crate::sources::{self, REGISTRY, SourceDef};

/// `NativeFailure` code: the doc source is present and answered, and the
/// article's true state is known (no such article under a present source) —
/// distinct from the transport class so the stale-cache fallback in `lib.rs`
/// degrades only when the source — not the article — is gone.
pub const NF_DOC_ERROR: i32 = 7;
/// `NativeFailure` code: the URL does not name a registered distro doc source
/// or carries no resolvable article path.
pub const NF_BAD_URL: i32 = 22;
/// `NativeFailure` code: transport-class breakdown (unreadable article body,
/// failed worker) — the article itself may still exist.
pub const NF_TRANSPORT: i32 = 1;

/// Maximum snippet length carried in a reference.
const SNIPPET_MAX_CHARS: usize = 200;

/// Mirror directory of one source under the guides root.
pub fn source_dir(guides_dir: &Path, source: &SourceDef) -> PathBuf {
    guides_dir.join(source.id)
}

/// True when the source's mirror directory exists and is readable.
pub fn source_reachable(guides_dir: &Path, source: &SourceDef) -> bool {
    std::fs::read_dir(source_dir(guides_dir, source)).is_ok()
}

/// Reads one article fresh from its source mirror. A present source without
/// the article answers with the article's true state ([`NF_DOC_ERROR`]); a
/// missing or unreadable source directory is the transport class — the source
/// is gone, the article may still exist and the cache may serve it stale.
pub fn read_article(
    guides_dir: &Path,
    resolved: &sources::ResolvedArticle,
) -> Result<String, ContractError> {
    let Some(source) = REGISTRY.iter().find(|s| s.id == resolved.source_id) else {
        return Err(ContractError::NativeFailure {
            code: NF_BAD_URL,
            payload: format!(
                "article names an unregistered source: {}",
                resolved.source_id
            ),
        });
    };
    let dir = source_dir(guides_dir, source);
    if !source_reachable(guides_dir, source) {
        // The whole source is unreachable: the transport class.
        return Err(ContractError::AdapterUnavailable);
    }
    let path = dir.join(format!("{}.md", resolved.slug));
    std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            // The source answered: no such article. The stale cache must stay
            // silent — serving it would let a dead URL retry forever.
            ContractError::NativeFailure {
                code: NF_DOC_ERROR,
                payload: format!("doc source {} has no article {}", source.id, path.display()),
            }
        } else {
            // Exists but unreadable: a source-side breakdown, degradable.
            ContractError::NativeFailure {
                code: NF_TRANSPORT,
                payload: format!("doc article {}: {e}", path.display()),
            }
        }
    })
}

/// Searches every reachable source mirror (no network); missing or unreadable
/// source directories contribute nothing. With zero reachable sources the
/// adapter reports [`ContractError::AdapterUnavailable`] instead of a silently
/// empty result set (honest degradation).
pub fn search_blocking(
    guides_dir: &Path,
    query_text: &str,
) -> Result<Vec<KbArticleRef>, ContractError> {
    // Reachability gates the answer: with zero reachable sources the adapter
    // reports [`ContractError::AdapterUnavailable`] instead of a silently
    // empty result set (honest degradation) — even for an empty query.
    let any_reachable = REGISTRY.iter().any(|s| source_reachable(guides_dir, s));
    if !any_reachable {
        return Err(ContractError::AdapterUnavailable);
    }
    let terms: Vec<String> = query_text
        .split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let mut refs = Vec::new();
    for source in REGISTRY {
        let entries = match std::fs::read_dir(source_dir(guides_dir, &source)) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|ext| ext == "md") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            let haystack = format!("{stem}\n{body}").to_ascii_lowercase();
            if !terms.iter().all(|term| haystack.contains(term.as_str())) {
                continue;
            }
            refs.push(KbArticleRef {
                title: sources::humanize(&stem),
                url: sources::article_url(&source, &stem),
                snippet: snippet_of(&body),
                source: String::from(source.id),
            });
        }
    }
    // Deterministic order: registry order per source, titles alphabetical
    // within one source.
    refs.sort_by(|a, b| a.source.cmp(&b.source).then_with(|| a.title.cmp(&b.title)));
    Ok(refs)
}

/// First non-empty lines of the body, capped to the snippet budget.
fn snippet_of(body: &str) -> String {
    let flattened = body.split_whitespace().collect::<Vec<&str>>().join(" ");
    flattened.chars().take(SNIPPET_MAX_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    #[test]
    fn search_matches_all_terms_across_sources_and_orders_deterministically() {
        let root = testutil::temp_dir("docs-search");
        let ubuntu = root.path().join("ubuntu-docs");
        std::fs::create_dir_all(&ubuntu).expect("source dir");
        std::fs::write(
            ubuntu.join("suspend-and-hibernate.md"),
            "# Suspend\nUbuntu fix: check the swap size before systemctl suspend.",
        )
        .expect("fixture");
        std::fs::write(
            ubuntu.join("suspend-deep-dive.md"),
            "# Suspend deep dive\nMore suspend notes; swap again.",
        )
        .expect("fixture");
        let debian = root.path().join("debian-wiki");
        std::fs::create_dir_all(&debian).expect("source dir");
        std::fs::write(
            debian.join("suspend.md"),
            "# Suspend\nDebian wiki: hibernate to swap with swsusp notes.",
        )
        .expect("fixture");

        let refs = search_blocking(root.path(), "suspend swap").expect("search");
        assert_eq!(refs.len(), 3, "both ubuntu articles and the debian one");
        assert!(
            refs.windows(2)
                .all(|w| (w[0].source.as_str(), w[0].title.as_str())
                    <= (w[1].source.as_str(), w[1].title.as_str()))
        );
        let ubuntu_ref = refs
            .iter()
            .find(|r| r.url == "https://help.ubuntu.com/suspend-and-hibernate")
            .expect("ubuntu ref");
        assert_eq!(ubuntu_ref.source, "ubuntu-docs");
        assert_eq!(ubuntu_ref.title, "suspend and hibernate");
        assert!(ubuntu_ref.snippet.contains("systemctl suspend"));

        // A term outside the corpus matches nothing — a valid empty answer.
        assert!(
            search_blocking(root.path(), "suspend x11-driver-mystery")
                .expect("present sources answer")
                .is_empty()
        );
        drop(root);
    }

    #[test]
    fn search_with_no_reachable_source_is_honestly_unavailable() {
        let root = testutil::temp_dir("docs-unreachable");
        // An empty guides root: no source dir exists at all.
        let result = search_blocking(root.path(), "suspend");
        assert!(
            matches!(result, Err(ContractError::AdapterUnavailable)),
            "zero reachable sources must not fold into an empty success: {result:?}"
        );
        drop(root);
    }

    #[test]
    fn read_article_maps_missing_file_and_unreachable_source() {
        let root = testutil::temp_dir("docs-read");
        let resolved = sources::resolve("https://wiki.debian.org/Suspend").expect("url");

        // Source unreachable: transport class.
        let unreachable = read_article(root.path(), &resolved);
        assert!(matches!(
            unreachable,
            Err(ContractError::AdapterUnavailable)
        ));

        // Source present, article missing: the article's true state.
        std::fs::create_dir_all(root.path().join("debian-wiki")).expect("source dir");
        let missing = read_article(root.path(), &resolved);
        match missing {
            Err(ContractError::NativeFailure { code, payload }) => {
                assert_eq!(code, NF_DOC_ERROR, "{payload}");
                assert!(payload.contains("no article"), "{payload}");
            }
            other => panic!("a missing article is a doc-level answer: {other:?}"),
        }

        // Fresh read of an existing article.
        std::fs::write(
            root.path().join("debian-wiki").join("suspend.md"),
            "# Suspend\nDebian wiki body.",
        )
        .expect("fixture");
        let text = read_article(root.path(), &resolved).expect("fresh read");
        assert!(text.contains("Debian wiki body"));
        drop(root);
    }
}
