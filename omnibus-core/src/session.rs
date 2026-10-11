//! Session = connection (specs/architecture/topology-and-lifecycle.md): the session
//! lives and dies with the instance process; the id exists only for audit
//! correlation; the core mints every opaque token.

use omnibus_contracts::dto::caps::SessionInfo;
use omnibus_contracts::tokens::{ElementId, MintKey, SessionId};

use crate::limits::limits_document;

/// The per-session context owned by the core.
#[derive(Debug)]
pub struct SessionContext {
    pub id: SessionId,
    /// Mint key for element tokens; never leaves the core.
    mint: MintKey,
    pub actor: omnibus_contracts::dto::audit::AuditActor,
    started_epoch_ms: i64,
    token_seq: std::sync::atomic::AtomicU64,
    tokens: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl SessionContext {
    /// Creates a session from entropy the composition root generated securely.
    pub fn new(entropy: [u8; 32], actor: omnibus_contracts::dto::audit::AuditActor) -> Self {
        let mint = MintKey::from_entropy(entropy);
        let id = SessionId::mint(&mint, "session", 0);
        Self {
            id,
            mint,
            actor,
            started_epoch_ms: omnibus_contracts::dto::Milliseconds::now_epoch().0,
            token_seq: std::sync::atomic::AtomicU64::new(1),
            tokens: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Mints an element token for an adapter-owned node reference (only the core
    /// mints; adapters hand over their opaque refs, CoA).
    pub fn mint_element(&self, node_ref: String) -> ElementId {
        let seq = self
            .token_seq
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let token = ElementId::mint(&self.mint, self.id.as_str(), seq);
        if let Ok(mut map) = self.tokens.lock() {
            map.insert(token.as_str().to_string(), node_ref);
        }
        token
    }

    /// Resolves a token's wire form back to the adapter-owned node reference. A
    /// foreign or stale token yields `None` (the facade maps it to `ElementStale`).
    pub fn resolve_element(&self, token: &str) -> Option<String> {
        self.tokens
            .lock()
            .ok()
            .and_then(|map| map.get(token).cloned())
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
        SessionContext::new(
            entropy,
            AuditActor {
                uid: 1000,
                pid: 7,
                client_name: Some(String::from("test-client")),
            },
        )
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
        let forged = ElementId::mint(&MintKey::from_entropy([9u8; 32]), "forged", 1);
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
}
