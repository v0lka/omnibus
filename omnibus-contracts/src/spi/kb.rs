//! `KnowledgeBase` — platform knowledge search and page retrieval (subsystem 7).

use async_trait::async_trait;

use crate::dto::kb::{KbArticle, KbArticleRef, KbPageRequest, KbQuery};
use crate::errors::ContractError;

/// Knowledge base operations. The kb-archwiki provider serves ArchWiki with the
/// Manjaro overlay (roadmap task 2.5.2); kb-guides (other distros) is MS2 scope.
/// Offline mode degrades honestly: stale cache marked in the article (roadmap 1.5.6).
#[async_trait]
pub trait KnowledgeBase: Send + Sync {
    /// The adapter report (sources, cache state).
    fn report(&self) -> crate::dto::probe::AdapterReport;

    /// `wiki_search` — article references matching the query.
    async fn search(&self, query: KbQuery) -> Result<Vec<KbArticleRef>, ContractError>;

    /// `wiki_page` — one article with cache provenance.
    async fn page(&self, request: KbPageRequest) -> Result<KbArticle, ContractError>;
}
