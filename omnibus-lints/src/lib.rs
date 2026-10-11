//! `omnibus-lints` — the four R4 connascence/boundary rules as code
//! (roadmap 1.1.3; specs/architecture/quality-gates.md R4; architecture §8.8.2).
//!
//! Rules:
//! 1. `COP` — no tuple types in trait signatures inside `omnibus-contracts`
//!    (CoP excluded by style: named structs only).
//! 2. `NATIVE` — `omnibus-contracts` has zero OS dependencies: its manifest may
//!    depend only on the serde/async allowlist, and its source may only `use`
//!    the allowlist, `std`/`core`/`alloc`, or the crate itself.
//! 3. `CON` — capability name literals are allowed only in the registry file
//!    `omnibus-contracts/src/capabilities.rs` (CoN weakened to constants).
//! 4. `EDGES` — Cargo.toml dependency direction:
//!    contracts <- adapters/core/probe; adapters depend only on contracts;
//!    adapters never depend on each other or on the core; the lint crate
//!    depends on nothing internal.
//!
//! Each rule is proven by negative fixtures (violation red, fixed green) in
//! `tests/fixtures.rs`. The fixtures live in this crate rather than in
//! `omnibus-contracts/tests` (the roadmap's suggested location) so that the
//! contracts crate stays free of internal dev-dependencies — its invariant
//! allows only serde/async types.

use std::fs;
use std::path::{Path, PathBuf};

/// Rule identifiers as reported in [`Finding::rule`].
pub const RULE_COP: &str = "R4-COP";
pub const RULE_NATIVE: &str = "R4-NATIVE";
pub const RULE_CON: &str = "R4-CON";
pub const RULE_EDGES: &str = "R4-EDGES";

/// The single registry file where capability literals are legal (rule CON).
pub const CAPABILITY_REGISTRY_PATH: &str = "omnibus-contracts/src/capabilities.rs";

/// External dependencies the contracts crate may carry (serde/async types only,
/// roadmap 1.1.2 AC: "contracts does not depend on anything except serde/async types").
pub const CONTRACTS_ALLOWED_DEPS: [&str; 6] = [
    "serde",
    "serde_json",
    "async-trait",
    "tokio",
    "futures-core",
    "thiserror",
];

/// One lint violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule: &'static str,
    pub path: String,
    pub line: usize,
    pub message: String,
}

impl Finding {
    fn new(rule: &'static str, path: &Path, line: usize, message: String) -> Self {
        Self {
            rule,
            path: path.display().to_string(),
            line,
            message,
        }
    }
}

/// Runs all four rules against the repository rooted at `root`.
pub fn run_all(root: &Path) -> Vec<Finding> {
    let mut findings = Vec::new();
    findings.extend(lint_cop(root));
    findings.extend(lint_native(root));
    findings.extend(lint_con(root));
    findings.extend(lint_edges(root));
    findings
}

/// Collects `*.rs` files under `root`/`dir`, skipping `target/` directories.
fn rust_files(root: &Path, dir: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.join(dir)];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn read_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("lint: cannot read {}: {e}", path.display()))
        .lines()
        .map(str::to_string)
        .collect()
}

/// Rule 1 (R4-COP): within `trait` bodies in `omnibus-contracts/src`, tuple types in
/// signatures are forbidden (`: (T, U)` parameters or `-> (T, U)` returns).
///
/// Trait bodies in the contracts crate are declaration-only by DTO discipline, so a
/// conservative line scan inside trait brace ranges is sufficient; it flags tuple
/// types anywhere inside a trait body.
pub fn lint_cop(root: &Path) -> Vec<Finding> {
    let mut findings = Vec::new();
    for path in rust_files(root, "omnibus-contracts/src") {
        let lines = read_lines(&path);
        let mut depth_in_trait: Option<usize> = None;
        for (idx, line) in lines.iter().enumerate() {
            let lineno = idx + 1;
            let trimmed = line.trim();
            match depth_in_trait {
                None => {
                    // Enter a trait body on `trait Name ... {`.
                    if is_trait_declaration(trimmed) && trimmed.contains('{') {
                        let opens = line.matches('{').count();
                        let closes = line.matches('}').count();
                        if opens > closes {
                            depth_in_trait = Some(opens - closes);
                        }
                    }
                }
                Some(balance) => {
                    check_tuple_in_signature(&path, lineno, trimmed, &mut findings);
                    let opens = line.matches('{').count();
                    let closes = line.matches('}').count();
                    let balance = balance + opens;
                    if closes >= balance {
                        depth_in_trait = None;
                    } else {
                        depth_in_trait = Some(balance - closes);
                    }
                }
            }
        }
    }
    findings
}

fn is_trait_declaration(line: &str) -> bool {
    let without_pub = line.strip_prefix("pub ").unwrap_or(line);
    without_pub.starts_with("trait ") || without_pub.starts_with("unsafe trait ")
}

