//! Negative fixtures for the four R4 rules (roadmap 1.1.3 AC):
//! every rule is proven by a violation fixture (red) and its fixed counterpart
//! inside `good` (green).
//!
//! Each fixture directory is a miniature workspace the lints scan as text; the
//! fixtures intentionally do not compile as part of this crate — cargo only sees
//! this test target, never the fixture trees.

use std::path::{Path, PathBuf};

use omnibus_lints::{
    CAPABILITY_DOMAINS, Finding, RULE_CON, RULE_COP, RULE_EDGES, RULE_NATIVE, lint_con, lint_cop,
    lint_native, run_all,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn only_rule(findings: &[Finding], rule: &'static str) -> bool {
    !findings.is_empty() && findings.iter().all(|f| f.rule == rule)
}

#[test]
fn good_tree_is_green_for_every_r4_rule() {
    let findings = run_all(&fixture("good"));
    assert!(
        findings.is_empty(),
        "the fixed tree must be green for all four rules, got: {findings:?}"
    );
}

#[test]
fn cop_tuple_signature_is_red() {
    let findings = run_all(&fixture("bad_cop"));
    assert!(
        only_rule(&findings, RULE_COP),
        "expected exactly R4-COP findings, got: {findings:?}"
    );
    // The fixed counterpart (named structs) is covered by good_tree_is_green.
}

#[test]
fn cop_finds_both_parameter_and_return_tuples() {
    let findings = lint_cop(&fixture("bad_cop"));
    assert_eq!(
        findings.len(),
        2,
        "parameter and return tuple both flagged: {findings:?}"
    );
}

#[test]
fn native_dependency_in_contracts_is_red() {
    let findings = run_all(&fixture("bad_native"));
    assert!(
        only_rule(&findings, RULE_NATIVE),
        "expected exactly R4-NATIVE findings, got: {findings:?}"
    );
}

#[test]
fn native_flags_manifest_and_source_separately() {
    let findings = lint_native(&fixture("bad_native"));
    assert_eq!(
        findings.len(),
        2,
        "manifest dep and use-statement both flagged: {findings:?}"
    );
}

#[test]
fn con_capability_literal_outside_registry_is_red() {
    let findings = run_all(&fixture("bad_con"));
    assert!(
        only_rule(&findings, RULE_CON),
        "expected exactly R4-CON findings, got: {findings:?}"
    );
}

#[test]
fn con_allows_the_literal_inside_the_registry() {
    // The good tree carries the literal in omnibus-contracts/src/capabilities.rs.
    let findings = lint_con(&fixture("good"));
    assert!(
        findings.is_empty(),
        "registry literals must stay legal: {findings:?}"
    );
}

#[test]
fn con_scans_the_composition_root() {
    // bad_con plants "files.scope.write" in omnibusd/src: the scan list must
    // cover the daemon crate, not only contracts/core/probe/lints/adapters.
    let findings = lint_con(&fixture("bad_con"));
    assert!(
        findings.iter().any(|f| f.path.contains("omnibusd/src")),
        "omnibusd sources must be scanned for capability literals: {findings:?}"
    );
}

#[test]
fn forbidden_dependency_edges_are_red() {
    let findings = run_all(&fixture("bad_edges"));
    assert!(
        only_rule(&findings, RULE_EDGES),
        "expected exactly R4-EDGES findings, got: {findings:?}"
    );
    // adapter -> core, adapter -> adapter and composition root -> lint tool
    // are the forbidden edges here.
    let messages: Vec<&str> = findings.iter().map(|f| f.message.as_str()).collect();
    assert!(messages.iter().any(|m| m.contains("omnibus-core")));
    assert!(messages.iter().any(|m| m.contains("omnibus-adapter-b")));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("composition root") && m.contains("omnibus-lints")),
        "the composition root must not link the lint tool: {findings:?}"
    );
}

#[test]
fn con_domain_list_covers_every_registry_domain() {
    // Drift guard: the R4-CON predicate fixes the first segment of a
    // capability-shaped literal to a CAPABILITY_DOMAINS entry, so every domain in
    // the registry must be on that list — a new capability domain added to
    // omnibus-contracts/src/capabilities.rs without extending the lint fails here.
    let registry =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../omnibus-contracts/src/capabilities.rs");
    let text = std::fs::read_to_string(&registry)
        .unwrap_or_else(|e| panic!("cannot read the capability registry: {e}"));
    let mut domains = std::collections::BTreeSet::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("pub const ") else {
            continue;
        };
        // `pub const NAME: &str = "value";` — `ALL` (an array) does not match.
        let Some((_name, tail)) = rest.split_once(": &str = \"") else {
            continue;
        };
        let Some(value) = tail.split('"').next() else {
            continue;
        };
        // The CAPABILITY_NAME_FORMAT template (`{domain}.{feature}...`) is a doc
        // placeholder, not a capability name; real names never contain braces.
        if value.contains('{') {
            continue;
        }
        let Some(domain) = value.split('.').next() else {
            continue;
        };
        domains.insert(domain.to_string());
    }
    assert!(
        !domains.is_empty(),
        "the registry scan must extract capability constants, got none"
    );
    for domain in &domains {
        assert!(
            CAPABILITY_DOMAINS.contains(&domain.as_str()),
            "capability domain `{domain}` exists in the registry but is missing \
             from omnibus_lints::CAPABILITY_DOMAINS"
        );
    }
}
