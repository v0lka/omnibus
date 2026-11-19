#![cfg_attr(test, allow(clippy::expect_used))] // unit tests may expect; the seam must not
//! apt adapter: package list/info/search/install/remove/history for the
//! Debian/Ubuntu/Mint family (roadmap tasks 2.2.1 and 2.2.2).
//!
//! Native backend: the apt toolchain reached through its command-line
//! interface — `dpkg-query` for the installed set, `apt-cache` for the
//! available metadata, `apt-get` for transactions — and
//! `/var/log/apt/history.log` for history (the ADR-002-style CLI mapping; the
//! `PackageManager` contract and its suite are byte-identical to the alpm
//! family's, R2). Native shapes never leave this crate — mapping to DTOs
//! happens inside the adapter.
//!
//! Honest no-undo degradation (roadmap 2.2.2, ladder §7.3.1): apt has no
//! transactional rollback, so `pkg.undo` is reported with value `none` and
//! [`UNDO_UNAVAILABLE_NOTE`]; `pkg_undo` stays a hidden tool (the core ladder
//! hides it), a direct call answers `CapabilityRequired` with a doctor hint,
//! and the adapter's own `undo()` never reaches for a transaction — the
//! refusal happens BEFORE any native work, never mid-transaction.
//!
//! Every subprocess is bounded: the wall-clock budget kills a stalled child
//! (`kill_on_drop`), stdout capture stops at a hard byte cap. A write whose
//! budget elapses answers [`NF_TIMEOUT_INDETERMINATE`] with the uncertainty
//! payload — writes are uncancellable at the native layer, and the only
//! airtight claim is that the outcome is unknown. Transactions never prompt:
//! `-y` and `-o=Dpkg::Use-Pty=0` flags plus `DEBIAN_FRONTEND=noninteractive`
//! (pinned by tests on the non-tty contract).

mod history;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use omnibus_contracts::capabilities::{PKG_READ, PKG_UNDO, PKG_WRITE};
use omnibus_contracts::dto::packages::{
    PackageBrief, PackageHistoryEntry, PackageHistoryQuery, PackageInfo, PackageListRequest,
    PackageNameRequest, PackageSearch, TransactionResult, UndoResult,
};
use omnibus_contracts::dto::probe::{AdapterReport, CapabilityFact};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::packages::PackageManager;
use omnibus_contracts::version::CONTRACT_VERSION;
use tokio::io::AsyncReadExt;

/// `NativeFailure` code: generic apt toolchain failure (spawn, IO).
pub const NF_APT: i32 = 1;
/// `NativeFailure` code: the named package is neither installed nor available.
pub const NF_PKG_NOT_FOUND: i32 = 2;
/// `NativeFailure` code: a native invocation failed (the code space stays the
/// closed, documented NF_* set — the raw apt/dpkg exit status is preserved in
/// the payload text, `... failed with exit status 100: <stderr>`, never as the
/// machine-readable code a client would have to know apt's exit codes to
/// read).
pub const NF_TRANSACTION: i32 = 3;
/// `NativeFailure` code: the history log cannot be read.
pub const NF_LOG_READ: i32 = 4;
/// `NativeFailure` code: a write-path budget elapsed — the outcome is unknown
/// (the child is killed with the call, but a package change may already have
/// landed; see [`WRITE_TIMEOUT_INDETERMINATE_PAYLOAD`]).
pub const NF_TIMEOUT_INDETERMINATE: i32 = 5;

/// The honest write-path timeout payload (single source for tests): the
/// budget killed the child, but whether a package change landed is unknown —
/// re-check state instead of blindly retrying.
pub const WRITE_TIMEOUT_INDETERMINATE_PAYLOAD: &str = "timeout-indeterminate: write budget elapsed and the apt child was killed; \
     whether a package change lands natively is unknown — re-check the package state before retrying";

/// The honest no-undo note. The core surfaces this text in the
/// `pkg_install`/`pkg_remove` schema marking on apt machines (roadmap 2.2.2),
/// so the wording is the agent-visible single source.
pub const UNDO_UNAVAILABLE_NOTE: &str = "undo unavailable: apt has no transactional rollback (recovery = journal history + a manual fix plan)";

/// The `pkg.undo` ladder STATUS this adapter reports (roadmap 2.2.2): an
/// unrecognized ladder status resolves to absence, so `pkg_undo` stays hidden.
/// The typed VALUE is `PkgUndoValue::None` from the contracts enum.
pub const UNDO_STATUS_NONE: &str = "none";

/// The transaction environment: apt must never prompt (roadmap 2.2.2 pins
/// this pair in tests on the non-tty contract).
pub const NONINTERACTIVE_FRONTEND: (&str, &str) = ("DEBIAN_FRONTEND", "noninteractive");

/// Subprocess budget. It sits strictly below the core's `TOOL_CALL_BUDGET`
/// (10 s, `omnibus-core/src/limits.rs`) so the adapter's typed answers stay
/// reachable through the tool path (see the journald adapter for the full
/// equality-hazard note); reads map an elapse to `TimeoutError`, writes to
/// [`NF_TIMEOUT_INDETERMINATE`].
const DEFAULT_BUDGET: Duration = Duration::from_secs(9);
/// Hard cap on stdout kept in memory per call: no invocation can buffer an
/// unbounded tool output (a `dpkg-query -W` on a ~2000-package system fits
/// easily; the cap bounds pathological output instead of trusting it).
const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
/// Diagnostics cap for the concurrently drained stderr pipe.
const MAX_STDERR_BYTES: usize = 64 * 1024;
/// Upper bound on history.log bytes read per `history` call: the most recent
/// tail. The log grows without bound, so the read is capped at this documented
/// constant; entries older than the tail are invisible to history. 8 MiB
/// covers years of transactions.
const HISTORY_TAIL_BYTES: u64 = 8 * 1024 * 1024;

/// The toolchain programs and paths this adapter invokes.
#[derive(Debug, Clone)]
pub struct AptPrograms {
    /// `apt-get` (transactions).
    pub apt_get: String,
    /// `apt-cache` (policy/show/search/rdepends).
    pub apt_cache: String,
    /// `dpkg-query` (installed set).
    pub dpkg_query: String,
    /// `/var/log/apt/history.log` (history).
    pub history_log: PathBuf,
}

impl Default for AptPrograms {
    fn default() -> Self {
        Self {
            apt_get: String::from("apt-get"),
            apt_cache: String::from("apt-cache"),
            dpkg_query: String::from("dpkg-query"),
            history_log: PathBuf::from("/var/log/apt/history.log"),
        }
    }
}

/// The apt-backed `PackageManager` provider. Cheap to clone; every clone
/// shares the same immutable configuration.
#[derive(Debug, Clone)]
pub struct AptAdapter {
    inner: Arc<AptState>,
}

#[derive(Debug)]
struct AptState {
    programs: AptPrograms,
    budget: Duration,
}

