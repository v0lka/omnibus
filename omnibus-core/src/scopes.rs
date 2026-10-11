//! Scope manifests — the single grant store (specs/contracts/config-and-grants.md).
//!
//! TOML files in `scopes.d` directories, sections `[write]` (gates every
//! `[+scope]` tool; parameterized grants carry their arguments, e.g. `paths`) and
//! `[optin]` (capabilities in status opt-in-required). Default is read-only: a
//! fresh install with no manifests exposes exactly the read-only tool set.
//!
//! Nothing in the daemon or the agent can write these files; `omnibus grant` (the
//! CLI, run by the human) edits them. A malformed manifest never takes the daemon
//! down: the error surfaces in doctor diagnostics, the remaining scopes work.

use std::collections::BTreeMap;
use std::path::PathBuf;

use omnibus_contracts::errors::ContractError;

use crate::grants::{Grant, GrantProvenance, GrantRegistry};

/// One unparsable manifest, surfaced by doctor (never fatal at probe time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    pub path: String,
    pub detail: String,
}

/// The parsed grant store of one fresh session.
#[derive(Debug, Clone, Default)]
pub struct ScopeManifests {
    /// Write grants: capability name → parameterized scope (paths), empty vec for
    /// the unparameterized `{}` form.
    pub write: BTreeMap<String, Vec<String>>,
    /// Opt-in capability names the human consented to.
    pub optin: Vec<String>,
    /// Manifest files that failed to parse; doctor reports them.
    pub errors: Vec<ManifestError>,
}

impl ScopeManifests {
    /// Reads every `*.toml` in the given directories (sorted for determinism).
    pub fn load(dirs: &[PathBuf]) -> Self {
        let mut manifests = Self::default();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
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
        manifests
    }

    /// Default directories: the system store and the user store (development and
    /// test layouts; the packaged install uses /etc/omnibus/scopes.d).
    pub fn default_dirs() -> Vec<PathBuf> {
        let mut dirs = vec![PathBuf::from("/etc/omnibus/scopes.d")];
        if let Some(config) = std::env::var_os("XDG_CONFIG_HOME") {
            dirs.push(PathBuf::from(config).join("omnibus/scopes.d"));
        } else if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".config/omnibus/scopes.d"));
        }
        dirs
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
                let paths = entry
                    .get("paths")
                    .and_then(|p| p.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
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
    /// (canonical-prefix match; the adapter enforces the same rule defensively).
    pub fn path_allowed(&self, capability: &str, canonical: &str) -> bool {
        self.write
            .get(capability)
            .is_some_and(|paths| paths.iter().any(|p| canonical.starts_with(p.as_str())))
    }

    /// Write-grant check for a `[+scope]` tool: the capability must be in `[write]`
    /// (and, for parameterized scopes, the target must be inside the paths).
    pub fn authorize_write(
        &self,
        capability: &str,
        canonical_path: Option<&str>,
    ) -> Result<(), ContractError> {
        match self.write.get(capability) {
            None => Err(ContractError::ScopeDenied),
            Some(paths) if paths.is_empty() => Ok(()),
            Some(paths) => {
                let target = canonical_path.unwrap_or_default();
                if paths.iter().any(|p| target.starts_with(p.as_str())) {
                    Ok(())
                } else {
                    Err(ContractError::ScopeDenied)
                }
            }
        }
    }
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
