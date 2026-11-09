//! dnf adapter: package list/info/search/install/remove/history and
//! transactional undo for the Fedora family (roadmap task 2.3.1).
//!
//! Native backend: the dnf toolchain reached through its command-line
//! interface — `rpm -qa`/`rpm -q` for the installed set, `dnf search`/`info`
//! for the available metadata, `dnf install`/`remove` for transactions, and
//! the `dnf history` database for history and undo (the ADR-002-style CLI
//! mapping; the `PackageManager` contract and its suite are byte-identical to
//! the alpm/apt families', R2). Both dnf4 and dnf5 are supported: detection
//! probes `dnf` first and falls back to `dnf5`; the resolved binary is used
//! for every dnf call. The swap is invisible to the contract — signatures and
//! capabilities never change (specs/domains/packages.md). Native shapes never
//! leave this crate — mapping to DTOs happens inside the adapter.
//!
//! Transactional undo (roadmap 2.3.1): dnf records every transaction in its
//! history database and can invert one (`dnf history undo <tid>`), so
//! `pkg.undo` is reported with value `transactional` and [`UNDO_VALUE_NOTE`].
//! `undo(name)` locates the most recent transaction that altered the package
//! (a bounded scan of `dnf history` + `history info`) and inverts THAT
//! transaction — the whole transaction point, exactly what dnf guarantees —
//! answering the per-package version facts the record honestly carries
//! (empty version string = absent or not recorded; the airtight claim is the
//! transaction-level inversion, never a guessed per-package version).
//!
//! Every subprocess is bounded: the wall-clock budget kills a stalled child
//! (`kill_on_drop`), stdout capture stops at a hard byte cap. A write whose
//! budget elapses answers [`NF_TIMEOUT_INDETERMINATE`] with the uncertainty
//! payload — writes are uncancellable at the native layer, and the only
//! airtight claim is that the outcome is unknown. Transactions never prompt:
//! the `-y` flag is part of every transaction argv (pinned by tests on the
//! non-interactive contract; stdin is nulled by the bounded spawn itself).

mod history;

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use omnibus_contracts::capabilities::{PKG_READ, PKG_UNDO, PKG_WRITE};
use omnibus_contracts::dto::packages::{
    PackageAction, PackageBrief, PackageHistoryEntry, PackageHistoryQuery, PackageInfo,
    PackageListRequest, PackageNameRequest, PackageSearch, TransactionResult, UndoResult,
    UndoSource,
};
use omnibus_contracts::dto::probe::{AdapterReport, CapabilityFact};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::packages::PackageManager;
use omnibus_contracts::version::CONTRACT_VERSION;
use tokio::io::AsyncReadExt;

/// `NativeFailure` code: generic dnf toolchain failure (spawn, IO).
pub const NF_DNF: i32 = 1;
/// `NativeFailure` code: the named package is neither installed nor available.
pub const NF_PKG_NOT_FOUND: i32 = 2;
/// `NativeFailure` code: a native invocation failed (the code space stays the
/// closed, documented NF_* set — the raw dnf/rpm exit status is preserved in
/// the payload text, `... failed with exit status 100: <stderr>`, never as the
/// machine-readable code a client would have to know dnf's exit codes to
/// read).
pub const NF_TRANSACTION: i32 = 3;
/// `NativeFailure` code: the history database cannot be read.
pub const NF_HISTORY: i32 = 4;
/// `NativeFailure` code: no transaction within the recent-history window
/// altered the named package, so there is no transaction point to return to.
pub const NF_UNDO_NO_TRANSACTION: i32 = 5;
/// `NativeFailure` code: a write-path budget elapsed — the outcome is unknown
/// (the child is killed with the call, but a package change may already have
/// landed; see [`WRITE_TIMEOUT_INDETERMINATE_PAYLOAD`]).
pub const NF_TIMEOUT_INDETERMINATE: i32 = 6;

/// The honest write-path timeout payload (single source for tests): the
/// budget killed the child, but whether a package change landed is unknown —
/// re-check state instead of blindly retrying.
pub const WRITE_TIMEOUT_INDETERMINATE_PAYLOAD: &str = "timeout-indeterminate: write budget elapsed and the dnf child was killed; \
     whether a package change lands natively is unknown — re-check the package state before retrying";

/// The honest undo refusal payload (single source for tests): the bounded
/// history scan found no transaction touching the package, so there is no
/// transaction point to return to. The refusal happens BEFORE any native
/// write — never a half-applied transaction.
pub const UNDO_NO_TRANSACTION_PAYLOAD: &str = "undo-no-transaction: no transaction in the recent history window altered this package; \
     nothing to return to";

/// The `pkg.undo` capability VALUE this adapter reports (roadmap 2.3.1):
/// dnf's history database makes undo transactional — typed by the contracts
/// enum.
pub const UNDO_VALUE: omnibus_contracts::capabilities::PkgUndoValue =
    omnibus_contracts::capabilities::PkgUndoValue::Transactional;

/// The agent-visible undo semantics note the core surfaces from the report.
pub const UNDO_VALUE_NOTE: &str =
    "undo via dnf history undo: the system returns to the previous transaction point";

/// The non-interactive transaction flag (roadmap 2.3.1 pins it in tests:
/// every transaction argv carries it, stdin is nulled by the bounded spawn).
pub const ASSUME_YES: &str = "-y";

/// Subprocess budget. It sits strictly below the core's `TOOL_CALL_BUDGET`
/// (10 s, `omnibus-core/src/limits.rs`) so the adapter's typed answers stay
/// reachable through the tool path (see the journald adapter for the full
/// equality-hazard note); reads map an elapse to `TimeoutError`, writes to
/// [`NF_TIMEOUT_INDETERMINATE`].
const DEFAULT_BUDGET: Duration = Duration::from_secs(9);
/// Hard cap on stdout kept in memory per call: no invocation can buffer an
/// unbounded tool output (a `rpm -qa` on a ~2000-package system fits easily;
/// the cap bounds pathological output instead of trusting it).
const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
/// Diagnostics cap for the concurrently drained stderr pipe.
const MAX_STDERR_BYTES: usize = 64 * 1024;
/// Upper bound on transactions inspected per `history` call: the most recent
/// window of the history database. Each transaction costs one SEQUENTIAL
/// bounded `history info` spawn, and every real dnf invocation spends ~1–2 s
/// of interpreter startup before any work; `pkg_history` is a READ, so the
/// core wraps the whole call in `tokio::time::timeout(TOOL_CALL_BUDGET)` (10 s,
/// `omnibus-core/src/limits.rs`). The window is sized for that envelope:
/// worst case (1 + N) × ~1.5 s stays inside it. Transactions older than the
/// window are invisible to history — the truncation is documented in the
/// adapter report notes (the alpm/apt family answers from the same bounded
/// recent-window class; `undo` scans [`UNDO_SCAN_LIMIT`]).
const HISTORY_SCAN_LIMIT: usize = 4;
/// Upper bound on transactions inspected per `undo` call while locating the
/// one that altered the package (the same budget class as
/// [`HISTORY_SCAN_LIMIT`], kept separate because the two windows answer
/// different questions: `undo` is a WRITE and the core budget does not wrap
/// it, but the same per-spawn startup cost compounds into one call — and the
/// same recent-window honesty applies to the
/// [`NF_UNDO_NO_TRANSACTION`](NF_UNDO_NO_TRANSACTION) refusal).
const UNDO_SCAN_LIMIT: usize = 4;

