//! `omnibusd grant <capability> [--path <dir>]...` — the single grant mechanism
//! (specs/contracts/config-and-grants.md): edits the user scope manifest. Only a
//! human runs this; there is no MCP method and no agent-provided token that
//! could issue a grant (no-confirm-API invariant, roadmap 1.3.3).
//!
//! Parameterized capabilities (today: `files.scope.write`) are granted only in
//! the parameterized form — `--path <dir>`, repeatable — so the manifest entry
//! is `paths = [...]` everywhere: the core reads it as a scoped allow and the
//! adapter as the same scope. A pathless `files.scope.write` (an empty table,
//! which the core must read as allow-all) is never issued by this CLI.

use omnibus_contracts::capabilities as cap;

/// The one capability whose grant carries arguments (spec, "Manifest Format"):
/// the write scope is the `paths` list.
const PARAMETERIZED_WRITE: &str = cap::FILES_SCOPE_WRITE;

/// Which manifest section a capability belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Write,
    OptIn,
}

fn section_of(capability: &str) -> Option<Section> {
    let write_gated = [
        cap::DESKTOP_WRITE_TYPED,
        cap::PORTAL_CLIPBOARD,
        cap::PORTAL_NOTIFY,
        cap::SERVICE_WRITE,
        cap::PKG_WRITE,
        cap::PKG_UNDO,
        cap::FILES_SCOPE_WRITE,
        // App launch/open (roadmap 2.7.2): the write arm of subsystem 11,
        // always behind the out-of-band `app.launch` grant.
        cap::APP_LAUNCH,
        // ps.signal (roadmap 2.8.3): the write arm of subsystem 12, always
        // behind the out-of-band `ps.signal` grant.
        cap::PS_SIGNAL,
    ];
    if write_gated.contains(&capability) {
        return Some(Section::Write);
    }
    if capability == cap::JOURNAL_PERSISTENT {
        return Some(Section::OptIn);
    }
    None
}

/// Runs the grant subcommand; `tail` is everything after the `grant` verb.
/// `Some(exit)` semantics mirror `main`.
pub async fn run(tail: &[String]) -> i32 {
    let mut capability: Option<&str> = None;
    let mut raw_paths: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < tail.len() {
        let arg = tail[i].as_str();
        if arg == "--path" {
            match tail.get(i + 1) {
                Some(value) => {
                    raw_paths.push(value.as_str());
                    i += 2;
                }
                None => {
                    eprintln!("omnibusd grant: --path requires a directory argument");
                    return 2;
                }
            }
        } else if let Some(value) = arg.strip_prefix("--path=") {
            raw_paths.push(value);
            i += 1;
        } else if arg == "--help" || arg == "-h" {
            print_usage();
            return 0;
        } else if arg.len() > 1 && arg.starts_with('-') {
            eprintln!("omnibusd grant: unknown flag {arg}");
            return 2;
        } else if capability.is_none() {
            capability = Some(arg);
            i += 1;
        } else {
            eprintln!("omnibusd grant: unexpected extra argument {arg}");
            return 2;
        }
    }

    let Some(capability) = capability else {
        eprintln!("omnibusd grant: missing capability name (see omnibusd grant --help)");
        return 2;
    };
    if !cap::ALL.contains(&capability) {
        eprintln!("omnibusd grant: unknown capability {capability}");
        return 2;
    }
    let Some(section) = section_of(capability) else {
        eprintln!(
            "omnibusd grant: {capability} is not a grantable capability (it is not a \
             [+scope] write nor an opt-in capability)"
        );
        return 2;
    };

    let parameterized = capability == PARAMETERIZED_WRITE;
    let paths = if parameterized {
        if raw_paths.is_empty() {
            eprintln!(
                "omnibusd grant: {capability} is a parameterized scope and requires at \
                 least one --path <dir> (repeat the flag for several paths), e.g. \
                 omnibusd grant {capability} --path ~/Projects"
            );
            return 2;
        }
        let mut expanded = Vec::with_capacity(raw_paths.len());
        for raw in &raw_paths {
            match expand_path(raw, std::env::var_os("HOME").as_deref()) {
                Ok(path) => expanded.push(path),
                Err(e) => {
                    eprintln!("omnibusd grant: --path {raw}: {e}");
                    return 2;
                }
            }
        }
        expanded.sort();
        expanded.dedup();
        expanded
    } else if !raw_paths.is_empty() {
        eprintln!(
            "omnibusd grant: --path applies only to the parameterized scope \
             {PARAMETERIZED_WRITE}; {capability} is granted unscoped"
        );
        return 2;
    } else {
        Vec::new()
    };

    let path = match grant_manifest_path() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("omnibusd grant: {e}");
            return 2;
        }
    };
    match upsert_entry(&path, section, capability, &paths) {
        Ok(changed) => {
            if changed {
                println!("granted {capability} (provenance cli)");
                if parameterized {
                    println!("paths: {}", paths.join(", "));
                }
                println!("manifest: {}", path.display());
                println!("a new omnibusd instance picks this up at its next probe.");
                audit_grant_event(capability, &path, &paths).await;
            } else {
                // The manifest is already in the requested state: printing
                // "granted" would claim a change that did not happen, and an
                // audit event would fabricate provenance (R5/R7 honesty).
                println!("already granted (manifest unchanged)");
            }
            0
        }
        Err(e) => {
            eprintln!("omnibusd grant: failed to update {}: {e}", path.display());
            1
        }
    }
}

