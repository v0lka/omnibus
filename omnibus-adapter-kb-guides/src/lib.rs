#![cfg_attr(test, allow(clippy::expect_used))] // unit tests may expect; the seam must not
//! kb-guides adapter: knowledge base search and pages over distro docs —
//! Debian wiki, Ubuntu docs, Mint docs, Fedora docs (roadmap 2.5.2). The
//! adapter mirrors kb-archwiki (Ф1.5.6): every fetched article is cached under
//! the adapter's cache directory and degradation is honest — when a doc
//! source is unreachable, cached articles come back flagged stale; with
//! nothing cached the failure is reported as-is. A present source that
//! answers "no such article" is the article's true state and propagates
//! untouched — stale text never masks it, so a dead URL fails instead of
//! retrying forever against cached content.
//!
//! Native backend types (per-source mirror paths, file contents) never leave
//! this crate — mapping to contract DTOs happens inside the adapter.

mod cache;
mod docs;
mod sources;

use async_trait::async_trait;
use omnibus_contracts::capabilities::KB_PRESENT;
use omnibus_contracts::dto::Milliseconds;
use omnibus_contracts::dto::kb::{KbArticle, KbArticleRef, KbPageRequest, KbQuery};
use omnibus_contracts::dto::probe::{AdapterReport, CapabilityFact};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::kb::KnowledgeBase;
use omnibus_contracts::version::CONTRACT_VERSION;

/// Search-result budget when the query carries no explicit limit.
const DEFAULT_SEARCH_LIMIT: u32 = 10;

/// KnowledgeBase provider over the four registered distro doc sources. Each
/// source's readable corpus is the `<guides_dir>/<source id>/` directory of
/// `.md` articles (the kb-archwiki Manjaro-overlay layout, one directory per
/// distro). Constructing performs no I/O; directories are touched lazily on
/// the first operation.
#[derive(Debug, Clone)]
pub struct KbGuidesAdapter {
    cache_dir: std::path::PathBuf,
    guides_dir: Option<std::path::PathBuf>,
}

impl KbGuidesAdapter {
    /// Creates the adapter. `cache_dir` receives one JSON file per fetched
    /// article; `guides_dir` (when given) is the root holding one `.md`
    /// article directory per registered distro doc source.
    pub fn new(cache_dir: std::path::PathBuf, guides_dir: Option<std::path::PathBuf>) -> Self {
        Self {
            cache_dir,
            guides_dir,
        }
    }

    /// Search round trip off the async worker thread. With no guides root
    /// configured no source is reachable — the honest unavailable answer, not
    /// a silent empty result set.
    async fn search_refs(&self, text: &str) -> Result<Vec<KbArticleRef>, ContractError> {
        let guides_dir = self.guides_dir.clone();
        let text = String::from(text);
        tokio::task::spawn_blocking(move || match guides_dir.as_deref() {
            Some(dir) => docs::search_blocking(dir, &text),
            None => Err(ContractError::AdapterUnavailable),
        })
        .await
        .map_err(|join| ContractError::NativeFailure {
            code: docs::NF_TRANSPORT,
            payload: format!("kb-guides search worker failed: {join}"),
        })?
    }

    /// One page fetch off the async worker thread; on success the article is
    /// persisted into the cache directory under the request-derived slug —
    /// the same key the offline fallback looks up.
    async fn fetch_and_cache(
        &self,
        url: String,
        resolved: sources::ResolvedArticle,
    ) -> Result<KbArticle, ContractError> {
        // Extracted before the blocking move: the closure owns the resolved
        // article, the builder below owns title/source/slug.
        let title = sources::humanize(&resolved.slug);
        let source = resolved.source_id.clone();
        let slug = resolved.slug.clone();
        let guides_dir = self.guides_dir.clone();
        let text = tokio::task::spawn_blocking(move || {
            guides_dir
                .as_deref()
                .ok_or(ContractError::AdapterUnavailable)
                .and_then(|dir| docs::read_article(dir, &resolved))
        })
        .await
        .map_err(|join| ContractError::NativeFailure {
            code: docs::NF_TRANSPORT,
            payload: format!("kb-guides page worker failed: {join}"),
        })??;
        let article = KbArticle {
            title,
            url,
            source,
            text,
            cached_at_epoch_ms: Some(Milliseconds::now_epoch().0),
            stale: false,
        };
        // Cache persistence is best-effort: a failed write never fails the
        // read that produced the article. The write itself runs on a blocking
        // thread.
        let cache_dir = self.cache_dir.clone();
        let cached = article.clone();
        let _ = tokio::task::spawn_blocking(move || cache::store(&cache_dir, &slug, &cached)).await;
        Ok(article)
    }