/// The toolchain programs this adapter invokes.
#[derive(Debug, Clone)]
pub struct DnfPrograms {
    /// The primary dnf CLI (dnf4 or the dnf5 compat binary).
    pub dnf: String,
    /// The explicit dnf5 binary, tried when `dnf` is absent.
    pub dnf5: String,
    /// `rpm` (installed-set queries).
    pub rpm: String,
}

impl Default for DnfPrograms {
    fn default() -> Self {
        Self {
            dnf: String::from("dnf"),
            dnf5: String::from("dnf5"),
            rpm: String::from("rpm"),
        }
    }
}

/// The dnf-backed `PackageManager` provider. Cheap to clone; every clone
/// shares the same immutable configuration.
#[derive(Debug, Clone)]
pub struct DnfAdapter {
    inner: Arc<DnfState>,
}

#[derive(Debug)]
struct DnfState {
    programs: DnfPrograms,
    budget: Duration,
    /// The dnf binary detection resolved for this machine (`dnf` or `dnf5`
    /// path); every history/transaction/search/info call goes through it.
    backend: String,
}

impl DnfAdapter {
    /// Detects the backend (composition-root entry): `dnf --version` (or
    /// `dnf5 --version` on dnf5-only machines) must run. Machines without the
    /// toolchain answer `AdapterUnavailable` and the subsystem stays honestly
    /// absent.
    pub async fn detect() -> Result<Self, ContractError> {
        Self::detect_with(DnfPrograms::default(), DEFAULT_BUDGET).await
    }

    /// [`detect`](Self::detect) over an explicit configuration (tests bind
    /// fixture toolchain scripts here; the probe itself stays the same bounded
    /// subprocess). `dnf` is probed first, `dnf5` is the fallback — the
    /// provider swap stays inside the adapter.
    pub async fn detect_with(
        programs: DnfPrograms,
        budget: Duration,
    ) -> Result<Self, ContractError> {
        let backend = match probe_program(&programs.dnf, budget).await {
            Ok(()) => programs.dnf.clone(),
            Err(ContractError::AdapterUnavailable) => {
                probe_program(&programs.dnf5, budget).await?;
                programs.dnf5.clone()
            }
            Err(other) => return Err(other),
        };
        Ok(Self {
            inner: Arc::new(DnfState {
                programs,
                budget,
                backend,
            }),
        })
    }

    /// Runs one bounded read subprocess.
    async fn run(&self, program: &str, args: &[String]) -> Result<CapturedOutput, ContractError> {
        capture_bounded(
            program,
            args,
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

/// Spawns the bounded child. A transient `ETXTBSY` is retried within a tiny
/// window: on Linux `execve` fails with "Text file busy" while ANY writer
/// still holds the binary open — under parallel test writers a just-written
/// fixture can hit it, and in production a running native dnf/rpm
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
                    code: NF_DNF,
                    payload: format!("spawn {program}: {e}"),
                });
            }
        }
    }
}

/// Runs `program` with `args` under `budget`: stdout is captured under
/// [`MAX_OUTPUT_BYTES`], stderr is drained under its own cap, and a timed-out
/// or overflowed child is killed (`kill_on_drop`). Every invocation in this
/// adapter goes through this one bounded path; `command.output()` would buffer
/// the child's whole stdout without bound and is never used. The environment
/// is inherited (the parent's PATH keeps resolving toolchain wrappers) and
/// stdin is nulled — a transaction can never inherit a prompt.
async fn capture_bounded(
    program: &str,
    args: &[String],
    budget: Duration,
    on_timeout: ContractError,
) -> Result<CapturedOutput, ContractError> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
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
            code: NF_DNF,
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
                    code: NF_DNF,
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
                    code: NF_DNF,
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
            // taxonomy treats adapter codes as a documented set), so raw dnf/
            // rpm exit codes never leak into it. The raw status still travels
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
    capture_bounded(program, &args, budget, ContractError::TimeoutError).await?;
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

/// Parses the `rpm -qa --qf=%{NAME}\t%{VERSION}-%{RELEASE}\t%{SUMMARY}\n`
/// table into briefs. rpm names the installed set directly, so every row is
/// installed; no repo is knowable from rpm alone.
fn parse_installed_table(stdout: &[u8]) -> Vec<PackageBrief> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?.trim();
            let version = fields.next()?.trim();
            let summary = fields.next().map(str::trim).filter(|d| !d.is_empty());
            (!name.is_empty() && !version.is_empty()).then(|| PackageBrief {
                name: String::from(name),
                version: String::from(version),
                description: summary.map(String::from),
                repo: None,
                source: omnibus_contracts::dto::packages::PackageSource::Native,
            })
        })
        .collect()
}

/// Parses the first line of `rpm -q --qf=%{NAME}\t%{VERSION}-%{RELEASE}\n`
/// into the installed version, provided the line answers for `name`.
fn parse_qf_line(stdout: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    let mut fields = text.lines().next()?.split('\t');
    let answered = fields.next()?.trim();
    let version = fields.next()?.trim();
    (answered == name && !version.is_empty()).then(|| String::from(version))
}

/// One parsed `dnf info` stanza (a block of `Key : value` fields).
#[derive(Debug, Default, PartialEq)]
struct InfoStanza {
    epoch: Option<String>,
    version: Option<String>,
    release: Option<String>,
    summary: Option<String>,
    size: Option<String>,
    repository: Option<String>,
    from_repo: Option<String>,
}

impl InfoStanza {
    /// Whether this stanza describes an installed package (`@System` repo).
    fn is_installed(&self) -> bool {
        self.repository
            .as_deref()
            .is_some_and(|r| r.eq_ignore_ascii_case("@System"))
    }

    /// The full `[epoch:]version-release` string, epoch kept only when real
    /// (never `0:`) and never guessed: missing fields collapse honestly.
    fn evr(&self) -> Option<String> {
        let version = self.version.as_deref()?.trim();
        let mut evr = String::from(version);
        if let Some(release) = self.release.as_deref().filter(|r| !r.trim().is_empty()) {
            evr.push('-');
            evr.push_str(release.trim());
        }
        if let Ok(epoch) = self
            .epoch
            .as_deref()
            .unwrap_or_default()
            .trim()
            .parse::<u64>()
            && epoch > 0
            && !evr.contains(':')
        {
            evr = format!("{epoch}:{evr}");
        }
        (!evr.is_empty()).then_some(evr)
    }
}

