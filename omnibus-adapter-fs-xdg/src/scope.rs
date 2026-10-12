//! Scope logic for the fs-xdg adapter (pure decision core over tiny native
//! probes, unit-tested): the default read scope is `$HOME` including dotfiles
//! (decision 2026-10-10); anything outside `$HOME` is only reachable through an
//! explicit granted scope; writes land only inside granted write paths, and
//! system locations — the grant store included — are never writable: defense in
//! depth under the core's own grant check (roadmap 1.5.5, 1.6.4).
//!
//! Paths are resolved with lstat semantics component by component, in the
//! kernel's order: every symlink in the existing part of the path is followed,
//! and a missing tail is kept verbatim so writes can create new files. A scope
//! decision is never made on a lexically re-attached symlink.

use std::path::{Path, PathBuf};

use omnibus_contracts::errors::ContractError;

/// Hard-deny prefixes: even a mis-granted scope must not make these writable.
const NEVER_WRITABLE: [&str; 7] = ["/etc", "/usr", "/boot", "/proc", "/sys", "/dev", "/var"];

/// Symlink budget for one path resolution, mirroring the kernel's ELOOP depth.
const MAX_SYMLINKS: usize = 40;

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
    /// `$HOME` when the environment carries a non-empty value. `None` means
    /// the default read scope does not exist: reads outside granted write
    /// scopes are refused and the capability report must say so (degraded)
    /// instead of claiming a present `$HOME` scope that reads nothing.
    home: Option<PathBuf>,
    write_paths: Vec<PathBuf>,
    /// The configuration root the grant store lives under:
    /// `$XDG_CONFIG_HOME` when set to a non-empty value (empty counts as
    /// unset), else `home/.config` — the core's rule (scopes.rs
    /// `user_store_dir`), so both layers always resolve the same store.
    config_root: Option<PathBuf>,
    /// The daemon's audit state directory (`$XDG_STATE_HOME/omnibus`, else
    /// `$HOME/.local/state/omnibus`, else the root-anchored
    /// `/.local/state/omnibus` — the same present-but-empty-counts-as-unset
    /// XDG rule and home fallback the wiring applies to the fallback path).
    /// Never writable — and never readable — through the file tools: the core
    /// denies it against the actual fallback path; this adapter-side deny is
    /// the second layer (audit.md). The read deny matters because the state
    /// directory holds the per-install `params_hash` key and the audit trail:
    /// an agent that could read the key could enumerate low-entropy arguments
    /// from journal `params_hash` values (audit.md's anti-enumeration
    /// invariant).
    state_root: Option<PathBuf>,
}

/// One step of a path resolution, owned (symlink targets join the queue).
enum Step {
    Parent,
    Name(std::ffi::OsString),
}

/// Why a path could not be fully resolved.
enum Resolution {
    Io(std::io::Error),
    Loop,
}

impl FileScopes {
    /// Builds scopes from the granted write paths. The read scope is `$HOME`
    /// when the environment provides a non-empty `HOME`; without it the
    /// default read scope does not exist and
    /// [`FileScopes::default_read_scope_available`] lets the capability
    /// surface degrade honestly instead of silently reading nothing.
    pub fn new(write_paths: Vec<String>) -> Self {
        let home = Self::home_from_env();
        let config_root = Self::config_root_from_env(&home);
        let state_root = Self::state_root_from_env(&home);
        Self::build(write_paths, home, config_root, state_root)
    }

    /// Explicit-home constructor (tests, and callers with a resolved user
    /// context — the composition root never uses it). The grant-store root is
    /// derived from the given home so hermetic fixtures stay hermetic;
    /// production resolves it from the environment in [`FileScopes::new`].
    pub fn with_home(write_paths: Vec<String>, home: PathBuf) -> Self {
        Self::build(
            write_paths,
            Some(home.clone()),
            Some(home.join(".config")),
            Some(home.join(".local/state/omnibus")),
        )
    }

    /// Test constructor: no `$HOME` at all (a scrubbed environment).
    #[cfg(test)]
    pub(crate) fn without_home(write_paths: Vec<String>) -> Self {
        Self::build(write_paths, None, None, None)
    }

