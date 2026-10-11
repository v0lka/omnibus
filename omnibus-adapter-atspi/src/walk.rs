//! Tree-walk budget accounting (pure; unit-tested without D-Bus).
//!
//! A full desktop tree is never required by a single `desktop.tree` call; the
//! walk is capped in depth and node count and reports honesty via the
//! `truncated` flag of the result. The tool layer still applies its own wall
//! clock on top.

/// Maximum tree depth measured from the application root (the root is depth 1).
pub const MAX_DEPTH: u32 = 12;

/// Maximum nodes materialized into the snapshot.
pub const MAX_NODES: usize = 2000;

/// Consumption ledger for one tree walk.
#[derive(Debug, Clone)]
pub struct WalkBudget {
    pub max_depth: u32,
    pub max_nodes: usize,
    /// Nodes materialized so far.
    pub nodes: usize,
    /// Set when any cap stopped the walk early.
    pub truncated: bool,
}

impl WalkBudget {
    pub fn new(max_depth: u32, max_nodes: usize) -> Self {
        Self {
            max_depth,
            max_nodes,
            nodes: 0,
            truncated: false,
        }
    }

    /// Whether children at `child_depth` may still be visited.
    pub fn may_descend(&self, child_depth: u32) -> bool {
        child_depth <= self.max_depth && self.nodes < self.max_nodes
    }

    /// Records one materialized node.
    pub fn record_node(&mut self) {
        self.nodes += 1;
    }

    /// Marks that the snapshot is incomplete (a cap or a dead subtree hit).
    pub fn mark_truncated(&mut self) {
        self.truncated = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_nodes_and_stays_under_the_cap() {
        let mut budget = WalkBudget::new(MAX_DEPTH, 3);
        for _ in 0..3 {
            budget.record_node();
        }
        assert!(!budget.may_descend(1));
        assert!(!budget.truncated, "cap not yet hit");
    }

    #[test]
    fn depth_cap_stops_descent_at_twelve() {
        let budget = WalkBudget::new(MAX_DEPTH, 2000);
        assert!(budget.may_descend(MAX_DEPTH));
        assert!(!budget.may_descend(MAX_DEPTH + 1));
    }

    #[test]
    fn truncation_flag_is_explicit() {
        let mut budget = WalkBudget::new(MAX_DEPTH, 2000);
        assert!(!budget.truncated);
        budget.mark_truncated();
        assert!(budget.truncated);
    }

    #[test]
    fn node_cap_stops_descent_exactly_at_the_limit() {
        let mut budget = WalkBudget::new(MAX_DEPTH, 2);
        budget.record_node();
        assert!(budget.may_descend(2));
        budget.record_node();
        assert!(!budget.may_descend(2));
    }
}
