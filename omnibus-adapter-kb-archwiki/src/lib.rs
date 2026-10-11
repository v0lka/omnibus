//! kb-archwiki adapter: knowledge base search and pages (ArchWiki + Manjaro
//! overlay; roadmap 1.5.6 reading, 2.5.2 overlay). The adapter caches every
//! fetched article under its cache directory and degrades honestly: when the
//! network is gone, cached articles come back flagged stale; with nothing
//! cached the adapter reports itself unavailable (stale-flag rule of the
//! knowledge domain spec).
//!
//! Native backend types (HTTP responses, MediaWiki payloads) never leave this
//! crate — mapping to contract DTOs happens inside the adapter.

mod cache;
mod overlay;
mod wiki;

use async_trait::async_trait;
use omnibus_contracts::capabilities::KB_PRESENT;
use omnibus_contracts::dto::kb::{KbArticle, KbArticleRef, KbPageRequest, KbQuery};
use omnibus_contracts::dto::probe::{AdapterReport, CapabilityFact};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::kb::KnowledgeBase;
use omnibus_contracts::version::CONTRACT_VERSION;

/// Search-result budget when the query carries no explicit limit.
const DEFAULT_SEARCH_LIMIT: u32 = 10;

/// KnowledgeBase provider over the ArchWiki MediaWiki API plus the Manjaro
/// overlay directory. Constructing performs no I/O; directories are touched
/// lazily on the first operation.
#[derive(Debug, Clone)]
pub struct KbArchWikiAdapter {
    cache_dir: std::path::PathBuf,
    overlay_dir: Option<std::path::PathBuf>,
    config: wiki::KbConfig,
}

impl KbArchWikiAdapter {
    /// Creates the adapter. `cache_dir` receives one JSON file per fetched
    /// article; `overlay_dir` (when given) is a directory of `.md` articles
    /// prepended to every search.
    pub fn new(cache_dir: std::path::PathBuf, overlay_dir: Option<std::path::PathBuf>) -> Self {
        Self {
            cache_dir,
            overlay_dir,
            config: wiki::KbConfig::from_env(),
        }
    }

    /// Test/diagnostic constructor with an explicit endpoint configuration
    /// (offline-degradation tests point the base URL at a dead port).
    #[doc(hidden)]
    pub fn with_config(
        cache_dir: std::path::PathBuf,
        overlay_dir: Option<std::path::PathBuf>,
        config: wiki::KbConfig,
    ) -> Self {
        Self {
            cache_dir,
            overlay_dir,
            config,
        }
    }

    /// The endpoint configuration this adapter talks to.
    pub fn config(&self) -> &wiki::KbConfig {
        &self.config
    }

    /// Overlay-backed search references, always ordered before wiki hits.
    fn overlay_refs(&self, query_text: &str) -> Vec<KbArticleRef> {
        overlay::search(self.overlay_dir.as_deref(), query_text)
    }

    /// One wiki search round trip off the async worker thread.
    async fn wiki_refs(&self, text: &str, limit: u32) -> Result<Vec<KbArticleRef>, ContractError> {
        let config = self.config.clone();
        let text = text.to_string();
        let hits =
            tokio::task::spawn_blocking(move || wiki::search_blocking(&config, &text, limit))
                .await
                .map_err(|join| ContractError::NativeFailure {
                    code: 1,
                    payload: format!("archwiki search worker failed: {join}"),
                })??;
        Ok(hits
            .into_iter()
            .map(|(title, url, snippet)| KbArticleRef {
                title,
                url,
                snippet,
                source: String::from("archwiki"),
            })
            .collect())
    }

    /// One wiki page fetch off the async worker thread; on success the article
    /// is persisted into the cache directory.
    async fn wiki_fetch_and_cache(
        &self,
        url: &str,
        title: &str,
    ) -> Result<KbArticle, ContractError> {
        let config = self.config.clone();
        let title = title.to_string();
        let fetched = tokio::task::spawn_blocking(move || wiki::wikitext_blocking(&config, &title))
            .await
            .map_err(|join| ContractError::NativeFailure {
                code: 1,
                payload: format!("archwiki page worker failed: {join}"),
            })??;
        let (canonical_title, wikitext) = fetched;
        let article = KbArticle {
            title: canonical_title,
            url: String::from(url),
            source: String::from("archwiki"),
            text: wikitext,
            cached_at_epoch_ms: Some(now_epoch_ms()),
            stale: false,
        };
        // Cache persistence is best-effort: a failed write never fails the
        // read that produced the article.
        let _ = cache::store(&self.cache_dir, &article);
        Ok(article)
    }
}

