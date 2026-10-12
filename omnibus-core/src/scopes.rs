//! Scope manifests — the single grant store (specs/contracts/config-and-grants.md).
//!
//! TOML files in `scopes.d` directories, sections `[write]` (gates every
//! `[+scope]` tool; parameterized grants carry their arguments, e.g. `paths`) and
//! `[optin]` (capabilities in status opt-in-required). Default is read-only: a
//! fresh install with no manifests exposes exactly the read-only tool set.
//!
//! Nothing in the daemon or the agent can write these files; `omnibusd grant`
//! (the CLI, run by the human) edits them — `authorize_write` refuses every target
//! under a loaded store directory, so a files write grant can never become grant
//! self-issuance. A malformed manifest never takes the daemon down and never
//! widens access: shape errors fail closed, the error surfaces in doctor
//! diagnostics, the remaining scopes work.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use omnibus_contracts::capabilities as cap;
use omnibus_contracts::errors::ContractError;

use crate::grants::{Grant, GrantProvenance, GrantRegistry};

/// One unusable manifest (or store directory), surfaced by doctor — never fatal
/// at probe time, and never a reason to widen access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    pub path: String,
    pub detail: String,
}

/// The parsed grant store of one fresh session.
#[derive(Debug, Clone, Default)]
pub struct ScopeManifests {
    /// Write grants: capability name → parameterized scope (paths). An empty vec
    /// is the unparameterized `{}` form, valid only for capabilities whose
    /// contract takes no path — `authorize_write` (fs_write) fails closed on it.
    pub write: BTreeMap<String, Vec<String>>,
    /// Opt-in capability names the human consented to.
    pub optin: Vec<String>,
    /// Manifest files (or directories) that failed to load; doctor reports them.
    pub errors: Vec<ManifestError>,
    /// The absolute directories the manifests were configured from. The grant
    /// store itself is never writable through the file tools.
    pub store_dirs: Vec<String>,
    /// The daemon-owned audit state directory (the file-fallback sink and the
    /// `params_hash` key live there). Never writable through the file tools —
    /// the audit trail is human-only, the same rule as `store_dirs` (audit.md).
    /// Set by the composition root from the actual fallback path; manifest-only
    /// uses (the grant CLI) leave it empty.
    pub state_dirs: Vec<String>,
    /// The default read scope: all of `$HOME`, dotfiles included (files.md,
    /// decision of 2026-10-10). Empty when `$HOME` is unset — nothing is
    /// readable core-side then; the adapter refuses too (defense in depth).
    pub read_roots: Vec<String>,
}

/// Component-boundary containment: `target` is inside `scope` when every path
/// component of `scope` prefixes `target`'s components. A plain string prefix
/// would authorize the sibling `/home/u/Projects-backup` under `/home/u/Projects`.
fn within_scope(target: &str, scope: &str) -> bool {
    !target.is_empty() && !scope.is_empty() && Path::new(target).starts_with(scope)
}

/// Root-side containment with symlinked-root support: the scope is resolved as
/// far as the filesystem allows first (the fs-xdg adapter's `resolved_root`
/// rule, `omnibus-adapter-fs-xdg/src/scope.rs`), because every checked target
/// arrives canonicalized — a grant written through a symlinked path
/// (`~/Projects` -> a data mount) would otherwise never match its own targets,
/// and a symlinked store or state root would slip the deny. The raw string is
/// checked too, so a root on an inaccessible mount keeps judging lexically
/// instead of matching nothing. `canonicalize` sits inside the core's OS
/// carve-out (layers-and-crates.md, the scopes.rs clause).
fn within_scope_resolved(target: &str, scope: &str) -> bool {
    if within_scope(target, scope) {
        return true;
    }
    match std::fs::canonicalize(scope) {
        Ok(resolved) => within_scope(target, &resolved.to_string_lossy()),
        Err(_) => false,
    }
}

/// The user grant store directory for the given environment values. An *empty*
/// `XDG_CONFIG_HOME` is unset (XDG spec): the store falls back to
/// `$HOME/.config/omnibus/scopes.d` — the same rule the grant CLI applies, so
/// daemon and CLI always resolve the same store.
fn user_store_dir(
    xdg: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    match xdg.filter(|value| !value.is_empty()) {
        Some(config) => Some(PathBuf::from(config).join("omnibus/scopes.d")),
        None => home
            .filter(|value| !value.is_empty())
            .map(|home| PathBuf::from(home).join(".config/omnibus/scopes.d")),
    }
}

/// The default read scope roots for the given `$HOME` value (files.md: reads
/// work inside `$HOME`, dotfiles included; everything beyond needs an entry).
fn default_read_roots(home: Option<&std::ffi::OsStr>) -> Vec<String> {
    home.filter(|value| !value.is_empty())
        .map(|home| vec![PathBuf::from(home).to_string_lossy().to_string()])
        .unwrap_or_default()
}