impl AptAdapter {
    /// Detects the backend (composition-root entry): `apt-get --version` must
    /// run. Machines without apt answer `AdapterUnavailable` and the subsystem
    /// stays honestly absent.
    pub async fn detect() -> Result<Self, ContractError> {
        Self::detect_with(AptPrograms::default(), DEFAULT_BUDGET).await
    }

    /// [`detect`](Self::detect) over an explicit configuration (tests bind
    /// fixture toolchain scripts here; the probe itself stays the same bounded
    /// subprocess).
    pub async fn detect_with(
        programs: AptPrograms,
        budget: Duration,
    ) -> Result<Self, ContractError> {
        probe_program(&programs.apt_get, budget).await?;
        Ok(Self {
            inner: Arc::new(AptState { programs, budget }),
        })
    }

    /// Runs one bounded read subprocess.
    async fn run(&self, program: &str, args: &[String]) -> Result<CapturedOutput, ContractError> {
        capture_bounded(
            program,
            args,
            &[],
            self.inner.budget,
            ContractError::TimeoutError,
        )
        .await
    }

    /// Runs one bounded WRITE subprocess: an elapse answers the honest
    /// indeterminate outcome, never a plain timeout (the native change may
    /// already have landed).
    async fn run_write(
        &self,
        program: &str,
        args: &[String],
    ) -> Result<CapturedOutput, ContractError> {
        capture_bounded(
            program,
            args,
            &[NONINTERACTIVE_FRONTEND],
            self.inner.budget,
            ContractError::NativeFailure {
                code: NF_TIMEOUT_INDETERMINATE,
                payload: String::from(WRITE_TIMEOUT_INDETERMINATE_PAYLOAD),
            },
        )
        .await
    }
}

/// stdout captured under [`MAX_OUTPUT_BYTES`], with the honest truncation flag
/// (no read DTO carries a truncation channel today, so no caller consumes it —
/// the flag keeps the honest answer available instead of dropping the fact).
#[derive(Debug)]
struct CapturedOutput {
    stdout: Vec<u8>,
    #[allow(dead_code)]
    truncated: bool,
}

/// Runs `program` with `args` under `budget`: stdout is captured under
/// [`MAX_OUTPUT_BYTES`], stderr is drained under its own cap, and a timed-out
/// or overflowed child is killed (`kill_on_drop`). Every invocation in this
/// adapter goes through this one bounded path; `command.output()` would buffer
/// the child's whole stdout without bound and is never used. The environment
/// is inherited (the parent's PATH keeps resolving toolchain wrappers); write
/// invocations add [`NONINTERACTIVE_FRONTEND`].
/// Spawns the bounded child. A transient `ETXTBSY` is retried within a tiny
/// window: on Linux `execve` fails with "Text file busy" while ANY writer
/// still holds the binary open — under parallel test writers a just-written
/// fixture can hit it, and in production a running native apt/dpkg
/// transaction can touch the toolchain binary the same way. Two extra
/// attempts a millisecond apart keep the honest error mapping (`NotFound` →
/// `AdapterUnavailable`) without masking a genuinely broken backend: a third
/// failure answers `NativeFailure` with the original text.
async fn spawn_bounded(
    command: &mut tokio::process::Command,
    program: &str,
) -> Result<tokio::process::Child, ContractError> {
    let mut attempt = 0;
    loop {
        match command.spawn() {
            Ok(child) => return Ok(child),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ContractError::AdapterUnavailable);
            }
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < 2 => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(attempt)).await;
            }
            Err(e) => {
                return Err(ContractError::NativeFailure {
                    code: NF_APT,
                    payload: format!("spawn {program}: {e}"),
                });
            }
        }
    }
}

async fn capture_bounded(
    program: &str,
    args: &[String],
    envs: &[(&str, &str)],
    budget: Duration,
    on_timeout: ContractError,
) -> Result<CapturedOutput, ContractError> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A timed-out or overflowed child must die with the call.
        .kill_on_drop(true);
    let mut child = spawn_bounded(&mut command, program).await?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ContractError::NativeFailure {
            code: NF_APT,
            payload: format!("{program} spawned without a stdout pipe"),
        })?;
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(async move { drain_bounded(stderr).await }));

    let captured = tokio::time::timeout(budget, async move {
        let (mut stdout, truncated) =
            read_bounded(stdout, MAX_OUTPUT_BYTES)
                .await
                .map_err(|e: std::io::Error| ContractError::NativeFailure {
                    code: NF_APT,
                    payload: e.to_string(),
                })?;
        if truncated {
            // The cap fired: stop the child before reaping, or it would block
            // forever on its full stdout pipe. The induced SIGPIPE exit is our
            // action, not a native failure to report.
            let _ = child.kill().await; // an already-exited child is fine
        }
        let status =
            child
                .wait()
                .await
                .map_err(|e: std::io::Error| ContractError::NativeFailure {
                    code: NF_APT,
                    payload: e.to_string(),
                })?;
        let stderr = match stderr_task {
            Some(task) => task.await.unwrap_or_default(),
            None => Vec::new(),
        };
        if truncated {
            return Ok(CapturedOutput {
                stdout: std::mem::take(&mut stdout),
                truncated: true,
            });
        }
        if !status.success() {
            // Every nonzero exit maps to the declared NF_TRANSACTION code: the
            // NativeFailure code space is the closed NF_* set (the error
            // taxonomy treats adapter codes as a documented set), so raw apt/
            // dpkg exit codes never leak into it. The raw status still travels
            // — in the payload text, alongside the stderr diagnostics.
            let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
            let reason = match status.code() {
                Some(code) => format!("exit status {code}"),
                None => String::from("a signal"),
            };
            let payload = if stderr.is_empty() {
                format!("{program} {args:?} failed with {reason}")
            } else {
                format!("{program} failed with {reason}: {stderr}")
            };
            return Err(ContractError::NativeFailure {
                code: NF_TRANSACTION,
                payload,
            });
        }
        Ok(CapturedOutput {
            stdout: std::mem::take(&mut stdout),
            truncated: false,
        })
    })
    .await;

    // On budget elapse the inner future is dropped: the child goes with it
    // (`kill_on_drop`), its pipes close, the detached stderr task ends.
    captured.map_err(|_| on_timeout)?
}

/// Checks the backend program is runnable (`--version` under the budget);
/// a missing program is the honest `AdapterUnavailable`.
async fn probe_program(program: &str, budget: Duration) -> Result<(), ContractError> {
    let args = [String::from("--version")];
    capture_bounded(program, &args, &[], budget, ContractError::TimeoutError).await?;
    Ok(())
}

