//! Scope logic for the fs-xdg adapter (pure, unit-tested): the default read
//! scope is `$HOME` including dotfiles (decision 2026-10-10); anything outside
//! `$HOME` is only reachable through an explicit granted scope; writes land
//! only inside granted write paths, and system locations are never writable —
//! defense in depth under the core's own grant check (roadmap 1.5.5, 1.6.4).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Hard-deny prefixes: even a mis-granted scope must not make these writable.
const NEVER_WRITABLE: [&str; 7] = ["/etc", "/usr", "/boot", "/proc", "/sys", "/dev", "/var"];

/// The resolved scope facts for one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeDecision {
    pub canonical: String,
    pub inside_home: bool,
    pub read_allowed: bool,
    pub write_allowed: bool,
}

/// The activated scope set of one adapter instance.
#[derive(Debug, Clone)]
pub struct FileScopes {
    home: PathBuf,
    write_paths: Vec<PathBuf>,
}

impl FileScopes {
    /// Builds scopes from the granted write paths (the read scope is `$HOME`).
    pub fn new(write_paths: Vec<String>) -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        Self {
            home,
            write_paths: write_paths.iter().map(PathBuf::from).collect(),
        }
    }

    /// Explicit-home constructor (tests, and callers with a resolved user
    /// context — the composition root never uses it).
    pub fn with_home(write_paths: Vec<String>, home: PathBuf) -> Self {
        Self {
            home,
            write_paths: write_paths.iter().map(PathBuf::from).collect(),
        }
    }

    /// Canonicalizes as far as the filesystem allows (missing tail is kept —
    /// writes may create new files); symlinks in the existing part resolve.
    pub fn canonicalize(&self, raw: &str) -> String {
        let path = self.expand(raw);
        match Self::canonical_prefix(&path) {
            Some(existing) => {
                let tail = path.strip_prefix(&existing).unwrap_or(Path::new(""));
                let joined = existing.join(tail);
                Self::normalize(&joined)
            }
            None => Self::normalize(&path),
        }
    }

    /// `~` expansion and relative-path resolution against `$HOME`.
    fn expand(&self, raw: &str) -> PathBuf {
        let trimmed = raw.trim();
        if let Some(rest) = trimmed.strip_prefix("~/") {
            return self.home.join(rest);
        }
        if trimmed == "~" {
            return self.home.clone();
        }
        let p = Path::new(trimmed);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.home.join(p)
        }
    }

    /// The longest existing ancestor (canonicalized) of `path`.
    fn canonical_prefix(path: &Path) -> Option<PathBuf> {
        let mut current = path.to_path_buf();
        loop {
            if let Ok(c) = current.canonicalize() {
                return Some(c);
            }
            if !current.pop() {
                return None;
            }
        }
    }

    /// Lexical normalization without touching the fs (dot removal only; the
    /// canonical prefix did the symlink work).
    fn normalize(path: &Path) -> String {
        let mut out = PathBuf::from("/");
        for comp in path.components() {
            match comp {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    out.pop();
                }
                other => out.push(other.as_os_str()),
            }
        }
        out.to_string_lossy().into_owned()
    }

    fn under(&self, canonical: &str, root: &Path) -> bool {
        let root_str = Self::normalize(root);
        canonical == root_str || canonical.starts_with(&format!("{root_str}/"))
    }

    /// The full decision for one raw path.
    pub fn decide(&self, raw: &str) -> ScopeDecision {
        let canonical = self.canonicalize(raw);
        let inside_home = self.under(&canonical, &self.home);
        let write_paths: BTreeSet<String> = self
            .write_paths
            .iter()
            .map(|p| Self::normalize(p))
            .collect();
        let in_write_scope = write_paths
            .iter()
            .any(|root| canonical == *root || canonical.starts_with(&format!("{root}/")));
        let never_writable = NEVER_WRITABLE
            .iter()
            .any(|prefix| canonical == *prefix || canonical.starts_with(&format!("{prefix}/")));
        ScopeDecision {
            canonical: canonical.clone(),
            inside_home,
            read_allowed: inside_home || in_write_scope,
            write_allowed: in_write_scope && !never_writable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scopes(write: Vec<&str>) -> FileScopes {
        FileScopes::with_home(
            write.into_iter().map(String::from).collect(),
            PathBuf::from("/home/u"),
        )
    }

    #[test]
    fn home_is_readable_including_dotfiles() {
        let s = scopes(vec![]);
        let d = s.decide("/home/u/Documents/report.txt");
        assert!(d.inside_home);
        assert!(d.read_allowed);
        assert!(!d.write_allowed);
        let dot = s.decide("~/.config/app/settings.ini");
        assert!(
            dot.read_allowed,
            "dotfiles are inside the default read scope"
        );
        assert_eq!(dot.canonical, "/home/u/.config/app/settings.ini");
    }

    #[test]
    fn outside_home_denied_without_a_grant() {
        let s = scopes(vec![]);
        let d = s.decide("/etc/os-release");
        assert!(!d.read_allowed);
        assert!(!d.write_allowed);
    }

    #[test]
    fn write_scope_grants_read_and_write_but_not_system_paths() {
        let s = scopes(vec!["/home/u/Projects"]);
        let d = s.decide("/home/u/Projects/omnibus/src/main.rs");
        assert!(d.read_allowed);
        assert!(d.write_allowed);
        // /var is in the never-writable set even when someone grants it.
        let s2 = scopes(vec!["/var/tmp/notes"]);
        let d2 = s2.decide("/var/tmp/notes/x.md");
        assert!(d2.read_allowed);
        assert!(!d2.write_allowed, "never-writable prefixes stay unwritable");
    }

    #[test]
    fn traversal_and_missing_tails_resolve_honestly() {
        let s = scopes(vec![]);
        let d = s.decide("/home/u/../u/./notes/../x.txt");
        assert_eq!(d.canonical, "/home/u/x.txt");
        // A missing tail still decides against the canonical prefix.
        let fresh = s.decide("/home/u/Projects/never/created.md");
        assert!(!fresh.write_allowed);
        assert!(fresh.read_allowed);
    }

    #[test]
    fn prefix_boundaries_are_exact() {
        let s = scopes(vec!["/home/u/Projects"]);
        let sneaky = s.decide("/home/u/Projects-evil/secret");
        assert!(!sneaky.write_allowed, "prefix match must not be textual");
    }
}