    fn build(
        write_paths: Vec<String>,
        home: Option<PathBuf>,
        config_root: Option<PathBuf>,
        state_root: Option<PathBuf>,
    ) -> Self {
        Self {
            home,
            write_paths: write_paths.iter().map(PathBuf::from).collect(),
            config_root,
            state_root,
        }
    }

    /// `$HOME` when set to a non-empty value; empty counts as unset.
    fn home_from_env() -> Option<PathBuf> {
        std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }

    /// The configuration root the grant store lives under: `$XDG_CONFIG_HOME`
    /// when set to a non-empty value, else `$HOME/.config`.
    fn config_root_from_env(home: &Option<PathBuf>) -> Option<PathBuf> {
        match std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
            Some(config) => Some(PathBuf::from(config)),
            None => home.as_ref().map(|home| home.join(".config")),
        }
    }

    /// The daemon's audit state directory: `$XDG_STATE_HOME/omnibus` when set
    /// to a non-empty *absolute* value, else `$HOME/.local/state/omnibus`, else
    /// (no resolvable `$HOME` at all) the root-anchored `/.local/state/omnibus`
    /// — the exact rule the wiring applies when it picks the audit fallback
    /// path (`xdg_dir_from` counts empty *and relative* values as unset,
    /// `home_dir_from` falls back to the filesystem root), so both layers
    /// resolve the same directory in every environment.
    fn state_root_from_env(home: &Option<PathBuf>) -> Option<PathBuf> {
        match std::env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .filter(|value| Path::new(value).is_absolute())
        {
            Some(state) => Some(PathBuf::from(state).join("omnibus")),
            None => Some(
                home.as_ref()
                    .map(|home| home.join(".local/state/omnibus"))
                    .unwrap_or_else(|| PathBuf::from("/.local/state/omnibus")),
            ),
        }
    }

    /// Whether the default read scope (`$HOME`) exists at all. `false` means
    /// every read outside granted write scopes is refused and the capability
    /// report must say so — never claim a present default scope.
    pub fn default_read_scope_available(&self) -> bool {
        self.home.is_some()
    }

    /// The grant-store directory the adapter itself must never write through:
    /// a files grant covering `$HOME` must not become grant self-issuance. The
    /// core enforces the same rule against its loaded store directories; this
    /// adapter-side deny is the second layer.
    fn grant_store_dirs(&self) -> Vec<PathBuf> {
        self.config_root
            .as_ref()
            .map(|root| vec![root.join("omnibus/scopes.d")])
            .unwrap_or_default()
    }

    /// Canonicalizes as far as the filesystem allows: existing components
    /// (symlinks included, wherever they sit) resolve, a missing tail is kept.
    pub fn canonicalize(&self, raw: &str) -> Result<String, ContractError> {
        Ok(self.decide(raw)?.canonical)
    }

    /// `~` expansion and relative-path resolution against `$HOME`. `None`
    /// means the path needs `$HOME` (or the cwd) and `$HOME` is not set: the
    /// decision fails closed.
    fn expand(&self, raw: &str) -> Option<PathBuf> {
        let trimmed = raw.trim();
        if let Some(rest) = trimmed.strip_prefix("~/") {
            return self.home.as_ref().map(|home| home.join(rest));
        }
        if trimmed == "~" {
            return self.home.clone();
        }
        let path = Path::new(trimmed);
        if path.is_absolute() {
            Some(path.to_path_buf())
        } else {
            self.home.as_ref().map(|home| home.join(path))
        }
    }

    /// The full decision for one raw path. Fails closed: an unresolvable or
    /// home-relative-without-`$HOME` path never decides as allowed.
    ///
    /// `..` semantics: dot-dot components are collapsed lexically on the raw
    /// path BEFORE any symlink is resolved (see [`lexical`]), so
    /// `a/link/../b` judges as `a/b`, never as the link target's parent. The
    /// semantics are self-consistent — the `canonical` reported here is
    /// exactly the component chain the adapter later opens, one O_NOFOLLOW
    /// step at a time, so decision and open can never disagree about which
    /// chain is being judged — and fail-safe: a decision is never widened by
    /// a link. The cost is honesty about the shape of the answer, not its
    /// safety: the reported `canonical` can differ from `realpath(1)` when a
    /// `..` pops across a symlinked component.
    pub fn decide(&self, raw: &str) -> Result<ScopeDecision, ContractError> {
        let Some(path) = self.expand(raw) else {
            return Ok(ScopeDecision {
                canonical: raw.trim().to_string(),
                inside_home: false,
                read_allowed: false,
                write_allowed: false,
            });
        };
        let canonical = resolve_links(&lexical(&path)).map_err(resolution_error)?;
        let canonical = canonical.to_string_lossy().into_owned();
        let inside_home = self
            .home
            .as_ref()
            .is_some_and(|home| contains(&canonical, &resolved_root(home)));
        let in_write_scope = self
            .write_paths
            .iter()
            .any(|root| contains(&canonical, &resolved_root(root)));
        let never_writable = NEVER_WRITABLE
            .iter()
            .any(|prefix| contains(&canonical, Path::new(prefix)));
        let in_grant_store = self
            .grant_store_dirs()
            .iter()
            .any(|dir| contains(&canonical, &resolved_root(dir)));
        let in_state_dir = self
            .state_root
            .as_ref()
            .is_some_and(|dir| contains(&canonical, &resolved_root(dir)));
        Ok(ScopeDecision {
            canonical: canonical.clone(),
            inside_home,
            read_allowed: (inside_home || in_write_scope) && !in_grant_store && !in_state_dir,
            write_allowed: in_write_scope && !never_writable && !in_grant_store && !in_state_dir,
        })
    }
}

