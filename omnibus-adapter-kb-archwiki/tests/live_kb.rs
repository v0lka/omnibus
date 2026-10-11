//! Live binding of the KnowledgeBase contract suite against the real ArchWiki
//! endpoint. The test probes reachability first and skips gracefully when the
//! network is down — offline behavior is covered by the offline unit suite.
//!
//! Hardening against transient blips: every live network leg runs through a
//! binder-side wrapper that retries the transport-class errors
//! (`AdapterUnavailable`, `TimeoutError`, HTTP 5xx `NativeFailure`) with a
//! bounded backoff. Consent-free by nature, this never masks a real contract
//! failure: after the retries are exhausted the flap-guard re-probes the
//! endpoint and downgrades to an honest skip only when the endpoint truly
//! went away mid-test; an answerable endpoint plus a red report stays red.

use async_trait::async_trait;
use omnibus_adapter_kb_archwiki::KbArchWikiAdapter;
use omnibus_contracts::dto::kb::{KbArticle, KbArticleRef, KbPageRequest, KbQuery};
use omnibus_contracts::dto::probe::AdapterReport;
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::kb::KnowledgeBase;
use omnibus_contracts::suite;

/// Total attempts per live call (one initial + three retries).
const ATTEMPTS: u32 = 4;
/// Base backoff between attempts; attempt n sleeps BASE * 2^n millis.
const BACKOFF_BASE_MS: u64 = 400;

/// True when the Arch wiki endpoint answers within a small budget. Three
/// probes: a single HTTP round trip is not a reliable reachability oracle
/// (transient DNS/TLS hiccups would turn a healthy machine into a red run).
fn archwiki_reachable() -> bool {
    for _ in 0..3 {
        let ok = ureq::get("https://wiki.archlinux.org/api.php")
            .timeout(std::time::Duration::from_secs(3))
            .query("action", "query")
            .query("meta", "siteinfo")
            .query("format", "json")
            .call()
            .is_ok();
        if ok {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(750));
    }
    false
}

/// The transport-error classes a live round trip may honestly hit on an
/// otherwise healthy network. Anything else (bad URL, 4xx, cache failure)
/// is a real defect and must fail through untouched.
fn transient(error: &ContractError) -> bool {
    match error {
        ContractError::AdapterUnavailable | ContractError::TimeoutError => true,
        ContractError::NativeFailure { code, .. } => *code >= 500,
        _ => false,
    }
}

/// Binder-side wrapper: delegates every call to the real adapter, retrying
/// only the transient classes with a bounded backoff.
#[derive(Debug, Clone)]
struct LiveKb(KbArchWikiAdapter);

/// Bounded retry of one live call over the transport-error classes only;
/// anything else fails through on the first attempt.
async fn retry<T, F, Fut>(what: &str, mut op: F) -> Result<T, ContractError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ContractError>> + Send,
{
    let mut last = ContractError::AdapterUnavailable;
    for attempt in 0..ATTEMPTS {
        match op().await {
            Ok(value) => return Ok(value),
            Err(e) if transient(&e) => {
                last = e;
                tokio::time::sleep(std::time::Duration::from_millis(BACKOFF_BASE_MS << attempt))
                    .await;
            }
            Err(e) => return Err(e),
        }
    }
    eprintln!("live {what}: {ATTEMPTS} attempts exhausted on {last}");
    Err(last)
}

/// The honest flap-guard: `error` survived the retries, so decide between
/// "network died mid-test" (skip) and a genuine failure (panic).
fn fail_or_skip(what: &str, error: ContractError) -> ! {
    if !archwiki_reachable() {
        panic!("skipped: the archwiki endpoint went away mid-test during {what} ({error})");
    }
    panic!("live {what} kept failing while the endpoint answers: {error}");
}

#[async_trait]
impl KnowledgeBase for LiveKb {
    fn report(&self) -> AdapterReport {
        self.0.report()
    }

    async fn search(&self, query: KbQuery) -> Result<Vec<KbArticleRef>, ContractError> {
        retry("search", || self.0.search(query.clone())).await
    }

    async fn page(&self, request: KbPageRequest) -> Result<KbArticle, ContractError> {
        retry("page", || self.0.page(request.clone())).await
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("test runtime must build")
}

#[test]
fn live_search_and_page_roundtrip_against_archwiki() {
    if !archwiki_reachable() {
        eprintln!("skipped: the archwiki endpoint is not reachable from this machine");
        return;
    }
    let rt = runtime();
    rt.block_on(async move {
        let cache_dir = tempfile::tempdir().expect("tempdir");
        let kb = LiveKb(KbArchWikiAdapter::new(cache_dir.path().to_path_buf(), None));
        let refs = match kb
            .search(KbQuery {
                text: String::from("suspend"),
                limit: Some(5),
            })
            .await
        {
            Ok(refs) => refs,
            Err(e) => fail_or_skip("search", e),
        };
        assert!(
            !refs.is_empty(),
            "the suspend query must match the wiki corpus"
        );
        let wiki_ref = refs
            .iter()
            .find(|r| r.source == "archwiki")
            .expect("archwiki hits must name their source");
        assert!(!wiki_ref.url.is_empty());
        let article = match kb
            .page(KbPageRequest {
                url: wiki_ref.url.clone(),
            })
            .await
        {
            Ok(article) => article,
            Err(e) => fail_or_skip("page", e),
        };
        assert!(!article.stale, "a fresh fetch is never stale");
        assert!(!article.text.is_empty(), "the article carries its wikitext");
        // The fetch must have persisted the article into the cache dir.
        let entries = std::fs::read_dir(cache_dir.path())
            .expect("cache dir readable")
            .count();
        assert!(entries >= 1, "fetched articles land in the cache dir");
    });
}

#[test]
fn kb_suite_exercises_the_live_endpoint() {
    if !archwiki_reachable() {
        eprintln!("skipped: the archwiki endpoint is not reachable from this machine");
        return;
    }
    let rt = runtime();
    rt.block_on(async move {
        let cache_dir = tempfile::tempdir().expect("tempdir");
        let kb = LiveKb(KbArchWikiAdapter::new(cache_dir.path().to_path_buf(), None));
        // Discover a known page for the suite's page() leg.
        let refs = match kb
            .search(KbQuery {
                text: String::from("suspend"),
                limit: Some(5),
            })
            .await
        {
            Ok(refs) => refs,
            Err(e) => fail_or_skip("search", e),
        };
        let known_url = refs
            .iter()
            .find(|r| r.source == "archwiki")
            .map(|r| r.url.clone())
            .expect("at least one archwiki hit for the suspend query");
        let report = suite::kb::kb_suite(
            move || Box::new(kb.clone()) as Box<dyn KnowledgeBase>,
            suite::kb::KbFixture {
                query_text: String::from("suspend"),
                expected_title_fragment: String::from("suspend"),
                known_url,
                offline: false,
            },
        )
        .await;
        if !report.is_clean() && !archwiki_reachable() {
            eprintln!("skipped: the archwiki endpoint went away mid-suite ({report:?})");
            return;
        }
        assert!(report.is_clean(), "live kb suite red: {report:?}");
    });
}
