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
    // Family-conditional (the `family-alpm` feature): the alpm adapter links
    // the system libalpm, so builds without the feature (the cross-family T2
    // legs, the deb/rpm channels) wire no alpm row — the honest product shape
    // for a Debian/Fedora package, whose family backend is apt/dnf.
    #[cfg(feature = "family-alpm")]
    if matches!(
        platform.distro.family,
        DistroFamily::Arch | DistroFamily::Manjaro
    ) && platform.backends_present.iter().any(|b| b == "pacman")
        && let Ok(adapter) = omnibus_adapter_alpm::AlpmAdapter::open()
    {
        registry.packages = Some(Arc::new(adapter));
    }
    // The apt/dnf rows of the Packages column (roadmap 2.3.x): the native
    // family backend, attempted only when the ladder keeps the pkg family
    // visible — below the systemd floor the family's tools stay hidden (AC
    // 2.1.2: bullseye, systemd 247 < 252). A failed detection leaves the slot
    // empty: the subsystem is honestly absent this session.
    match pkg_candidate(platform) {
        Some(PkgBackend::Apt) => {
            if let Ok(adapter) = omnibus_adapter_apt::AptAdapter::detect().await {
                registry.packages = Some(Arc::new(adapter));
            }
        }
        Some(PkgBackend::Dnf) => {
            if let Ok(adapter) = omnibus_adapter_dnf::DnfAdapter::detect().await {
                registry.packages = Some(Arc::new(adapter));
            }
        }
        None => {}
    }
    // The store inventory (v1.5, roadmap 2.9.1): wired ALONGSIDE the native
    // manager whenever a store CLI is present — the one documented exception
    // to the one-active-adapter rule (§9, spi-registry.md; the spec text
    // lives in specs/contracts/core-adapters.md). The adapter is read-only by
    // contract: pkg_list/pkg_info aggregate it, writes never reach it. A
    // failed detection (neither store live at wire time) leaves the slot
    // empty and the aggregated output exactly the native manager's (R7).
    if inventory_candidate(platform)
        && let Ok(adapter) = omnibus_adapter_flatpak::FlatpakInventory::detect().await
    {
        registry.packages_inventory = Some(Arc::new(adapter));
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
    // kb: the Knowledge column of the platform matrix (task 2.5.2). Gated by
    // the same family condition the probe applies in `adapters_selected`
    // (mirroring the alpm row): off the promised rows the probe claims the
    // subsystem absent, so the registry must not install it — whatever the
    // probe did not select stays absent. The kb slot has no floor of its own
    // (the ladder hides pkg tools below the systemd floor; it says nothing
    // about reading distro docs).
    match kb_candidate(platform.distro.family) {
        Some(KbSource::ArchWiki) => {
            let overlay = kb_overlay_dir();
            registry.kb = Some(Arc::new(
                omnibus_adapter_kb_archwiki::KbArchWikiAdapter::new(kb_cache_dir(), overlay),
            ));
        }
        Some(KbSource::Guides) => {
            registry.kb = Some(Arc::new(omnibus_adapter_kb_guides::KbGuidesAdapter::new(
                kb_cache_dir(),
                kb_guides_dir(),
            )));
        }
        None => {}
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
    // App launch (v1.5, subsystem 11, roadmap 2.7.2): wired on every Linux
    // stand — the `.desktop` inventory is plain filesystem fact, and the
    // per-arm backend presence (systemd-run / xdg-open from the probe's
    // backends_present) decides which tools the ladder serves. A stand
    // without systemd-run gets no app.launch capability fact at all (both
    // write tools hide); a stand without xdg-open keeps app.launch while the
    // report's fact note names the hidden app.open — the R3 reason lives in
    // capabilities(), never in a log error.
    // App launch (v1.5, subsystem 11, roadmap 2.7.2): wired on every Linux
    // stand — the `.desktop` inventory is plain filesystem fact, and the
    // per-arm backend presence (systemd-run / xdg-open from the probe's
    // backends_present) decides which tools the ladder serves. A stand
    // without systemd-run gets no app.launch capability fact at all (both
    // write tools hide); a stand without xdg-open keeps app.launch while the
    // report's fact note names the hidden app.open — the R3 reason lives in
    // capabilities(), never in a log error.
    registry.app_launcher = Some(Arc::new(
        omnibus_adapter_launch::LaunchAdapter::from_backends(&platform.backends_present),
    ));
    // Process telemetry (v1.5, subsystem 12, roadmap 2.8.2/2.8.3): wired on
    // every Linux stand — /proc and sysfs carry no version floor, so the
    // read part activates everywhere. The UPower liveness row (probe
    // bus-name detection) decides the power arm per stand: without
    // UPower+battery the adapter emits no power.read fact (power.status
    // hides, the ps.read note names the reason — R3/R7, clean log).
    registry.process_telemetry = Some(Arc::new(omnibus_adapter_proc::ProcAdapter::from_probe(
        alive("UPower"),
    )));
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

/// The guides root for `kb-guides` (apt/dnf rows of the Knowledge column):
/// packaged root first, then a user-local one — the same overlay layout as
/// kb-manjaro, one article directory per registered distro doc source. `None`
/// means no packaged or local mirror exists; the adapter still serves cached
/// articles and reports the sources honestly.
fn kb_guides_dir() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("/usr/share/omnibus/kb-guides"),
        xdg_dir("XDG_DATA_HOME", ".local/share").join("omnibus/kb-guides"),
    ];
    candidates.into_iter().find(|p| p.is_dir())
}