fn check_tuple_in_signature(path: &Path, lineno: usize, line: &str, findings: &mut Vec<Finding>) {
    for (marker, what) in [
        (": (", "tuple parameter type"),
        ("-> (", "tuple return type"),
    ] {
        if line.contains(marker) {
            findings.push(Finding::new(
                RULE_COP,
                path,
                lineno,
                format!("{what} in a trait signature (named structs only, §8.5 CoP)"),
            ));
            return; // one finding per line is enough
        }
    }
}

/// Rule 2 (R4-NATIVE): `omnibus-contracts` may depend only on the serde/async
/// allowlist — checked in the manifest and in every `use` statement of its sources.
pub fn lint_native(root: &Path) -> Vec<Finding> {
    let mut findings = Vec::new();

    let manifest = root.join("omnibus-contracts/Cargo.toml");
    for dep in manifest_dependency_names(&manifest) {
        if dep.starts_with("omnibus-") {
            findings.push(Finding::new(
                RULE_NATIVE,
                &manifest,
                0,
                format!("contracts must not depend on workspace crates (`{dep}`)"),
            ));
        } else if !CONTRACTS_ALLOWED_DEPS.contains(&dep.as_str()) {
            findings.push(Finding::new(
                RULE_NATIVE,
                &manifest,
                0,
                format!(
                    "contracts dependency `{dep}` is outside the serde/async allowlist (zero OS dependencies)"
                ),
            ));
        }
    }

    let module_names = declared_modules(root);
    for path in rust_files(root, "omnibus-contracts/src") {
        for (idx, line) in read_lines(&path).iter().enumerate() {
            let Some(crate_name) = external_use_crate(line) else {
                continue;
            };
            let normalized = crate_name.replace('-', "_");
            let allowed = normalized == "std"
                || normalized == "core"
                || normalized == "alloc"
                || normalized == "crate"
                || normalized == "self"
                || normalized == "super"
                || module_names.contains(&normalized)
                || CONTRACTS_ALLOWED_DEPS
                    .iter()
                    .any(|d| d.replace('-', "_") == normalized);
            if !allowed {
                findings.push(Finding::new(
                    RULE_NATIVE,
                    &path,
                    idx + 1,
                    format!("`use {crate_name}` leaks a native type into contracts (§8.5 CoT)"),
                ));
            }
        }
    }
    findings
}

/// In-tree module names of `omnibus-contracts/src` (plain `pub mod x;` / `mod x;`
/// declarations). A 2015-style relative `use families::…` inside the same tree is
/// internal, not a native leak; the text lint needs this set to tell them apart.
fn declared_modules(root: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for path in rust_files(root, "omnibus-contracts/src") {
        for line in read_lines(&path) {
            let trimmed = line.trim();
            let rest = trimmed
                .strip_prefix("pub mod ")
                .or_else(|| trimmed.strip_prefix("mod "));
            if let Some(rest) = rest
                && let Some(name) = rest.split([';', '{', ' ']).next()
                && !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                names.push(name.to_string());
            }
        }
    }
    names
}

/// Extracts the crate name from a `use` line, if it is a plain `use x::...` item
/// (not `use crate::`/`super::`/`self::` — those are handled by the caller).
fn external_use_crate(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let rest = trimmed
        .strip_prefix("pub use ")
        .or_else(|| trimmed.strip_prefix("use "))?;
    if rest.starts_with("crate::") || rest.starts_with("super::") || rest.starts_with("self::") {
        return None;
    }
    let head = rest.split("::").next()?;
    let head = head.split_whitespace().next()?;
    if head.is_empty() || head.contains('{') || head.contains('}') || head.contains(',') {
        return None; // grouped or unusual form; not a plain external use
    }
    Some(head.to_string())
}

/// Reads dependency names from all `[dependencies]`-style sections of a Cargo.toml.
/// The tool parses the manifest as text on purpose: zero dependencies for the linter.
pub fn manifest_dependency_names(manifest: &Path) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(text) = fs::read_to_string(manifest) else {
        return names;
    };
    let mut in_deps = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_deps = trimmed.starts_with("[dependencies]")
                || trimmed.starts_with("[dev-dependencies]")
                || trimmed.starts_with("[build-dependencies]")
                || trimmed.starts_with("[workspace.dependencies]");
            continue;
        }
        if in_deps {
            let Some((name, _)) = trimmed.split_once('=') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() || name.starts_with('#') {
                continue;
            }
            names.push(name.trim_end_matches(".workspace").to_string());
        }
    }
    names
}

