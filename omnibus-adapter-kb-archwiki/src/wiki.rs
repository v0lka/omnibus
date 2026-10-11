//! ArchWiki MediaWiki API client: search and wikitext retrieval over `ureq`
//! (blocking), plus the URL/slug helpers the cache and the contract URLs need.
//! All blocking I/O runs inside `tokio::task::spawn_blocking` (the SPI is
//! async); native HTTP types never leave this module.

use std::time::Duration;

use omnibus_contracts::errors::ContractError;
use serde::Deserialize;

/// Default MediaWiki endpoint of the Arch wiki.
pub const DEFAULT_BASE_URL: &str = "https://wiki.archlinux.org";
/// Default overall request budget for one HTTP round trip.
pub const DEFAULT_TIMEOUT_MS: u64 = 5000;

/// Environment override for the base URL (tests point it at a dead port).
pub const ENV_BASE_URL: &str = "OMNIBUS_KB_BASE_URL";
/// Environment override for the per-request timeout, in milliseconds.
pub const ENV_TIMEOUT_MS: &str = "OMNIBUS_KB_TIMEOUT_MS";

/// Effective endpoint configuration of one adapter instance.
#[derive(Debug, Clone)]
pub struct KbConfig {
    /// MediaWiki endpoint base, e.g. the Arch wiki root.
    pub base_url: String,
    /// Overall HTTP timeout for one request.
    pub timeout: Duration,
}

impl KbConfig {
    /// Reads the overrides from the environment, falling back to the defaults.
    pub fn from_env() -> Self {
        let base_url = std::env::var(ENV_BASE_URL)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| String::from(DEFAULT_BASE_URL));
        let timeout_ms = std::env::var(ENV_TIMEOUT_MS)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .filter(|ms| *ms > 0)
            .unwrap_or(DEFAULT_TIMEOUT_MS);
        Self {
            base_url,
            timeout: Duration::from_millis(timeout_ms),
        }
    }
}

/// One `list=search` hit of the MediaWiki API.
#[derive(Debug, Deserialize)]
struct SearchHit {
    title: String,
    #[serde(default)]
    snippet: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    #[serde(default)]
    search: Vec<SearchHit>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    query: Option<SearchQuery>,
}

/// The `prop=wikitext` payload nests the content under the `"*"` key.
#[derive(Debug, Deserialize)]
struct WikitextBody {
    #[serde(rename = "*")]
    content: String,
}

#[derive(Debug, Deserialize)]
struct ParseData {
    #[serde(default)]
    title: Option<String>,
    wikitext: WikitextBody,
}

#[derive(Debug, Deserialize)]
struct ParseResponse {
    #[serde(default)]
    parse: Option<ParseData>,
    #[serde(default)]
    error: Option<ApiErrorBody>,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    info: Option<String>,
}

/// MediaWiki title of the article a contract URL points at (`/title/<name>`).
pub fn title_from_url(url: &str) -> Option<String> {
    let rest = url.split("/title/").nth(1)?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    if rest.is_empty() {
        return None;
    }
    let title = percent_decode(rest).replace('_', " ");
    if title.trim().is_empty() {
        None
    } else {
        Some(title)
    }
}

/// Cache key of an article URL: a filesystem-safe lowercase slug.
pub fn slug_from_url(url: &str) -> Option<String> {
    let title = title_from_url(url)?;
    Some(slugify(&title))
}

/// Filesystem-safe lowercase slug of an article title.
pub fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut last_dash = true; // trims leading dashes for free
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// Builds the contract URL of an article from its MediaWiki title.
pub fn article_url(base_url: &str, title: &str) -> String {
    let path = title.trim().replace(' ', "_");
    format!(
        "{}/title/{}",
        base_url.trim_end_matches('/'),
        percent_encode(&path)
    )
}

/// Search request (`action=query&list=search`), blocking; intended to run on a
/// blocking thread. Transport-level failures (offline, dead endpoint) are
/// adapter-unavailable; HTTP-level failures preserve the status as native.
pub fn search_blocking(
    config: &KbConfig,
    text: &str,
    limit: u32,
) -> Result<Vec<(String, String, String)>, ContractError> {
    // returns (title, url, snippet) triples in API order
    let agent = ureq::AgentBuilder::new().timeout(config.timeout).build();
    let response = agent
        .get(&format!(
            "{}/api.php",
            config.base_url.trim_end_matches('/')
        ))
        .query("action", "query")
        .query("list", "search")
        .query("srsearch", text)
        .query("srlimit", &limit.to_string())
        .query("format", "json")
        .query("utf8", "1")
        .call();
    let body = read_body(response)?;
    let parsed: SearchResponse =
        serde_json::from_str(&body).map_err(|e| ContractError::NativeFailure {
            code: 1,
            payload: format!("archwiki search response is not valid json: {e}"),
        })?;
    let hits = parsed.query.map(|q| q.search).unwrap_or_default();
    Ok(hits
        .into_iter()
        .map(|hit| {
            let url = article_url(&config.base_url, &hit.title);
            let snippet = hit.snippet.as_deref().map(strip_html).unwrap_or_default();
            (hit.title, url, snippet)
        })
        .collect())
}