impl ScopeManifests {
    /// Reads every `*.toml` in the given directories (sorted for determinism).
    /// Only absolute directories are loadable: a CWD-relative store would make
    /// grant provenance depend on the daemon's launch directory. A directory
    /// that exists but cannot be read is recorded as an error so doctor can
    /// name it (the grants it holds would otherwise be silently absent); a
    /// missing directory is the normal fresh install and stays silent.
    pub fn load(dirs: &[PathBuf]) -> Self {
        let mut manifests = Self::default();
        for dir in dirs {
            if !dir.is_absolute() {
                manifests.errors.push(ManifestError {
                    path: dir.display().to_string(),
                    detail: String::from("grant store directory is not absolute; skipped"),
                });
                continue;
            }
            manifests.store_dirs.push(dir.display().to_string());
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) => {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        manifests.errors.push(ManifestError {
                            path: dir.display().to_string(),
                            detail: format!("grant store directory is unreadable: {e}"),
                        });
                    }
                    continue;
                }
            };
            let mut files: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                .collect();
            files.sort();
            for file in files {
                manifests.load_file(&file);
            }
        }
        manifests.read_roots = default_read_roots(std::env::var_os("HOME").as_deref());
        manifests
    }

    /// Default directories: the system store and the user store (development and
    /// test layouts; the packaged install uses /etc/omnibus/scopes.d).
    pub fn default_dirs() -> Vec<PathBuf> {
        let mut dirs = vec![PathBuf::from("/etc/omnibus/scopes.d")];
        if let Some(store) = user_store_dir(
            std::env::var_os("XDG_CONFIG_HOME").as_deref(),
            std::env::var_os("HOME").as_deref(),
        ) {
            dirs.push(store);
        }
        dirs
    }

    fn shape_error(&mut self, path: &Path, capability: &str, detail: &str) {
        self.errors.push(ManifestError {
            path: path.display().to_string(),
            detail: format!("[write].{capability}: {detail}; the entry is refused"),
        });
    }

    fn load_file(&mut self, path: &PathBuf) {
        let Ok(text) = std::fs::read_to_string(path) else {
            self.errors.push(ManifestError {
                path: path.display().to_string(),
                detail: String::from("unreadable file"),
            });
            return;
        };
        let value: toml::Table = match toml::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                self.errors.push(ManifestError {
                    path: path.display().to_string(),
                    detail: format!("parse error: {e}"),
                });
                return;
            }
        };
        if let Some(write) = value.get("write").and_then(|w| w.as_table()) {
            for (name, entry) in write {
                // The grant store must fail closed: an entry that is not a table,
                // or whose paths are not non-empty strings, is recorded and
                // refused — never silently promoted to a grant.
                let Some(entry) = entry.as_table() else {
                    self.shape_error(path, name, "entry is not a table");
                    continue;
                };
                let paths = match entry.get("paths") {
                    None => Vec::new(), // the `{}` unparameterized form
                    Some(value) => match value.as_array() {
                        Some(array) => {
                            let mut paths = Vec::new();
                            let mut well_formed = true;
                            for item in array {
                                match item.as_str() {
                                    Some(s) if !s.is_empty() && Path::new(s).is_absolute() => {
                                        paths.push(s.to_string())
                                    }
                                    // A relative or `~`-prefixed entry would
                                    // otherwise silently grant nothing (no
                                    // absolute canonical target ever matches
                                    // it). The grant CLI stores absolute paths
                                    // only; a hand-edited manifest gets the
                                    // same recorded, doctor-visible refusal as
                                    // any other shape error.
                                    Some(_) => {
                                        self.shape_error(
                                            path,
                                            name,
                                            "paths must be absolute (`~` is not expanded in manifests)",
                                        );
                                        well_formed = false;
                                        break;
                                    }
                                    _ => {
                                        self.shape_error(
                                            path,
                                            name,
                                            "paths must be non-empty strings",
                                        );
                                        well_formed = false;
                                        break;
                                    }
                                }
                            }
                            if !well_formed {
                                continue;
                            }
                            paths
                        }
                        None => {
                            self.shape_error(path, name, "paths must be an array of strings");
                            continue;
                        }
                    },
                };
                if name.as_str() == cap::FILES_SCOPE_WRITE && paths.is_empty() {
                    // fs_write is path-parameterized by contract: an entry
                    // without real paths must never widen to allow-all.
                    self.shape_error(
                        path,
                        name,
                        "files.scope.write is path-parameterized and requires a non-empty paths array",
                    );
                    continue;
                }
                self.write.insert(name.clone(), paths);
            }
        }
        if let Some(optin) = value.get("optin").and_then(|o| o.as_table()) {
            for name in optin.keys() {
                if !self.optin.contains(name) {
                    self.optin.push(name.clone());
                }
            }
        }
    }

    /// Joins the manifests into the grant registry (provenance `config`).
    pub fn into_grant_registry(self) -> (GrantRegistry, Self) {
        let mut registry = GrantRegistry::new();
        for name in self.write.keys() {
            registry.issue(Grant::new(name.clone(), GrantProvenance::Config));
        }
        for name in &self.optin {
            registry.issue(Grant::new(name.clone(), GrantProvenance::Config));
        }
        (registry, self)
    }

    /// True when `path` falls inside one of the parameterized scope's paths
    /// (canonical-prefix match at path-component boundaries, scope roots
    /// resolved as far as the filesystem allows; the adapter enforces the same
    /// rule defensively).
    pub fn path_allowed(&self, capability: &str, canonical: &str) -> bool {
        self.write
            .get(capability)
            .is_some_and(|paths| paths.iter().any(|p| within_scope_resolved(canonical, p)))
    }

    /// Resolves the raw fs_write path argument to the string the scope decision
    /// runs on: the canonical path when the target exists; otherwise the target
    /// anchored at its nearest existing ancestor (that ancestor canonicalized,
    /// the whole missing tail re-attached verbatim). `~`-prefixed and relative
    /// arguments anchor at `$HOME` — the same rule the adapter's
    /// `FileScopes::expand` applies — so the core check and the adapter decision
    /// judge the same path. `None` means the target cannot be verified in scope
    /// — an empty argument, a `$HOME`-relative path with no `$HOME`, or a chain
    /// where nothing up to the root canonicalizes — and the caller denies (a
    /// new-file write is never scope-checked on a raw, un-normalized string).
    ///
    /// The anchoring mirrors the fs-xdg adapter
    /// (`omnibus-adapter-fs-xdg/src/scope.rs` `resolve_links`): no symlink can
    /// hide in a nonexistent component, so judging the resolved ancestor plus
    /// the verbatim missing tail is exactly the chain the adapter later opens
    /// one `O_NOFOLLOW` step at a time — `fs_write` with `create_parents` stays
    /// checkable all the way into a not-yet-existing directory tree. One
    /// divergence class remains, fail-closed on both sides: `..` popping
    /// across a symlinked *existing* component — the core judges the
    /// kernel-resolved chain, the adapter its lexical normalization, so a
    /// target of that shape may be denied by one layer and (never written
    /// outside the chain the adapter verified) refused or allowed by the
    /// other; the adapter's walk always decides what actually lands.
    pub fn canonical_write_target(&self, raw: &str) -> Option<String> {
        self.write_target_at(raw, std::env::var_os("HOME").as_deref())
    }

    /// The `$HOME`-parameterized core of [`Self::canonical_write_target`]
    /// (tests pass an explicit `$HOME`; production reads the environment).
    fn write_target_at(&self, raw: &str, home: Option<&std::ffi::OsStr>) -> Option<String> {
        if raw.is_empty() {
            return None;
        }
        let expanded = expand_home(raw, home)?;
        if let Ok(canonical) = std::fs::canonicalize(&expanded) {
            return Some(canonical.to_string_lossy().to_string());
        }
        let normalized = lexical_absolute(&expanded.to_string_lossy())?;
        let mut probe = Path::new(&normalized).to_path_buf();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        loop {
            match std::fs::canonicalize(&probe) {
                Ok(base) => {
                    let mut anchored = base;
                    for name in tail.iter().rev() {
                        anchored.push(name);
                    }
                    return Some(anchored.to_string_lossy().to_string());
                }
                Err(_) => {
                    // `file_name` is `None` at the root: nothing canonicalizes
                    // anywhere up the chain, so the target cannot be verified
                    // and the caller denies (fail closed).
                    let name = probe.file_name()?.to_os_string();
                    probe.pop();
                    tail.push(name);
                }
            }
        }
    }

    /// The read-side variant: a missing target is not a scope question (the
    /// adapter answers with the honest native error), so the lexically
    /// normalized absolute path is enough to decide containment. `~`-prefixed
    /// and relative arguments anchor at `$HOME` like the adapter's
    /// `FileScopes::expand`; a `$HOME`-relative path with no `$HOME` fails
    /// closed (`None`).
    pub fn canonical_read_target(&self, raw: &str) -> Option<String> {
        self.read_target_at(raw, std::env::var_os("HOME").as_deref())
    }

    /// The `$HOME`-parameterized core of [`Self::canonical_read_target`].
    fn read_target_at(&self, raw: &str, home: Option<&std::ffi::OsStr>) -> Option<String> {
        if raw.is_empty() {
            return None;
        }
        let expanded = expand_home(raw, home)?;
        if let Ok(canonical) = std::fs::canonicalize(&expanded) {
            return Some(canonical.to_string_lossy().to_string());
        }
        lexical_absolute(&expanded.to_string_lossy())
    }

    /// Write-grant check for the `[+scope]` file tool: the capability must be in
    /// `[write]` with real paths (there is deliberately no allow-all form) and
    /// the target must sit inside them — but never inside the grant store or
    /// the audit state directory, whatever a generous grant says: both are
    /// human/daemon-only (a files grant covering `$HOME` must not become grant
    /// self-issuance or audit-trail tampering).
    pub fn authorize_write(
        &self,
        capability: &str,
        canonical_path: Option<&str>,
    ) -> Result<(), ContractError> {
        let Some(paths) = self.write.get(capability) else {
            return Err(ContractError::ScopeDenied);
        };
        // Fail closed: no target, or an unparameterized entry, is a denial.
        let Some(target) = canonical_path else {
            return Err(ContractError::ScopeDenied);
        };
        if !paths.iter().any(|p| within_scope_resolved(target, p)) {
            return Err(ContractError::ScopeDenied);
        }
        if self
            .store_dirs
            .iter()
            .chain(self.state_dirs.iter())
            .any(|dir| within_scope_resolved(target, dir))
        {
            return Err(ContractError::ScopeDenied);
        }
        Ok(())
    }

    /// Read-scope check for the files read tools: the canonical path must sit
    /// inside the default read scope (`$HOME`, dotfiles included) *or* inside
    /// any granted `files.scope.write` path — a write grant carries read access
    /// to its own scope (files.md; the adapter's `decide()` answers the same
    /// rule, and the core check matches so an adapter-allowed read is not
    /// refused core-side). The grant store and the audit state directory are
    /// excluded in both layers: they hold the per-install `params_hash` key
    /// and the audit trail itself, and the audit invariant (audit.md —
    /// low-entropy parameters cannot be recovered by enumeration) only holds
    /// when the agent cannot read the key out from under the digests.
    pub fn authorize_read(&self, canonical: &str) -> Result<(), ContractError> {
        if self
            .store_dirs
            .iter()
            .chain(self.state_dirs.iter())
            .any(|dir| within_scope_resolved(canonical, dir))
        {
            return Err(ContractError::ScopeDenied);
        }
        let in_read_roots = self
            .read_roots
            .iter()
            .any(|root| within_scope_resolved(canonical, root));
        let in_write_scope = self
            .write
            .get(cap::FILES_SCOPE_WRITE)
            .is_some_and(|paths| paths.iter().any(|p| within_scope_resolved(canonical, p)));
        if in_read_roots || in_write_scope {
            Ok(())
        } else {
            Err(ContractError::ScopeDenied)
        }
    }
}