/// Component-boundary containment, the core's `within_scope` semantics
/// (omnibus-core/src/scopes.rs): `target` is inside `root` when every
/// component of `root` prefixes `target`'s components — never a bare string
/// prefix, so the sibling `/home/u/Projects-backup` is not inside
/// `/home/u/Projects`.
fn contains(target: &str, root: &Path) -> bool {
    !target.is_empty() && Path::new(target).starts_with(root)
}

/// Resolves `root` as far as the filesystem allows, falling back to its
/// lexical normalization when the root cannot be probed (grant roots and
/// `$HOME` may sit on inaccessible mounts; the core judges those lexically
/// too). Symlinks in the existing part always resolve.
fn resolved_root(root: &Path) -> PathBuf {
    let flat = lexical(root);
    resolve_links(&flat).unwrap_or(flat)
}

/// Lexical normalization without touching the fs: `.` components collapse and
/// `..` pops (symlinks, if any, are resolved afterwards by [`resolve_links`]).
/// The `..` pop is deliberately pre-symlink (see [`FileScopes::decide`] for
/// the full rationale): fail-safe for scope decisions, but unlike
/// `realpath(1)` a `..` popping across a symlinked component does not follow
/// the link backwards.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::from("/");
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn steps_of(path: &Path) -> Vec<Step> {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::ParentDir => Some(Step::Parent),
            std::path::Component::Normal(name) => Some(Step::Name(name.to_os_string())),
            std::path::Component::RootDir
            | std::path::Component::CurDir
            | std::path::Component::Prefix(_) => None,
        })
        .collect()
}