/// Rule 3 (R4-CON): capability name literals may appear only in the registry file.
///
/// A capability literal is a double-quoted string of `{domain}.{feature}[.{qualifier}]`
/// shape: at least two dot-separated segments, every segment lowercase, and the
/// first segment a capability domain of the registry (desktop, journal, service,
/// pkg, portal, files, kb, net). Restricting the first segment to registry domains
/// keeps D-Bus names like `org.freedesktop.systemd1` out of scope while every
/// capability-shaped literal stays registry-only.
pub fn lint_con(root: &Path) -> Vec<Finding> {
    let mut findings = Vec::new();
    for dir in [
        "omnibus-contracts/src",
        "omnibus-core/src",
        "omnibus-probe/src",
        "omnibus-lints/src",
    ] {
        for path in rust_files(root, dir) {
            if path == root.join(CAPABILITY_REGISTRY_PATH) {
                continue; // the registry is the one legal home of the literals
            }
            scan_capability_literals(root, &path, &mut findings);
        }
    }
    // Every adapter crate directory.
    for entry in fs::read_dir(root).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        if name.starts_with("omnibus-adapter-") {
            for path in rust_files(root, &name) {
                scan_capability_literals(root, &path, &mut findings);
            }
        }
    }
    findings
}

fn scan_capability_literals(root: &Path, path: &Path, findings: &mut Vec<Finding>) {
    for (idx, line) in read_lines(path).iter().enumerate() {
        let mut rest = line.as_str();
        while let Some(open) = rest.find('"') {
            let after = &rest[open + 1..];
            let Some(close) = after.find('"') else { break };
            let inner = &after[..close];
            if looks_like_capability_name(inner) && path != root.join(CAPABILITY_REGISTRY_PATH) {
                findings.push(Finding::new(
                    RULE_CON,
                    path,
                    idx + 1,
                    format!(
                        "capability literal ({inner}) outside the registry; use the omnibus-contracts constant"
                    ),
                ));
            }
            rest = &after[close + 1..];
        }
    }
}

fn looks_like_capability_name(s: &str) -> bool {
    const DOMAINS: [&str; 8] = [
        "desktop", "journal", "service", "pkg", "portal", "files", "kb", "net",
    ];
    let segments: Vec<&str> = s.split('.').collect();
    segments.len() >= 2
        && segments.iter().all(|seg| {
            !seg.is_empty()
                && seg.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && seg
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        })
        && segments
            .first()
            .is_some_and(|first| DOMAINS.contains(first))
}

/// Rule 4 (R4-EDGES): the dependency direction over all workspace manifests.
pub fn lint_edges(root: &Path) -> Vec<Finding> {
    let mut findings = Vec::new();
    for member in workspace_members(root) {
        let manifest = root.join(&member).join("Cargo.toml");
        let deps = manifest_dependency_names(&manifest);
        let internal: Vec<&String> = deps.iter().filter(|d| d.starts_with("omnibus-")).collect();
        let violation = |message: String| Finding::new(RULE_EDGES, &manifest, 0, message);
        if member == "omnibus-contracts" {
            for dep in &internal {
                findings.push(violation(format!(
                    "contracts must not depend on workspace crates (`{dep}`)"
                )));
            }
        } else if member == "omnibus-lints" {
            for dep in &internal {
                findings.push(violation(format!(
                    "the lint tool must stay dependency-free (`{dep}`)"
                )));
            }
        } else if member == "omnibus-core" || member == "omnibus-probe" {
            for dep in &internal {
                if dep.as_str() != "omnibus-contracts" {
                    findings.push(violation(format!(
                        "`{member}` depends on `{dep}`: only omnibus-contracts is allowed (core never links adapters)"
                    )));
                }
            }
        } else if member.starts_with("omnibus-adapter-") {
            for dep in &internal {
                if dep.as_str() != "omnibus-contracts" {
                    findings.push(violation(format!(
                        "adapter depends on `{dep}`: adapters may depend only on omnibus-contracts (P6, no adapter-to-adapter, no core)"
                    )));
                }
            }
        }
    }
    findings
}

/// Reads the workspace `members` list from the root Cargo.toml.
fn workspace_members(root: &Path) -> Vec<String> {
    let manifest = root.join("Cargo.toml");
    let Ok(text) = fs::read_to_string(&manifest) else {
        return Vec::new();
    };
    let mut members = Vec::new();
    let mut in_members = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("members") && trimmed.contains('[') {
            in_members = true;
            if let Some(rest) = trimmed.split('[').nth(1) {
                collect_member(rest, &mut members);
            }
            if trimmed.contains(']') {
                in_members = false;
            }
            continue;
        }
        if in_members {
            if trimmed.contains(']') {
                collect_member(trimmed.split(']').next().unwrap_or(""), &mut members);
                in_members = false;
            } else {
                collect_member(trimmed, &mut members);
            }
        }
    }
    members.retain(|m| !m.is_empty());
    members
}

fn collect_member(fragment: &str, members: &mut Vec<String>) {
    for part in fragment.split(',') {
        // Member names are crate names: keep only name characters, dropping quotes,
        // brackets and whitespace in one pass.
        let name: String = part
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if !name.is_empty() {
            members.push(name);
        }
    }
}
