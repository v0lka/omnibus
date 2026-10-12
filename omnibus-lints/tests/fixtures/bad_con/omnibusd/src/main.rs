//! Violation: a capability literal in the composition root, outside the registry.
pub fn grant_hint() -> &'static str {
    "files.scope.write"
}
