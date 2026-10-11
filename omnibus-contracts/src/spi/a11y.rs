//! `DesktopA11y` — a11y tree reading and typed writes (subsystem 1).

use async_trait::async_trait;

use crate::dto::desktop::{
    A11yElement, A11yTree, InvokeRequest, SetValueRequest, TreeSelector, WriteOutcome,
};
use crate::errors::ContractError;

/// Tree reading (`tree`, `element`, `read_attr`) and typed writes (`set_value`,
/// `invoke`). Address resolution belongs to the adapter (CoA); a vanished node is a
/// typed [`ContractError::ElementStale`]. Writes require the typed-write capability
/// plus a write grant — the grant check happens in the core, never here.
#[async_trait]
pub trait DesktopA11y: Send + Sync {
    /// The adapter report feeding the capabilities document (provider, versions).
    fn report(&self) -> crate::dto::probe::AdapterReport;

    /// `desktop.tree` — the tree of one application, addressed by role+name.
    async fn tree(&self, selector: TreeSelector) -> Result<A11yTree, ContractError>;

    /// `desktop.element` — flat detail of one node (attributes, states, actions).
    async fn element(&self, node_ref: String) -> Result<A11yElement, ContractError>;

    /// Reads one named attribute of a node.
    async fn read_attr(&self, node_ref: String, attribute: String)
    -> Result<String, ContractError>;

    /// Typed write: sets the value of an editable element.
    async fn set_value(&self, request: SetValueRequest) -> Result<WriteOutcome, ContractError>;

    /// Typed write: invokes a named action (default action when `None`).
    async fn invoke(&self, request: InvokeRequest) -> Result<WriteOutcome, ContractError>;
}
