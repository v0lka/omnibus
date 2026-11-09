#![cfg_attr(test, allow(clippy::expect_used))] // unit tests may expect; the seam must not
//! kb-archwiki adapter: knowledge base search and pages (ArchWiki + Manjaro
//! overlay; roadmap 1.5.6 reading, 2.5.2 overlay). The adapter caches every
//! fetched article under its cache directory and degrades honestly: when the
//! network is gone, cached articles come back flagged stale; with nothing
//! cached the transport failure is reported as-is (stale-flag rule of the
//! knowledge domain spec). API-level answers (`wiki::NF_API_ERROR`: invalid
//! title, deleted article) are the article's true state and propagate
//! untouched — stale text never masks them, so a dead URL fails instead of
//! retrying forever against cached content.
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

    /// Overlay-backed search references, always ordered before wiki hits. The
    /// directory scan and body reads run on a blocking thread (the SPI is
    /// async; wiki.rs module rule).
    async fn overlay_refs(&self, query_text: &str) -> Vec<KbArticleRef> {
        let overlay_dir = self.overlay_dir.clone();
        let query_text = String::from(query_text);
        tokio::task::spawn_blocking(move || overlay::search(overlay_dir.as_deref(), &query_text))
            .await
            // The overlay is an additive source: a failed blocking worker
            // contributes no entries, never a hard failure.
            .unwrap_or_default()
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
    /// is persisted into the cache directory under the request-derived slug —
    /// the same key the offline fallback looks up, so a URL naming a redirect
    /// still finds its cached copy (the canonical title stays in the article).
    async fn wiki_fetch_and_cache(
        &self,
        url: &str,
        title: &str,
        slug: &str,
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
        // read that produced the article. The write itself runs on a blocking
        // thread.
        let cache_dir = self.cache_dir.clone();
        let slug = String::from(slug);
        let cached = article.clone();
        let _ = tokio::task::spawn_blocking(move || cache::store(&cache_dir, &slug, &cached)).await;
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
    /// overlay still answers; with nothing local, the failure is honest. An
    /// API-level wiki rejection propagates even when overlay hits exist —
    /// only transport-class failures degrade to overlay-only results.
    async fn search(&self, query: KbQuery) -> Result<Vec<KbArticleRef>, ContractError> {
        let limit = query.limit.unwrap_or(DEFAULT_SEARCH_LIMIT).max(1);
        let mut refs = self.overlay_refs(&query.text).await;
        if refs.len() < usize::try_from(limit).unwrap_or(usize::MAX) {
            let wiki_limit = limit.saturating_sub(u32::try_from(refs.len()).unwrap_or(u32::MAX));
            match self.wiki_refs(&query.text, wiki_limit).await {
                Ok(mut wiki_hits) => refs.append(&mut wiki_hits),
                // Offline degradation: transport-class failures degrade to
                // overlay results standing alone; an empty overlay plus a
                // dead network is an unavailable adapter. An API-level
                // rejection (`wiki::NF_API_ERROR`: the endpoint answered and
                // refused the query) is not transport — folding it into an
                // overlay-only success would hide the wiki's true state, so
                // it propagates even with overlay hits (the same doctrine as
                // `page`).
                Err(e) if refs.is_empty() => return Err(e),
                Err(e) if !is_transport_failure(&e) => return Err(e),
                Err(_) => {}
            }
        }
        refs.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
        Ok(refs)
    }

    /// One article with cache provenance: fresh fetches are stored under the
    /// cache dir; when the fetch fails at the transport layer, the stale
    /// cached copy is served with `stale: true`; nothing cached means the
    /// transport failure is reported as-is. API-level failures (invalid
    /// title, deleted article) propagate even when a stale copy exists — the
    /// article's true state must reach the caller.
    async fn page(&self, request: KbPageRequest) -> Result<KbArticle, ContractError> {
        if request.url.starts_with(overlay::OVERLAY_SCHEME) {
            let overlay_dir = self.overlay_dir.clone();
            let article = tokio::task::spawn_blocking(move || {
                overlay::page(overlay_dir.as_deref(), &request)
            })
            .await
            .map_err(|join| ContractError::NativeFailure {
                code: 1,
                payload: format!("archwiki overlay worker failed: {join}"),
            })??;
            return Ok(article);
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
        match self.wiki_fetch_and_cache(&request.url, &title, &slug).await {
            Ok(article) => Ok(article),
            // Only transport-class failures degrade to the stale cache.
            Err(network_error) if is_transport_failure(&network_error) => {
                let cache_dir = self.cache_dir.clone();
                let cached = tokio::task::spawn_blocking(move || cache::load(&cache_dir, &slug))
                    .await
                    .ok()
                    .flatten();
                match cached {
                    Some(mut cached) => {
                        cached.stale = true;
                        cached.url = request.url;
                        Ok(cached)
                    }
                    None => Err(network_error),
                }
            }
            Err(e) => Err(e),
        }
    }
}

/// True when a failed page fetch stayed at the transport layer — deadline,
/// offline/refused/DNS, HTTP status, unreadable or non-JSON body — so the
/// article itself may still exist and a cached copy may serve, flagged
/// stale. An API-level `NativeFailure` (`wiki::NF_API_ERROR`: the endpoint
/// answered and the article's state is known) is not a transport breakdown:
/// serving stale text for it would let a dead URL retry forever.
fn is_transport_failure(err: &ContractError) -> bool {
    match err {
        ContractError::TimeoutError | ContractError::AdapterUnavailable => true,
        ContractError::NativeFailure { code, .. } => *code != wiki::NF_API_ERROR,
        _ => false,
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

    /// Serves one canned HTTP 200 body per connection on an ephemeral port —
    /// a MediaWiki API stand-in without an HTTP-server dependency. The
    /// listener thread lives for the whole test process; harmless in tests.
    fn spawn_canned_api(body: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                // The request head is consumed before answering: a close with
                // unread inbound bytes makes the kernel send RST, which can
                // reach the client before it drained the buffered response
                // and turn a deterministic answer into ConnectionReset.
                read_request_head(&mut stream);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = std::io::Write::write_all(&mut stream, head.as_bytes());
                let _ = std::io::Write::write_all(&mut stream, body.as_bytes());
                // Anything still inbound is drained so the receive queue is
                // empty at drop — a clean FIN, never RST.
                drain_inbound(&mut stream);
            }
        });
        format!("http://{addr}")
    }

    /// Reads until the blank line that ends an HTTP request head, bounded in
    /// size and wall clock so a malformed peer cannot hang the thread; the
    /// canned GET requests have no body, so the blank line ends the request.
    fn read_request_head(stream: &mut std::net::TcpStream) {
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
        let mut buf = [0u8; 1024];
        let mut seen = Vec::new();
        loop {
            match std::io::Read::read(stream, &mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    seen.extend_from_slice(&buf[..n]);
                    if seen.windows(4).any(|w| w == b"\r\n\r\n") {
                        return;
                    }
                }
            }
        }
    }

    /// Consumes leftover inbound bytes (small cap) until EOF or a short
    /// timeout, so the socket closes with an empty receive queue.
    fn drain_inbound(stream: &mut std::net::TcpStream) {
        const DRAIN_CAP_BYTES: usize = 64 * 1024;
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(500)));
        let mut buf = [0u8; 4096];
        let mut drained = 0usize;
        loop {
            match std::io::Read::read(stream, &mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    drained += n;
                    if drained > DRAIN_CAP_BYTES {
                        return;
                    }
                }
            }
        }
    }

    #[test]
    fn transport_failures_degrade_but_api_errors_propagate() {
        use wiki::NF_API_ERROR;
        // The transport classes: the article itself may still exist.
        for transport in [
            ContractError::TimeoutError,
            ContractError::AdapterUnavailable,
            ContractError::NativeFailure {
                code: 1,
                payload: String::from("archwiki transport failure: refused"),
            },
            ContractError::NativeFailure {
                code: 503,
                payload: String::from("archwiki api returned http 503"),
            },
        ] {
            assert!(
                is_transport_failure(&transport),
                "must degrade: {transport}"
            );
        }
        // The API-level class: the endpoint answered, the article's state is
        // known — never served stale.
        let api_error = ContractError::NativeFailure {
            code: NF_API_ERROR,
            payload: String::from("archwiki api error invalidtitle: bad title"),
        };
        assert!(!is_transport_failure(&api_error), "must propagate");
        assert!(!is_transport_failure(&ContractError::ElementStale));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn api_error_propagates_even_when_a_stale_cache_exists() {
        let api = spawn_canned_api(
            r#"{"error":{"code":"invalidtitle","info":"Bad title: the page is gone"},"servedby":"mw1"}"#,
        );
        let cache_dir = tempfile::tempdir().expect("tempdir");
        let adapter = KbArchWikiAdapter::with_config(
            cache_dir.path().to_path_buf(),
            None,
            wiki::KbConfig {
                base_url: api,
                // Classification subject, not timing: a generous budget keeps
                // parallel test load from fabricating a transport timeout.
                timeout: std::time::Duration::from_secs(30),
            },
        );
        let url = wiki::article_url(wiki::DEFAULT_BASE_URL, "Deleted Article");
        let slug = wiki::slug_from_url(&url).unwrap_or_default();
        cache::store(
            cache_dir.path(),
            &slug,
            &KbArticle {
                title: String::from("Deleted Article"),
                url: url.clone(),
                source: String::from("archwiki"),
                text: String::from("cached wikitext body"),
                cached_at_epoch_ms: Some(1730000000000),
                stale: false,
            },
        )
        .expect("seed cache");
        // The endpoint answered at the API level: the stale copy must stay
        // silent, and the caller must learn the article is gone.
        let result = adapter.page(KbPageRequest { url }).await;
        let Err(err) = result else {
            panic!("an api-level error must propagate even with a stale cache");
        };
        match err {
            ContractError::NativeFailure { code, payload } => {
                assert_eq!(code, wiki::NF_API_ERROR, "{payload}");
                assert!(payload.contains("invalidtitle"), "{payload}");
            }
            other => panic!("api errors surface as native failures: {other:?}"),
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
    async fn api_rejected_search_propagates_even_when_overlay_hits_exist() {
        // The wiki leg answers at the API level (the endpoint is alive and
        // rejects the query) while the overlay holds matching entries: the
        // error must reach the caller, never fold into an overlay-only
        // success — only transport-class failures degrade to overlay.
        let api = spawn_canned_api(
            r#"{"error":{"code":"invalidtitle","info":"Bad title: the query is rejected"},"servedby":"mw1"}"#,
        );
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
            wiki::KbConfig {
                base_url: api,
                // The subject is API-vs-transport classification, not timing:
                // a generous budget keeps parallel test load from turning the
                // round trip into a (transport-class) timeout.
                timeout: std::time::Duration::from_secs(30),
            },
        );
        let result = adapter
            .search(KbQuery {
                text: String::from("suspend swap"),
                limit: Some(5),
            })
            .await;
        match result {
            Err(ContractError::NativeFailure { code, payload }) => {
                assert_eq!(code, wiki::NF_API_ERROR, "{payload}");
                assert!(payload.contains("invalidtitle"), "{payload}");
            }
            other => panic!("an api-level search error must propagate: {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn offline_page_serves_stale_cache_and_reports_transport_failure_without_it() {
        let cache_dir = tempfile::tempdir().expect("tempdir");
        let overlay_dir = tempfile::tempdir().expect("tempdir");
        let adapter = KbArchWikiAdapter::with_config(
            cache_dir.path().to_path_buf(),
            Some(overlay_dir.path().to_path_buf()),
            dead_endpoint_config(),
        );
        let url = wiki::article_url(wiki::DEFAULT_BASE_URL, "Suspend");

        // Nothing cached yet: the dead network is an honest native failure —
        // connection refused is a transport breakdown, not a missing adapter.
        let fresh = adapter.page(KbPageRequest { url: url.clone() }).await;
        assert!(matches!(fresh, Err(ContractError::NativeFailure { .. })));

        // Seed the cache the way the fresh path stores it, then the same
        // request degrades to a stale hit.
        let seeded = KbArticle {
            title: String::from("Suspend"),
            url: url.clone(),
            source: String::from("archwiki"),
            text: String::from("cached wikitext body"),
            cached_at_epoch_ms: Some(1730000000000),
            stale: false,
        };
        let slug = wiki::slug_from_url(&url).unwrap_or_default();
        cache::store(cache_dir.path(), &slug, &seeded).expect("seed cache");
        let stale = adapter
            .page(KbPageRequest { url: url.clone() })
            .await
            .expect("stale cache must serve");
        assert!(stale.stale, "offline copy must be flagged stale");
        assert_eq!(stale.text, "cached wikitext body");
        assert_eq!(stale.cached_at_epoch_ms, Some(1730000000000));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn offline_page_finds_redirect_urls_via_the_request_slug() {
        // A URL naming a redirect stores under the request-derived slug while
        // the article carries the canonical title; store and stale lookup must
        // agree on that key or the offline fallback loses a fetched article.
        let cache_dir = tempfile::tempdir().expect("tempdir");
        let request_url = wiki::article_url(wiki::DEFAULT_BASE_URL, "PMutils");
        let canonical = KbArticle {
            title: String::from("pm-utils"),
            url: request_url.clone(),
            source: String::from("archwiki"),
            text: String::from("canonical wikitext body"),
            cached_at_epoch_ms: Some(1730000000000),
            stale: false,
        };
        let slug = wiki::slug_from_url(&request_url).unwrap_or_default();
        assert_ne!(
            slug,
            wiki::slugify(&canonical.title),
            "fixture must use titles whose slugs differ"
        );
        cache::store(cache_dir.path(), &slug, &canonical).expect("fresh-path store");
        let adapter = KbArchWikiAdapter::with_config(
            cache_dir.path().to_path_buf(),
            None,
            dead_endpoint_config(),
        );
        let stale = adapter
            .page(KbPageRequest { url: request_url })
            .await
            .expect("the request slug must find the cached copy offline");
        assert!(stale.stale, "offline copy must be flagged stale");
        assert_eq!(stale.title, "pm-utils");
        assert_eq!(stale.text, "canonical wikitext body");
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
        let slug = wiki::slug_from_url(&known_url).unwrap_or_default();
        cache::store(
            cache_dir.path(),
            &slug,
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