fn print_usage() {
    println!("usage: omnibusd grant <capability> [--path <dir>]...");
    println!(
        "  --path <dir>   scope a parameterized grant (repeatable; required by \
         {PARAMETERIZED_WRITE})"
    );
    println!("  a leading ~ in --path expands to $HOME");
}

/// The user store: $XDG_CONFIG_HOME/omnibus/scopes.d/90-grants.toml. An empty
/// `XDG_CONFIG_HOME` counts as unset — the exact rule the daemon's manifest
/// loader applies — so the CLI and the daemon resolve the same store (issue
/// #53). A resolution no daemon would load is refused: a relative one (a
/// relative `XDG_CONFIG_HOME`, or no `$HOME` to anchor the fallback) would
/// depend on the directory the human happened to run the CLI from, and with
/// *both* variables unset the daemon skips the user store entirely, so the
/// root-anchored `/.config` store must not be written either.
fn grant_manifest_path() -> Result<std::path::PathBuf, String> {
    let path = super::wiring::grant_config_dir()?.join("omnibus/scopes.d/90-grants.toml");
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(format!(
            "the resolved grant store {} is not an absolute path — refusing to \
             write a CWD-relative store; set $HOME (or $XDG_CONFIG_HOME) to an \
             absolute directory",
            path.display()
        ))
    }
}

/// Expands a leading `~` (via `$HOME`) and requires an absolute path: the
/// manifest stores scope roots the core and the adapter match canonically, so a
/// relative or unexpanded path would silently never match. An empty-but-set
/// `$HOME` counts as unset (the core's env rule) — expanding against it would
/// mint the root-anchored `/x`.
fn expand_path(raw: &str, home: Option<&std::ffi::OsStr>) -> Result<String, String> {
    const NO_HOME: &str = "$HOME is not set (an empty value counts as unset) or not valid UTF-8; \
         pass an absolute path";
    let home = home.filter(|h| !h.is_empty());
    let expanded = match raw {
        "~" => String::from(home.and_then(|h| h.to_str()).ok_or(NO_HOME)?),
        p if let Some(rest) = p.strip_prefix("~/") => {
            format!("{}/{rest}", home.and_then(|h| h.to_str()).ok_or(NO_HOME)?)
        }
        p if p.starts_with('~') => {
            return Err(String::from(
                "~user expansion is not supported; pass an absolute path",
            ));
        }
        p => String::from(p),
    };
    if !std::path::Path::new(&expanded).is_absolute() {
        return Err(format!(
            "{expanded} is not an absolute path (the manifest stores scope roots, \
             not relative globs)"
        ));
    }
    Ok(expanded)
}

