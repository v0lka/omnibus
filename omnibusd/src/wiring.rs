//! The composition root: assembles probe → registry → core → facade from the
//! platform report. This binary is the only place adapter crates meet (R4-EDGES
//! keeps the core library adapter-free); whatever the probe did not select stays
//! absent — the capabilities document says so honestly.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use omnibus_contracts::dto::audit::AuditActor;
use omnibus_contracts::dto::probe::{DistroFamily, PlatformReport};
use omnibus_contracts::spi::probe::PlatformProbe as _;
use omnibus_core::registry::Registry;
use omnibus_core::{Core, audit, scopes, session, write_gate};

/// Builds the wired core for one fresh session (probe → register → serve).
pub async fn build_core(actor: &AuditActor) -> Result<(Core, PlatformReport), String> {
    // --- Probe (runtime detection wins; every session re-probes, §7.2) ---
    let probe = omnibus_probe::Probe::system(Arc::new(omnibus_probe::runner::SystemRunner));
    let platform = probe
        .detect()
        .await
        .map_err(|e| format!("probe failed: {e}"))?;

    // --- Session (entropy from the OS; tokens are core-minted) ---
    let (entropy, entropy_degraded) = entropy_32();
    let ctx = session::SessionContext::try_new(entropy, actor.clone()).map_err(|e| {
        // The all-zero mint key (the predictable degenerate key) is refused by
        // the core; it must never install silently — every token it would mint
        // is forgeable — so startup fails loudly instead.
        format!(
            "refusing to start: the OS entropy source yielded a degenerate \
             (all-zero) session mint key and the core rejected it ({e}); \
             check /dev/urandom availability and retry"
        )
    })?;
    let session_id = ctx.id.as_str().to_string();
    if entropy_degraded {
        // Honest degradation (R3): a weak mint key must never be installed
        // silently — the operator sees it on stderr and in the audit trail.
        eprintln!(
            "omnibusd: DEGRADED: /dev/urandom unavailable — the session mint key \
             is derived from weak entropy (clock + pid), so session tokens are \
             guessable; token-addressed writes remain grant-gated"
        );
    }

    // --- Grants from the scope manifests (provenance `config` at read time;
    // the issuing `omnibusd grant` CLI event carries its own `cli` provenance) ---
    let manifest_dirs = scopes::ScopeManifests::default_dirs();
    let (grant_registry, mut scope_manifests) =
        scopes::ScopeManifests::load(&manifest_dirs).into_grant_registry();

    // --- Audit pipeline: journald primary, file fallback ---
    let primary = omnibus_adapter_journald::JournaldAuditSink::new()
        .ok()
        .map(|sink| Arc::new(sink) as Arc<dyn omnibus_contracts::spi::audit_sink::AuditSink>);
    let audit_fallback = audit_fallback_path();
    // The audit state directory (file fallback + params_hash key) is
    // daemon-owned: no files grant may write it, whatever scope it names
    // (audit.md; the fs-xdg adapter denies the same directory as the second
    // layer, resolving it with the same XDG rule).
    scope_manifests.state_dirs = state_dirs_for(&audit_fallback);
    let audit_pipeline =
        audit::AuditPipeline::new(session_id, actor.clone(), primary, audit_fallback);
    if entropy_degraded {
        let _ = audit_pipeline
            .record(
                // Not a doctor event: the degradation happens during session
                // wiring, before any doctor run. scope_ref stays None — the
                // config|cli|doctor vocabulary is grant provenance, and this
                // is not a grant event.
                "core",
                "core",
                "entropy:degraded",
                &serde_json::json!({ "source": "clock-pid-fallback" }),
                "degraded",
                None,
            )
            .await;
    }

    // --- Adapters per the fresh probe (one active per subsystem, §9) ---
    let registry = build_registry(&platform, &scope_manifests.write).await;

    let core = Core::new(
        ctx,
        registry,
        grant_registry,
        scope_manifests,
        audit_pipeline,
        write_gate::WriteGate::system_default(),
        platform.clone(),
    );
    Ok((core, platform))
}