    /// The stale cached copy of an article, if the cache holds one; the load
    /// runs on a blocking thread.
    async fn stale_cached(&self, slug: &str) -> Option<KbArticle> {
        let cache_dir = self.cache_dir.clone();
        let slug = String::from(slug);
        tokio::task::spawn_blocking(move || cache::load(&cache_dir, &slug))
            .await
            .ok()
            .flatten()
    }
}

#[async_trait]
impl KnowledgeBase for KbGuidesAdapter {
    /// The adapter report: identity, the KB_PRESENT fact, and the configured
    /// sources (guides root and cache location). The fact is honest per the
    /// R3 ladder: with a configured docs mirror the capability is `present`;
    /// without one every `wiki_search`/`wiki_page` call answers
    /// `AdapterUnavailable` (no reachable source, an empty cache on a fresh
    /// install), so the fact is `degraded` (ladder rung 1 — the tools stay
    /// wired, the limitation is visible) and carries the limitation plus the
    /// exact remedy IN the fact — the capabilities document surfaces the fact
    /// fields; report-level notes never reach it.
    fn report(&self) -> AdapterReport {
        let source_list = sources::REGISTRY
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>()
            .join(", ");
        let mut notes = vec![format!("registered distro doc sources: {source_list}")];
        let (status, note, remedy) = match &self.guides_dir {
            Some(dir) => {
                notes.push(format!("distro docs root at {}", dir.display()));
                (
                    "present",
                    Some(String::from(
                        "Debian wiki, Ubuntu docs, Mint docs and Fedora docs",
                    )),
                    None,
                )
            }
            None => {
                notes.push(String::from("no distro docs root configured"));
                (
                    "degraded",
                    Some(String::from(
                        "no distro docs mirror configured: wiki_search/wiki_page answer \
                         AdapterUnavailable until a mirror exists",
                    )),
                    Some(format!(
                        "create the mirror root /usr/share/omnibus/kb-guides (or \
                         ~/.local/share/omnibus/kb-guides) with one .md article directory per \
                         registered source ({source_list})"
                    )),
                )
            }
        };
        notes.push(format!("article cache at {}", self.cache_dir.display()));
        AdapterReport {
            adapter: String::from("kb-guides"),
            subsystem: String::from("kb"),
            contract_version: String::from(CONTRACT_VERSION),
            capabilities: vec![CapabilityFact {
                name: String::from(KB_PRESENT),
                ver: String::from(CONTRACT_VERSION),
                status: String::from(status),
                value: None,
                note,
                remedy,
            }],
            notes,
        }
    }

    /// Article references matching the query, gathered from every reachable
    /// source mirror in registry order. With no reachable source the honest
    /// unavailable answer surfaces — a search against a dead corpus never
    /// folds into a silent empty result set.
    async fn search(&self, query: KbQuery) -> Result<Vec<KbArticleRef>, ContractError> {
        let limit = query.limit.unwrap_or(DEFAULT_SEARCH_LIMIT).max(1);
        let mut refs = self.search_refs(&query.text).await?;
        refs.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(refs)
    }

    /// One article with cache provenance: fresh fetches are read from the
    /// source mirror and stored under the cache dir; when the source is
    /// unreachable the stale cached copy is served with `stale: true`;
    /// nothing cached means the failure is reported as-is. A doc-level answer
    /// (the source is present and has no such article) propagates even when a
    /// stale copy exists — the article's true state must reach the caller.
    async fn page(&self, request: KbPageRequest) -> Result<KbArticle, ContractError> {
        let Some(resolved) = sources::resolve(&request.url) else {
            return Err(ContractError::NativeFailure {
                code: docs::NF_BAD_URL,
                payload: format!(
                    "page url does not name a registered distro doc source: {}",
                    request.url
                ),
            });
        };
        match self
            .fetch_and_cache(request.url.clone(), resolved.clone())
            .await
        {
            Ok(article) => Ok(article),
            // Only transport-class failures degrade to the stale cache.
            Err(source_error) if is_transport_failure(&source_error) => {
                match self.stale_cached(&resolved.slug).await {
                    Some(mut cached) => {
                        cached.stale = true;
                        cached.url = request.url;
                        Ok(cached)
                    }
                    None => Err(source_error),
                }
            }
            Err(e) => Err(e),
        }
    }
}