/// Wikitext of one page (`action=parse&prop=wikitext`), blocking; intended to
/// run on a blocking thread.
pub fn wikitext_blocking(
    config: &KbConfig,
    title: &str,
) -> Result<(String, String), ContractError> {
    // returns (canonical title, wikitext)
    let agent = ureq::AgentBuilder::new().timeout(config.timeout).build();
    let response = agent
        .get(&format!(
            "{}/api.php",
            config.base_url.trim_end_matches('/')
        ))
        .query("action", "parse")
        .query("page", title)
        .query("prop", "wikitext")
        .query("format", "json")
        .query("utf8", "1")
        .call();
    let body = read_body(response)?;
    let parsed: ParseResponse =
        serde_json::from_str(&body).map_err(|e| ContractError::NativeFailure {
            code: 1,
            payload: format!("archwiki parse response is not valid json: {e}"),
        })?;
    if let Some(api_error) = parsed.error {
        return Err(ContractError::NativeFailure {
            code: 1,
            payload: format!(
                "archwiki api error {}: {}",
                api_error.code.unwrap_or_else(|| String::from("unknown")),
                api_error.info.unwrap_or_default()
            ),
        });
    }
    match parsed.parse {
        Some(data) => Ok((
            data.title.unwrap_or_else(|| title.to_string()),
            data.wikitext.content,
        )),
        None => Err(ContractError::NativeFailure {
            code: 1,
            payload: format!("archwiki returned no parse payload for page {title}"),
        }),
    }
}

/// Unwraps a ureq result into the response body string, mapping failures onto
/// the contract taxonomy (offline/dead endpoint vs HTTP status).
fn read_body(response: Result<ureq::Response, ureq::Error>) -> Result<String, ContractError> {
    match response {
        Ok(resp) => resp
            .into_string()
            .map_err(|e| ContractError::NativeFailure {
                code: 1,
                payload: format!("archwiki response body unreadable: {e}"),
            }),
        Err(ureq::Error::Status(code, resp)) => Err(ContractError::NativeFailure {
            code: i32::from(code),
            payload: format!("archwiki api returned http {code}: {}", resp.get_url()),
        }),
        // Transport-level failures (offline, refused, timed out) mean the
        // knowledge source is unreachable — the honest adapter-unavailable.
        Err(ureq::Error::Transport(_)) => Err(ContractError::AdapterUnavailable),
    }
}

/// Drops MediaWiki search-highlight tags from a snippet.
fn strip_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

/// Percent-encodes a URL path segment (MediaWiki title form).
pub fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Reverses [`percent_encode`]; malformed escapes pass through unchanged.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'%' && idx + 2 < bytes.len() {
            let hex = text
                .get(idx + 1..idx + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            match hex {
                Some(byte) => {
                    out.push(byte);
                    idx += 3;
                }
                None => {
                    out.push(bytes[idx]);
                    idx += 1;
                }
            }
        } else {
            out.push(bytes[idx]);
            idx += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_is_filesystem_safe_and_lowercase() {
        assert_eq!(slugify("Suspend and Hibernate"), "suspend-and-hibernate");
        assert_eq!(slugify("  --Weird!! Title--  "), "weird-title");
        assert_eq!(slugify("!!!"), "");
    }

    #[test]
    fn titles_and_slugs_roundtrip_through_urls() {
        let url = article_url(DEFAULT_BASE_URL, "Suspend and Hibernate");
        assert_eq!(
            url,
            "https://wiki.archlinux.org/title/Suspend_and_Hibernate"
        );
        assert_eq!(
            title_from_url(&url).as_deref(),
            Some("Suspend and Hibernate")
        );
        assert_eq!(
            slug_from_url(&url).as_deref(),
            Some("suspend-and-hibernate")
        );
        assert!(title_from_url("https://example.org/other").is_none());
        assert!(title_from_url("https://wiki.archlinux.org/title/").is_none());
    }

    #[test]
    fn percent_coding_roundtrips_titles() {
        let encoded = percent_encode("System time/Maintenance");
        assert_eq!(encoded, "System%20time%2FMaintenance");
        assert_eq!(percent_decode(&encoded), "System time/Maintenance");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn html_is_stripped_from_snippets() {
        assert_eq!(strip_html("a <span class=\"x\">b</span> c"), "a b c");
    }
}