/// Emits one CLI/doctor audit event through the same pipeline the daemon uses
/// (journald primary, file fallback) — the CLI is not a second, weaker channel
/// (R5: grant issuances and doctor mutations reach the canonical audit stream).
/// The return value is delivery *health*: `true` when the primary sink accepted
/// the event or the file fallback did; `false` means the event was lost
/// everywhere and the caller must say so loudly (R5 audit totality) while
/// keeping the user-facing outcome unaffected.
pub(crate) async fn audit_cli_event(
    session: &str,
    client_name: &str,
    tool: &str,
    op: &str,
    arguments: &serde_json::Value,
    result: &str,
    scope_ref: Option<String>,
) -> bool {
    let primary = omnibus_adapter_journald::JournaldAuditSink::new()
        .ok()
        .map(|sink| Arc::new(sink) as Arc<dyn omnibus_contracts::spi::audit_sink::AuditSink>);
    let pipeline = audit::AuditPipeline::new(
        String::from(session),
        current_actor(Some(String::from(client_name))),
        primary,
        audit_fallback_path(),
    );
    let delivered = pipeline
        .record(tool, "core", op, arguments, result, scope_ref)
        .await;
    delivered || pipeline.fallback_healthy()
}

/// Fills the registry slots from the probe report; a failed connection leaves
/// the slot empty (the subsystem is honestly absent this session).
async fn build_registry(
    platform: &PlatformReport,
    write_scopes: &std::collections::BTreeMap<String, Vec<String>>,
) -> Registry {
    let mut registry = Registry::default();
    let alive = |name: &str| -> bool {
        platform
            .liveness
            .iter()
            .find(|l| l.name == name)
            .map(|l| l.alive)
            .unwrap_or(false)
    };
    let floor_met = |component: &str| -> bool {
        platform
            .floors
            .iter()
            .find(|f| f.component == component)
            .map(|f| f.met)
            .unwrap_or(false)
    };

    if alive("at-spi2")
        && floor_met("at-spi2")
        && let Ok(adapter) = omnibus_adapter_atspi::AtspiAdapter::connect().await
    {
        registry.a11y = Some(Arc::new(adapter));
    }
    if let Ok(adapter) = omnibus_adapter_journald::JournaldAdapter::detect().await {
        registry.journal = Some(Arc::new(adapter));
    }
    if let Ok(sink) = omnibus_adapter_journald::JournaldAuditSink::new() {
        registry.audit_sink = Some(Arc::new(sink));
    }
    if alive("systemd1")
        && floor_met("systemd")
        && let Ok(adapter) = omnibus_adapter_systemd1::Systemd1Adapter::connect().await
    {
        registry.services = Some(Arc::new(adapter));
    }
    if matches!(
        platform.distro.family,
        DistroFamily::Arch | DistroFamily::Manjaro
    ) && platform.backends_present.iter().any(|b| b == "pacman")
        && let Ok(adapter) = omnibus_adapter_alpm::AlpmAdapter::open()
    {
        registry.packages = Some(Arc::new(adapter));
    }
    if alive("xdg-desktop-portal")
        && let Ok(adapter) = omnibus_adapter_xdg_portals::PortalsAdapter::connect().await
    {
        registry.screen = Some(Arc::new(adapter.clone()));
        registry.notify = Some(Arc::new(adapter));
    }
    // fs-xdg: read scope is the default ($HOME); write paths come from the grants.
    registry.files = Some(Arc::new(omnibus_adapter_fs_xdg::FsXdgAdapter::new(
        granted_write_paths(write_scopes),
    )));
    // kb: ArchWiki + the Manjaro overlay when installed (task 2.5.2). Gated by
    // the same DistroFamily condition the probe applies in `adapters_selected`
    // (mirroring the alpm row of the platform matrix): off Arch/Manjaro the
    // probe claims the subsystem absent, so the registry must not install it —
    // whatever the probe did not select stays absent.
    if matches!(
        platform.distro.family,
        DistroFamily::Arch | DistroFamily::Manjaro
    ) {
        let overlay = kb_overlay_dir();
        registry.kb = Some(Arc::new(
            omnibus_adapter_kb_archwiki::KbArchWikiAdapter::new(kb_cache_dir(), overlay),
        ));
    }
    // The probe itself is always present.
    registry.probe = Some(Arc::new(omnibus_probe::Probe::system(Arc::new(
        omnibus_probe::runner::SystemRunner,
    ))));
    if alive("NetworkManager")
        && let Ok(adapter) = omnibus_adapter_nm::NmAdapter::connect().await
    {
        registry.net = Some(Arc::new(adapter));
    }
    registry
}