/// `~` expansion and relative-path resolution against `$HOME` — mirrors the
/// adapter's `FileScopes::expand` (omnibus-adapter-fs-xdg/src/scope.rs) so the
/// core's canonical target and the adapter's decision always judge the same
/// path: `~/x` and relative paths anchor at `$HOME`, absolute paths pass
/// through, and a missing or empty `$HOME` yields `None` for every path that
/// needs it — the decision fails closed.
fn expand_home(raw: &str, home: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let trimmed = raw.trim();
    let home = home.filter(|value| !value.is_empty());
    if let Some(rest) = trimmed.strip_prefix("~/") {
        return home.map(|home| Path::new(home).join(rest));
    }
    if trimmed == "~" {
        return home.map(PathBuf::from);
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        home.map(|home| Path::new(home).join(path))
    }
}

/// Lexically normalizes to an absolute path: `.` components collapse, `..`
/// pops, symlinks are untouched (the caller canonicalizes what exists).
fn lexical_absolute(raw: &str) -> Option<String> {
    let path = Path::new(raw);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for component in absolute.components() {
        match component {
            Component::Normal(c) => parts.push(c.to_os_string()),
            Component::ParentDir => {
                parts.pop();
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    let mut resolved = PathBuf::from("/");
    for part in parts {
        resolved.push(part);
    }
    Some(resolved.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap_or_else(|e| panic!("write manifest: {e}"));
        path
    }

    #[test]
    fn default_install_is_read_only() {
        // No directories exist -> empty manifests -> exactly the read-only set.
        let manifests = ScopeManifests::load(&[PathBuf::from("/nonexistent/omnibus/scopes.d")]);
        assert!(manifests.write.is_empty());
        assert!(manifests.optin.is_empty());
        let (registry, _) = manifests.clone().into_grant_registry();
        assert_eq!(
            crate::grants::authorize(&registry, omnibus_contracts::capabilities::PKG_WRITE),
            Err(ContractError::ScopeDenied)
        );
    }

    #[test]
    fn write_and_optin_sections_become_grants() {
        use omnibus_contracts::capabilities::{FILES_SCOPE_WRITE, JOURNAL_PERSISTENT, PKG_WRITE};
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        write_manifest(
            dir.path(),
            "90-agent.toml",
            &format!(
                "[write]\n\"{PKG_WRITE}\" = {{}}\n\"{FILES_SCOPE_WRITE}\" = {{ paths = [\"/home/u/Projects\"] }}\n\n[optin]\n\"{JOURNAL_PERSISTENT}\" = {{}}\n"
            ),
        );
        let manifests = ScopeManifests::load(&[dir.path().to_path_buf()]);
        assert!(manifests.write.contains_key(PKG_WRITE));
        let granted = manifests.write.get(FILES_SCOPE_WRITE).map(Vec::as_slice);
        assert_eq!(
            granted,
            Some(vec![String::from("/home/u/Projects")].as_slice())
        );
        assert_eq!(manifests.optin, vec![JOURNAL_PERSISTENT.to_string()]);
        let (registry, _) = manifests.clone().into_grant_registry();
        assert_eq!(crate::grants::authorize(&registry, PKG_WRITE), Ok(()));
        assert!(registry.has_grant(JOURNAL_PERSISTENT));
    }

    #[test]
    fn parameterized_scope_enforces_paths() {
        use omnibus_contracts::capabilities::{FILES_SCOPE_WRITE, PKG_UNDO};
        let mut manifests = ScopeManifests::default();
        manifests.write.insert(
            FILES_SCOPE_WRITE.to_string(),
            vec![String::from("/home/u/Projects")],
        );
        assert_eq!(
            manifests.authorize_write(FILES_SCOPE_WRITE, Some("/home/u/Projects/x")),
            Ok(())
        );
        assert_eq!(
            manifests.authorize_write(FILES_SCOPE_WRITE, Some("/etc/passwd")),
            Err(ContractError::ScopeDenied)
        );
        assert_eq!(
            manifests.authorize_write(PKG_UNDO, None),
            Err(ContractError::ScopeDenied)
        );
    }

    /// A granted directory never authorizes its siblings: the prefix match is
    /// at path-component boundaries, not raw string prefixes.
    #[test]
    fn sibling_prefix_directory_is_not_authorized() {
        use omnibus_contracts::capabilities::FILES_SCOPE_WRITE;
        let mut manifests = ScopeManifests::default();
        manifests.write.insert(
            FILES_SCOPE_WRITE.to_string(),
            vec![String::from("/home/u/Projects")],
        );
        assert_eq!(
            manifests.authorize_write(FILES_SCOPE_WRITE, Some("/home/u/Projects-backup/x")),
            Err(ContractError::ScopeDenied),
            "a sibling whose name extends the granted path stays denied"
        );
        assert!(!manifests.path_allowed(FILES_SCOPE_WRITE, "/home/u/Projects-archive/deep/file"));
        // A trailing slash in the granted path changes nothing.
        manifests.write.insert(
            FILES_SCOPE_WRITE.to_string(),
            vec![String::from("/home/u/Projects/")],
        );
        assert_eq!(
            manifests.authorize_write(FILES_SCOPE_WRITE, Some("/home/u/Projects/x")),
            Ok(())
        );
        assert_eq!(
            manifests.authorize_write(FILES_SCOPE_WRITE, Some("/home/u/Projects2/x")),
            Err(ContractError::ScopeDenied)
        );
    }

    /// An empty `XDG_CONFIG_HOME` is unset: the user store resolves to
    /// `$HOME/.config/omnibus/scopes.d`, the same rule the grant CLI applies.
    #[test]
    fn empty_xdg_config_home_is_unset() {
        let resolve = |xdg: Option<&str>, home: Option<&str>| {
            user_store_dir(
                xdg.map(std::ffi::OsStr::new),
                home.map(std::ffi::OsStr::new),
            )
        };
        assert_eq!(
            resolve(Some(""), Some("/home/u")),
            Some(PathBuf::from("/home/u/.config/omnibus/scopes.d"))
        );
        assert_eq!(
            resolve(Some("/custom/xdg"), Some("/home/u")),
            Some(PathBuf::from("/custom/xdg/omnibus/scopes.d"))
        );
        assert_eq!(resolve(Some(""), None), None, "no HOME, no user store");
        assert_eq!(
            resolve(None, Some("/home/u")),
            Some(PathBuf::from("/home/u/.config/omnibus/scopes.d"))
        );
        // The default read scope follows the same empty-is-unset rule.
        assert_eq!(
            default_read_roots(Some(std::ffi::OsStr::new("/home/u"))),
            vec![String::from("/home/u")]
        );
        assert!(default_read_roots(Some(std::ffi::OsStr::new(""))).is_empty());
        assert!(default_read_roots(None).is_empty());
    }

    /// A relative store directory is refused (recorded, never loaded): grant
    /// provenance must not depend on the daemon's launch directory.
    #[test]
    fn relative_store_directory_is_refused() {
        let manifests = ScopeManifests::load(&[PathBuf::from("relative/scopes.d")]);
        assert_eq!(manifests.write.len(), 0);
        assert_eq!(manifests.store_dirs.len(), 0);
        assert_eq!(manifests.errors.len(), 1);
        assert!(manifests.errors[0].detail.contains("not absolute"));
    }

    /// Shape errors inside a parseable `[write]` section fail closed: scalar
    /// entries, missing/empty paths on the parameterized capability, and empty
    /// string paths are recorded and never become allow-all grants.
    #[test]
    fn malformed_write_entries_fail_closed() {
        use omnibus_contracts::capabilities::{FILES_SCOPE_WRITE, PKG_WRITE};
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        write_manifest(
            dir.path(),
            "10-shapes.toml",
            &format!(
                "[write]\n\"{PKG_WRITE}\" = true\n\"{FILES_SCOPE_WRITE}\" = 3\n[write.\"{FILES_SCOPE_WRITE}-dup\"]\npaths = [\"\", ]\n"
            ),
        );
        let manifests = ScopeManifests::load(&[dir.path().to_path_buf()]);
        assert_eq!(
            manifests.errors.len(),
            3,
            "scalar entry, scalar entry 2, empty-string path: all recorded"
        );
        assert!(
            !manifests.write.contains_key(FILES_SCOPE_WRITE),
            "no files write grant may arise from malformed shapes"
        );
        assert_eq!(
            manifests.authorize_write(FILES_SCOPE_WRITE, Some("/home/u/anything")),
            Err(ContractError::ScopeDenied),
            "a malformed parameterized capability never widens to allow-all"
        );
        // A missing-paths entry for the path-parameterized capability is refused.
        let dir2 = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        write_manifest(
            dir2.path(),
            "20-missing-paths.toml",
            &format!("[write]\n\"{FILES_SCOPE_WRITE}\" = {{}}\n"),
        );
        let manifests = ScopeManifests::load(&[dir2.path().to_path_buf()]);
        assert_eq!(manifests.errors.len(), 1);
        assert!(
            manifests.errors[0].detail.contains("path-parameterized"),
            "the error names the failed-closed rule"
        );
        assert!(!manifests.write.contains_key(FILES_SCOPE_WRITE));
        // The unparameterized `{}` form stays valid for pathless capabilities.
        let dir3 = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        write_manifest(
            dir3.path(),
            "30-pkg.toml",
            &format!("[write]\n\"{PKG_WRITE}\" = {{}}\n"),
        );
        let manifests = ScopeManifests::load(&[dir3.path().to_path_buf()]);
        assert!(manifests.errors.is_empty());
        assert!(manifests.write.contains_key(PKG_WRITE));
    }

    /// Hand-edited manifests with relative or `~`-prefixed scope paths are
    /// recorded and refused — the same strictness class as any other shape
    /// error (doctor names the file and entry), never a silent no-op grant.
    /// Absolute paths keep loading, and the healthy entries of the same file
    /// survive.
    #[test]
    fn non_absolute_scope_paths_are_recorded_and_refused() {
        use omnibus_contracts::capabilities::{FILES_SCOPE_WRITE, PKG_WRITE};
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        write_manifest(
            dir.path(),
            "10-hand-edited.toml",
            &format!(
                "[write]\n\"{PKG_WRITE}\" = {{}}\n[write.\"{FILES_SCOPE_WRITE}\"]\npaths = [\"Projects\", \"~/Notes\"]\n"
            ),
        );
        let manifests = ScopeManifests::load(&[dir.path().to_path_buf()]);
        assert_eq!(
            manifests.errors.len(),
            1,
            "one non-absolute entry, one recorded shape error"
        );
        assert!(
            manifests.errors[0].detail.contains("absolute"),
            "the error names the absolute-path rule: {}",
            manifests.errors[0].detail
        );
        assert!(
            !manifests.write.contains_key(FILES_SCOPE_WRITE),
            "a non-absolute scope is refused, never silently loaded"
        );
        assert!(
            manifests.write.contains_key(PKG_WRITE),
            "healthy entries of the same manifest keep working"
        );
        // The refused entry grants nothing: no relative prefix survives into
        // the scope decision.
        assert!(!manifests.path_allowed(
            FILES_SCOPE_WRITE,
            &format!("{}/Projects/x", dir.path().display())
        ));
    }

    /// The grant store itself is never writable through the file tools, even
    /// under a write grant that covers it: no grant self-issuance.
    #[test]
    fn grant_store_is_never_writable() {
        use omnibus_contracts::capabilities::FILES_SCOPE_WRITE;
        let store = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let scope = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let scope_root = scope.path().display().to_string();
        write_manifest(
            store.path(),
            "50-agent.toml",
            &format!(
                "[write]\n\"{FILES_SCOPE_WRITE}\" = {{ paths = [\"{scope_root}\", \"{}\"] }}\n",
                store.path().display()
            ),
        );
        let manifests = ScopeManifests::load(&[store.path().to_path_buf()]);
        let inside_store = format!("{}/scopes.d/90-self.toml", store.path().display());
        assert_eq!(
            manifests.authorize_write(FILES_SCOPE_WRITE, Some(&inside_store)),
            Err(ContractError::ScopeDenied),
            "a files grant on the store directory must not enable self-issuance"
        );
        let inside_scope = format!("{scope_root}/Projects/notes.txt");
        assert_eq!(
            manifests.authorize_write(FILES_SCOPE_WRITE, Some(&inside_scope)),
            Ok(()),
            "the ordinary granted scope keeps working"
        );
    }

    /// An existing-but-unreadable store directory is recorded as a manifest
    /// error so doctor can name it: the grants it holds are silently absent,
    /// the exact misconfiguration that must surface (unreadable files inside a
    /// readable store are recorded the same way). Skipped honestly when the
    /// environment cannot express an unreadable directory (root reads through
    /// every mode bit).
    #[test]
    fn unreadable_store_directory_is_recorded() {
        use omnibus_contracts::capabilities::JOURNAL_PERSISTENT;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let store = dir.path().join("scopes.d");
        std::fs::create_dir(&store).unwrap_or_else(|e| panic!("mkdir: {e}"));
        write_manifest(
            &store,
            "90-agent.toml",
            &format!("[optin]\n\"{JOURNAL_PERSISTENT}\" = {{}}\n"),
        );
        let lock = |mode: u32| {
            std::fs::set_permissions(&store, std::fs::Permissions::from_mode(mode))
                .unwrap_or_else(|e| panic!("chmod: {e}"))
        };
        lock(0o000);
        if std::fs::read_dir(&store).is_ok() {
            lock(0o755); // cannot express unreadability here (e.g. root): skip
            return;
        }
        let manifests = ScopeManifests::load(std::slice::from_ref(&store));
        lock(0o755); // let the tempdir drop clean up
        assert_eq!(
            manifests.errors.len(),
            1,
            "the unreadable store is recorded exactly once"
        );
        assert_eq!(manifests.errors[0].path, store.display().to_string());
        assert!(
            manifests.errors[0].detail.contains("unreadable"),
            "the detail names the problem: {}",
            manifests.errors[0].detail
        );
        assert!(
            manifests.optin.is_empty(),
            "the hidden grants stay absent, not half-loaded"
        );
    }

    /// Reads are confined to the default read scope (`$HOME`, dotfiles
    /// included); with no `$HOME` nothing is readable core-side.
    #[test]
    fn read_scope_is_home() {
        let manifests = ScopeManifests {
            read_roots: default_read_roots(Some(std::ffi::OsStr::new("/home/u"))),
            ..Default::default()
        };
        assert_eq!(manifests.authorize_read("/home/u/file.txt"), Ok(()));
        assert_eq!(manifests.authorize_read("/home/u/.ssh/id_ed25519"), Ok(()));
        assert_eq!(
            manifests.authorize_read("/home/other/file.txt"),
            Err(ContractError::ScopeDenied)
        );
        assert_eq!(
            manifests.authorize_read("/etc/passwd"),
            Err(ContractError::ScopeDenied)
        );
        let empty = ScopeManifests::default();
        assert_eq!(
            empty.authorize_read("/home/u/file.txt"),
            Err(ContractError::ScopeDenied),
            "no read scope configured: nothing is readable"
        );
    }

    /// A granted `files.scope.write` path carries read access to its own scope
    /// (files.md): a write grant outside `$HOME` lets fs_read/fs_list targets
    /// inside it pass the core check — the adapter's `decide()` grants exactly
    /// the same (`read_allowed = inside_home || in_write_scope`).
    #[test]
    fn write_grant_extends_the_read_scope() {
        use omnibus_contracts::capabilities::FILES_SCOPE_WRITE;
        let mut manifests = ScopeManifests {
            read_roots: default_read_roots(Some(std::ffi::OsStr::new("/home/u"))),
            ..Default::default()
        };
        manifests.write.insert(
            FILES_SCOPE_WRITE.to_string(),
            vec![String::from("/srv/projects")],
        );
        assert_eq!(
            manifests.authorize_read("/srv/projects/x/src/main.rs"),
            Ok(()),
            "a granted write scope is readable"
        );
        // The $HOME read root keeps working, and everything else stays denied.
        assert_eq!(manifests.authorize_read("/home/u/notes.txt"), Ok(()));
        assert_eq!(
            manifests.authorize_read("/srv/projects-backup/x"),
            Err(ContractError::ScopeDenied),
            "a sibling of the granted write scope stays denied"
        );
        assert_eq!(
            manifests.authorize_read("/etc/passwd"),
            Err(ContractError::ScopeDenied)
        );
        // Without the write grant, /srv/projects is unreachable again.
        manifests.write.remove(FILES_SCOPE_WRITE);
        assert_eq!(
            manifests.authorize_read("/srv/projects/x"),
            Err(ContractError::ScopeDenied)
        );
    }

    /// Path arguments resolve against `$HOME` exactly like the adapter's
    /// `FileScopes::expand`: `~/x` and relative paths anchor at `$HOME`,
    /// absolute paths pass through, and a missing/empty `$HOME` fails closed
    /// for every path that needs it.
    #[test]
    fn home_relative_arguments_anchor_at_home() {
        let manifests = ScopeManifests::default();
        let home = std::ffi::OsStr::new("/home/u");
        // `~` expansion (resolution needs no existing target on the read side).
        assert_eq!(
            manifests.read_target_at("~/notes", Some(home)),
            lexical_absolute("/home/u/notes"),
            "~/notes resolves under $HOME"
        );
        // A relative path anchors at $HOME (never at the daemon CWD).
        assert_eq!(
            manifests.read_target_at("notes/todo.txt", Some(home)),
            lexical_absolute("/home/u/notes/todo.txt"),
            "a relative path resolves under $HOME"
        );
        // Absolute paths pass through untouched (no $HOME anchor; a missing
        // target stays on its lexical normalization, symlinks resolve).
        assert_eq!(
            manifests.read_target_at("/nonexistent-omnibus-test/os-release", Some(home)),
            lexical_absolute("/nonexistent-omnibus-test/os-release")
        );
        // A real target under an explicit home canonicalizes (read and write).
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        std::fs::write(dir.path().join("notes"), b"data").unwrap_or_else(|e| panic!("write: {e}"));
        let expected = dir
            .path()
            .join("notes")
            .canonicalize()
            .unwrap_or_else(|e| panic!("canonicalize: {e}"))
            .to_string_lossy()
            .to_string();
        let home_dir = dir.path().as_os_str();
        assert_eq!(
            manifests.read_target_at("~/notes", Some(home_dir)),
            Some(expected.clone())
        );
        assert_eq!(
            manifests.write_target_at("~/notes", Some(home_dir)),
            Some(expected)
        );
        // Fail closed: no $HOME (or an empty one) for a home-relative path.
        assert_eq!(manifests.read_target_at("~/notes", None), None);
        assert_eq!(manifests.read_target_at("rel/notes", None), None);
        assert_eq!(
            manifests.read_target_at("~/notes", Some(std::ffi::OsStr::new(""))),
            None
        );
        assert_eq!(manifests.write_target_at("~/notes", None), None);
        // Absolute paths never needed $HOME.
        assert_eq!(
            manifests.read_target_at("/nonexistent-omnibus-test/os-release", None),
            lexical_absolute("/nonexistent-omnibus-test/os-release")
        );
    }

    /// A new-file write is scope-checked on a normalized path: `..` collapse
    /// lexically, and a missing parent chain anchors at its nearest existing
    /// ancestor (canonicalized, missing tail verbatim) so `create_parents`
    /// writes stay checkable into a not-yet-existing tree.
    #[test]
    fn canonical_write_target_normalizes_and_anchors_at_existing_ancestor() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        std::fs::create_dir(dir.path().join("Projects")).unwrap_or_else(|e| panic!("mkdir: {e}"));
        let manifests = ScopeManifests::default();

        // Existing target: the canonical path.
        let existing = dir.path().join("Projects");
        assert_eq!(
            manifests.canonical_write_target(existing.to_str().unwrap_or_default()),
            Some(
                existing
                    .canonicalize()
                    .unwrap_or_else(|e| panic!("canonicalize: {e}"))
                    .to_string_lossy()
                    .to_string()
            )
        );
        // New file in an existing directory: parent canonicalized, name kept.
        let new_file = dir.path().join("Projects").join("new.txt");
        let expected = dir
            .path()
            .join("Projects")
            .canonicalize()
            .unwrap_or_else(|e| panic!("canonicalize: {e}"))
            .join("new.txt");
        assert_eq!(
            manifests.canonical_write_target(new_file.to_str().unwrap_or_default()),
            Some(expected.to_string_lossy().to_string())
        );
        // Traversal collapses lexically before the parent check.
        let inner = dir.path().join("Projects");
        let traversal = format!("{}/../../..", inner.display());
        let resolved = manifests
            .canonical_write_target(&traversal)
            .unwrap_or_default();
        assert!(
            !resolved.contains(".."),
            "the scope decision never runs on an un-normalized string: {resolved}"
        );
        // A nonexistent parent chain anchors at the nearest existing ancestor:
        // the anchor is canonicalized, the missing tail stays verbatim (no
        // symlink can hide in a nonexistent component).
        let orphan = dir.path().join("missing").join("deeper").join("file.txt");
        let anchored = dir
            .path()
            .canonicalize()
            .unwrap_or_else(|e| panic!("canonicalize: {e}"))
            .join("missing")
            .join("deeper")
            .join("file.txt");
        assert_eq!(
            manifests.canonical_write_target(orphan.to_str().unwrap_or_default()),
            Some(anchored.to_string_lossy().to_string())
        );
        assert_eq!(manifests.canonical_write_target(""), None);
    }

    /// The anchor canonicalizes through a symlinked existing ancestor, exactly
    /// like the adapter's `resolve_links`: the judgment string is the resolved
    /// ancestor plus the verbatim missing tail, never a link re-attached
    /// lexically.
    #[test]
    fn canonical_write_target_anchor_resolves_symlinked_ancestor() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let real = dir.path().join("data");
        std::fs::create_dir_all(&real).unwrap_or_else(|e| panic!("mkdir: {e}"));
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap_or_else(|e| panic!("mkdir: {e}"));
        std::os::unix::fs::symlink(&real, home.join("link"))
            .unwrap_or_else(|e| panic!("symlink: {e}"));
        let manifests = ScopeManifests::default();
        let orphan = home.join("link").join("a").join("b").join("file.txt");
        let expected = real
            .canonicalize()
            .unwrap_or_else(|e| panic!("canonicalize: {e}"))
            .join("a")
            .join("b")
            .join("file.txt");
        assert_eq!(
            manifests.canonical_write_target(orphan.to_str().unwrap_or_default()),
            Some(expected.to_string_lossy().to_string())
        );
    }

    /// The audit state directory is never writable through the file tools,
    /// whatever a generous write grant says (audit.md; the wiring sets the
    /// actual fallback directory, the adapter denies the same dir as the
    /// second layer).
    #[test]
    fn audit_state_dir_is_never_writable_through_the_core() {
        use omnibus_contracts::capabilities::FILES_SCOPE_WRITE;
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let state_dir = dir.path().join("state").join("omnibus");
        std::fs::create_dir_all(&state_dir).unwrap_or_else(|e| panic!("mkdir: {e}"));
        let mut manifests = ScopeManifests::default();
        manifests.write.insert(
            FILES_SCOPE_WRITE.to_string(),
            vec![dir.path().to_string_lossy().to_string()],
        );
        manifests.state_dirs = vec![state_dir.to_string_lossy().to_string()];
        let inside = state_dir.join("audit.jsonl").to_string_lossy().to_string();
        assert_eq!(
            manifests.authorize_write(FILES_SCOPE_WRITE, Some(&inside)),
            Err(ContractError::ScopeDenied)
        );
        // The rest of the granted scope stays writable.
        let outside = dir.path().join("notes.txt").to_string_lossy().to_string();
        assert_eq!(
            manifests.authorize_write(FILES_SCOPE_WRITE, Some(&outside)),
            Ok(())
        );
    }

    /// The grant store and the audit state directory are never readable
    /// through the file read tools either: the state directory holds the
    /// per-install `params_hash` key and the audit trail, and the audit
    /// anti-enumeration invariant (audit.md) only holds when the agent cannot
    /// read the key out from under the digests. Mirrors the adapter's
    /// `decide()` deny (the second layer).
    #[test]
    fn grant_store_and_state_dir_are_never_readable_through_the_core() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let home = dir.path().join("home");
        let state_dir = home.join(".local/state/omnibus");
        let store_dir = home.join(".config/omnibus/scopes.d");
        std::fs::create_dir_all(&state_dir).unwrap_or_else(|e| panic!("mkdir: {e}"));
        std::fs::create_dir_all(&store_dir).unwrap_or_else(|e| panic!("mkdir: {e}"));
        let mut manifests = ScopeManifests {
            read_roots: default_read_roots(Some(std::ffi::OsStr::new(
                home.to_string_lossy().as_ref(),
            ))),
            ..Default::default()
        };
        manifests.state_dirs = vec![state_dir.to_string_lossy().to_string()];
        manifests.store_dirs = vec![store_dir.to_string_lossy().to_string()];
        // Both daemon directories stay unreadable even though $HOME is the
        // default read scope.
        for denied in [
            state_dir.join("params-hash.key"),
            state_dir.join("audit.jsonl"),
            store_dir.join("90-grants.toml"),
        ] {
            assert_eq!(
                manifests.authorize_read(&denied.to_string_lossy()),
                Err(ContractError::ScopeDenied),
                "{} must not be agent-readable",
                denied.display()
            );
        }
        // The rest of $HOME — including sibling daemon-adjacent paths — stays
        // readable.
        assert_eq!(
            manifests.authorize_read(&home.join("notes.txt").to_string_lossy()),
            Ok(())
        );
        assert_eq!(
            manifests.authorize_read(&home.join(".local/state/other/app.json").to_string_lossy()),
            Ok(())
        );
    }

    #[test]
    fn malformed_manifest_is_recorded_not_fatal() {
        use omnibus_contracts::capabilities::PKG_WRITE;
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        write_manifest(dir.path(), "10-bad.toml", "[write\nthis is not toml ==");
        write_manifest(
            dir.path(),
            "20-good.toml",
            &format!("[write]\n\"{PKG_WRITE}\" = {{}}\n"),
        );
        let manifests = ScopeManifests::load(&[dir.path().to_path_buf()]);
        assert_eq!(manifests.errors.len(), 1);
        assert!(manifests.errors[0].path.ends_with("10-bad.toml"));
        assert!(
            manifests.write.contains_key(PKG_WRITE),
            "healthy scopes keep working"
        );
    }
}