/// Parses `dnf info <name>` output into its stanzas. A stanza starts at its
/// `Name` field; banner lines (`Installed Packages`, `Available Packages`)
/// and unknown keys are skipped — attribution goes through the `Repository`
/// field (`@System` = installed), which both dnf4 and dnf5 print.
fn parse_info_stanzas(stdout: &[u8]) -> Vec<InfoStanza> {
    let mut stanzas: Vec<InfoStanza> = Vec::new();
    for line in String::from_utf8_lossy(stdout).lines() {
        let Some((key, value)) = history::split_key(line) else {
            continue; // banners, blank lines, wrapped description continuations
        };
        let value = value.trim();
        // A `Name` field opens the first stanza, and re-opens the next one
        // once the current stanza has progressed past its name.
        if key == "name" && stanzas.last().is_none_or(|s| s.version.is_some()) {
            stanzas.push(InfoStanza::default());
        }
        let Some(stanza) = stanzas.last_mut() else {
            continue;
        };
        match key.as_str() {
            "epoch" => stanza.epoch = Some(String::from(value)),
            "version" => stanza.version = Some(String::from(value)),
            "release" => stanza.release = Some(String::from(value)),
            "summary" => stanza.summary = Some(String::from(value)),
            "size" => stanza.size = Some(String::from(value)),
            "repository" => stanza.repository = Some(String::from(value)),
            "from repo" => stanza.from_repo = Some(String::from(value)),
            // Description, URL, License, Vendor and anything future dnf
            // adds: not part of the mapped metadata — skipped, never a
            // parse failure (wrapped description continuations carry no
            // `key : value` shape and never even reach this match).
            _ => {}
        }
    }
    stanzas
}

/// Parses human-readable dnf sizes (`1.2 M`, `512.0 k`, `3.4 G`, bare bytes)
/// into bytes. dnf prints binary units, so k/M/G scale by 1024^n.
fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let (number, multiplier) = match text.chars().last()? {
        'k' | 'K' => (&text[..text.len() - 1], 1024_u64),
        'M' => (&text[..text.len() - 1], 1024_u64 * 1024),
        'G' => (&text[..text.len() - 1], 1024_u64 * 1024 * 1024),
        _ => (text, 1),
    };
    let value: f64 = number.trim().parse().ok()?;
    Some((value * multiplier as f64) as u64)
}

/// Parses `dnf search` answer rows (`name : summary`), skipping the
/// `===== ... =====` match banners; honors the requested limit.
fn parse_search_lines(stdout: &[u8], limit: Option<u32>) -> Vec<PackageBrief> {
    let cap = limit.map_or(usize::MAX, |l| usize::try_from(l).unwrap_or(usize::MAX));
    let mut out: Vec<PackageBrief> = Vec::new();
    for line in String::from_utf8_lossy(stdout).lines() {
        if out.len() >= cap {
            break;
        }
        let line = line.trim();
        if line.is_empty() || line.starts_with('=') {
            continue;
        }
        let Some((name, summary)) = line.split_once(": ") else {
            continue;
        };
        if name.trim().is_empty() {
            continue;
        }
        out.push(PackageBrief {
            name: String::from(name.trim()),
            version: String::new(),
            description: Some(String::from(summary.trim())),
            repo: None,
            source: omnibus_contracts::dto::packages::PackageSource::Native,
        });
    }
    out
}

/// Best-effort rpm query of `name`'s installed version: used both for the
/// advisory `version_applied` after a transaction (the DTO field is
/// audit-facing and explicitly best-effort) and for the current version an
/// undo starts from. A failed or negative rpm query honestly answers
/// "unknown/absent" (`None`).
async fn rpm_query_version(state: &DnfState, name: &str) -> Option<String> {
    let args = [
        String::from("-q"),
        String::from("--qf=%{NAME}\t%{VERSION}-%{RELEASE}\n"),
        String::from(name),
    ];
    let captured = capture_bounded(
        &state.programs.rpm,
        &args,
        state.budget,
        ContractError::TimeoutError,
    )
    .await
    .ok()?;
    parse_qf_line(&captured.stdout, name)
}

/// Fetches and parses `history info <tid>`; a purged transaction (native
/// failure) degrades to `Ok(None)`, other errors propagate.
async fn transaction_info(
    adapter: &DnfAdapter,
    tid: u32,
) -> Result<Option<history::TransactionInfo>, ContractError> {
    let args = [
        String::from("history"),
        String::from("info"),
        tid.to_string(),
    ];
    match adapter.run(&adapter.inner.backend, &args).await {
        Ok(captured) => Ok(history::parse_history_info(&captured.stdout)),
        Err(ContractError::NativeFailure { .. }) => Ok(None),
        Err(other) => Err(other),
    }
}

/// Applies the most-recent limit to chronologically ordered entries.
fn keep_most_recent(entries: &mut Vec<PackageHistoryEntry>, limit: Option<u32>) {
    if let Some(limit) = limit {
        let keep = usize::try_from(limit).unwrap_or(usize::MAX);
        let skip = entries.len().saturating_sub(keep);
        entries.drain(..skip);
    }
}

/// The rpm package-name grammar (rpmts/po regex the distro enforces):
/// alphanumeric first character, then alphanumerics plus `. _ + -`. Anything
/// else never reaches the native argv: dnf/rpm parse leading-dash elements as
/// option sets (`--enablerepo=…`, `--setopt=…`), so an unvalidated "package
/// name" would silently change what the native command does beyond the
/// audited semantics of the call.
fn is_rpm_pkg_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
}

/// The honest refusal for a name outside the grammar: the same
/// [`NF_PKG_NOT_FOUND`] contract the unknown-package path answers (a name the
/// grammar rejects can never name a package), raised BEFORE any native work.
fn pkg_name_refused(name: &str) -> ContractError {
    ContractError::NativeFailure {
        code: NF_PKG_NOT_FOUND,
        payload: format!(
            "pkg-not-found: {name:?} is not a valid package name (expected the rpm name grammar: \
             alphanumerics plus . _ + -)"
        ),
    }
}

/// Closes the one injection vector a search term has: a LEADING DASH makes
/// dnf parse the term as an option set instead of a query. Search terms are
/// otherwise free text on this contract (the alpm family matches
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