/// The fs write paths from the already-loaded grant manifests (defense-in-depth:
/// the core checks too; the adapter re-checks at the seam). Reading the same
/// in-memory manifests the core was built from — not a second disk read — keeps
/// both layers on one snapshot even if the store changes mid-start.
fn granted_write_paths(
    write_scopes: &std::collections::BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    write_scopes
        .get(omnibus_contracts::capabilities::FILES_SCOPE_WRITE)
        .cloned()
        .unwrap_or_default()
}

/// The state dirs the core's scope check must deny: the audit state directory
/// (the fallback JSONL plus the params key live next to it), derived from the
/// same fallback path the audit pipeline is built with (audit.md; the fs-xdg
/// adapter denies the same directory as the second layer).
fn state_dirs_for(fallback: &std::path::Path) -> Vec<String> {
    fallback
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| vec![dir.display().to_string()])
        .unwrap_or_default()
}

fn kb_cache_dir() -> PathBuf {
    xdg_dir("XDG_CACHE_HOME", ".cache").join("omnibus/kb")
}

fn kb_overlay_dir() -> Option<PathBuf> {
    // Packaged overlay first (task 2.5.2), then a user-local one.
    let candidates = [
        PathBuf::from("/usr/share/omnibus/kb-manjaro"),
        xdg_dir("XDG_DATA_HOME", ".local/share").join("omnibus/kb-manjaro"),
    ];
    candidates.into_iter().find(|p| p.is_dir())
}

pub fn audit_fallback_path() -> PathBuf {
    xdg_dir("XDG_STATE_HOME", ".local/state").join("omnibus/audit.jsonl")
}