/// The package backend the composition root attempts for this report's family,
/// decided before any I/O: the Packages column's family mapping plus the
/// ladder gate — below the systemd floor the pkg family's tools stay hidden
/// (AC 2.1.2: bookworm 252 is exactly on the floor and stays active; bullseye
/// 247 < 252 hides the family even with the backend binary present). Arch and
/// Manjaro are wired by the alpm row; `None` always means an empty slot.
fn pkg_candidate(platform: &PlatformReport) -> Option<PkgBackend> {
    let present = |name: &str| platform.backends_present.iter().any(|b| b == name);
    let systemd_floor_met = platform
        .floors
        .iter()
        .any(|f| f.component == "systemd" && f.met);
    match platform.distro.family {
        DistroFamily::Debian | DistroFamily::Ubuntu | DistroFamily::Mint
            if systemd_floor_met && present("apt") =>
        {
            Some(PkgBackend::Apt)
        }
        DistroFamily::Fedora if systemd_floor_met && present("dnf") => Some(PkgBackend::Dnf),
        _ => None,
    }
}

/// The pkg backends wired on the apt/dnf rows (one active adapter per
/// subsystem, §9; the native family wins when several backends coexist).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PkgBackend {
    Apt,
    Dnf,
}

/// Whether the store inventory row wires on this stand (v1.5, roadmap 2.9.1):
/// a flatpak or snap CLI in the probe's `backends_present` — a pure decision
/// made before any I/O, family-independent (a store can sit next to any
/// native manager, or alone on a stand with none). Whether a store actually
/// serves is decided inside the adapter's detection (per-store wire-time
/// liveness); a failed detection leaves the slot honestly empty (R7).
fn inventory_candidate(platform: &PlatformReport) -> bool {
    platform
        .backends_present
        .iter()
        .any(|backend| backend == "flatpak" || backend == "snap")
}