/// True when a failed page fetch stayed at the transport layer — an
/// unreachable source root or source directory, an unreadable article body, a
/// failed worker — so the article itself may still exist and a cached copy
/// may serve, flagged stale. A doc-level `NativeFailure` (`docs::NF_DOC_ERROR`:
/// the source answered and the article's state is known) and a malformed
/// request (`docs::NF_BAD_URL`) are not transport breakdowns: serving stale
/// text for them would let a dead URL retry forever.
fn is_transport_failure(err: &ContractError) -> bool {
    match err {
        ContractError::TimeoutError | ContractError::AdapterUnavailable => true,
        ContractError::NativeFailure { code, .. } => {
            *code != docs::NF_DOC_ERROR && *code != docs::NF_BAD_URL
        }
        _ => false,
    }
}

#[cfg(test)]
pub mod testutil {
    //! Unique temporary directories without external dev-dependencies; the
    //! guard removes the whole tree on drop.

    use std::path::{Path, PathBuf};

    /// A temporary directory removed with its contents when dropped.
    pub struct TempRoot {
        path: PathBuf,
    }

    impl TempRoot {
        /// The root path to build fixtures under.
        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Creates a unique temporary root; `name` separates test families.
    pub fn temp_dir(name: &str) -> TempRoot {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "omnibus-kb-guides-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp root");
        TempRoot { path }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_contracts::suite;

    const SUSPEND_URL: &str = "https://help.ubuntu.com/suspend-and-hibernate";
    const DEBIAN_SUSPEND_URL: &str = "https://wiki.debian.org/Suspend";

    /// A guides root with one populated source (ubuntu-docs) and one absent
    /// source (debian-wiki) — the offline-fixture shape.
    fn offline_fixture() -> (testutil::TempRoot, testutil::TempRoot) {
        let guides = testutil::temp_dir("guides");
        let ubuntu = guides.path().join("ubuntu-docs");
        std::fs::create_dir_all(&ubuntu).expect("source dir");
        std::fs::write(
            ubuntu.join("suspend-and-hibernate.md"),
            "# Suspend\nUbuntu fix: check the swap size before systemctl suspend.",
        )
        .expect("fixture");
        let cache = testutil::temp_dir("cache");
        (guides, cache)
    }