/// The user config base for *grant writes*: the same XDG rule as the daemon
/// applies when loading (`XDG_CONFIG_HOME`, with an *empty* value treated as
/// unset — issue #53), but refusing the corner where no config base can be
/// resolved at all. With both `$HOME` and `$XDG_CONFIG_HOME` missing or empty,
/// the naive resolution is the root-anchored `/.config` — absolute, so a writer
/// would accept it, while the daemon's manifest loader (the core resolves the
/// same envs in `user_store_dir`) skips the user store entirely: the grant
/// would be written where no daemon will ever read it.
pub(crate) fn grant_config_dir() -> Result<PathBuf, String> {
    grant_config_dir_from(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// The checked rule, pure for testability: the user store is writable exactly
/// when some daemon would load it — a non-empty `XDG_CONFIG_HOME`, or a
/// non-empty `HOME` to anchor the `.config` fallback (the core's
/// `user_store_dir` rule, kept in lockstep; the `omnibus/scopes.d` tail is
/// appended by the caller).
fn grant_config_dir_from(
    xdg: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, String> {
    match xdg.filter(|value| !value.is_empty()) {
        Some(config) => Ok(PathBuf::from(config)),
        None => match home.filter(|value| !value.is_empty()) {
            Some(home) => Ok(std::path::Path::new(home).join(".config")),
            None => Err(String::from(
                "$XDG_CONFIG_HOME and $HOME are both unset (an empty value counts \
                 as unset), so the daemon would load no user store — refusing to \
                 write a grant it would never read; set $HOME and retry",
            )),
        },
    }
}

fn xdg_dir(var: &str, default_suffix: &str) -> PathBuf {
    xdg_dir_from(std::env::var_os(var).as_deref(), default_suffix)
}

/// The XDG rule, pure for testability: a *present but empty* value counts as
/// unset (never a CWD-relative store — issue #53), and so does a *relative*
/// value — every sibling surface is absolute-strict (`WriteGate::system_default`
/// filters a non-absolute `XDG_RUNTIME_DIR`, the manifest loader refuses
/// relative store dirs), and a relative `XDG_STATE_HOME` would put the audit
/// fallback and the per-install key under the launch directory, silently
/// churning keys across launch dirs; anything else is taken verbatim; the
/// fallback is `$HOME/<default_suffix>` (root-anchored when `$HOME` is missing
/// or empty — absolute, so nothing silently resolves against the CWD).
fn xdg_dir_from(value: Option<&std::ffi::OsStr>, default_suffix: &str) -> PathBuf {
    match value {
        Some(v) if !v.is_empty() && Path::new(v).is_absolute() => PathBuf::from(v),
        _ => home_dir().join(default_suffix),
    }
}

/// `$HOME`, with a *present but empty* value treated as unset — the exact rule
/// the core's scope loading applies to env values — because a verbatim
/// `Some("")` would make every fallback store CWD-relative and diverge from
/// `$HOME/...`. When HOME is truly unset the fallback is the filesystem root:
/// absolute, so nothing silently resolves against the CWD.
fn home_dir() -> PathBuf {
    home_dir_from(std::env::var_os("HOME").as_deref())
}

/// The `$HOME` rule, pure for testability: an empty value counts as unset
/// (never a CWD-relative store), a missing one falls back to the filesystem
/// root — absolute, so nothing resolves against the CWD.
fn home_dir_from(value: Option<&std::ffi::OsStr>) -> PathBuf {
    match value {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from("/"),
    }
}

/// 32 bytes from `/dev/urandom`. When the device cannot be opened or read
/// (minimal container, chroot, hardened sandbox), a degraded fallback mixes the
/// clock and the pid through splitmix64 so token minting still works — and the
/// caller is told: the second return value is `true` exactly when the fallback
/// ran, and `build_core` then surfaces the degradation on stderr and in the
/// audit trail (R3: degradation is never silent).
fn entropy_32() -> ([u8; 32], bool) {
    use std::io::Read;
    let mut bytes = [0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom")
        && f.read_exact(&mut bytes).is_ok()
    {
        return (bytes, false);
    }
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ (u64::from(std::process::id()) << 32);
    for b in bytes.iter_mut() {
        state = state
            .wrapping_add(0x9E37_79B9_7F4A_7C15)
            .wrapping_mul(0xBF58_476D_1CE4_E5B9);
        *b = (state >> 24) as u8;
    }
    (bytes, true)
}

/// The current actor: uid from /proc/self/status, pid from the process. On a
/// system without procfs the uid reads as 0 (root) — the least-wrong constant
/// for the `u32` DTO: the trail keeps attributing the events to the local
/// privileged daemon user rather than inventing a less-privileged identity.
pub fn current_actor(client_name: Option<String>) -> AuditActor {
    let uid = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("Uid:")
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|uid| uid.parse::<u32>().ok())
            })
        })
        .unwrap_or(0);
    AuditActor {
        uid,
        pid: std::process::id(),
        client_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entropy_32_returns_key_and_honest_flag() {
        let (key, degraded) = entropy_32();
        assert_eq!(key.len(), 32);
        assert!(key.iter().any(|b| *b != 0), "key is never all-zero");
        let _ = degraded; // environment-dependent; both values are legal
    }

    #[test]
    fn try_new_refuses_the_degenerate_mint_key() {
        // The contract-level guarantee build_core's startup refusal relies on:
        // an all-zero key never installs (every token it would mint is
        // forgeable), while a normal key builds the session.
        let actor = current_actor(None);
        assert!(session::SessionContext::try_new([0u8; 32], actor.clone()).is_err());
        assert!(session::SessionContext::try_new([1u8; 32], actor).is_ok());
    }

    #[test]
    fn home_dir_treats_empty_value_as_unset() {
        // A verbatim Some("") would leave every fallback store CWD-relative
        // (diverging from the core's scope rule, issue #53 family); it counts
        // as unset, and a truly missing HOME falls back to the absolute root.
        assert_eq!(
            home_dir_from(Some(std::ffi::OsStr::new(""))),
            PathBuf::from("/")
        );
        assert_eq!(home_dir_from(None), PathBuf::from("/"));
        assert_eq!(
            home_dir_from(Some(std::ffi::OsStr::new("/home/u"))),
            PathBuf::from("/home/u")
        );
    }

    #[test]
    fn xdg_dir_treats_empty_value_as_unset() {
        // Empty XDG_* must fall back to $HOME/<suffix>, never to a CWD-relative
        // path (the #53 divergence between CLI and daemon).
        let suffix = ".cache";
        assert_eq!(
            xdg_dir_from(Some(std::ffi::OsStr::new("")), suffix),
            home_dir().join(suffix)
        );
        assert_eq!(xdg_dir_from(None, suffix), home_dir().join(suffix));
        assert_eq!(
            xdg_dir_from(Some(std::ffi::OsStr::new("/custom/xdg")), suffix),
            PathBuf::from("/custom/xdg")
        );
        for dir in [
            xdg_dir_from(Some(std::ffi::OsStr::new("")), suffix),
            xdg_dir_from(None, suffix),
        ] {
            assert!(!dir.as_os_str().is_empty());
            assert!(dir.is_absolute(), "got {dir:?}");
        }
        // A relative XDG value is unset like an empty one: a store under the
        // launch directory would be CWD-dependent (key churn across launch
        // dirs), so it falls back to the home-anchored absolute path.
        assert_eq!(
            xdg_dir_from(Some(std::ffi::OsStr::new("state")), ".local/state"),
            home_dir().join(".local/state")
        );
    }

    #[test]
    fn state_dirs_pin_the_audit_state_directory() {
        let fallback = std::path::PathBuf::from("/home/u/.local/state/omnibus/audit.jsonl");
        assert_eq!(
            state_dirs_for(&fallback),
            vec![String::from("/home/u/.local/state/omnibus")]
        );
        // A parentless or bare-name fallback denies nothing: the deny anchors
        // on the daemon-owned directory, never on `/` or the empty string.
        assert!(state_dirs_for(std::path::Path::new("audit.jsonl")).is_empty());
    }

    #[test]
    fn grant_config_dir_refuses_when_no_home_is_resolvable() {
        // The both-unset corner: the daemon's manifest loader resolves no user
        // store at all, so the grant CLI must refuse to write one — never mint
        // the absolute-looking `/.config` store nothing will ever read.
        let Err(msg) = grant_config_dir_from(None, None) else {
            panic!("both envs unset must be refused");
        };
        assert!(msg.contains("$HOME"), "the refusal names the env: {msg}");
        // An empty value counts as unset by the same rule, on either variable.
        assert!(grant_config_dir_from(Some(std::ffi::OsStr::new("")), None).is_err());
        assert!(grant_config_dir_from(None, Some(std::ffi::OsStr::new(""))).is_err());
        assert!(
            grant_config_dir_from(
                Some(std::ffi::OsStr::new("")),
                Some(std::ffi::OsStr::new(""))
            )
            .is_err()
        );
    }

    #[test]
    fn grant_config_dir_matches_the_daemons_user_store_rule() {
        // XDG wins when present and non-empty; otherwise HOME anchors
        // `.config` — the exact paths the core's `user_store_dir` yields for
        // the same env values, so the CLI never writes a store the daemon
        // would skip.
        assert_eq!(
            grant_config_dir_from(
                Some(std::ffi::OsStr::new("/custom/xdg")),
                Some(std::ffi::OsStr::new("/home/u"))
            ),
            Ok(PathBuf::from("/custom/xdg"))
        );
        assert_eq!(
            grant_config_dir_from(None, Some(std::ffi::OsStr::new("/home/u"))),
            Ok(PathBuf::from("/home/u/.config"))
        );
        assert_eq!(
            grant_config_dir_from(
                Some(std::ffi::OsStr::new("")),
                Some(std::ffi::OsStr::new("/home/u"))
            ),
            Ok(PathBuf::from("/home/u/.config"))
        );
    }
}
