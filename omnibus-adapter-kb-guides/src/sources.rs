//! The per-distro source registry (roadmap 2.5.2): the four Linux doc sources
//! the kb-guides provider serves, each a named entry with its public docs root.
//! A source's readable corpus is the `<guides root>/<source id>/` directory of
//! `.md` articles (the kb-archwiki Manjaro-overlay layout, one directory per
//! distro source). URL and slug helpers live here: every article address is a
//! plain distro-docs URL, resolved to its source plus a filesystem-safe slug
//! that doubles as the cache key — the same key the offline fallback looks up.

/// One registered distro doc source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceDef {
    /// Registry id; also the mirror directory name and the contract `source`
    /// label of the source's articles.
    pub id: &'static str,
    /// Public docs root every article URL of this source starts with.
    pub base_url: &'static str,
}

/// Debian wiki (MediaWiki pages under the wiki root).
pub const DEBIAN_WIKI: SourceDef = SourceDef {
    id: "debian-wiki",
    base_url: "https://wiki.debian.org",
};
/// Ubuntu documentation.
pub const UBUNTU_DOCS: SourceDef = SourceDef {
    id: "ubuntu-docs",
    base_url: "https://help.ubuntu.com",
};
/// Mint user guide / docs (forum topics are not cacheable articles and stay
/// out of the registry).
pub const MINT_DOCS: SourceDef = SourceDef {
    id: "mint-docs",
    base_url: "https://linuxmint-user-guide.readthedocs.io",
};
/// Fedora docs (quick docs and friends).
pub const FEDORA_DOCS: SourceDef = SourceDef {
    id: "fedora-docs",
    base_url: "https://docs.fedoraproject.org",
};

/// The registry, in roadmap order (Debian wiki, Ubuntu docs, Mint docs,
/// Fedora docs — specs/domains/knowledge.md provider table).
pub const REGISTRY: [SourceDef; 4] = [DEBIAN_WIKI, UBUNTU_DOCS, MINT_DOCS, FEDORA_DOCS];

/// One resolved article address: the source it belongs to and the slug that
/// keys both the mirror file and the cache entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedArticle {
    /// Registry id of the owning source.
    pub source_id: String,
    /// Filesystem-safe cache key derived from the URL path.
    pub slug: String,
}

/// Resolves a contract URL to its source and slug. The URL must name a
/// registered source (longest matching base wins); the remainder of the path
/// (percent-decoded, query and fragment stripped) flattens into one slug, so
/// nested doc paths key a single mirror file and cache entry.
pub fn resolve(url: &str) -> Option<ResolvedArticle> {
    let source = REGISTRY
        .iter()
        .filter(|s| url.starts_with(s.base_url))
        .max_by_key(|s| s.base_url.len())?;
    let rest = &url[source.base_url.len()..];
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let rest = percent_decode(rest.trim_start_matches('/'));
    if rest.trim().is_empty() {
        return None;
    }
    let slug = slugify(&rest.replace('/', " "));
    if slug.is_empty() {
        return None;
    }
    Some(ResolvedArticle {
        source_id: String::from(source.id),
        slug,
    })
}

/// Builds the contract URL of an article from its source and slug (the slug
/// is already URL-path-safe, and [`resolve`] maps it back to itself).
pub fn article_url(source: &SourceDef, slug: &str) -> String {
    format!("{}/{}", source.base_url.trim_end_matches('/'), slug)
}

/// Filesystem-safe lowercase slug of a URL path remainder or article name.
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

/// Human-facing title of a slug (mirror and overlay convention: dashes to
/// spaces; applicability is judged by the agent, not dressed up here).
pub fn humanize(slug: &str) -> String {
    slug.replace('-', " ")
}

/// Reverses percent-encoding for path remainders; malformed escapes pass
/// through unchanged.
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
    fn registry_holds_the_four_distro_sources() {
        let ids: Vec<&str> = REGISTRY.iter().map(|s| s.id).collect();
        assert_eq!(
            ids,
            vec!["debian-wiki", "ubuntu-docs", "mint-docs", "fedora-docs"]
        );
    }

    #[test]
    fn resolve_maps_urls_to_their_source_and_slug() {
        let resolved = resolve("https://wiki.debian.org/Suspend").expect("debian wiki url");
        assert_eq!(resolved.source_id, "debian-wiki");
        assert_eq!(resolved.slug, "suspend");

        let resolved = resolve(
            "https://docs.fedoraproject.org/en-US/quick-docs/Adding%20Users%20to%20sudoers",
        )
        .expect("fedora docs url");
        assert_eq!(resolved.source_id, "fedora-docs");
        assert_eq!(resolved.slug, "en-us-quick-docs-adding-users-to-sudoers");

        // Query strings and fragments never leak into the slug.
        let resolved = resolve("https://help.ubuntu.com/Suspend?highlight=sleep#after");
        assert_eq!(
            resolved,
            Some(ResolvedArticle {
                source_id: String::from("ubuntu-docs"),
                slug: String::from("suspend"),
            })
        );
    }

    #[test]
    fn foreign_and_empty_urls_do_not_resolve() {
        assert!(resolve("https://wiki.archlinux.org/title/Suspend").is_none());
        assert!(resolve("https://wiki.debian.org").is_none());
        assert!(resolve("https://wiki.debian.org/???").is_none());
    }

    #[test]
    fn article_urls_roundtrip_through_resolve() {
        for source in REGISTRY {
            let slug = slugify("Suspend and Hibernate/en-US quick docs");
            let url = article_url(&source, &slug);
            let resolved = resolve(&url).expect("roundtrip");
            assert_eq!(resolved.source_id, source.id);
            assert_eq!(resolved.slug, slug, "slugs are resolve-idempotent");
        }
    }

    #[test]
    fn slugify_is_filesystem_safe_and_lowercase() {
        assert_eq!(slugify("Suspend and Hibernate"), "suspend-and-hibernate");
        assert_eq!(slugify("  --Weird!! Title--  "), "weird-title");
        assert_eq!(slugify("!!!"), "");
    }

    #[test]
    fn humanize_turns_dashes_into_spaces() {
        assert_eq!(humanize("suspend-and-hibernate"), "suspend and hibernate");
    }

    #[test]
    fn percent_decoding_handles_escapes_and_malformed_input() {
        assert_eq!(
            percent_decode("Suspend%20and%20Hibernate"),
            "Suspend and Hibernate"
        );
        assert_eq!(percent_decode("100%"), "100%");
    }
}