    /// Seeds the cache the way the fresh path stores an article.
    fn seed_cache(cache_dir: &std::path::Path, url: &str, text: &str) {
        let resolved = sources::resolve(url).expect("fixture url");
        cache::store(
            cache_dir,
            &resolved.slug,
            &KbArticle {
                title: sources::humanize(&resolved.slug),
                url: String::from(url),
                source: resolved.source_id,
                text: String::from(text),
                cached_at_epoch_ms: Some(1730000000000),
                stale: false,
            },
        )
        .expect("seed cache");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fresh_page_reads_the_mirror_and_caches_the_article() {
        let (guides, cache) = offline_fixture();
        let adapter = KbGuidesAdapter::new(
            cache.path().to_path_buf(),
            Some(guides.path().to_path_buf()),
        );
        let article = adapter
            .page(KbPageRequest {
                url: String::from(SUSPEND_URL),
            })
            .await
            .expect("fresh read");
        assert!(!article.stale, "a fresh read is never flagged stale");
        assert_eq!(article.source, "ubuntu-docs");
        assert_eq!(article.title, "suspend and hibernate");
        assert!(article.text.contains("systemctl suspend"));
        assert!(article.cached_at_epoch_ms.is_some());
        // The fresh path stored the article under the request-derived slug.
        let slug = sources::resolve(SUSPEND_URL).expect("url").slug;
        assert!(cache::load(cache.path(), &slug).is_some());
        drop((guides, cache));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn nested_doc_paths_flatten_to_one_slug_fresh_and_offline() {
        let (guides, cache) = offline_fixture();
        let fedora = guides.path().join("fedora-docs");
        std::fs::create_dir_all(&fedora).expect("source dir");
        let body = "# Adding users to sudoers\nFedora quick doc body.";
        std::fs::write(
            fedora.join("en-us-quick-docs-adding-users-to-sudoers.md"),
            body,
        )
        .expect("fixture");
        let adapter = KbGuidesAdapter::new(
            cache.path().to_path_buf(),
            Some(guides.path().to_path_buf()),
        );
        let url = String::from(
            "https://docs.fedoraproject.org/en-US/quick-docs/Adding%20Users%20to%20sudoers",
        );
        let fresh = adapter
            .page(KbPageRequest { url: url.clone() })
            .await
            .expect("fresh nested read");
        assert!(!fresh.stale);
        assert!(fresh.text.contains("Fedora quick doc body"));

        // The same URL degrades to the cached copy once the source is gone.
        std::fs::remove_dir_all(&fedora).expect("remove source");
        let stale = adapter
            .page(KbPageRequest { url })
            .await
            .expect("stale nested read");
        assert!(stale.stale, "offline copy must be flagged stale");
        assert_eq!(stale.text, body);
        drop((guides, cache));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn offline_page_serves_stale_cache_and_reports_the_failure_without_it() {
        let (guides, cache) = offline_fixture();
        let adapter = KbGuidesAdapter::new(
            cache.path().to_path_buf(),
            Some(guides.path().to_path_buf()),
        );
        // debian-wiki has no mirror directory: the source is unreachable.
        seed_cache(cache.path(), DEBIAN_SUSPEND_URL, "cached debian body");
        let stale = adapter
            .page(KbPageRequest {
                url: String::from(DEBIAN_SUSPEND_URL),
            })
            .await
            .expect("stale cache must serve");
        assert!(stale.stale, "offline copy must be flagged stale");
        assert_eq!(stale.text, "cached debian body");
        assert_eq!(stale.source, "debian-wiki");
        assert_eq!(stale.cached_at_epoch_ms, Some(1730000000000));

        // Nothing cached: the unreachable source is an honest failure, not a
        // missing adapter and not a silent empty answer.
        let fresh = adapter
            .page(KbPageRequest {
                url: String::from("https://wiki.debian.org/PowerManagement"),
            })
            .await;
        assert!(
            matches!(fresh, Err(ContractError::AdapterUnavailable)),
            "an unreachable source without cache fails honestly: {fresh:?}"
        );
        drop((guides, cache));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn doc_level_missing_article_propagates_even_with_a_stale_cache() {
        let (guides, cache) = offline_fixture();
        // debian-wiki present but the article is gone: the source answered.
        std::fs::create_dir_all(guides.path().join("debian-wiki")).expect("source dir");
        seed_cache(cache.path(), DEBIAN_SUSPEND_URL, "cached debian body");
        let adapter = KbGuidesAdapter::new(
            cache.path().to_path_buf(),
            Some(guides.path().to_path_buf()),
        );
        let result = adapter
            .page(KbPageRequest {
                url: String::from(DEBIAN_SUSPEND_URL),
            })
            .await;
        match result {
            Err(ContractError::NativeFailure { code, payload }) => {
                assert_eq!(code, docs::NF_DOC_ERROR, "{payload}");
                assert!(payload.contains("no article"), "{payload}");
            }
            other => panic!("a doc-level answer must propagate: {other:?}"),
        }
        drop((guides, cache));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn foreign_urls_are_rejected_and_never_serve_stale() {
        let (guides, cache) = offline_fixture();
        seed_cache(cache.path(), DEBIAN_SUSPEND_URL, "cached debian body");
        let adapter = KbGuidesAdapter::new(
            cache.path().to_path_buf(),
            Some(guides.path().to_path_buf()),
        );
        let result = adapter
            .page(KbPageRequest {
                url: String::from("https://wiki.archlinux.org/title/Suspend"),
            })
            .await;
        match result {
            Err(ContractError::NativeFailure { code, payload }) => {
                assert_eq!(code, docs::NF_BAD_URL, "{payload}");
            }
            other => panic!("a foreign url must be rejected: {other:?}"),
        }
        drop((guides, cache));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_finds_the_known_fix_and_honors_the_limit() {
        let (guides, cache) = offline_fixture();
        let adapter = KbGuidesAdapter::new(
            cache.path().to_path_buf(),
            Some(guides.path().to_path_buf()),
        );
        let refs = adapter
            .search(KbQuery {
                text: String::from("suspend swap"),
                limit: Some(5),
            })
            .await
            .expect("reachable sources answer");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].source, "ubuntu-docs");
        assert_eq!(refs[0].url, SUSPEND_URL);
        assert!(refs[0].snippet.contains("systemctl suspend"));

        // A term outside the corpus matches nothing — a valid empty answer
        // over reachable sources.
        let none = adapter
            .search(KbQuery {
                text: String::from("suspend x11-driver-mystery"),
                limit: None,
            })
            .await
            .expect("reachable sources answer");
        assert!(none.is_empty());
        drop((guides, cache));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_without_a_docs_root_is_honestly_unavailable() {
        let cache = testutil::temp_dir("cache-only");
        let adapter = KbGuidesAdapter::new(cache.path().to_path_buf(), None);
        let result = adapter
            .search(KbQuery {
                text: String::from("suspend"),
                limit: None,
            })
            .await;
        assert!(
            matches!(result, Err(ContractError::AdapterUnavailable)),
            "no reachable source must not fold into an empty success: {result:?}"
        );
        drop(cache);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kb_suite_passes_against_the_offline_fixture() {
        let (guides, cache) = offline_fixture();
        // Seed the cache so the suite's page() leg has a stale answer for the
        // source whose mirror is absent (debian-wiki).
        seed_cache(cache.path(), DEBIAN_SUSPEND_URL, "cached debian body");
        let adapter = KbGuidesAdapter::new(
            cache.path().to_path_buf(),
            Some(guides.path().to_path_buf()),
        );
        let report = suite::kb::kb_suite(
            move || Box::new(adapter.clone()) as Box<dyn KnowledgeBase>,
            suite::kb::KbFixture {
                query_text: String::from("suspend"),
                expected_title_fragment: String::from("suspend"),
                known_url: String::from(DEBIAN_SUSPEND_URL),
                offline: true,
            },
        )
        .await;
        assert!(report.is_clean(), "offline kb suite red: {report:?}");
        drop((guides, cache));
    }

    #[test]
    fn report_names_the_capability_constant_and_sources() {
        let (guides, cache) = offline_fixture();
        let adapter = KbGuidesAdapter::new(
            cache.path().to_path_buf(),
            Some(guides.path().to_path_buf()),
        );
        let report = adapter.report();
        assert_eq!(report.adapter, "kb-guides");
        assert_eq!(report.subsystem, "kb");
        assert_eq!(report.capabilities.len(), 1);
        assert_eq!(report.capabilities[0].name, KB_PRESENT);
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("debian-wiki") && n.contains("fedora-docs")),
            "the registered sources are named: {:?}",
            report.notes
        );
        assert!(report.notes.iter().any(|n| n.contains("cache")));
        drop((guides, cache));
    }

    #[test]
    fn report_degrades_honestly_without_a_docs_mirror() {
        // R3 rung 1: with no configured mirror every wiki call answers
        // AdapterUnavailable (no reachable source, an empty cache on a fresh
        // install), so the fact must not advertise `present` — the degraded
        // status plus the limitation and the exact remedy travel IN the fact,
        // because the capabilities document surfaces the fact fields while
        // report-level notes never reach it.
        let cache = testutil::temp_dir("report-degraded");
        let adapter = KbGuidesAdapter::new(cache.path().to_path_buf(), None);
        let report = adapter.report();
        assert_eq!(report.capabilities.len(), 1);
        let fact = &report.capabilities[0];
        assert_eq!(fact.name, KB_PRESENT);
        assert_eq!(fact.status, "degraded", "no mirror is never `present`");
        assert!(
            fact.note
                .as_deref()
                .is_some_and(|n| n.contains("AdapterUnavailable")),
            "the note names the honest tool behavior: {:?}",
            fact.note
        );
        let remedy = fact.remedy.as_deref().unwrap_or_default();
        assert!(
            remedy.contains("/usr/share/omnibus/kb-guides") && remedy.contains("debian-wiki"),
            "the remedy names exactly what to create, per source: {remedy}"
        );
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("no distro docs root")),
            "the report note stays too: {:?}",
            report.notes
        );
        drop(cache);
    }

    #[test]
    fn transport_failures_degrade_but_doc_answers_propagate() {
        // The transport classes: the article itself may still exist.
        for transport in [
            ContractError::TimeoutError,
            ContractError::AdapterUnavailable,
            ContractError::NativeFailure {
                code: docs::NF_TRANSPORT,
                payload: String::from("kb-guides: unreadable article body"),
            },
        ] {
            assert!(
                is_transport_failure(&transport),
                "transport class must degrade: {transport:?}"
            );
        }
        // The non-transport classes: the article's true state is known, the
        // stale cache must stay silent.
        for terminal in [
            ContractError::NativeFailure {
                code: docs::NF_DOC_ERROR,
                payload: String::from("doc source debian-wiki has no article suspend.md"),
            },
            ContractError::NativeFailure {
                code: docs::NF_BAD_URL,
                payload: String::from("page url does not name a registered distro doc source"),
            },
            ContractError::CapabilityRequired,
        ] {
            assert!(
                !is_transport_failure(&terminal),
                "terminal answers must propagate: {terminal:?}"
            );
        }
    }
}