/// Resolves `path` with lstat semantics: the longest existing prefix resolves
/// component by component (following every symlink, wherever it sits), and the
/// missing tail is kept verbatim so writes can create new files.
fn resolve_links(path: &Path) -> Result<PathBuf, Resolution> {
    let mut stack = steps_of(path);
    stack.reverse();
    let mut resolved = PathBuf::from("/");
    let mut followed = 0usize;
    while let Some(step) = stack.pop() {
        let name = match step {
            Step::Parent => {
                resolved.pop();
                continue;
            }
            Step::Name(name) => name,
        };
        resolved.push(&name);
        match std::fs::symlink_metadata(&resolved) {
            Ok(meta) if meta.file_type().is_symlink() => {
                followed += 1;
                if followed > MAX_SYMLINKS {
                    return Err(Resolution::Loop);
                }
                let target = std::fs::read_link(&resolved).map_err(Resolution::Io)?;
                resolved.pop();
                if target.is_absolute() {
                    resolved = PathBuf::from("/");
                }
                let mut link_steps = steps_of(&target);
                link_steps.reverse();
                stack.extend(link_steps);
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // The tail does not exist (yet): keep the remaining components
                // verbatim — a write may create them.
                while let Some(rest) = stack.pop() {
                    match rest {
                        Step::Name(name) => resolved.push(name),
                        Step::Parent => {
                            resolved.pop();
                        }
                    }
                }
                return Ok(resolved);
            }
            Err(e) => return Err(Resolution::Io(e)),
        }
    }
    Ok(resolved)
}