#[async_trait]
impl KnowledgeBase for KbArchWikiAdapter {
    /// The adapter report: identity, the KB_PRESENT fact, and the configured
    /// sources (cache and overlay locations).
    fn report(&self) -> AdapterReport {
        let mut notes = vec![format!(
            "archwiki endpoint {} with article cache at {}",
            self.config.base_url,
            self.cache_dir.display()
        )];
        match &self.overlay_dir {
            Some(dir) => notes.push(format!("manjaro overlay at {}", dir.display())),
            None => notes.push(String::from("no manjaro overlay configured")),
        }
        AdapterReport {
            adapter: String::from("kb-archwiki"),
            subsystem: String::from("kb"),
            contract_version: String::from(CONTRACT_VERSION),
            capabilities: vec![CapabilityFact {
                name: String::from(KB_PRESENT),
                ver: String::from(CONTRACT_VERSION),
                status: String::from("present"),
                value: None,
                note: Some(String::from("ArchWiki plus the manjaro-overlay directory")),
                remedy: None,
            }],
            notes,
        }
    }

    /// Article references matching the query: overlay entries first (they are
    /// local and distro-specific), wiki hits after. With the network gone, the
    /// overlay still answers; with nothing local, the failure is honest.
    async fn search(&self, query: KbQuery) -> Result<Vec<KbArticleRef>, ContractError> {
        let limit = query.limit.unwrap_or(DEFAULT_SEARCH_LIMIT).max(1);
        let mut refs = self.overlay_refs(&query.text);
        if refs.len() < usize::try_from(limit).unwrap_or(usize::MAX) {
            let wiki_limit = limit.saturating_sub(u32::try_from(refs.len()).unwrap_or(u32::MAX));
            match self.wiki_refs(&query.text, wiki_limit).await {
                Ok(mut wiki_hits) => refs.append(&mut wiki_hits),
                // Offline degradation: overlay results stand alone; an empty
                // overlay plus a dead network is an unavailable adapter.
                Err(e) if refs.is_empty() => return Err(e),
                Err(_) => {}
            }
        }
        refs.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(refs)
    }

    /// One article with cache provenance: fresh fetches are stored under the
    /// cache dir; when the fetch fails, the stale cached copy is served with
    /// `stale: true`; nothing cached means the adapter is unavailable.
    async fn page(&self, request: KbPageRequest) -> Result<KbArticle, ContractError> {
        if request.url.starts_with(overlay::OVERLAY_SCHEME) {
            return overlay::page(self.overlay_dir.as_deref(), &request);
        }
        let Some(title) = wiki::title_from_url(&request.url) else {
            return Err(ContractError::NativeFailure {
                code: 22,
                payload: format!("page url does not name a wiki article: {}", request.url),
            });
        };
        let Some(slug) = wiki::slug_from_url(&request.url) else {
            return Err(ContractError::NativeFailure {
                code: 22,
                payload: format!("page url has no cacheable slug: {}", request.url),
            });
        };
        match self.wiki_fetch_and_cache(&request.url, &title).await {
            Ok(article) => Ok(article),
            Err(network_error) => match cache::load(&self.cache_dir, &slug) {
                Some(mut cached) => {
                    cached.stale = true;
                    cached.url = request.url;
                    Ok(cached)
                }
                None => Err(network_error),
            },
        }
    }
}