/// Reads at most `cap` bytes from the pipe; reports whether the cap cut it.
async fn read_bounded<R: tokio::io::AsyncRead + Unpin>(
    mut pipe: R,
    cap: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut buf = Vec::with_capacity(cap.min(64 * 1024));
    let mut chunk = [0_u8; 8192];
    loop {
        let read = pipe.read(&mut chunk).await?;
        if read == 0 {
            return Ok((buf, false));
        }
        let room = cap.saturating_sub(buf.len());
        if read > room {
            buf.extend_from_slice(&chunk[..room]);
            return Ok((buf, true));
        }
        buf.extend_from_slice(&chunk[..read]);
    }
}

/// Drains stderr under [`MAX_STDERR_BYTES`] so a chatty child can neither
/// wedge on a full pipe nor inflate the memory bound.
async fn drain_bounded<R: tokio::io::AsyncRead + Unpin>(mut pipe: R) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => return buf,
            Ok(read) => read,
        };
        let room = MAX_STDERR_BYTES.saturating_sub(buf.len());
        if read > room {
            buf.extend_from_slice(&chunk[..room]);
            return buf;
        }
        buf.extend_from_slice(&chunk[..read]);
    }
}

/// Reads at most the last `cap` bytes of `path`; a leading partial line left
/// by the tail seek is dropped (byte-identical semantics to the alpm reader).
/// A MISSING log is not a failure: a machine that never ran a transaction
/// (fresh install, disposable container) has no history log yet, and the read
/// answers an empty history — the same degradation the dnf history database
/// applies (R3: a missing auxiliary artifact degrades to the empty read; a
/// PRESENT log that cannot be read stays a `NativeFailure`).
fn read_log_tail(path: &Path, cap: u64) -> Result<Vec<u8>, ContractError> {
    use std::io::{Read, Seek, SeekFrom};
    let log_err = |what: &str, e: std::io::Error| ContractError::NativeFailure {
        code: NF_LOG_READ,
        payload: format!("log: cannot {what} {}: {e}", path.display()),
    };
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(log_err("open", e)),
    };
    let len = file.metadata().map_err(|e| log_err("stat", e))?.len();
    let skip = len.saturating_sub(cap);
    let mut buf = Vec::new();
    if skip == 0 {
        file.take(cap)
            .read_to_end(&mut buf)
            .map_err(|e| log_err("read", e))?;
        return Ok(buf);
    }
    let boundary = {
        file.seek(SeekFrom::Start(skip - 1))
            .map_err(|e| log_err("seek", e))?;
        let mut probe = [0_u8; 1];
        file.read_exact(&mut probe)
            .is_ok_and(|()| probe[0] == b'\n')
    };
    file.seek(SeekFrom::Start(skip))
        .map_err(|e| log_err("seek", e))?;
    // `take` hard-bounds the read even if the log grows between stat and read.
    file.take(cap)
        .read_to_end(&mut buf)
        .map_err(|e| log_err("read", e))?;
    if !boundary && let Some(pos) = buf.iter().position(|&byte| byte == b'\n') {
        buf.drain(..=pos);
    }
    Ok(buf)
}

/// Strips the `:arch` multiarch suffix from a `${binary:Package}` field.
fn strip_arch(name: &str) -> &str {
    name.split(':').next().unwrap_or(name)
}

/// Parses the `dpkg-query -W -f='${Status}\t${binary:Package}\t${Version}\n'`
/// table, keeping every INSTALLED package. dpkg's Status field is the triple
/// `<want> <flag> <status>`, and both installed wants count: `install ok
/// installed` and `hold ok installed` — a package pinned by `apt-mark hold`
/// (kernels, drivers) is installed all the same, and dropping it would make
/// `pkg_list` report an incomplete installed set that disagrees with
/// `pkg_info` about the same machine. Non-installed statuses (`config-files`,
/// `deinstall ok …`) keep their skip.
fn parse_installed_table(stdout: &[u8]) -> Vec<PackageBrief> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let status = fields.next()?.trim();
            let name = fields.next()?;
            let version = fields.next()?;
            let triple = status.split_whitespace().collect::<Vec<_>>();
            matches!(triple.as_slice(), ["install" | "hold", "ok", "installed"]).then(|| {
                PackageBrief {
                    name: String::from(strip_arch(name.trim())),
                    version: String::from(version.trim()),
                    description: None,
                    repo: None,
                    source: omnibus_contracts::dto::packages::PackageSource::Native,
                }
            })
        })
        .collect()
}

/// One `apt-cache policy` answer: installed + candidate + one repo source.
#[derive(Debug, Default, Clone)]
struct Policy {
    installed: Option<String>,
    candidate: Option<String>,
    repo: Option<String>,
}

/// Parses `apt-cache policy <name>` output.
fn parse_policy(stdout: &[u8]) -> Policy {
    let mut policy = Policy::default();
    for line in String::from_utf8_lossy(stdout).lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Installed:") {
            let v = rest.trim();
            policy.installed = (v != "(none)" && !v.is_empty()).then(|| String::from(v));
        } else if let Some(rest) = trimmed.strip_prefix("Candidate:") {
            let v = rest.trim();
            policy.candidate = (v != "(none)" && !v.is_empty()).then(|| String::from(v));
        } else if policy.repo.is_none() && trimmed.starts_with("500 ") {
            // A pinned source row: `500 http://… trixie/main amd64 Packages`.
            policy.repo = trimmed.split_whitespace().nth(1).map(String::from);
        }
    }
    policy
}

/// Parses the first stanza of `apt-cache show <name>` into the metadata triple
/// (description, depends, installed size in bytes). Only the exact
/// `Description:` field counts — `Description-md5:` and translations are not
/// summaries.
fn parse_show_stanza(stdout: &[u8]) -> (Option<String>, Vec<String>, Option<u64>) {
    let mut description = None;
    let mut depends: Vec<String> = Vec::new();
    let mut size_bytes = None;
    for line in String::from_utf8_lossy(stdout).lines() {
        if line.is_empty() {
            // Stanza separator: the first stanza ends here.
            if description.is_some() || !depends.is_empty() || size_bytes.is_some() {
                break;
            }
            continue;
        }
        if line.starts_with(' ') {
            // Continuation line (long description): not a field.
            continue;
        }
        if let Some(rest) = line.strip_prefix("Description:") {
            let value = rest.trim();
            if !value.is_empty() {
                description = Some(String::from(value));
            }
        } else if let Some(rest) = line.strip_prefix("Depends:") {
            depends = rest
                .split(", ")
                .map(|dep| dep.split('|').next().unwrap_or(dep).trim())
                .filter(|dep| !dep.is_empty())
                .map(|dep| match dep.find(" (") {
                    // Drop the version constraint: `libc6 (>= 2.34)` -> `libc6`.
                    Some(idx) => String::from(&dep[..idx]),
                    None => String::from(dep),
                })
                .collect();
        } else if let Some(rest) = line.strip_prefix("Installed-Size:")
            && let Ok(kib) = rest.trim().parse::<u64>()
        {
            size_bytes = Some(kib.saturating_mul(1024));
        }
    }
    (description, depends, size_bytes)
}