fn resolution_error(fail: Resolution) -> ContractError {
    match fail {
        Resolution::Io(e) => ContractError::NativeFailure {
            code: e.raw_os_error().unwrap_or(-1),
            payload: format!("fs scope resolution: {e}"),
        },
        Resolution::Loop => ContractError::NativeFailure {
            code: -1,
            payload: String::from(
                "fs scope resolution: symlink budget exceeded (possible symlink loop)",
            ),
        },
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
        let d = s.decide("/home/u/Documents/report.txt").expect("decide");
        assert!(d.inside_home);
        assert!(d.read_allowed);
        assert!(!d.write_allowed);
        let dot = s.decide("~/.config/app/settings.ini").expect("decide");
        assert!(
            dot.read_allowed,
            "dotfiles are inside the default read scope"
        );
        assert_eq!(dot.canonical, "/home/u/.config/app/settings.ini");
    }

    #[test]
    fn outside_home_denied_without_a_grant() {
        let s = scopes(vec![]);
        let d = s.decide("/etc/os-release").expect("decide");
        assert!(!d.read_allowed);
        assert!(!d.write_allowed);
    }

    #[test]
    fn write_scope_grants_read_and_write_but_not_system_paths() {
        let s = scopes(vec!["/home/u/Projects"]);
        let d = s
            .decide("/home/u/Projects/omnibus/src/main.rs")
            .expect("decide");
        assert!(d.read_allowed);
        assert!(d.write_allowed);
        // /var is in the never-writable set even when someone grants it.
        let s2 = scopes(vec!["/var/tmp/notes"]);
        let d2 = s2.decide("/var/tmp/notes/x.md").expect("decide");
        assert!(d2.read_allowed);
        assert!(!d2.write_allowed, "never-writable prefixes stay unwritable");
    }

    #[test]
    fn traversal_and_missing_tails_resolve_honestly() {
        let s = scopes(vec![]);
        let d = s.decide("/home/u/../u/./notes/../x.txt").expect("decide");
        assert_eq!(d.canonical, "/home/u/x.txt");
        // A missing tail still decides against the canonical prefix.
        let fresh = s
            .decide("/home/u/Projects/never/created.md")
            .expect("decide");
        assert!(!fresh.write_allowed);
        assert!(fresh.read_allowed);
    }

    #[test]
    fn prefix_boundaries_are_exact() {
        let s = scopes(vec!["/home/u/Projects"]);
        let sneaky = s.decide("/home/u/Projects-evil/secret").expect("decide");
        assert!(!sneaky.write_allowed, "prefix match must not be textual");
    }

    #[test]
    fn missing_home_fails_closed_but_absolute_granted_paths_decide() {
        let s = FileScopes::without_home(vec![String::from("/srv/data")]);
        assert!(!s.default_read_scope_available());
        let granted = s.decide("/srv/data/x.md").expect("decide");
        assert!(granted.write_allowed, "absolute grants still decide");
        assert!(granted.read_allowed);
        let home_relative = s.decide("~/notes.txt").expect("decide");
        assert!(
            !home_relative.read_allowed && !home_relative.write_allowed,
            "a home-relative path without $HOME fails closed"
        );
        let absolute = s.decide("/etc/os-release").expect("decide");
        assert!(!absolute.read_allowed && !absolute.write_allowed);
    }

    #[test]
    fn symlinked_scope_root_keeps_the_tail() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        let real = dir.path().join("real");
        std::fs::create_dir_all(&home).expect("home dir");
        std::fs::create_dir_all(&real).expect("real dir");
        std::os::unix::fs::symlink(&real, home.join("projects")).expect("symlink");
        let s = FileScopes::with_home(
            vec![home.join("projects").to_string_lossy().into_owned()],
            home.clone(),
        );
        // The grant root itself is a symlink: it matches in canonical space.
        let d = s
            .decide(&home.join("projects").join("new.md").to_string_lossy())
            .expect("decide");
        assert_eq!(
            d.canonical,
            real.join("new.md").to_string_lossy().into_owned(),
            "the missing tail survives symlinked scope roots"
        );
        assert!(d.write_allowed);
        assert!(d.read_allowed);
    }

    #[test]
    fn symlink_in_the_tail_is_resolved_not_judged_lexically() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join("projects/w")).expect("dirs");
        std::os::unix::fs::symlink("/etc", home.join("projects/w/link")).expect("symlink");
        let s = FileScopes::with_home(
            vec![home.join("projects").to_string_lossy().into_owned()],
            home.clone(),
        );
        let d = s
            .decide(
                home.join("projects/w/link/newfile")
                    .to_string_lossy()
                    .into_owned()
                    .as_str(),
            )
            .expect("decide");
        assert_eq!(
            d.canonical, "/etc/newfile",
            "a symlink in the unresolvable tail is followed, never judged lexically"
        );
        assert!(!d.write_allowed, "the escaped target stays unwritable");
        assert!(!d.read_allowed);
    }

    #[test]
    fn grant_store_dir_is_never_writable_through_the_adapter() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().to_path_buf();
        // A generous grant on all of $HOME.
        let s = FileScopes::with_home(vec![home.to_string_lossy().into_owned()], home.clone());
        let ordinary = s.decide(
            home.join("notes.txt")
                .to_string_lossy()
                .into_owned()
                .as_str(),
        );
        let ordinary = ordinary.expect("decide");
        assert!(ordinary.write_allowed, "the grant itself still works");
        let store = s
            .decide(
                home.join(".config/omnibus/scopes.d/90-self.toml")
                    .to_string_lossy()
                    .into_owned()
                    .as_str(),
            )
            .expect("decide");
        assert!(
            !store.write_allowed,
            "the grant store stays unwritable under a grant on all of $HOME"
        );
        assert!(
            !store.read_allowed,
            "the store stays unreadable too: grant manifests are not agent-facing"
        );
        let sibling = s
            .decide(
                home.join(".config/other/app.toml")
                    .to_string_lossy()
                    .into_owned()
                    .as_str(),
            )
            .expect("decide");
        assert!(
            sibling.write_allowed,
            "only the store directory is denied, not all of .config"
        );
    }

    #[test]
    fn audit_state_dir_is_never_writable_through_the_adapter() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().to_path_buf();
        // A generous grant on all of $HOME (the state dir lives under it).
        let s = FileScopes::with_home(vec![home.to_string_lossy().into_owned()], home.clone());
        let audit = s
            .decide(
                home.join(".local/state/omnibus/audit.jsonl")
                    .to_string_lossy()
                    .into_owned()
                    .as_str(),
            )
            .expect("decide");
        assert!(
            !audit.write_allowed,
            "the audit state dir stays unwritable under a grant on all of $HOME"
        );
        assert!(
            !audit.read_allowed,
            "the state dir stays unreadable: the params_hash key and the audit trail must not be agent-readable, or the audit anti-enumeration invariant (audit.md) is void"
        );
        let sibling = s
            .decide(
                home.join(".local/state/other/app.json")
                    .to_string_lossy()
                    .into_owned()
                    .as_str(),
            )
            .expect("decide");
        assert!(
            sibling.write_allowed,
            "only the daemon's own state dir is denied, not the state parent"
        );
    }
}