/// Wall-clock epoch milliseconds; a clock running before the epoch saturates
/// instead of panicking.
fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_contracts::suite;

    /// Endpoint config aimed at an unreachable localhost port: connections
    /// fail fast, which is exactly the offline-degradation fixture.
    fn dead_endpoint_config() -> wiki::KbConfig {
        wiki::KbConfig {
            base_url: String::from("http://127.0.0.1:9"),
            timeout: std::time::Duration::from_millis(200),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn overlay_search_with_tempdir_needs_no_network() {
        let overlay_dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            overlay_dir.path().join("suspend-and-hibernate.md"),
            "# Suspend\nManjaro fix: check swap before systemctl suspend.",
        )
        .expect("fixture");
        let cache_dir = tempfile::tempdir().expect("tempdir");
        let adapter = KbArchWikiAdapter::with_config(
            cache_dir.path().to_path_buf(),
            Some(overlay_dir.path().to_path_buf()),
            dead_endpoint_config(),
        );
        let refs = adapter
            .search(KbQuery {
                text: String::from("suspend swap"),
                limit: Some(5),
            })
            .await
            .expect("overlay hits stand alone even with a dead network");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].source, overlay::OVERLAY_SOURCE);
        assert_eq!(refs[0].url, "overlay:suspend-and-hibernate");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn offline_page_serves_stale_cache_and_adapter_unavailable_without_it() {
        let cache_dir = tempfile::tempdir().expect("tempdir");
        let overlay_dir = tempfile::tempdir().expect("tempdir");
        let adapter = KbArchWikiAdapter::with_config(
            cache_dir.path().to_path_buf(),
            Some(overlay_dir.path().to_path_buf()),
            dead_endpoint_config(),
        );
        let url = wiki::article_url(wiki::DEFAULT_BASE_URL, "Suspend");

        // Nothing cached yet: the dead network is an honest unavailability.
        let fresh = adapter.page(KbPageRequest { url: url.clone() }).await;
        assert!(matches!(fresh, Err(ContractError::AdapterUnavailable)));

        // Seed the cache, then the same request degrades to a stale hit.
        let seeded = KbArticle {
            title: String::from("Suspend"),
            url: url.clone(),
            source: String::from("archwiki"),
            text: String::from("cached wikitext body"),
            cached_at_epoch_ms: Some(1730000000000),
            stale: false,
        };
        cache::store(cache_dir.path(), &seeded).expect("seed cache");
        let stale = adapter
            .page(KbPageRequest { url: url.clone() })
            .await
            .expect("stale cache must serve");
        assert!(stale.stale, "offline copy must be flagged stale");
        assert_eq!(stale.text, "cached wikitext body");
        assert_eq!(stale.cached_at_epoch_ms, Some(1730000000000));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn kb_suite_passes_against_the_offline_overlay_fixture() {
        let cache_dir = tempfile::tempdir().expect("tempdir");
        let overlay_dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            overlay_dir.path().join("suspend-and-hibernate.md"),
            "# Suspend\nManjaro fix: check swap before systemctl suspend.",
        )
        .expect("fixture");
        let adapter = KbArchWikiAdapter::with_config(
            cache_dir.path().to_path_buf(),
            Some(overlay_dir.path().to_path_buf()),
            dead_endpoint_config(),
        );
        // Seed the wiki cache so the suite's page() leg has a stale answer.
        let known_url = wiki::article_url(wiki::DEFAULT_BASE_URL, "Suspend");
        cache::store(
            cache_dir.path(),
            &KbArticle {
                title: String::from("Suspend"),
                url: known_url.clone(),
                source: String::from("archwiki"),
                text: String::from("cached wikitext body"),
                cached_at_epoch_ms: Some(1730000000000),
                stale: false,
            },
        )
        .expect("seed cache");
        let report = suite::kb::kb_suite(
            move || Box::new(adapter.clone()) as Box<dyn KnowledgeBase>,
            suite::kb::KbFixture {
                query_text: String::from("suspend"),
                expected_title_fragment: String::from("suspend"),
                known_url,
                offline: true,
            },
        )
        .await;
        assert!(report.is_clean(), "offline kb suite red: {report:?}");
    }

    #[test]
    fn report_names_the_capability_constant_and_sources() {
        let cache_dir = tempfile::tempdir().expect("tempdir");
        let adapter = KbArchWikiAdapter::new(cache_dir.path().to_path_buf(), None);
        let report = adapter.report();
        assert_eq!(report.adapter, "kb-archwiki");
        assert_eq!(report.capabilities.len(), 1);
        assert_eq!(report.capabilities[0].name, KB_PRESENT);
        assert!(report.notes.iter().any(|n| n.contains("cache")));
    }
}
