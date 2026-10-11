//! `omnibus grant <capability>` — the single grant mechanism (specs/contracts/
//! config-and-grants.md): edits the user scope manifest. Only a human runs this;
//! there is no MCP method and no agent-provided token that could issue a grant
//! (no-confirm-API invariant, roadmap 1.3.3).

use omnibus_contracts::capabilities as cap;

/// Which manifest section a capability belongs to.
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
    ];
    if write_gated.contains(&capability) {
        return Some(Section::Write);
    }
    if capability == cap::JOURNAL_PERSISTENT {
        return Some(Section::OptIn);
    }
    None
}

/// Runs the grant subcommand; `Some(exit)` semantics mirror `main`.
pub fn run(capability: Option<&str>) -> i32 {
    let Some(capability) = capability else {
        eprintln!("omnibus grant: missing capability name (see omnibusd --help)");
        return 2;
    };
    if !cap::ALL.contains(&capability) {
        eprintln!("omnibus grant: unknown capability {capability}");
        return 2;
    }
    let Some(section) = section_of(capability) else {
        eprintln!(
            "omnibus grant: {capability} is not a grantable capability (it is not a \
             [+scope] write nor an opt-in capability)"
        );
        return 2;
    };

    let path = grant_manifest_path();
    match upsert_entry(&path, section, capability) {
        Ok(()) => {
            println!("granted {capability} (provenance cli)");
            println!("manifest: {}", path.display());
            println!("a new omnibusd instance picks this up at its next probe.");
            audit_grant_event(capability, &path);
            0
        }
        Err(e) => {
            eprintln!("omnibus grant: failed to update {}: {e}", path.display());
            1
        }
    }
}

/// The user store: $XDG_CONFIG_HOME/omnibus/scopes.d/90-grants.toml.
fn grant_manifest_path() -> std::path::PathBuf {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(v) if !v.is_empty() => std::path::PathBuf::from(v),
        _ => std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("/"))
            .join(".config"),
    };
    base.join("omnibus/scopes.d/90-grants.toml")
}

/// Parses the manifest (or starts a fresh table), inserts the entry under the
/// section, and writes the file back. The rewrite drops comments — documented
/// behavior of the CLI editor (temporary grant = grant then revoke by removal).
fn upsert_entry(path: &std::path::Path, section: Section, capability: &str) -> Result<(), String> {
    let section_name = match section {
        Section::Write => "write",
        Section::OptIn => "optin",
    };
    let mut table: toml::Table = match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("parse: {e}"))?,
        Err(_) => toml::Table::new(),
    };
    let entry = table
        .entry(section_name)
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let toml::Value::Table(section_table) = entry else {
        return Err(format!("[{section_name}] is not a table in the manifest"));
    };
    if section_table.contains_key(capability) {
        return Ok(()); // idempotent: re-granting is a no-op
    }
    section_table.insert(
        String::from(capability),
        toml::Value::Table(toml::Table::new()),
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create dirs: {e}"))?;
    }
    let rendered = toml::to_string(&table).map_err(|e| format!("render: {e}"))?;
    std::fs::write(path, rendered).map_err(|e| format!("write: {e}"))
}

/// The grant event itself is audited with provenance `cli` (roadmap 1.3.3 AC:
/// every grant event carries provenance config|cli|doctor).
fn audit_grant_event(capability: &str, manifest: &std::path::Path) {
    let actor = super::wiring::current_actor(Some(String::from("omnibus-grant-cli")));
    let event = omnibus_contracts::dto::audit::AuditEvent {
        ts_epoch_ms: omnibus_contracts::dto::Milliseconds::now_epoch().0,
        session: String::from("cli"),
        actor,
        tool: String::from("grant"),
        adapter: String::from("core"),
        op: format!("grant:{capability}"),
        params_hash: omnibus_core::audit::params_hash(
            &serde_json::json!({ "capability": capability }),
        ),
        result: String::from("ok"),
        scope_ref: Some(manifest.display().to_string()),
    };
    let fallback = super::wiring::audit_fallback_path();
    if let Some(parent) = fallback.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    omnibus_core::audit::FileSink::new(fallback).append(&event);
}