/// Parses `apt-cache rdepends --installed <name>` reverse-dependency names.
fn parse_rdepends(stdout: &[u8]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut in_section = false;
    for line in String::from_utf8_lossy(stdout).lines() {
        let trimmed = line.trim();
        if trimmed == "Reverse Depends:" {
            in_section = true;
            continue;
        }
        if in_section {
            if trimmed.is_empty() {
                break;
            }
            names.push(String::from(trimmed));
        }
    }
    names
}

/// Parses `apt-cache search` lines (`name — description` with an em dash, or
/// `name - description` with a plain hyphen-minus depending on locale data).
fn parse_search_lines(stdout: &[u8], limit: Option<u32>) -> Vec<PackageBrief> {
    let cap = limit.map_or(usize::MAX, |l| usize::try_from(l).unwrap_or(usize::MAX));
    let mut out: Vec<PackageBrief> = Vec::new();
    for line in String::from_utf8_lossy(stdout).lines() {
        if out.len() >= cap {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (name, description) = split_search_line(line);
        if name.is_empty() {
            continue;
        }
        out.push(PackageBrief {
            name: String::from(name),
            version: String::new(),
            description: description.map(String::from),
            repo: None,
            source: omnibus_contracts::dto::packages::PackageSource::Native,
        });
    }
    out
}

/// Splits one search line on its first ` — ` (em dash) or ` - ` separator.
fn split_search_line(line: &str) -> (&str, Option<&str>) {
    if let Some((name, desc)) = line.split_once(" — ") {
        return (name.trim(), Some(desc.trim()));
    }
    match line.split_once(" - ") {
        Some((name, desc)) => (name.trim(), Some(desc.trim())),
        None => (line.trim(), None),
    }
}

/// The dpkg package-name grammar (Debian policy): lowercase letters, digits,
/// `+`, `-`, `.`, starting with an alphanumeric. Anything else never reaches
/// the native argv: apt-get parses leading-dash elements as options, so an
/// unvalidated "package name" would change what the privileged command does —
/// `-f` is the fix-broken repair transaction with zero package names,
/// `-o=APT::Get::AllowUnauthenticated=true` downgrades verification, a
/// `/path/to.deb` turns the install into a local-deb installer.
fn is_deb_pkg_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '-' | '.'))
}

/// The honest refusal for a name outside the grammar: the same
/// [`NF_PKG_NOT_FOUND`] contract the unknown-package path answers (a name the
/// grammar rejects can never name a Debian package), raised BEFORE any native
/// work.
fn pkg_name_refused(name: &str) -> ContractError {
    ContractError::NativeFailure {
        code: NF_PKG_NOT_FOUND,
        payload: format!(
            "pkg-not-found: {name:?} is not a valid package name (expected the dpkg name grammar: \
             lowercase alphanumerics plus + - .)"
        ),
    }
}

/// Closes the one injection vector a search term has: a LEADING DASH makes
/// apt-cache parse the term as an option set instead of a query. Search terms
/// are otherwise free text on this contract (the alpm family matches
/// descriptions too), so the full package-name grammar does not apply here —
/// the option shape does. The refusal answers before any native work.
fn search_term_refused(query: &str) -> ContractError {
    ContractError::NativeFailure {
        code: NF_PKG_NOT_FOUND,
        payload: format!(
            "pkg-not-found: {query:?} is parsed as an option, not a search term (terms must not \
             start with '-')"
        ),
    }
}

/// Builds the non-interactive transaction argv (pinned by tests, roadmap
/// 2.2.2): options first (`-y`, pty suppressed), then the command and target.
fn transaction_args(command: &str, name: &str) -> [String; 4] {
    [
        String::from("-y"),
        String::from("-o=Dpkg::Use-Pty=0"),
        String::from(command),
        String::from(name),
    ]
}

/// Best-effort advisory version of `name` after a transaction (the DTO's
/// `version_applied` is audit-facing and explicitly best-effort).
async fn advisory_version(state: &AptState, name: &str) -> Option<String> {
    let args = [
        String::from("-W"),
        String::from("-f=${Version}\t${Status}\n"),
        String::from(name),
    ];
    let captured = capture_bounded(
        &state.programs.dpkg_query,
        &args,
        &[],
        state.budget,
        ContractError::TimeoutError,
    )
    .await
    .ok()?;
    let text = String::from_utf8_lossy(&captured.stdout);
    let mut fields = text.lines().next()?.split('\t');
    let version = fields.next()?.trim();
    let status = fields.next().unwrap_or("").trim();
    (status == "install ok installed" && !version.is_empty()).then(|| String::from(version))
}

#[async_trait]
impl PackageManager for AptAdapter {
    fn report(&self) -> AdapterReport {
        AdapterReport {
            adapter: String::from("apt"),
            subsystem: String::from("packages"),
            contract_version: String::from(CONTRACT_VERSION),
            capabilities: vec![
                CapabilityFact {
                    name: String::from(PKG_READ),
                    ver: String::from(CONTRACT_VERSION),
                    status: String::from("present"),
                    value: None,
                    note: None,
                    remedy: None,
                },
                CapabilityFact {
                    name: String::from(PKG_WRITE),
                    ver: String::from(CONTRACT_VERSION),
                    status: String::from("present"),
                    value: None,
                    note: None,
                    remedy: None,
                },
                CapabilityFact {
                    name: String::from(PKG_UNDO),
                    ver: String::from(CONTRACT_VERSION),
                    // Honest no-undo: an unrecognized ladder status resolves to
                    // absence in the core, so `pkg_undo` stays hidden and a
                    // direct call answers `CapabilityRequired` (roadmap 2.2.2).
                    status: String::from(UNDO_STATUS_NONE),
                    value: Some(omnibus_contracts::capabilities::PkgUndoValue::None.to_json()),
                    note: Some(String::from(UNDO_UNAVAILABLE_NOTE)),
                    remedy: None,
                },
            ],
            notes: vec![String::from(
                "history from /var/log/apt/history.log; install/remove via apt-get, never interactive",
            )],
        }
    }

    async fn list(&self, request: PackageListRequest) -> Result<Vec<PackageBrief>, ContractError> {
        let args = [
            String::from("-W"),
            String::from("-f=${Status}\t${binary:Package}\t${Version}\n"),
        ];
        let captured = self.run(&self.inner.programs.dpkg_query, &args).await?;
        let mut briefs = parse_installed_table(&captured.stdout);
        if let Some(limit) = request.limit {
            let keep = usize::try_from(limit).unwrap_or(usize::MAX);
            briefs.truncate(keep);
        }
        Ok(briefs)
    }