#[async_trait]
impl PackageManager for DnfAdapter {
    fn report(&self) -> AdapterReport {
        AdapterReport {
            adapter: String::from("dnf"),
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
                    // Transactional undo: dnf inverts a recorded transaction
                    // (`history undo`), so the tool is real and reported.
                    status: String::from("present"),
                    value: Some(UNDO_VALUE.to_json()),
                    note: Some(String::from(UNDO_VALUE_NOTE)),
                    remedy: None,
                },
            ],
            notes: vec![
                String::from(
                    "history from the dnf history database; install/remove via dnf/dnf5, never interactive",
                ),
                // The honest truncation (R7): history is a read inside the
                // core's 10 s budget, and every dnf spawn costs ~1–2 s of
                // startup, so the scan window is bounded (see
                // HISTORY_SCAN_LIMIT) — entries beyond it are invisible.
                format!(
                    "history scans at most the {HISTORY_SCAN_LIMIT} most recent transactions per call (read-budget window); older entries are invisible"
                ),
            ],
        }
    }

    async fn list(&self, request: PackageListRequest) -> Result<Vec<PackageBrief>, ContractError> {
        let args = [
            String::from("-qa"),
            String::from("--qf=%{NAME}\t%{VERSION}-%{RELEASE}\t%{SUMMARY}\n"),
        ];
        let captured = self.run(&self.inner.programs.rpm, &args).await?;
        let mut briefs = parse_installed_table(&captured.stdout);
        if let Some(limit) = request.limit {
            let keep = usize::try_from(limit).unwrap_or(usize::MAX);
            briefs.truncate(keep);
        }
        Ok(briefs)
    }

    async fn info(&self, name: String) -> Result<PackageInfo, ContractError> {
        if !is_rpm_pkg_name(&name) {
            return Err(pkg_name_refused(&name));
        }
        let installed = rpm_query_version(&self.inner, &name).await;
        let available = match self
            .run(&self.inner.backend, &[String::from("info"), name.clone()])
            .await
        {
            Ok(captured) => parse_info_stanzas(&captured.stdout),
            // `dnf info` exits nonzero for "no match": honestly no available
            // metadata; other native failures degrade the same way because
            // the rpm query above remains the authoritative installed set.
            Err(ContractError::NativeFailure { .. }) => Vec::new(),
            Err(other) => return Err(other),
        };
        let installed_stanza = available.iter().position(|s| s.is_installed());
        let available_stanza = available.iter().position(|s| !s.is_installed());
        if installed.is_none() && installed_stanza.is_none() && available_stanza.is_none() {
            return Err(ContractError::NativeFailure {
                code: NF_PKG_NOT_FOUND,
                payload: format!("pkg-not-found: {name} is neither installed nor available"),
            });
        }
        let (summary, size, repo) = match (installed_stanza, available_stanza) {
            (Some(i), _) => {
                let stanza = &available[i];
                (
                    stanza.summary.clone(),
                    stanza.size.as_deref().and_then(parse_size),
                    stanza
                        .from_repo
                        .clone()
                        .or_else(|| stanza.repository.clone()),
                )
            }
            (None, Some(a)) => {
                let stanza = &available[a];
                (
                    stanza.summary.clone(),
                    stanza.size.as_deref().and_then(parse_size),
                    stanza.repository.clone(),
                )
            }
            (None, None) => (None, None, None),
        };
        // `version_available` is honest only when it differs from what is
        // installed (an identical available version is not an upgrade).
        let installed_evr = installed_stanza.and_then(|i| available[i].evr());
        let available_evr = available_stanza.and_then(|a| available[a].evr());
        let version_available = match (&installed_evr, &available_evr) {
            (Some(have), Some(cand)) if have == cand => None,
            (_, cand) => cand.clone(),
        };
        Ok(PackageInfo {
            version: installed
                .or(installed_evr)
                .or(available_evr)
                .unwrap_or_default(),
            name,
            description: summary,
            repo,
            size_bytes: size,
            install_reason: None,
            depends: Vec::new(),
            required_by: Vec::new(),
            version_available,
        })
    }

    async fn search(&self, query: PackageSearch) -> Result<Vec<PackageBrief>, ContractError> {
        if query.query.trim_start().starts_with('-') {
            return Err(search_term_refused(&query.query));
        }
        let args = [String::from("search"), query.query];
        let captured = self.run(&self.inner.backend, &args).await?;
        Ok(parse_search_lines(&captured.stdout, query.limit))
    }

    async fn install(
        &self,
        request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError> {
        if !is_rpm_pkg_name(&request.name) {
            return Err(pkg_name_refused(&request.name));
        }
        let args = [
            String::from("install"),
            String::from(ASSUME_YES),
            request.name.clone(),
        ];
        self.run_write(&self.inner.backend, &args).await?;
        let version_applied = rpm_query_version(&self.inner, &request.name).await;
        Ok(TransactionResult {
            name: request.name,
            version_applied,
        })
    }

    async fn remove(
        &self,
        request: PackageNameRequest,
    ) -> Result<TransactionResult, ContractError> {
        if !is_rpm_pkg_name(&request.name) {
            return Err(pkg_name_refused(&request.name));
        }
        let args = [
            String::from("remove"),
            String::from(ASSUME_YES),
            request.name.clone(),
        ];
        self.run_write(&self.inner.backend, &args).await?;
        let version_applied = rpm_query_version(&self.inner, &request.name).await;
        Ok(TransactionResult {
            name: request.name,
            version_applied,
        })
    }

    async fn history(
        &self,
        query: PackageHistoryQuery,
    ) -> Result<Vec<PackageHistoryEntry>, ContractError> {
        let ids_args = [String::from("history")];
        let captured = self.run(&self.inner.backend, &ids_args).await?;
        let ids = history::parse_history_table(&captured.stdout);
        let window = ids.len().min(HISTORY_SCAN_LIMIT);
        let needed = usize::try_from(query.limit.unwrap_or(u32::MAX)).unwrap_or(usize::MAX);
        // One chunk per transaction, in the order the events appear inside it
        // (chronological within the transaction).
        let mut chunks: Vec<Vec<PackageHistoryEntry>> = Vec::new();
        let mut matched = 0usize;
        // Newest → oldest across the recent window: ids arrive
        // most-recent-first, and a bounded limit promises the MOST RECENT
        // matching entries — so the scan starts at the newest transaction and
        // stops as soon as the newest transactions have filled `needed`.
        // Scanning oldest-first would stop inside the old end of the window
        // whenever the window holds more events than the limit, silently
        // dropping the newest transactions (the answers would be well within
        // budget yet wrong).
        for tid in ids[..window].iter() {
            let Some(info) = transaction_info(self, *tid).await? else {
                continue; // a purged transaction stays invisible
            };
            let ts = info.begin_epoch_ms.unwrap_or_default();
            let mut chunk: Vec<PackageHistoryEntry> = Vec::new();
            for (action, name, from, to) in &info.events {
                if query.name.as_deref().is_none_or(|filter| filter == name) {
                    chunk.push(PackageHistoryEntry {
                        ts_epoch_ms: ts,
                        action: *action,
                        name: name.clone(),
                        from_version: from.clone(),
                        to_version: to.clone(),
                    });
                }
            }
            matched += chunk.len();
            chunks.push(chunk);
            if matched >= needed {
                break;
            }
        }
        // The contract history is chronological: the newest-first scan
        // accumulated the transaction chunks backwards, so flatten them
        // newest-chunk-LAST — reversing a flat list would also reverse the
        // event order INSIDE each transaction. This runs before the
        // most-recent trim (which drops the OLDEST entries).
        let mut entries: Vec<PackageHistoryEntry> = Vec::new();
        for chunk in chunks.into_iter().rev() {
            entries.extend(chunk);
        }
        keep_most_recent(&mut entries, query.limit);
        Ok(entries)
    }

    /// Transactional undo (roadmap 2.3.1): locate the most recent transaction
    /// that altered the package (a bounded `history` + `history info` scan),
    /// then invert THAT transaction with `dnf history undo <tid>` — the
    /// refusal for an untouched package happens BEFORE any native write.
    async fn undo(&self, request: PackageNameRequest) -> Result<UndoResult, ContractError> {
        let name = request.name;
        // The name feeds the `rpm -q` lookup below (and every comparison
        // against parsed history): the grammar gate comes first, before any
        // native work.
        if !is_rpm_pkg_name(&name) {
            return Err(pkg_name_refused(&name));
        }
        let ids_args = [String::from("history")];
        let captured = self.run(&self.inner.backend, &ids_args).await?;
        let ids = history::parse_history_table(&captured.stdout);
        let mut found: Option<history::TransactionInfo> = None;
        for tid in ids.iter().take(UNDO_SCAN_LIMIT) {
            if let Some(info) = transaction_info(self, *tid).await?
                && info
                    .events
                    .iter()
                    .any(|(_, altered, _, _)| *altered == name)
            {
                found = Some(info);
                break;
            }
        }
        let Some(tx) = found else {
            return Err(ContractError::NativeFailure {
                code: NF_UNDO_NO_TRANSACTION,
                payload: String::from(UNDO_NO_TRANSACTION_PAYLOAD),
            });
        };
        // Version facts are read BEFORE the inversion: afterwards the rpm
        // state belongs to the undone world, not the answered one. Empty
        // string = absent or not recorded — never a guessed version.
        let from_version = rpm_query_version(&self.inner, &name)
            .await
            .unwrap_or_default();
        let to_version = match tx.events.iter().find(|(_, altered, _, _)| *altered == name) {
            Some((PackageAction::Remove, _, removed, _)) => removed.clone().unwrap_or_default(),
            // Install: undo leaves the package absent. Upgrade/Downgrade:
            // dnf's line never carried the source version, and it is not
            // guessed here.
            _ => String::new(),
        };
        let tid = tx.id.to_string();
        let args = [
            String::from("history"),
            String::from("undo"),
            tid,
            String::from(ASSUME_YES),
        ];
        self.run_write(&self.inner.backend, &args).await?;
        Ok(UndoResult {
            name,
            from_version,
            to_version,
            source: UndoSource::Transaction,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_contracts::dto::packages::PackageAction;
    use omnibus_contracts::suite::families::{SENTINEL_MISSING, SENTINEL_POISON, SENTINEL_SLOW};
    use omnibus_contracts::suite::packages::PackagesFixture;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    // ------------------------------------------------------------------
    // Fixture toolchain: tiny POSIX-sh scripts standing in for dnf and
    // rpm, so the REAL adapter code paths (bounded spawn, parse, error
    // mapping, history scan, transactional undo) run hermetically on any
    // machine. The scripts double as executable assertions: a transaction
    // without the `-y` flag is REFUSED by the fixture, so a passing
    // install/remove/undo proves the non-interactive contract was sent
    // (roadmap 2.3.1, pinned on non-interactive).
    // ------------------------------------------------------------------

    fn write_fixture(dir: &Path, name: &str, body: &str) -> String {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("fixture write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("fixture chmod");
        path.to_string_lossy().into_owned()
    }

    /// The dnf CLI fixture: version probe, search, info, history table,
    /// per-transaction info blocks, and transactions that refuse to run
    /// without `-y`. The same body serves as the dnf5 fallback fixture.
    const DNF_SCRIPT: &str = r#"#!/bin/sh
cmd="$1"; shift
if [ "$cmd" = "--version" ]; then echo "4.19.2 fixture"; exit 0; fi

if [ "$cmd" = "search" ]; then
  echo "===== Name & Summary Matched: $1 ====="
  echo "suite-tool : fixture package for tests"
  echo "suitekit : another fixture package"
  exit 0
fi

if [ "$cmd" = "info" ]; then
  name="$1"
  if [ "$name" = "foo" ]; then
    echo "Installed Packages"
    echo "Name         : foo"
    echo "Epoch        : 0"
    echo "Version      : 1.0.0"
    echo "Release      : 1.fc40"
    echo "Size         : 2.0 M"
    echo "Repository   : @System"
    echo "From Repo    : updates"
    echo "Summary      : fixture package installed"
    echo ""
    echo "Available Packages"
    echo "Name         : foo"
    echo "Version      : 1.1.0"
    echo "Release      : 2.fc40"
    echo "Size         : 2.1 M"
    echo "Repository   : updates"
    echo "Summary      : fixture package available"
    exit 0
  fi
  if [ "$name" = "bar" ]; then
    echo "Installed Packages"
    echo "Name         : bar"
    echo "Version      : 2.1.0"
    echo "Release      : 1.fc40"
    echo "Size         : 512.0 k"
    echo "Repository   : @System"
    echo "From Repo    : fedora"
    echo "Summary      : installed bar"
    exit 0
  fi
  echo "Error: Unable to find a match: $name" >&2
  exit 1
fi

if [ "$cmd" = "history" ]; then
  if [ "$1" = "info" ]; then
    tid="$2"
    case "$tid" in
      23)
        echo "Transaction ID   : 23"
        echo "Begin time       : 2024-06-01 08:00:02"
        echo "Return-Code      : Success"
        echo "Packages Altered:"
        echo "    Install  bar-2.1.0-1.fc40.x86_64"
        ;;
      22)
        echo "Transaction ID   : 22"
        echo "Begin time       : 2024-05-12 10:23:45"
        echo "Return-Code      : Success"
        echo "Packages Altered:"
        echo "    Erase    gone-1.0.0-1.fc40.x86_64"
        ;;
      21)
        echo "Transaction ID   : 21"
        echo "Begin time       : 2024-05-01 09:00:00"
        echo "Return-Code      : Success"
        echo "Packages Altered:"
        echo "    Install  foo-1.0.0-1.fc40.x86_64"
        echo "    Upgrade  oldlib-2.0.0-1.fc40.x86_64"
        ;;
      *)
        echo "Error: no transaction id $tid" >&2
        exit 1
        ;;
    esac
    exit 0
  fi
  if [ "$1" = "undo" ]; then
    has_y=0
    for arg in "$@"; do
      [ "$arg" = "-y" ] && has_y=1
    done
    if [ "$has_y" != "1" ]; then
      echo "non-interactive contract violated (no -y)" >&2
      exit 95
    fi
    echo "Undoing transaction $2"
    exit 0
  fi
  echo "ID     | Command line             | Date and time    | Action(s)       | Altered"
  echo "    23 | install bar              | 2024-06-01 08:00 | I               |    1"
  echo "    22 | erase gone               | 2024-05-12 10:23 | E               |    1"
  echo "    21 | install foo bar-upgrade  | 2024-05-01 09:00 | I               |    2"
  exit 0
fi

if [ "$cmd" = "install" ] || [ "$cmd" = "remove" ]; then
  has_y=0
  for arg in "$@"; do
    [ "$arg" = "-y" ] && has_y=1
  done
  if [ "$has_y" != "1" ]; then
    echo "non-interactive contract violated (no -y)" >&2
    exit 95
  fi
  target=""
  for last in "$@"; do target="$last"; done
  if [ "$target" = "omnibus-suite-poison" ]; then
    echo "Error: Unable to find a match: $target" >&2
    exit 100
  fi
  if [ "$target" = "omnibus-suite-slow" ]; then sleep 30; fi
  echo "$cmd of $target done"
  exit 0
fi

echo "unknown fixture invocation: $cmd" >&2
exit 97
"#;

    /// The rpm fixture: version probe, the canned installed table, and
    /// single-package queries for the names the tests exercise.
    const RPM_SCRIPT: &str = r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo "RPM version 4.19.1.1 fixture"; exit 0; fi
if [ "$1" = "-qa" ]; then
  printf 'bash\t5.2.15-2.fc40\tGNU Bourne Again shell\n'
  printf 'foo\t1.0.0-1.fc40\tfixture package installed\n'
  printf 'bar\t2.1.0-1.fc40\tinstalled bar\n'
  exit 0
fi
if [ "$1" = "-q" ]; then
  name="$3"
  case "$name" in
    bash)
      printf 'bash\t5.2.15-2.fc40\n'
      exit 0
      ;;
    foo)
      printf 'foo\t1.0.0-1.fc40\n'
      exit 0
      ;;
    bar)
      printf 'bar\t2.1.0-1.fc40\n'
      exit 0
      ;;
  esac
  echo "package $name is not installed" >&2
  exit 1
