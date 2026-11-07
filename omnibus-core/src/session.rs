//! Session = connection (specs/architecture/topology-and-lifecycle.md): the session
//! lives and dies with the instance process; the id exists only for audit
//! correlation; the core mints every opaque token.

use omnibus_contracts::dto::caps::SessionInfo;
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::tokens::{ElementId, MintKey, SessionId};

use crate::limits::limits_document;

/// Upper bound on live element tokens per session. Every `desktop.tree`
/// delivery mints fresh tokens per node by design; minting past the bound
/// starts a new generation — previously minted tokens resolve to `ElementStale`
/// (the contract's modeled staleness — re-read the tree). This keeps the map
/// bounded for a long-lived polling session and makes same-session staleness
/// actually reachable instead of dead code. A delivered tree at or above the
/// bound swaps the generation *before* minting (`tools::mint_tree_tokens`), so
/// the first `ELEMENT_TOKEN_MAP_CAP` nodes of one delivery stay mutually
/// resolvable and nodes beyond it address as stale.
pub(crate) const ELEMENT_TOKEN_MAP_CAP: usize = 8192;

/// The per-session context owned by the core.
#[derive(Debug)]
pub struct SessionContext {
    pub id: SessionId,
    /// Mint key for element tokens; never leaves the core. The audit
    /// `params_hash` key is deliberately *not* derived here: it is per-install,
    /// owned by `crate::audit` (`load_or_create_params_key`, audit.md).
    mint: MintKey,
    pub actor: omnibus_contracts::dto::audit::AuditActor,
    started_epoch_ms: i64,
    token_seq: std::sync::atomic::AtomicU64,
    tokens: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl SessionContext {
    /// Creates a session from entropy the composition root generated securely.
    /// All-zero entropy is rejected with `ContractError::Unsupported` — the
    /// [`MintKey::try_from_entropy`] semantics: an all-zero key is the
    /// predictable degenerate key and is never installed silently.
    pub fn try_new(
        entropy: [u8; 32],
        actor: omnibus_contracts::dto::audit::AuditActor,
    ) -> Result<Self, ContractError> {
        let mint = MintKey::try_from_entropy(entropy)?;
        let id = SessionId::mint(&mint, "session", 0);
        Ok(Self {
            id,
            mint,
            actor,
            started_epoch_ms: omnibus_contracts::dto::Milliseconds::now_epoch().0,
            token_seq: std::sync::atomic::AtomicU64::new(1),
            tokens: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Mints an element token for an adapter-owned node reference (only the core
    /// mints; adapters hand over their opaque refs, CoA). Minting past the map
    /// bound starts a new token generation: earlier tokens resolve to
    /// `ElementStale` from then on.
    pub fn mint_element(&self, node_ref: String) -> ElementId {
        let seq = self
            .token_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let token = ElementId::mint(&self.mint, self.id.as_str(), seq);
        // The map is a plain HashMap: a foreign panic cannot have corrupted it,
        // so a poisoned lock is recovered — a silently skipped insert would
        // mint a token that can never resolve.
        let mut map = self
            .tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if map.len() >= ELEMENT_TOKEN_MAP_CAP {
            map.clear();
        }
        map.insert(token.as_str().to_string(), node_ref);
        token
    }

    /// Starts a fresh token generation: every previously minted token becomes
    /// unresolvable and the map starts empty. A `desktop.tree` delivery whose
    /// own node count plus the map's current occupancy would cross
    /// [`ELEMENT_TOKEN_MAP_CAP`] calls this *before* minting, so minting the
    /// delivery's tokens never clears earlier tokens of the same response
    /// (the old mid-mint clear made one oversized tree self-invalidating, and
    /// re-reading it could never converge).
    pub(crate) fn swap_generation(&self) {
        self.tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// Current token-map occupancy: the tree mint adds the delivery's node
    /// count to it to detect a mid-mint cap crossing before the first mint.
    pub(crate) fn tokens_len(&self) -> usize {
        self.tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Resolves a token's wire form back to the adapter-owned node reference. A
    /// foreign, stale, or evicted token yields `None` (the facade maps it to
    /// `ElementStale`).
    pub fn resolve_element(&self, token: &str) -> Option<String> {
        self.tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(token)
            .cloned()
    }

    /// `session_info` tool result.
    pub fn info(&self) -> SessionInfo {
        SessionInfo {
            session_id: self.id.as_str().to_string(),
            started_epoch_ms: self.started_epoch_ms,
            pid: self.actor.pid,
            client_name: self.actor.client_name.clone(),
            limits: limits_document(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_contracts::dto::audit::AuditActor;

    fn ctx() -> SessionContext {
        let mut entropy = [0u8; 32];
        for (i, b) in entropy.iter_mut().enumerate() {
            *b = (i as u8) % 251 + 1;
        }
        SessionContext::try_new(
            entropy,
            AuditActor {
                uid: 1000,
                pid: 7,
                client_name: Some(String::from("test-client")),
            },
        )
        .expect("non-zero test entropy is a valid mint key")
    }

    #[test]
    fn tokens_mint_and_resolve_within_the_session() {
        let s = ctx();
        let token = s.mint_element(String::from("app#1/entry#1"));
        assert_eq!(
            s.resolve_element(token.as_str()).as_deref(),
            Some("app#1/entry#1")
        );
        assert_eq!(s.info().session_id, s.id.as_str());
    }

    #[test]
    fn foreign_token_does_not_resolve() {
        let s1 = ctx();
        let s2 = ctx();
        let foreign = s2.mint_element(String::from("other"));
        assert_eq!(s1.resolve_element(foreign.as_str()), None);
        // A token minted under a DIFFERENT key (not this instance's, not any
        // real instance's): same rejection half as a hand-forged string — the
        // integration-level forgery case is pinned by `forged_token_is_rejected`
        // (tests/session_lifecycle.rs).
        let foreign_key = MintKey::try_from_entropy([9u8; 32]).expect("non-zero test key");
        let forged = ElementId::mint(&foreign_key, "forged", 1);
        assert_eq!(s1.resolve_element(forged.as_str()), None);
    }

    #[test]
    fn element_tokens_are_distinct_per_mint() {
        let s = ctx();
        let a = s.mint_element(String::from("same"));
        let b = s.mint_element(String::from("same"));
        assert_ne!(a, b, "each delivery mints a fresh token");
        assert_eq!(s.resolve_element(a.as_str()).as_deref(), Some("same"));
        assert_eq!(s.resolve_element(b.as_str()).as_deref(), Some("same"));
    }

    /// The token map stays bounded: minting past the cap swaps the generation
    /// instead of growing without bound (a polling session cannot accumulate
    /// millions of entries); the newest tokens keep resolving.
    #[test]
    fn token_map_is_bounded_by_generation_swap() {
        let s = ctx();
        for i in 0..ELEMENT_TOKEN_MAP_CAP * 3 {
            s.mint_element(format!("node{i}"));
        }
        let len = s
            .tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        assert!(
            len <= ELEMENT_TOKEN_MAP_CAP,
            "the live token set stays bounded, got {len}"
        );
        let latest = s.mint_element(String::from("newest"));
        assert_eq!(
            s.resolve_element(latest.as_str()).as_deref(),
            Some("newest")
        );
    }

    /// A poisoned token-map mutex recovers: the guarded HashMap has no
    /// invariant a foreign panic could break, so the poison must never mint
    /// permanently unresolvable tokens. The panic unwinds *while the lock is
    /// held* — that is what poisons the mutex — so this test really exercises
    /// the `unwrap_or_else(PoisonError::into_inner)` recovery in
    /// [`SessionContext::mint_element`] and
    /// [`SessionContext::resolve_element`].
    #[test]
    fn poisoned_token_map_still_mints_resolvable_tokens() {
        let s = ctx();
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // The guard is alive when the panic fires: the unwind releases it
            // in the poisoned state (dropping it first would unlock cleanly
            // and leave nothing to recover from).
            let _guard = s
                .tokens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            panic!("poison the token map");
        }));
        assert!(
            poisoned.is_err(),
            "the panic happened while holding the lock"
        );
        let token = s.mint_element(String::from("after-poison"));
        assert_eq!(
            s.resolve_element(token.as_str()).as_deref(),
            Some("after-poison"),
            "mint -> resolve round-trips across a poisoned lock"
        );
    }

    /// All-zero entropy is a typed rejection, never a silently installed
    /// predictable mint key (`MintKey::try_from_entropy` semantics).
    #[test]
    fn all_zero_entropy_is_rejected() {
        let actor = omnibus_contracts::dto::audit::AuditActor {
            uid: 1000,
            pid: 7,
            client_name: Some(String::from("test-client")),
        };
        let rejected = SessionContext::try_new([0u8; 32], actor.clone());
        assert!(
            matches!(rejected, Err(ContractError::Unsupported { .. })),
            "an all-zero mint key must not install: got {rejected:?}"
        );
        assert!(SessionContext::try_new([1u8; 32], actor).is_ok());
    }

    /// One delivered tree is internally consistent for its first
    /// ELEMENT_TOKEN_MAP_CAP nodes: after the pre-mint generation swap, minting
    /// the whole batch leaves every token of the batch resolvable (the old
    /// mid-mint clear invalidated the same response's earlier tokens, and
    /// re-reading the tree could never converge).
    #[test]
    fn one_oversized_generation_stays_internally_consistent() {
        let s = ctx();
        s.swap_generation();
        let tokens: Vec<ElementId> = (0..ELEMENT_TOKEN_MAP_CAP)
            .map(|i| s.mint_element(format!("node{i}")))
            .collect();
        assert_eq!(tokens.len(), ELEMENT_TOKEN_MAP_CAP);
        for (i, token) in tokens.iter().enumerate() {
            let node = format!("node{i}");
            assert_eq!(
                s.resolve_element(token.as_str()).as_deref(),
                Some(node.as_str()),
                "every token of the same oversized delivery resolves"
            );
        }
    }

    /// A generation swap invalidates exactly the previous generation: tokens
    /// minted before the swap stop resolving, the new generation resolves.
    #[test]
    fn generation_swap_invalidates_only_previous_tokens() {
        let s = ctx();
        let old = s.mint_element(String::from("old"));
        s.swap_generation();
        assert_eq!(s.resolve_element(old.as_str()), None);
        let fresh = s.mint_element(String::from("new"));
        assert_eq!(s.resolve_element(fresh.as_str()).as_deref(), Some("new"));
    }
}