    async fn info(&self, name: String) -> Result<PackageInfo, ContractError> {
        if !is_deb_pkg_name(&name) {
            return Err(pkg_name_refused(&name));
        }
        let policy_args = [String::from("policy"), name.clone()];
        let captured = self
            .run(&self.inner.programs.apt_cache, &policy_args)
            .await?;
        let policy = parse_policy(&captured.stdout);
        if policy.installed.is_none() && policy.candidate.is_none() {
            return Err(ContractError::NativeFailure {
                code: NF_PKG_NOT_FOUND,
                payload: format!("pkg-not-found: {name} is neither installed nor available"),
            });
        }
        let show_args = [String::from("show"), name.clone()];
        let (description, depends, size_bytes) =
            match self.run(&self.inner.programs.apt_cache, &show_args).await {
                Ok(captured) => parse_show_stanza(&captured.stdout),
                Err(ContractError::NativeFailure { .. }) => (None, Vec::new(), None),
                Err(other) => return Err(other),
            };
        let rdepends_args = [
            String::from("rdepends"),
            String::from("--installed"),
            name.clone(),
        ];
        let required_by = match self
            .run(&self.inner.programs.apt_cache, &rdepends_args)
            .await
        {
            Ok(captured) => parse_rdepends(&captured.stdout),
            _ => Vec::new(),
        };
        // `version_available` is honest only when it differs from what is
        // installed (an identical candidate is not an "available upgrade").
        let version_available = match (&policy.installed, &policy.candidate) {
            (Some(installed), Some(candidate)) if installed == candidate => None,
            (_, Some(candidate)) => Some(candidate.clone()),
            _ => None,
        };
        Ok(PackageInfo {
            version: policy.installed.or(policy.candidate).unwrap_or_default(),
            name,
            description,
            repo: policy.repo,
            size_bytes,
            install_reason: None,
            depends,
            required_by,
            version_available,
        })
    }

    async fn search(&self, query: PackageSearch) -> Result<Vec<PackageBrief>, ContractError> {
        if query.query.trim_start().starts_with('-') {
            return Err(search_term_refused(&query.query));
        }
        let args = [String::from("search"), query.query];
        let captured = self.run(&self.inner.programs.apt_cache, &args).await?;
        Ok(parse_search_lines(&captured.stdout, query.limit))
    }