/// Parses the manifest (or starts a fresh table), inserts or updates the entry
/// under the section, and writes the file back atomically. The rewrite drops
/// comments — documented behavior of the CLI editor (temporary grant = grant
/// then revoke by removal). Returns `Ok(true)` when the manifest was written
/// and `Ok(false)` when the entry was already identical (a no-op re-grant:
/// nothing changed, so nothing is printed as granted nor audited).
fn upsert_entry(
    path: &std::path::Path,
    section: Section,
    capability: &str,
    paths: &[String],
) -> Result<bool, String> {
    let section_name = match section {
        Section::Write => "write",
        Section::OptIn => "optin",
    };
    let mut table: toml::Table = match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("parse: {e}"))?,
        // Only a genuinely absent manifest starts fresh. A manifest that
        // exists but cannot be read back (EACCES on a root-owned 0600 file, a
        // stray non-UTF-8 byte) must never be silently replaced: the atomic
        // rename below only needs a writable directory, so the CLI would
        // print "granted" while every previously recorded entry is dropped.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(e) => return Err(format!("read {}: {e}", path.display())),
    };
    let entry = table
        .entry(section_name)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let toml::Value::Table(section_table) = entry else {
        return Err(format!("[{section_name}] is not a table in the manifest"));
    };
    let new_value = if capability == PARAMETERIZED_WRITE {
        let mut scoped = toml::Table::new();
        scoped.insert(
            String::from("paths"),
            toml::Value::Array(
                paths
                    .iter()
                    .map(|p| toml::Value::String(p.clone()))
                    .collect(),
            ),
        );
        toml::Value::Table(scoped)
    } else {
        toml::Value::Table(toml::Table::new())
    };
    if section_table.get(capability) == Some(&new_value) {
        return Ok(false); // idempotent: re-granting an identical entry is a no-op
    }
    section_table.insert(String::from(capability), new_value);
    let rendered = toml::to_string(&table).map_err(|e| format!("render: {e}"))?;
    write_atomic(path, &rendered)?;
    Ok(true)
}

