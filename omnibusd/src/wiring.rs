//! The composition root: assembles probe → registry → core → facade from the
//! platform report. This binary is the only place adapter crates meet (R4-EDGES
//! keeps the core library adapter-free); whatever the probe did not select stays
//! absent — the capabilities document says so honestly.

use std::path::PathBuf;
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
    let ctx = session::SessionContext::new(entropy_32(), actor.clone());
    let session_id = ctx.id.as_str().to_string();

    // --- Grants from the scope manifests (provenance `config` at read time;
    // the issuing `omnibus grant` CLI event carries its own `cli` provenance) ---
    let manifest_dirs = scopes::ScopeManifests::default_dirs();
    let (grant_registry, scope_manifests) =
        scopes::ScopeManifests::load(&manifest_dirs).into_grant_registry();

    // --- Audit pipeline: journald primary, file fallback ---
    let primary = omnibus_adapter_journald::JournaldAuditSink::new()
        .ok()
        .map(|sink| Arc::new(sink) as Arc<dyn omnibus_contracts::spi::audit_sink::AuditSink>);
    let audit_pipeline =
        audit::AuditPipeline::new(session_id, actor.clone(), primary, audit_fallback_path());

    // --- Adapters per the fresh probe (one active per subsystem, §9) ---
    let registry = build_registry(&platform).await;

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

/// Fills the registry slots from the probe report; a failed connection leaves
/// the slot empty (the subsystem is honestly absent this session).
async fn build_registry(platform: &PlatformReport) -> Registry {
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
        granted_write_paths(platform),
    )));
    // kb: ArchWiki + the Manjaro overlay when installed (task 2.5.2).
    let overlay = kb_overlay_dir();
    registry.kb = Some(Arc::new(
        omnibus_adapter_kb_archwiki::KbArchWikiAdapter::new(kb_cache_dir(), overlay),
    ));
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

/// The fs write paths from the issued grants (defense-in-depth: the core checks
/// too; the adapter re-checks at the seam).
fn granted_write_paths(_platform: &PlatformReport) -> Vec<String> {
    let dirs = scopes::ScopeManifests::default_dirs();
    scopes::ScopeManifests::load(&dirs)
        .write
        .get(omnibus_contracts::capabilities::FILES_SCOPE_WRITE)
        .cloned()
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

fn xdg_dir(var: &str, default_suffix: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home_dir().join(default_suffix),
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// 32 bytes from /dev/urandom; a degraded fallback mixes the clock and the pid
/// through splitmix64 so token minting still works (entropy quality is weaker
/// and doctor surfaces the degradation).
fn entropy_32() -> [u8; 32] {
    use std::io::Read;
    let mut bytes = [0u8; 32];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom")
        && f.read_exact(&mut bytes).is_ok()
    {
        return bytes;
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
    bytes
}

/// The current actor: uid from /proc/self/status, pid from the process.
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