    async fn install(
        &self,
        request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError> {
        // The name is the last element of a privileged apt-get argv: the
        // grammar gate comes first, before any native work (R5/R6 — the
        // granted call may never name an option set instead of a package).
        if !is_deb_pkg_name(&request.name) {
            return Err(pkg_name_refused(&request.name));
        }
        let args = transaction_args("install", &request.name);
        self.run_write(&self.inner.programs.apt_get, &args).await?;
        let version_applied = advisory_version(&self.inner, &request.name).await;
        Ok(TransactionResult {
            name: request.name,
            version_applied,
        })
    }

    async fn remove(
        &self,
        request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError> {
        if !is_deb_pkg_name(&request.name) {
            return Err(pkg_name_refused(&request.name));
        }
        let args = transaction_args("remove", &request.name);
        self.run_write(&self.inner.programs.apt_get, &args).await?;
        let version_applied = advisory_version(&self.inner, &request.name).await;
        Ok(TransactionResult {
            name: request.name,
            version_applied,
        })
    }

    async fn history(
        &self,
        query: PackageHistoryQuery,
    ) -> Result<Vec<PackageHistoryEntry>, ContractError> {
        let bytes = read_log_tail(&self.inner.programs.history_log, HISTORY_TAIL_BYTES)?;
        Ok(history::parse(&bytes, query.name.as_deref(), query.limit))
    }

    /// Honest no-undo (roadmap 2.2.2): the refusal happens BEFORE any native
    /// work — `CapabilityRequired`, never a half-applied transaction and never
    /// an error discovered mid-flight.
    async fn undo(&self, _request: PackageNameRequest) -> Result<UndoResult, ContractError> {
        Err(ContractError::CapabilityRequired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_contracts::dto::packages::PackageAction;
    use omnibus_contracts::suite::families::{SENTINEL_MISSING, SENTINEL_POISON, SENTINEL_SLOW};
    use omnibus_contracts::suite::packages::PackagesFixture;
    use std::os::unix::fs::PermissionsExt;

    // ------------------------------------------------------------------
    // Fixture toolchain: tiny POSIX-sh scripts standing in for apt-get /
    // apt-cache / dpkg-query, so the REAL adapter code paths (bounded spawn,
    // parse, error mapping) run hermetically on any machine. The scripts
    // double as executable assertions: a transaction without the non-tty
    // contract flags is REFUSED by the fixture, so a passing install proves
    // the flags were sent (roadmap 2.2.2, pinned on non-tty).
    // ------------------------------------------------------------------

    fn write_fixture(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("fixture write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("fixture chmod");
        path.to_string_lossy().into_owned()
    }

    fn write_fixtures(dir: &Path, history_body: &str) -> AptPrograms {
        let history_path = dir.join("history.log");
        std::fs::write(&history_path, history_body).expect("history fixture write");

        // apt-get: refuses any transaction missing -y / Use-Pty=0, refuses a
        // foreign DEBIAN_FRONTEND, fails the poisoned name with exit 100
        // (stderr preserved), stalls the slow name past any test budget.
        let apt_get = write_fixture(
            dir,
            "apt-get",
            "#!/bin/sh\n\
             has_y=0; has_pty=0\n\
             for arg in \"$@\"; do\n\
               [ \"$arg\" = \"-y\" ] && has_y=1\n\
               case \"$arg\" in -o=Dpkg::Use-Pty=0) has_pty=1 ;; esac\n\
             done\n\
             if [ \"$1\" = \"--version\" ]; then echo \"apt 3.0.0 fixture\"; exit 0; fi\n\
             if [ -n \"$DEBIAN_FRONTEND\" ] && [ \"$DEBIAN_FRONTEND\" != \"noninteractive\" ]; then\n\
               echo \"interactive frontend: $DEBIAN_FRONTEND\" >&2; exit 96\n\
             fi\n\
             if [ \"$has_y\" != \"1\" ] || [ \"$has_pty\" != \"1\" ]; then\n\
               echo \"non-tty contract violated (no -y / Use-Pty=0)\" >&2; exit 95\n\
             fi\n\
             target=\n\
             for last in \"$@\"; do target=\"$last\"; done\n\
             if [ \"$target\" = \"omnibus-suite-poison\" ]; then\n\
               echo \"E: Unable to locate package $target\" >&2; exit 100\n\
             fi\n\
             if [ \"$target\" = \"omnibus-suite-slow\" ]; then sleep 30; fi\n\
             echo \"Setting up $target ...\"\n\
             exit 0\n",
        );

        // apt-cache: policy/show/search/rdepends canned answers.
        let apt_cache = write_fixture(
            dir,
            "apt-cache",
            "#!/bin/sh\n\
             cmd=\"$1\"; shift\n\
             name=\"$1\"\n\
             case \"$name\" in --*) name=\"$2\" ;; esac\n\
             if [ \"$cmd\" = \"policy\" ]; then\n\
               if [ \"$name\" = \"omnibus-suite-missing\" ]; then\n\
                 echo \"$name:\"; echo \"  Installed: (none)\"; echo \"  Candidate: (none)\"; exit 0\n\
               fi\n\
               if [ \"$name\" = \"omnibus-suite-slow\" ]; then sleep 30; fi\n\
               if [ \"$name\" = \"bash\" ]; then\n\
                 echo \"bash:\"; echo \"  Installed: 5.2.15-2\"; echo \"  Candidate: 5.2.15-2\"\n\
               else\n\
                 echo \"$name:\"; echo \"  Installed: (none)\"; echo \"  Candidate: 1.4.0-1\"\n\
               fi\n\
               echo \"  Version table:\"; echo \" 500 http://deb.debian.org/debian trixie/main amd64 Packages\"\n\
               exit 0\n\
             fi\n\
             if [ \"$cmd\" = \"show\" ]; then\n\
               if [ \"$name\" = \"foo\" ]; then\n\
                 echo \"Package: foo\"; echo \"Version: 1.4.0-1\"; echo \"Installed-Size: 1024\"\n\
                 echo \"Depends: libfoo1 (>= 1.2), libc6 (>= 2.34) | libc6-alt\"\n\
                 echo \"Description: fixture package for tests\"\n\
                 echo \"Description-md5: 0000fix0000\"\n\
                 echo \" long description line stays out of the summary\"\n\
               fi\n\
               exit 0\n\
             fi\n\
             if [ \"$cmd\" = \"search\" ]; then\n\
               echo \"foo — fixture package for tests\"\n\
               echo \"foobar - another fixture match\"\n\
               exit 0\n\
             fi\n\
             if [ \"$cmd\" = \"rdepends\" ]; then\n\
               echo \"$name\"; echo \"Reverse Depends:\"; echo \"  bar\"; echo \"  baz\"; exit 0\n\
             fi\n\
             exit 0\n",
        );

        // dpkg-query: the 2-arg shape prints the installed table; the named
        // shape answers only `bash`.
        let dpkg_query = write_fixture(
            dir,
            "dpkg-query",
            "#!/bin/sh\n\
             if [ \"$#\" -ge 3 ]; then\n\
               name=\"$3\"\n\
               if [ \"$name\" = \"bash\" ]; then\n\
                 printf '5.2.15-2\\tinstall ok installed\\n'; exit 0\n\
               fi\n\
               exit 1\n\
             fi\n\
             printf 'install ok installed\\tbash\\t5.2.15-2\\n'\n\
             printf 'install ok installed\\thtop\\t3.3.0-1\\n'\n\
             printf 'hold ok installed\\tlinux-headers-amd64\\t6.1.0-27\\n'\n\
             printf 'deinstall ok config-files\\toldpkg\\t1.0-1\\n'\n",
        );

        AptPrograms {
            apt_get,
            apt_cache,
            dpkg_query,
            history_log: history_path,
        }
    }

    const FIXTURE_HISTORY: &str = "\
Start-Date: 2024-05-12  10:23:45
Commandline: apt-get install -y foo
Install: libfoo1:amd64 (1.2.3-1, automatic), foo:amd64 (1.2.3-1)
End-Date: 2024-05-12  10:23:47

Start-Date: 2024-06-01  08:00:02
Commandline: apt-get remove -y baz
Upgrade: bar:amd64 (2.0-1, 2.1-1)
Remove: baz:amd64 (3.3-1)
End-Date: 2024-06-01  08:00:09
";

    async fn fixture_adapter(dir: &Path) -> AptAdapter {
        let programs = write_fixtures(dir, FIXTURE_HISTORY);
        AptAdapter::detect_with(programs, Duration::from_secs(9))
            .await
            .expect("fixture adapter must detect")
    }

    fn fixture_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "omnibus-apt-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("fixture dir");
        dir
    }

    // --------------------------- pure parsers ---------------------------

    #[test]
    fn installed_table_keeps_only_installed_rows_and_strips_arch() {
        let raw = b"install ok installed\tbash:amd64\t5.2.15-2\n\
                    install ok installed\tlibfoo1:i386\t1.2.3-1\n\
                    deinstall ok config-files\toldpkg:amd64\t1.0-1\n\
                    broken-line\n";
        let briefs = parse_installed_table(raw);
        let names: Vec<&str> = briefs.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["bash", "libfoo1"]);
        assert_eq!(briefs[0].version, "5.2.15-2");
    }

    #[test]
    fn installed_table_keeps_held_packages() {
        // dpkg's Status triple is `<want> <flag> <status>`: `apt-mark hold`
        // (kernels, drivers) installs with want = `hold`, and dropping that
        // row would make pkg_list hide exactly the packages a debugging agent
        // needs to see first. The regression is pinned red/green here AND in
        // the fixture table (list_reads_the_fixture_table).
        let raw = b"hold ok installed\tlinux-headers-amd64\t6.1.0-27\n\
                    hold ok installed\tlibfoo1:i386\t1.2.3-1\n\
                    install ok config-files\tgone:amd64\t1.0-1\n\
                    hold ok reinst-required\tbroken:amd64\t2.0-1\n";
        let briefs = parse_installed_table(raw);
        let names: Vec<&str> = briefs.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["linux-headers-amd64", "libfoo1"],
            "hold + installed stays; non-installed statuses stay out"
        );
        assert_eq!(briefs[0].version, "6.1.0-27");
    }

    #[test]
    fn policy_parses_installed_candidate_and_repo() {
        let raw = b"foo:\n  Installed: (none)\n  Candidate: 1.4.0-1\n  Version table:\n     1.4.0-1 500\n        500 http://deb.debian.org/debian trixie/main amd64 Packages\n";
        let policy = parse_policy(raw);
        assert_eq!(policy.installed, None);
        assert_eq!(policy.candidate.as_deref(), Some("1.4.0-1"));
        assert_eq!(policy.repo.as_deref(), Some("http://deb.debian.org/debian"));
        let none_values = parse_policy(b"x:\n  Installed: (none)\n  Candidate: (none)\n");
        assert_eq!(none_values.installed, None);
        assert_eq!(none_values.candidate, None);
    }

    #[test]
    fn show_stanza_parses_description_depends_and_size() {
        let raw = b"Package: foo\nVersion: 1.4.0-1\nInstalled-Size: 1024\nDepends: libfoo1 (>= 1.2), libc6 (>= 2.34) | libc6-alt\nDescription: fixture package for tests\nDescription-md5: 0000deadbeef0000\n long description line\n\nPackage: other\n";
        let (description, depends, size_bytes) = parse_show_stanza(raw);
        assert_eq!(description.as_deref(), Some("fixture package for tests"));
        assert_eq!(
            depends,
            vec![String::from("libfoo1"), String::from("libc6")]
        );
        assert_eq!(size_bytes, Some(1024 * 1024));
    }

    #[test]
    fn search_lines_parse_both_dash_shapes() {
        let raw = b"foo \xE2\x80\x94 fixture package for tests\nfoobar - another fixture match\n\nstray\n";
        let briefs = parse_search_lines(raw, Some(10));
        assert_eq!(briefs.len(), 3);
        assert_eq!(
            briefs[0].description.as_deref(),
            Some("fixture package for tests")
        );
        assert_eq!(
            briefs[1].description.as_deref(),
            Some("another fixture match")
        );
        let limited = parse_search_lines(raw, Some(1));
        assert_eq!(limited.len(), 1);
    }

    #[test]
    fn transaction_args_are_non_tty() {
        // Roadmap 2.2.2: the flags are pinned. Options precede the command.
        for command in ["install", "remove"] {
            let args = transaction_args(command, "bash");
            assert_eq!(&args[0], "-y", "transactions must be pre-approved");
            assert_eq!(&args[1], "-o=Dpkg::Use-Pty=0", "no pty on transactions");
            assert_eq!(&args[2], command);
            assert_eq!(&args[3], "bash");
        }
        assert_eq!(NONINTERACTIVE_FRONTEND.0, "DEBIAN_FRONTEND");
        assert_eq!(NONINTERACTIVE_FRONTEND.1, "noninteractive");
    }

    // ------------------------ fixture-backend legs ------------------------

    #[tokio::test]
    async fn contract_suite_passes_on_the_fixture_backend() {
        let dir = fixture_dir("suite");
        let adapter = fixture_adapter(&dir).await;
        let report = omnibus_contracts::suite::packages::packages_suite(
            || Box::new(adapter.clone()),
            PackagesFixture {
                undo_expected: false,
            },
        )
        .await;
        assert!(report.is_clean(), "{report}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn report_declares_honest_no_undo() {
        let dir = fixture_dir("report");
        let adapter = fixture_adapter(&dir).await;
        let report = adapter.report();
        assert_eq!(report.adapter, "apt");
        assert_eq!(report.subsystem, "packages");
        let undo = report
            .capabilities
            .iter()
            .find(|c| c.name == PKG_UNDO)
            .expect("pkg.undo fact present");
        assert_eq!(undo.status, "none");
        assert_eq!(
            undo.value,
            Some(omnibus_contracts::capabilities::PkgUndoValue::None.to_json())
        );
        assert!(
            undo.note
                .as_deref()
                .is_some_and(|n| n.contains("undo unavailable")),
            "the note is the agent-visible no-undo marking"
        );
        assert!(report.capabilities.iter().any(|c| c.name == PKG_READ));
        assert!(report.capabilities.iter().any(|c| c.name == PKG_WRITE));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn list_reads_the_fixture_table() {
        let dir = fixture_dir("list");
        let adapter = fixture_adapter(&dir).await;
        let all = adapter
            .list(PackageListRequest { limit: None })
            .await
            .expect("list must read the fixture table");
        let names: Vec<&str> = all.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["bash", "htop", "linux-headers-amd64"],
            "config-files rows stay out; a HELD package (hold ok installed) stays in — \
             pkg_list must agree with pkg_info about the installed set"
        );
        let limited = adapter
            .list(PackageListRequest { limit: Some(1) })
            .await
            .expect("limited list");
        assert_eq!(limited.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn info_assembles_installed_and_available_metadata() {
        let dir = fixture_dir("info");
        let adapter = fixture_adapter(&dir).await;
        let bash = adapter
            .info(String::from("bash"))
            .await
            .expect("installed info");
        assert_eq!(bash.version, "5.2.15-2");
        assert_eq!(bash.repo.as_deref(), Some("http://deb.debian.org/debian"));
        assert_eq!(
            bash.required_by,
            vec![String::from("bar"), String::from("baz")]
        );
        assert_eq!(
            bash.version_available, None,
            "an identical candidate is not an available upgrade"
        );
        let foo = adapter
            .info(String::from("foo"))
            .await
            .expect("available info");
        assert_eq!(
            foo.version, "1.4.0-1",
            "not installed: the candidate IS the version"
        );
        assert_eq!(
            foo.description.as_deref(),
            Some("fixture package for tests")
        );
        assert_eq!(
            foo.depends,
            vec![String::from("libfoo1"), String::from("libc6")]
        );
        assert_eq!(foo.size_bytes, Some(1024 * 1024));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn unknown_package_is_an_honest_pkg_not_found() {
        let dir = fixture_dir("missing");
        let adapter = fixture_adapter(&dir).await;
        let missing = adapter.info(String::from(SENTINEL_MISSING)).await;
        match missing {
            Err(ContractError::NativeFailure { code, payload }) => {
                assert_eq!(code, NF_PKG_NOT_FOUND);
                assert!(payload.contains("pkg-not-found"));
                assert!(payload.contains(SENTINEL_MISSING));
            }
            other => panic!("expected pkg-not-found, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn install_and_remove_refuse_interactive_backends() {
        // The fixture script exits 95 when the non-tty flags are missing, so a
        // passing transaction proves the flags and env were sent (roadmap
        // 2.2.2: the non-tty contract is pinned by a test, not by docs).
        let dir = fixture_dir("write");
        let adapter = fixture_adapter(&dir).await;
        let installed = adapter
            .install(PackageNameRequest {
                name: String::from("bash"),
            })
            .await
            .expect("fixture install must succeed with the flags");
        assert_eq!(installed.name, "bash");
        assert_eq!(installed.version_applied.as_deref(), Some("5.2.15-2"));
        let removed = adapter
            .remove(PackageNameRequest {
                name: String::from("htop"),
            })
            .await
            .expect("fixture remove must succeed with the flags");
        assert_eq!(removed.name, "htop");
        assert_eq!(
            removed.version_applied, None,
            "removed: dpkg-query answers nothing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn poisoned_transaction_maps_to_native_failure() {
        let dir = fixture_dir("poison");
        let adapter = fixture_adapter(&dir).await;
        let denied = adapter
            .install(PackageNameRequest {
                name: String::from(SENTINEL_POISON),
            })
            .await;
        match denied {
            Err(ContractError::NativeFailure { code, payload }) => {
                // The NativeFailure code space stays the closed NF_* set: the
                // raw apt exit status travels in the payload, never as the
                // machine-readable code.
                assert_eq!(code, NF_TRANSACTION, "the code space stays closed");
                assert!(
                    payload.contains("exit status 100"),
                    "the raw status is preserved in the payload: {payload}"
                );
                assert!(payload.contains("Unable to locate package"));
            }
            other => panic!("expected an honest native failure, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn option_like_names_never_reach_the_native_argv() {
        // Argument-injection pin (R5/R6): a crafted "package name" is refused
        // with the honest NF_PKG_NOT_FOUND BEFORE any native work. The fixture
        // would echo success for any non-poison target, so an `Ok` here would
        // mean the string reached the apt-get argv — `-f` is the classic
        // fix-broken repair transaction, `-o=…` downgrades verification, a
        // path turns the install into a local-deb installer.
        let dir = fixture_dir("injection");
        let adapter = fixture_adapter(&dir).await;
        for hostile in [
            "-f",
            "-y",
            "-o=APT::Get::AllowUnauthenticated=true",
            "/tmp/evil.deb",
        ] {
            let write = adapter
                .install(PackageNameRequest {
                    name: String::from(hostile),
                })
                .await;
            match write {
                Err(ContractError::NativeFailure { code, payload }) => {
                    assert_eq!(code, NF_PKG_NOT_FOUND, "refused for {hostile}");
                    assert!(payload.contains(hostile), "payload names the input");
                }
                other => panic!("expected the honest refusal for {hostile:?}, got {other:?}"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn read_paths_refuse_option_shaped_input() {
        let dir = fixture_dir("injection-read");
        let adapter = fixture_adapter(&dir).await;
        // info: a leading-dash "name" would be parsed as an option set.
        let info = adapter.info(String::from("--force-yes")).await;
        match info {
            Err(ContractError::NativeFailure { code, payload }) => {
                assert_eq!(code, NF_PKG_NOT_FOUND);
                assert!(payload.contains("--force-yes"));
            }
            other => panic!("expected the honest info refusal, got {other:?}"),
        }
        // search: terms are free text, but a leading dash is the option
        // injection vector — refused before any apt-cache spawn.
        let search = adapter
            .search(PackageSearch {
                query: String::from("-o=APT::Get::AllowUnauthenticated=true"),
                limit: None,
            })
            .await;
        match search {
            Err(ContractError::NativeFailure { code, payload }) => {
                assert_eq!(code, NF_PKG_NOT_FOUND);
                assert!(payload.contains("-o="));
            }
            other => panic!("expected the honest search refusal, got {other:?}"),
        }
        // Free-text terms (no leading dash) still flow through unchanged.
        let found = adapter
            .search(PackageSearch {
                query: String::from("fixture"),
                limit: Some(5),
            })
            .await
            .expect("a plain search term is not refused");
        assert!(!found.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn stalled_write_answers_timeout_indeterminate() {
        let dir = fixture_dir("slow");
        let adapter = fixture_adapter(&dir).await;
        tokio::time::pause();
        let task = tokio::spawn(async move {
            adapter
                .install(PackageNameRequest {
                    name: String::from(SENTINEL_SLOW),
                })
                .await
        });
        // Past the write budget on the paused clock; the fixture child keeps
        // sleeping in real time and is killed with the call.
        tokio::time::advance(Duration::from_secs(30)).await;
        let result = task.await.expect("join");
        match result {
            Err(ContractError::NativeFailure { code, payload }) => {
                assert_eq!(code, NF_TIMEOUT_INDETERMINATE);
                assert_eq!(payload, WRITE_TIMEOUT_INDETERMINATE_PAYLOAD);
            }
            other => panic!("expected timeout-indeterminate, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn undo_refuses_before_any_native_work() {
        let dir = fixture_dir("undo");
        let adapter = fixture_adapter(&dir).await;
        let refused = adapter
            .undo(PackageNameRequest {
                name: String::from("bash"),
            })
            .await;
        assert!(
            matches!(refused, Err(ContractError::CapabilityRequired)),
            "apt undo is a CapabilityRequired refusal, never a transaction"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn history_reads_the_fixture_log() {
        let dir = fixture_dir("history");
        let adapter = fixture_adapter(&dir).await;
        let all = adapter
            .history(PackageHistoryQuery {
                name: None,
                limit: Some(10),
            })
            .await
            .expect("history must read the fixture log");
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].ts_epoch_ms, 1_715_509_425_000);
        assert_eq!(all[0].action, PackageAction::Install);
        assert_eq!(all[0].name, "libfoo1");
        assert_eq!(all[0].to_version.as_deref(), Some("1.2.3-1"));
        let bar = all.iter().find(|e| e.name == "bar").expect("upgrade entry");
        assert_eq!(bar.action, PackageAction::Upgrade);
        assert_eq!(bar.from_version.as_deref(), Some("2.0-1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn history_without_a_log_answers_empty_not_a_failure() {
        // A machine that never ran an apt transaction (fresh install,
        // disposable container) has no /var/log/apt/history.log yet: the read
        // answers an empty history, never NativeFailure — the shared packages
        // suite pins `history yields Ok` on every backend, and the dnf
        // history-database and pacman.log reads carry the same degradation
        // (R3: a missing auxiliary artifact degrades to the empty read).
        let dir = fixture_dir("no-history-log");
        let mut programs = write_fixtures(&dir, FIXTURE_HISTORY);
        programs.history_log = dir.join("no-such-history.log");
        let adapter = AptAdapter::detect_with(programs, Duration::from_secs(9))
            .await
            .expect("fixture adapter must detect");
        let history = adapter
            .history(PackageHistoryQuery {
                name: None,
                limit: Some(10),
            })
            .await
            .expect("an absent history log is an empty history, not an error");
        assert!(
            history.is_empty(),
            "no log means no recorded transactions, got {history:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn detect_answers_adapter_unavailable_without_the_backend() {
        let dir = fixture_dir("absent");
        let programs = AptPrograms {
            apt_get: dir.join("no-such-apt-get").to_string_lossy().into_owned(),
            ..AptPrograms::default()
        };
        let detected = AptAdapter::detect_with(programs, Duration::from_millis(500)).await;
        assert!(
            matches!(detected, Err(ContractError::AdapterUnavailable)),
            "a missing apt is an honest AdapterUnavailable"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --------------------------- live-backend legs ---------------------------
    // (Real Debian/Ubuntu/Mint machines and the T2 CI container legs; other
    // machines skip honestly, mirroring the alpm adapter's live tests.)

    #[tokio::test]
    async fn contract_suite_passes_on_the_live_backend() {
        let Ok(adapter) = AptAdapter::detect().await else {
            eprintln!("skipping live apt suite: no runnable apt-get on this machine");
            return;
        };
        let report = omnibus_contracts::suite::packages::packages_suite(
            || Box::new(adapter.clone()),
            PackagesFixture {
                undo_expected: false,
            },
        )
        .await;
        assert!(report.is_clean(), "{report}");
    }

    #[tokio::test]
    async fn live_list_respects_the_limit() {
        let Ok(adapter) = AptAdapter::detect().await else {
            eprintln!("skipping live list leg: no runnable apt-get on this machine");
            return;
        };
        let pkgs = adapter
            .list(PackageListRequest { limit: Some(5000) })
            .await
            .expect("dpkg-query must answer on a live apt system");
        assert!(pkgs.len() <= 5000, "list respects the limit");
    }
}