/// Durability for the one file whose loss silently deletes recorded grants
/// (issue #103): write a sibling temp file, fsync it, rename over the target
/// (atomic within the filesystem), then fsync the parent directory. The
/// previous manifest's permissions are preserved on the replacement.
fn write_atomic(path: &std::path::Path, rendered: &str) -> Result<(), String> {
    use std::io::Write;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| String::from("manifest path has no parent directory"))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("create dirs: {e}"))?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| String::from("manifest path has no file name"))?;
    let mut temp = parent.to_path_buf();
    temp.push(format!(".{file_name}.{}.tmp", std::process::id()));

    let write_temp = || -> Result<(), String> {
        let mut file = std::fs::File::create(&temp).map_err(|e| format!("create temp: {e}"))?;
        if let Ok(meta) = std::fs::metadata(path) {
            let _ = file.set_permissions(meta.permissions());
        }
        file.write_all(rendered.as_bytes())
            .map_err(|e| format!("write temp: {e}"))?;
        file.sync_all().map_err(|e| format!("fsync temp: {e}"))?;
        Ok(())
    };
    if let Err(e) = write_temp() {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&temp, path) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("rename: {e}"));
    }
    // Best-effort: make the rename itself durable (failure here cannot be
    // undone; the renamed file is already the good state).
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// The grant event itself is audited through the same pipeline the daemon uses
/// (journald primary, file fallback) — never a private file-only channel — with
/// provenance `cli` carried in `scope_ref` (roadmap 1.3.3 AC: every grant event
/// carries provenance config|cli|doctor). The resolved paths land in the hashed
/// arguments for a parameterized scope: the hash stays irreversible, but two
/// different `--path` grants become distinguishable events. A delivery failure
/// on both sinks is surfaced loudly (audit totality) without changing the exit
/// status.
async fn audit_grant_event(capability: &str, manifest: &std::path::Path, paths: &[String]) {
    let arguments = serde_json::json!({ "capability": capability, "paths": paths });
    let delivered = super::wiring::audit_cli_event(
        "cli",
        "omnibus-grant-cli",
        "grant",
        &format!("grant:{capability}"),
        &arguments,
        "ok",
        Some(format!("cli:{}", manifest.display())),
    )
    .await;
    if !delivered {
        eprintln!(
            "omnibusd grant: WARNING: grant audit could not be delivered; \
             the fallback sink also failed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift guard: every `grant_key` the tool registry carries must be
    /// grantable through the CLI. A new write tool whose grant key never
    /// lands in `section_of` would answer `ScopeDenied` with a remedy —
    /// `omnibusd grant <capability>` — that this CLI then rejects as not
    /// grantable, and no other test would notice (the refusal text looks
    /// correct to the agent).
    #[test]
    fn every_tool_grant_key_is_grantable_through_the_cli() {
        for spec in omnibus_core::tools::all_specs() {
            let Some(key) = spec.grant_key else { continue };
            assert!(
                section_of(key).is_some(),
                "tool {} carries grant key {key} that `omnibusd grant` cannot issue",
                spec.name
            );
        }
    }

    fn manifest_in(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("90-grants.toml")
    }

    #[test]
    fn parameterized_grant_is_accepted_by_the_core_loader() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let manifest = manifest_in(dir.path());
        let paths = vec![String::from("/home/u/Notes")];
        upsert_entry(&manifest, Section::Write, PARAMETERIZED_WRITE, &paths)
            .unwrap_or_else(|e| panic!("upsert: {e}"));
        println!(
            "rendered parameterized manifest:\n{}",
            std::fs::read_to_string(&manifest).unwrap_or_else(|e| panic!("read: {e}"))
        );

        // The exact shape the (fixed) core loader reads back:
        let manifests = omnibus_core::scopes::ScopeManifests::load(&[dir.path().to_path_buf()]);
        assert!(
            manifests.errors.is_empty(),
            "the CLI-written manifest must parse: {:?}",
            manifests.errors
        );
        assert_eq!(
            manifests.write.get(PARAMETERIZED_WRITE).map(Vec::as_slice),
            Some(vec![String::from("/home/u/Notes")].as_slice())
        );
        // And the grant registry promotes it with config provenance:
        let (registry, _) = manifests.clone().into_grant_registry();
        assert!(registry.has_grant(PARAMETERIZED_WRITE));
    }

    #[test]
    fn unparameterized_grant_writes_the_empty_table_form() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let manifest = manifest_in(dir.path());
        upsert_entry(&manifest, Section::Write, cap::PKG_WRITE, &[])
            .unwrap_or_else(|e| panic!("upsert: {e}"));
        let text = std::fs::read_to_string(&manifest).unwrap_or_else(|e| panic!("read: {e}"));
        println!("rendered manifest:\n{text}");
        let manifests = omnibus_core::scopes::ScopeManifests::load(&[dir.path().to_path_buf()]);
        assert!(
            manifests.errors.is_empty(),
            "the CLI-written manifest must parse: {:?}",
            manifests.errors
        );
        // The loader reads the capability back as the *unparameterized* form
        // (empty path set) — whatever surface syntax toml renders it with.
        assert_eq!(
            manifests.write.get(cap::PKG_WRITE).map(Vec::as_slice),
            Some([].as_slice())
        );
        assert_eq!(
            manifests.write.len(),
            1,
            "only the granted capability is in [write]: {text}"
        );
    }

    #[test]
    fn re_grant_updates_paths_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let manifest = manifest_in(dir.path());
        let first = vec![String::from("/home/u/A"), String::from("/home/u/B")];
        let again = vec![String::from("/home/u/A"), String::from("/home/u/B")];
        let updated = vec![String::from("/home/u/C")];
        let wrote_first = upsert_entry(&manifest, Section::Write, PARAMETERIZED_WRITE, &first)
            .unwrap_or_else(|e| panic!("upsert 1: {e}"));
        assert!(wrote_first, "the first grant writes the manifest");
        let wrote_again = upsert_entry(&manifest, Section::Write, PARAMETERIZED_WRITE, &again)
            .unwrap_or_else(|e| panic!("upsert 2 (no-op): {e}"));
        assert!(
            !wrote_again,
            "an identical re-grant is a no-op: no change to report or audit"
        );
        let wrote_update = upsert_entry(&manifest, Section::Write, PARAMETERIZED_WRITE, &updated)
            .unwrap_or_else(|e| panic!("upsert 3: {e}"));
        assert!(wrote_update, "a changed grant writes the manifest");
        let manifests = omnibus_core::scopes::ScopeManifests::load(&[dir.path().to_path_buf()]);
        assert_eq!(
            manifests.write.get(PARAMETERIZED_WRITE).map(Vec::as_slice),
            Some(vec![String::from("/home/u/C")].as_slice())
        );
    }

    #[test]
    fn atomic_write_leaves_no_temp_files_and_preserves_permissions() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        // The parent directory does not exist yet: the first-ever grant must
        // create the scopes.d hierarchy itself.
        let manifest = dir.path().join("omnibus/scopes.d/90-grants.toml");
        upsert_entry(&manifest, Section::Write, cap::PKG_WRITE, &[])
            .unwrap_or_else(|e| panic!("upsert 1: {e}"));
        let before = std::fs::metadata(&manifest)
            .unwrap_or_else(|e| panic!("metadata: {e}"))
            .permissions();
        // The opt-in grant rewrites the existing manifest through the
        // temp+fsync+rename path.
        upsert_entry(&manifest, Section::OptIn, cap::JOURNAL_PERSISTENT, &[])
            .unwrap_or_else(|e| panic!("upsert optin: {e}"));
        let after = std::fs::metadata(&manifest)
            .unwrap_or_else(|e| panic!("metadata: {e}"))
            .permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(before.mode(), after.mode(), "permissions are preserved");
        }
        #[cfg(not(unix))]
        {
            let _ = (&before, &after);
        }
        let mut entries: Vec<String> =
            std::fs::read_dir(manifest.parent().unwrap_or_else(|| panic!("no parent")))
                .unwrap_or_else(|e| panic!("read_dir: {e}"))
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
        entries.sort();
        assert_eq!(
            entries,
            vec![String::from("90-grants.toml")],
            "exactly the manifest, no temp files left behind"
        );
    }

    #[test]
    fn expand_path_expands_tilde_and_requires_absolute() {
        let home = Some(std::ffi::OsStr::new("/home/u"));
        assert_eq!(
            expand_path("~/Notes", home),
            Ok(String::from("/home/u/Notes"))
        );
        assert_eq!(expand_path("~", home), Ok(String::from("/home/u")));
        assert!(expand_path("Notes", home).is_err(), "relative is rejected");
        assert!(expand_path("", home).is_err(), "empty is rejected");
        assert!(
            expand_path("~other/x", home).is_err(),
            "~user is not expanded"
        );
        assert!(
            expand_path("~/x", None).is_err(),
            "missing $HOME refuses rather than granting a wrong root"
        );
        // An empty-but-set $HOME counts as unset (the core's env rule):
        // expanding against it would mint the root-anchored "/x".
        let empty = Some(std::ffi::OsStr::new(""));
        assert!(
            expand_path("~/x", empty).is_err(),
            "empty $HOME refuses tilde expansion"
        );
        assert!(expand_path("~", empty).is_err(), "empty $HOME refuses ~");
    }
}