/// The Knowledge source the platform matrix promises for the family, decided
/// before any I/O: ArchWiki (+ the Manjaro overlay) on the Arch rows, the
/// distro-docs guides (Debian wiki, Ubuntu docs, Mint docs, Fedora docs) on
/// the apt/dnf rows, nothing on Other (R7: no promised-but-absent source).
fn kb_candidate(family: DistroFamily) -> Option<KbSource> {
    match family {
        DistroFamily::Arch | DistroFamily::Manjaro => Some(KbSource::ArchWiki),
        DistroFamily::Debian | DistroFamily::Ubuntu | DistroFamily::Mint | DistroFamily::Fedora => {
            Some(KbSource::Guides)
        }
        DistroFamily::Other => None,
    }
}

/// The knowledge providers wired per the matrix Knowledge column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KbSource {
    ArchWiki,
    Guides,
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

    // --- Fixture reports: the composition root's family decisions. The
    // selection logic is pure, so fixture reports pin it deterministically —
    // the toolchain probes behind the decisions (apt-get/dnf --version) stay
    // unmocked and only run on a real wire-up. ---

    fn fixture_report(
        family: DistroFamily,
        backends: &[&str],
        systemd_floor_met: bool,
    ) -> PlatformReport {
        PlatformReport {
            distro: omnibus_contracts::dto::probe::DistroInfo {
                id: String::from("fixture"),
                name: String::from("Fixture Linux"),
                release: String::new(),
                family,
            },
            kernel: String::from("test"),
            desktop: None,
            session_kind: None,
            systemd_version: systemd_floor_met.then(|| String::from("257")),
            atspi_version: None,
            liveness: vec![],
            backends_present: backends.iter().map(|b| String::from(*b)).collect(),
            floors: vec![omnibus_contracts::dto::probe::FloorStatus {
                component: String::from("systemd"),
                actual: None,
                floor: String::from("252.0"),
                met: systemd_floor_met,
                reason: None,
            }],
            adapters_selected: vec![],
            manjaro: None,
        }
    }

    #[test]
    fn pkg_candidate_selects_apt_and_dnf_on_their_families() {
        // Packages column: the apt family wires apt, Fedora wires dnf — with
        // the backend present and the systemd floor met.
        for family in [
            DistroFamily::Debian,
            DistroFamily::Ubuntu,
            DistroFamily::Mint,
        ] {
            let report = fixture_report(family, &["apt"], true);
            assert_eq!(pkg_candidate(&report), Some(PkgBackend::Apt), "{family:?}");
        }
        let fedora = fixture_report(DistroFamily::Fedora, &["dnf"], true);
        assert_eq!(pkg_candidate(&fedora), Some(PkgBackend::Dnf));
        // Several coexisting backends: the native family backend wins (§9).
        let mixed = fixture_report(DistroFamily::Mint, &["apt", "dnf", "pacman"], true);
        assert_eq!(pkg_candidate(&mixed), Some(PkgBackend::Apt));
    }

    #[test]
    fn pkg_candidate_stays_empty_without_backend_floor_or_family() {
        // Missing backend: no detection attempt, the slot stays honestly empty.
        assert_eq!(
            pkg_candidate(&fixture_report(DistroFamily::Debian, &[], true)),
            None
        );
        assert_eq!(
            pkg_candidate(&fixture_report(DistroFamily::Fedora, &["apt"], true)),
            None,
            "a foreign backend never wires the family slot"
        );
        // Below the systemd floor the pkg family stays hidden (AC 2.1.2:
        // bullseye, systemd 247 < 252) even with the backend present.
        assert_eq!(
            pkg_candidate(&fixture_report(DistroFamily::Debian, &["apt"], false)),
            None
        );
        assert_eq!(
            pkg_candidate(&fixture_report(DistroFamily::Fedora, &["dnf"], false)),
            None
        );
        // No floor row at all is never read as met.
        let mut no_floor_row = fixture_report(DistroFamily::Debian, &["apt"], true);
        no_floor_row.floors.clear();
        assert_eq!(pkg_candidate(&no_floor_row), None);
        // Arch/Manjaro belong to the alpm row; Other promises no backend (§9).
        assert_eq!(
            pkg_candidate(&fixture_report(
                DistroFamily::Arch,
                &["apt", "pacman"],
                true
            )),
            None
        );
        assert_eq!(
            pkg_candidate(&fixture_report(DistroFamily::Manjaro, &["apt"], true)),
            None
        );
        assert_eq!(
            pkg_candidate(&fixture_report(
                DistroFamily::Other,
                &["apt", "dnf", "pacman"],
                true
            )),
            None
        );
    }

    #[test]
    fn inventory_candidate_follows_the_store_cli_presence() {
        // v1.5 (roadmap 2.9.1): a store CLI wires the inventory on any
        // family — next to the native manager (the §9 exception) or alone.
        for family in [
            DistroFamily::Arch,
            DistroFamily::Manjaro,
            DistroFamily::Debian,
            DistroFamily::Ubuntu,
            DistroFamily::Mint,
            DistroFamily::Fedora,
            DistroFamily::Other,
        ] {
            assert!(
                inventory_candidate(&fixture_report(family, &["flatpak"], true)),
                "{family:?}: flatpak present wires the inventory"
            );
            assert!(
                inventory_candidate(&fixture_report(family, &["snap"], true)),
                "{family:?}: snap present wires the inventory"
            );
            assert!(
                !inventory_candidate(&fixture_report(family, &[], true)),
                "{family:?}: no store CLI, no inventory (plain absence, R7)"
            );
        }
        // The floor row is irrelevant to the inventory: reads carry no
        // version floor (platform-matrix.md), so the decision ignores it.
        assert!(inventory_candidate(&fixture_report(
            DistroFamily::Debian,
            &["flatpak"],
            false
        )));
    }

    #[test]
    fn kb_candidate_follows_the_knowledge_column() {
        assert_eq!(kb_candidate(DistroFamily::Arch), Some(KbSource::ArchWiki));
        assert_eq!(
            kb_candidate(DistroFamily::Manjaro),
            Some(KbSource::ArchWiki)
        );
        for family in [
            DistroFamily::Debian,
            DistroFamily::Ubuntu,
            DistroFamily::Mint,
            DistroFamily::Fedora,
        ] {
            assert_eq!(kb_candidate(family), Some(KbSource::Guides), "{family:?}");
        }
        assert_eq!(kb_candidate(DistroFamily::Other), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn build_registry_wires_kb_on_promised_rows_and_nothing_on_other() {
        // The registry-level view of the same decisions, through the real
        // build_registry: kb installs exactly on the promised rows (pure
        // construction), while the pkg slot stays empty on Other and below the
        // floor even with the backend binary present — detect is never
        // attempted for a hidden family (AC 2.1.2).
        let scopes = std::collections::BTreeMap::new();
        let other = build_registry(
            &fixture_report(DistroFamily::Other, &["apt", "dnf", "pacman"], true),
            &scopes,
        )
        .await;
        assert!(other.packages.is_none(), "Other wires no pkg backend");
        assert!(other.kb.is_none(), "Other wires no knowledge source");

        let bullseye = build_registry(
            &fixture_report(DistroFamily::Debian, &["apt"], false),
            &scopes,
        )
        .await;
        assert!(
            bullseye.packages.is_none(),
            "below the systemd floor the pkg family stays hidden"
        );
        assert!(bullseye.kb.is_some(), "knowledge is not floor-gated");

        for family in [
            DistroFamily::Debian,
            DistroFamily::Ubuntu,
            DistroFamily::Mint,
            DistroFamily::Fedora,
        ] {
            let registry = build_registry(&fixture_report(family, &[], false), &scopes).await;
            assert!(registry.kb.is_some(), "{family:?} promises its distro docs");
        }
        for family in [DistroFamily::Arch, DistroFamily::Manjaro] {
            let registry = build_registry(&fixture_report(family, &[], false), &scopes).await;
            assert!(registry.kb.is_some(), "{family:?} promises ArchWiki");
        }
    }
}