fi
echo "unknown fixture invocation: $1" >&2
exit 97
"#;

    fn write_fixtures(dir: &Path, with_dnf4: bool, with_dnf5: bool) -> DnfPrograms {
        let mut programs = DnfPrograms {
            dnf: dir.join("no-such-dnf").to_string_lossy().into_owned(),
            dnf5: dir.join("no-such-dnf5").to_string_lossy().into_owned(),
            rpm: dir.join("no-such-rpm").to_string_lossy().into_owned(),
        };
        if with_dnf4 {
            programs.dnf = write_fixture(dir, "dnf", DNF_SCRIPT);
        }
        if with_dnf5 {
            programs.dnf5 = write_fixture(dir, "dnf5", DNF_SCRIPT);
        }
        programs.rpm = write_fixture(dir, "rpm", RPM_SCRIPT);
        programs
    }

    async fn fixture_adapter(tag: &str) -> (DnfAdapter, PathBuf) {
        let dir = fixture_dir(tag);
        let programs = write_fixtures(&dir, true, false);
        let adapter = DnfAdapter::detect_with(programs, Duration::from_secs(9))
            .await
            .expect("fixture adapter must detect");
        (adapter, dir)
    }

    fn fixture_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "omnibus-dnf-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("fixture dir");
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    // --------------------------- pure parsers ---------------------------

    #[test]
    fn installed_table_parses_rpm_rows() {
        let raw = b"bash\t5.2.15-2.fc40\tGNU Bourne Again shell\n\
                    foo\t1.0.0-1.fc40\tfixture package installed\n\
                    \n\
                    half-row-only\n";
        let briefs = parse_installed_table(raw);
        assert_eq!(briefs.len(), 2);
        assert_eq!(briefs[0].name, "bash");
        assert_eq!(briefs[0].version, "5.2.15-2.fc40");
        assert_eq!(
            briefs[0].description.as_deref(),
            Some("GNU Bourne Again shell")
        );
        assert_eq!(briefs[1].version, "1.0.0-1.fc40");
    }

    #[test]
    fn qf_line_answers_only_for_the_asked_name() {
        let raw = b"foo\t1.0.0-1.fc40\n";
        assert_eq!(parse_qf_line(raw, "foo").as_deref(), Some("1.0.0-1.fc40"));
        assert_eq!(parse_qf_line(raw, "bar"), None);
        assert_eq!(parse_qf_line(b"", "foo"), None);
    }

    #[test]
    fn info_stanzas_split_installed_and_available() {
        let raw = b"Installed Packages\n\
                    Name         : foo\n\
                    Epoch        : 0\n\
                    Version      : 1.0.0\n\
                    Release      : 1.fc40\n\
                    Size         : 2.0 M\n\
                    Repository   : @System\n\
                    From Repo    : updates\n\
                    Summary      : fixture package installed\n\
                    Description  : long text\n\
                    wrapped continuation line\n\
                    \n\
                    Available Packages\n\
                    Name         : foo\n\
                    Version      : 1.1.0\n\
                    Release      : 2.fc40\n\
                    Size         : 2.1 M\n\
                    Repository   : updates\n\
                    Summary      : fixture package available\n";
        let stanzas = parse_info_stanzas(raw);
        assert_eq!(stanzas.len(), 2);
        assert!(stanzas[0].is_installed());
        assert!(!stanzas[1].is_installed());
        assert_eq!(stanzas[0].evr().as_deref(), Some("1.0.0-1.fc40"));
        assert_eq!(stanzas[1].evr().as_deref(), Some("1.1.0-2.fc40"));
        assert_eq!(
            stanzas[0].summary.as_deref(),
            Some("fixture package installed")
        );
    }

    #[test]
    fn real_epochs_get_prefixed_and_zero_epochs_do_not() {
        let mut stanza = InfoStanza {
            epoch: Some(String::from("2")),
            version: Some(String::from("3.0")),
            release: Some(String::from("1.fc40")),
            ..InfoStanza::default()
        };
        assert_eq!(stanza.evr().as_deref(), Some("2:3.0-1.fc40"));
        stanza.epoch = Some(String::from("0"));
        assert_eq!(stanza.evr().as_deref(), Some("3.0-1.fc40"));
        stanza.version = Some(String::from("4:3.0"));
        assert_eq!(stanza.evr().as_deref(), Some("4:3.0-1.fc40"));
        stanza.version = None;
        assert_eq!(stanza.evr(), None);
    }

    #[test]
    fn human_sizes_convert_to_bytes() {
        assert_eq!(parse_size("2.0 M"), Some(2 * 1024 * 1024));
        assert_eq!(parse_size("512.0 k"), Some(512 * 1024));
        assert_eq!(parse_size("1.5 G"), Some(1_610_612_736));
        assert_eq!(parse_size("12345"), Some(12_345));
        assert_eq!(parse_size("not-a-size"), None);
    }

    #[test]
    fn search_rows_honor_banners_and_limit() {
        let raw = b"===== Name Exactly Matched: suite =====\n\
                    suite : fixture package for tests\n\
                    ===== Name & Summary Matched: suite =====\n\
                    suite-tool : another fixture package\n\
                    no-colon-line\n";
        let briefs = parse_search_lines(raw, None);
        assert_eq!(briefs.len(), 2);
        assert_eq!(briefs[0].name, "suite");
        assert_eq!(
            briefs[1].description.as_deref(),
            Some("another fixture package")
        );
        assert_eq!(parse_search_lines(raw, Some(1)).len(), 1);
    }

    #[test]
    fn transaction_argv_is_noninteractive() {
        // Roadmap 2.3.1: the `-y` flag is pinned; install/remove carry it
        // after the command, undo after the transaction id.
        assert_eq!(ASSUME_YES, "-y");
    }

    // ------------------------ fixture-backend legs ------------------------

    #[tokio::test]
    async fn contract_suite_passes_on_the_fixture_backend() {
        let (adapter, dir) = fixture_adapter("suite").await;
        let report = omnibus_contracts::suite::packages::packages_suite(
            || Box::new(adapter.clone()),
            PackagesFixture {
                undo_expected: true,
            },
        )
        .await;
        assert!(report.is_clean(), "{report}");
        cleanup(&dir);
    }

    #[tokio::test]
    async fn report_declares_transactional_undo() {
        let (adapter, dir) = fixture_adapter("report").await;
        let report = adapter.report();
        assert_eq!(report.adapter, "dnf");
        assert_eq!(report.subsystem, "packages");
        let undo = report
            .capabilities
            .iter()
            .find(|c| c.name == PKG_UNDO)
            .expect("undo fact present");
        assert_eq!(undo.status, "present");
        assert_eq!(undo.value, Some(UNDO_VALUE.to_json()));
        assert!(
            undo.note
                .as_deref()
                .is_some_and(|n| n.contains("history undo")),
            "the note is the agent-visible transactional marking"
        );
        assert!(report.capabilities.iter().any(|c| c.name == PKG_READ));
        assert!(report.capabilities.iter().any(|c| c.name == PKG_WRITE));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn list_reads_the_rpm_table() {
        let (adapter, dir) = fixture_adapter("list").await;
        let all = adapter
            .list(PackageListRequest { limit: None })
            .await
            .expect("list must read the rpm table");
        let names: Vec<&str> = all.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["bash", "foo", "bar"]);
        let limited = adapter
            .list(PackageListRequest { limit: Some(2) })
            .await
            .expect("limited list");
        assert_eq!(limited.len(), 2);
        cleanup(&dir);
    }

    #[tokio::test]
    async fn info_assembles_installed_and_available_metadata() {
        let (adapter, dir) = fixture_adapter("info").await;
        let foo = adapter
            .info(String::from("foo"))
            .await
            .expect("installed+available info");
        assert_eq!(foo.version, "1.0.0-1.fc40");
        assert_eq!(foo.repo.as_deref(), Some("updates"));
        assert_eq!(foo.size_bytes, Some(2 * 1024 * 1024));
        assert_eq!(
            foo.description.as_deref(),
            Some("fixture package installed")
        );
        assert_eq!(
            foo.version_available.as_deref(),
            Some("1.1.0-2.fc40"),
            "a differing available version IS an available upgrade"
        );
        let bar = adapter
            .info(String::from("bar"))
            .await
            .expect("installed-only info");
        assert_eq!(bar.version, "2.1.0-1.fc40");
        assert_eq!(bar.size_bytes, Some(512 * 1024));
        assert_eq!(
            bar.version_available, None,
            "no available stanza: nothing to upgrade to"
        );
        cleanup(&dir);
    }

    #[tokio::test]
    async fn unknown_package_is_an_honest_pkg_not_found() {
        let (adapter, dir) = fixture_adapter("missing").await;
        let missing = adapter.info(String::from(SENTINEL_MISSING)).await;
        match missing {
            Err(ContractError::NativeFailure { code, payload }) => {
                assert_eq!(code, NF_PKG_NOT_FOUND);
                assert!(payload.contains("pkg-not-found"));
                assert!(payload.contains(SENTINEL_MISSING));
            }
            other => panic!("expected pkg-not-found, got {other:?}"),
        }
        cleanup(&dir);
    }

    #[tokio::test]
    async fn install_and_remove_refuse_interactive_backends() {
        // The fixture script exits 95 when `-y` is missing, so a passing
        // transaction proves the non-interactive flag was sent (roadmap
        // 2.3.1: the contract is pinned by a test, not by docs).
        let (adapter, dir) = fixture_adapter("write").await;
        let installed = adapter
            .install(PackageNameRequest {
                name: String::from("bash"),
            })
            .await
            .expect("fixture install must succeed with -y");
        assert_eq!(installed.name, "bash");
        assert_eq!(installed.version_applied.as_deref(), Some("5.2.15-2.fc40"));
        let removed = adapter
            .remove(PackageNameRequest {
                name: String::from("htop"),
            })
            .await
            .expect("fixture remove must succeed with -y");
        assert_eq!(removed.name, "htop");
        assert_eq!(
            removed.version_applied, None,
            "removed: rpm answers nothing"
        );
        cleanup(&dir);
    }

    #[tokio::test]
    async fn poisoned_transaction_maps_to_native_failure() {
        let (adapter, dir) = fixture_adapter("poison").await;
        let denied = adapter
            .install(PackageNameRequest {
                name: String::from(SENTINEL_POISON),
            })
            .await;
        match denied {
            Err(ContractError::NativeFailure { code, payload }) => {
                // The NativeFailure code space stays the closed NF_* set: the
                // raw dnf exit status travels in the payload, never as the
                // machine-readable code.
                assert_eq!(code, NF_TRANSACTION, "the code space stays closed");
                assert!(
                    payload.contains("exit status 100"),
                    "the raw status is preserved in the payload: {payload}"
                );
                assert!(payload.contains("Unable to find a match"));
            }
            other => panic!("expected an honest native failure, got {other:?}"),
        }
        cleanup(&dir);
    }

    #[tokio::test]
    async fn option_like_names_never_reach_the_native_argv() {
        // Argument-injection pin: a crafted "package name" is refused with the
        // honest NF_PKG_NOT_FOUND BEFORE any native work. The fixture would
        // echo success for any non-poison target, so an `Ok` here would mean
        // the string reached the dnf/rpm argv — the refusals prove the gate
        // sits in front of every spawn (transaction, rpm lookup, undo).
        let (adapter, dir) = fixture_adapter("injection").await;
        for hostile in [
            "-f",
            "-y",
            "--setopt=dnf.main.excludepkgs=kernel-core",
            "/tmp/evil.rpm",
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
            let undo = adapter
                .undo(PackageNameRequest {
                    name: String::from(hostile),
                })
                .await;
            assert!(
                matches!(
                    undo,
                    Err(ContractError::NativeFailure {
                        code: NF_PKG_NOT_FOUND,
                        ..
                    })
                ),
                "undo refuses the hostile name too: {undo:?}"
            );
        }
        cleanup(&dir);
    }

    #[tokio::test]
    async fn read_paths_refuse_option_shaped_input() {
        let (adapter, dir) = fixture_adapter("injection-read").await;
        // info: a leading-dash "name" would be parsed as an option set.
        let info = adapter.info(String::from("--enablerepo=updates")).await;
        match info {
            Err(ContractError::NativeFailure { code, payload }) => {
                assert_eq!(code, NF_PKG_NOT_FOUND);
                assert!(payload.contains("--enablerepo=updates"));
            }
            other => panic!("expected the honest info refusal, got {other:?}"),
        }
        // search: terms are free text, but a leading dash is the option
        // injection vector — refused before any dnf spawn.
        let search = adapter
            .search(PackageSearch {
                query: String::from("--setopt=dnf.main.excludepkgs=kernel-core"),
                limit: None,
            })
            .await;
        match search {
            Err(ContractError::NativeFailure { code, payload }) => {
                assert_eq!(code, NF_PKG_NOT_FOUND);
                assert!(payload.contains("--setopt"));
            }
            other => panic!("expected the honest search refusal, got {other:?}"),
        }
        // Free-text terms (no leading dash) still flow through unchanged.
        let found = adapter
            .search(PackageSearch {
                query: String::from("suite"),
                limit: Some(5),
            })
            .await
            .expect("a plain search term is not refused");
        assert!(!found.is_empty());
        cleanup(&dir);
    }

    #[tokio::test]
    async fn stalled_write_answers_timeout_indeterminate() {
        let (adapter, dir) = fixture_adapter("slow").await;
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
        cleanup(&dir);
    }

    #[tokio::test]
    async fn history_reads_the_transaction_window() {
        let (adapter, dir) = fixture_adapter("history").await;
        let all = adapter
            .history(PackageHistoryQuery {
                name: None,
                limit: Some(10),
            })
            .await
            .expect("history must read the fixture database");
        // Transaction 21 altered two packages; 22 and 23 one each.
        assert_eq!(all.len(), 4);
        assert_eq!(
            all[0].name, "foo",
            "entries are chronological: the oldest transaction first"
        );
        // The events INSIDE a transaction keep their native order too (a flat
        // newest-first accumulation reversed wholesale would swap them).
        assert_eq!(
            all[1].name, "oldlib",
            "tx21's second event follows its first"
        );
        assert_eq!(all[0].action, PackageAction::Install);
        // 2024-05-01 09:00:00 UTC == 1714554000.
        assert_eq!(all[0].ts_epoch_ms, 1_714_554_000_000);
        assert_eq!(all[0].to_version.as_deref(), Some("1.0.0-1.fc40"));
        let erased = all.iter().find(|e| e.name == "gone").expect("erase entry");
        assert_eq!(erased.action, PackageAction::Remove);
        assert_eq!(erased.from_version.as_deref(), Some("1.0.0-1.fc40"));
        assert_eq!(erased.to_version, None);
        let recent = adapter
            .history(PackageHistoryQuery {
                name: None,
                limit: Some(2),
            })
            .await
            .expect("limited history");
        assert_eq!(recent.len(), 2);
        // Non-vacuous pin of the newest-first scan: tx23 (install bar) is the
        // NEWEST transaction and tx22 (erase gone) the second-newest, so the
        // kept tail must be tx23's event — the oldest transaction (tx21:
        // foo + oldlib) differs from the newest one's events, and an
        // oldest-first scan that stops at `needed` would keep tx21's rows
        // instead and fail this assertion.
        assert_eq!(
            recent[recent.len() - 1].name,
            "bar",
            "the kept tail is the MOST RECENT transaction"
        );
        // 2024-06-01 08:00:02 UTC == 1717228802: the tail entry really is
        // tx23's event, not tx21's same-named row (tx21 no longer has one).
        assert_eq!(recent[recent.len() - 1].ts_epoch_ms, 1_717_228_802_000);
        assert_eq!(recent[0].name, "gone", "the second-newest transaction");
        // A limit smaller than one transaction's event count: the newest
        // transaction alone fills `needed` (its single event), so the answer
        // is exactly tx23's event — an oldest-first scan would have answered
        // with tx21's rows (two events there).
        let single = adapter
            .history(PackageHistoryQuery {
                name: None,
                limit: Some(1),
            })
            .await
            .expect("single-entry history");
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].name, "bar");
        assert_eq!(single[0].ts_epoch_ms, 1_717_228_802_000);
        let filtered = adapter
            .history(PackageHistoryQuery {
                name: Some(String::from("gone")),
                limit: None,
            })
            .await
            .expect("filtered history");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "gone");
        cleanup(&dir);
    }

    #[tokio::test]
    async fn undo_reverts_to_the_previous_transaction_point() {
        // `gone` was erased by transaction 22; undo must find that
        // transaction, invert it through `dnf history undo 22 -y` (the
        // fixture refuses without `-y`, so success proves the flag), and
        // report the honest per-package facts: currently absent, restored
        // to the erased version, source = transaction.
        let (adapter, dir) = fixture_adapter("undo").await;
        let undone = adapter
            .undo(PackageNameRequest {
                name: String::from("gone"),
            })
            .await
            .expect("transactional undo must run");
        assert_eq!(undone.name, "gone");
        assert_eq!(
            undone.from_version, "",
            "the package is currently absent: no from version exists"
        );
        assert_eq!(undone.to_version, "1.0.0-1.fc40");
        assert_eq!(undone.source, UndoSource::Transaction);
        cleanup(&dir);
    }

    #[tokio::test]
    async fn undo_of_an_installed_package_reports_the_current_version() {
        // `bar` was installed by transaction 23; undo removes it again.
        let (adapter, dir) = fixture_adapter("undo-install").await;
        let undone = adapter
            .undo(PackageNameRequest {
                name: String::from("bar"),
            })
            .await
            .expect("undo of the install transaction");
        assert_eq!(undone.from_version, "2.1.0-1.fc40");
        assert_eq!(
            undone.to_version, "",
            "inverting an install leaves the package absent"
        );
        assert_eq!(undone.source, UndoSource::Transaction);
        cleanup(&dir);
    }

    #[tokio::test]
    async fn undo_without_a_touching_transaction_refuses_before_writing() {
        let (adapter, dir) = fixture_adapter("undo-none").await;
        let refused = adapter
            .undo(PackageNameRequest {
                name: String::from(SENTINEL_MISSING),
            })
            .await;
        match refused {
            Err(ContractError::NativeFailure { code, payload }) => {
                assert_eq!(code, NF_UNDO_NO_TRANSACTION);
                assert_eq!(payload, UNDO_NO_TRANSACTION_PAYLOAD);
            }
            other => panic!("expected an honest undo refusal, got {other:?}"),
        }
        cleanup(&dir);
    }

    #[tokio::test]
    async fn detect_falls_back_to_dnf5() {
        // dnf5-only machine: the `dnf` path is missing, `dnf5` answers, and
        // every dnf call goes through the resolved backend (the provider
        // swap stays inside the adapter, specs/domains/packages.md).
        let dir = fixture_dir("dnf5-fallback");
        let programs = write_fixtures(&dir, false, true);
        let adapter = DnfAdapter::detect_with(programs, Duration::from_secs(9))
            .await
            .expect("dnf5-only machines still get the packages subsystem");
        let entries = adapter
            .history(PackageHistoryQuery {
                name: None,
                limit: Some(1),
            })
            .await
            .expect("history must run through the resolved dnf5 backend");
        assert_eq!(entries.len(), 1);
        cleanup(&dir);
    }

    #[tokio::test]
    async fn detect_answers_adapter_unavailable_without_the_backend() {
        let dir = fixture_dir("absent");
        let programs = write_fixtures(&dir, false, false);
        let detected = DnfAdapter::detect_with(programs, Duration::from_millis(500)).await;
        assert!(
            matches!(detected, Err(ContractError::AdapterUnavailable)),
            "a missing dnf toolchain is an honest AdapterUnavailable"
        );
        cleanup(&dir);
    }

    // --------------------------- live-backend legs ---------------------------
    // (Real Fedora machines; other machines skip honestly, mirroring the
    // alpm/apt adapters' live tests.)

    #[tokio::test]
    async fn contract_suite_passes_on_the_live_backend() {
        let Ok(adapter) = DnfAdapter::detect().await else {
            eprintln!("skipping live dnf suite: no runnable dnf/dnf5 on this machine");
            return;
        };
        let report = omnibus_contracts::suite::packages::packages_suite(
            || Box::new(adapter.clone()),
            PackagesFixture {
                undo_expected: true,
            },
        )
        .await;
        assert!(report.is_clean(), "{report}");
    }

    #[tokio::test]
    async fn live_list_respects_the_limit() {
        let Ok(adapter) = DnfAdapter::detect().await else {
            eprintln!("skipping live list leg: no runnable dnf/dnf5 on this machine");
            return;
        };
        let pkgs = adapter
            .list(PackageListRequest { limit: Some(5000) })
            .await
            .expect("rpm -qa must answer on a live rpm system");
        assert!(pkgs.len() <= 5000, "list respects the limit");
    }
}
