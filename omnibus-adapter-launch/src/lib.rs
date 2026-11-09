//! launch adapter: the Linux `AppLauncher` implementation (subsystem 11,
//! roadmap task 2.7.2, specs/domains/app-launch.md). Three surfaces:
//!
//! - `app.list` inventories `.desktop` files from `/usr/share/applications`
//!   plus `$XDG_DATA_HOME` (default `~/.local/share/applications`); `Exec`,
//!   `Name`, `Icon` and `NoDisplay` feed the DTO.
//! - `app.launch` starts an application as a transient user unit through
//!   `systemd-run --user`: the process lands in the user's cgroup and
//!   journald, so `ps.list` and `journal_query` observe it (the launch ->
//!   observe side of the closed loop). The unit name is minted by this
//!   adapter and travels back in [`LaunchOutcome`] as the observation join
//!   key.
//! - `app.open` opens a URI or file with the associated application through
//!   `xdg-open`.
//!
//! No shell anywhere (the invariant specs/domains/app-launch.md pins):
//! every child is a direct `execve` of `/usr/bin/systemd-run` or
//! `/usr/bin/xdg-open` with a typed argument list, so metacharacters
//! (`;`, `|`, backtick) inside arguments are data reaching the target
//! process verbatim — the `argv` construction is unit-tested against those
//! exact characters, and the live binder observes them unchanged in the
//! launched unit's journald output.
//!
//! Error mapping (specs/architecture/error-taxonomy.md): native failures
//! become [`ContractError::NativeFailure`] with the original message in the
//! payload; the typed `code` is numeric, the symbolic name rides the payload
//! prefix (`app-not-found`, `no-exec`, `spawn`, `os-denied`, `open`,
//! `launch-indeterminate`).
//!
//! Write-path timeout semantics: `systemd-run` mints the unit name this
//! adapter chose (`--unit=`), so when the launch budget elapses the answer is
//! [`ContractError::NativeFailure`] `launch-indeterminate` naming the
//! candidate unit — the transient unit may or may not exist, and the caller
//! re-checks with `ps.list` / `journal_query` before retrying (the systemd1
//! doctrine: a dispatched write is not recalled by dropping the call). For
//! `open` a budget elapse answers a plain [`ContractError::TimeoutError`]:
//! `xdg-open` delegates and forks, the opener's outcome is not the adapter's
//! fact, and "the adapter stopped waiting" is the whole truth.
//!
//! Backend honesty: the composition root passes the probe's
//! `backends_present` facts (install-time detection of `systemd-run` and
//! `xdg-open`) into the constructor. A missing launch backend means the
//! adapter emits no `app.launch` capability fact — the core ladder hides both
//! write tools (rung 2, never a runtime surprise). A missing `xdg-open`
//! leaves `app.launch` served while the report's fact note names the hidden
//! `app.open` (R3: the reason lives in `capabilities()`), and the adapter's
//! own `open` refuses before any native work.
//!
//! R2 failure-injection binding: the suite sentinels from
//! `omnibus_contracts::suite` are bound here — a `launch`/`open` on
//! [`SENTINEL_SLOW`] surfaces as [`ContractError::TimeoutError`], and on
//! [`SENTINEL_POISON`] as the `os-denied` native failure, exactly as
//! `suite::families::app_launcher` asserts.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use omnibus_contracts::capabilities::{APP_LAUNCH, APP_READ};
use omnibus_contracts::dto::app_launcher::{AppEntry, AppSpec, LaunchOutcome, OpenRequest};
use omnibus_contracts::dto::probe::{AdapterReport, CapabilityFact};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::app_launcher::AppLauncher;
use omnibus_contracts::suite::families::{SENTINEL_POISON, SENTINEL_SLOW};
use omnibus_contracts::version::CONTRACT_VERSION;

/// `NativeFailure` code: the requested `app_id` is not in the inventory.
pub const NF_APP_NOT_FOUND: i32 = 1;
/// `NativeFailure` code: the child could not be spawned (binary vanished
/// between detection and exec, or `execve` failed for another reason).
pub const NF_SPAWN: i32 = 2;
/// `NativeFailure` code: the OS or the target refused the action — the
/// stand-in the [`SENTINEL_POISON`] sentinel binds (the suite's grant-missing
/// shape at the seam; the MCP surface answers `ScopeDenied` from the core's
/// grant gate before the adapter is ever reached).
pub const NF_OS_DENIED: i32 = 3;
/// `NativeFailure` code: `xdg-open` exited non-zero (no association, bad
/// target); the original stderr is preserved in the payload.
pub const NF_OPEN: i32 = 4;
/// `NativeFailure` code: the `.desktop` entry carries no usable `Exec`.
pub const NF_NO_EXEC: i32 = 5;
/// `NativeFailure` code: the launch budget elapsed; the transient unit may
/// or may not exist (the payload names the candidate unit to re-check).
pub const NF_LAUNCH_INDETERMINATE: i32 = 6;

/// The `systemd-run` binary the launch arm executes.
const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";
/// The `xdg-open` binary the open arm executes.
const XDG_OPEN: &str = "/usr/bin/xdg-open";
/// The system application directory of the inventory.
const SYSTEM_APPLICATIONS: &str = "/usr/share/applications";
/// The `systemd-run` argv budget. A write self-bounds (no core timer on the
/// write path): dispatch + start-job must answer well below the core's 10 s
/// tool budget, so a healthy launch returns typed and a wedged one answers
/// `launch-indeterminate` instead of being dropped.
const LAUNCH_BUDGET: std::time::Duration = std::time::Duration::from_secs(8);
/// The `xdg-open` budget: the opener delegates and exits quickly; a wedged
/// opener answers `TimeoutError` (the adapter stopped waiting — the whole
/// truth for a delegated fire-and-forget open).
const OPEN_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// The launch adapter: inventory + transient-unit launch + opener.
#[derive(Debug, Clone)]
pub struct LaunchAdapter {
    /// `systemd-run` present per the probe (`backends_present`); drives both
    /// the `app.launch` capability fact and the pre-work refusal.
    launch_backend: bool,
    /// `xdg-open` present per the probe; a missing opener hides `app.open`
    /// through the ladder and makes the report's `app.launch` fact note name
    /// the reason (R3).
    opener_present: bool,
    /// The application directories inventoried (the `applications/` roots).
    data_dirs: Vec<PathBuf>,
}

impl LaunchAdapter {
    /// Builds from the probe's `backends_present` list and the default
    /// application directories (system + `$XDG_DATA_HOME` with the
    /// `~/.local/share` fallback). The composition root's entry point.
    pub fn from_backends(backends_present: &[String]) -> Self {
        Self::with_data_dirs(backends_present, default_data_dirs())
    }

    /// Builds over explicit application directories — the test stand (the
    /// suite binder points the inventory at a synthetic `.desktop` fixture).
    pub fn with_data_dirs(backends_present: &[String], data_dirs: Vec<PathBuf>) -> Self {
        let present = |name: &str| backends_present.iter().any(|b| b == name);
        Self {
            launch_backend: present("systemd-run"),
            opener_present: present("xdg-open"),
            data_dirs,
        }
    }

    /// The `app.read` capability fact: present when at least one application
    /// directory exists to inventory; absent otherwise (no directory — no
    /// inventory — no tool; absence of the entry is the ladder's hiding rung,
    /// never a runtime surprise).
    fn read_fact(&self) -> Option<CapabilityFact> {
        let any_dir = self.data_dirs.iter().any(|dir| dir.is_dir());
        if !any_dir {
            return None;
        }
        Some(CapabilityFact {
            name: String::from(APP_READ),
            ver: String::from(CONTRACT_VERSION),
            status: String::from("present"),
            value: None,
            note: Some(String::from(
                "the .desktop inventory (system and user application directories)",
            )),
            remedy: None,
        })
    }

    /// The `app.launch` capability fact: emitted only when `systemd-run`
    /// exists (a missing launch backend is provider absence — both write
    /// tools hide through the ladder). The note carries the honest surface
    /// description, and — on a stand without `xdg-open` — names the hidden
    /// `app.open` and its reason, which is how the R3 hiding reason reaches
    /// `capabilities()` without a second registry constant.
    fn launch_fact(&self) -> Option<CapabilityFact> {
        if !self.launch_backend {
            return None;
        }
        let note = if self.opener_present {
            String::from(
                "launch as transient user units (systemd-run --user); open URIs/files via xdg-open",
            )
        } else {
            String::from(
                "launch as transient user units (systemd-run --user); app.open is hidden: \
                 xdg-open is not installed (no association opener on this machine)",
            )
        };
        Some(CapabilityFact {
            name: String::from(APP_LAUNCH),
            ver: String::from(CONTRACT_VERSION),
            status: String::from("present"),
            value: None,
            note: Some(note),
            remedy: None,
        })
    }

    /// Resolves one `app_id` to its inventory entry: the desktop-file id is
    /// the `.desktop` path relative to its application root with `/`
    /// collapsed to `-` (the freedesktop id rule), so the lookup re-walks the
    /// same roots `list` served. `inventory` never fails (unreadable roots
    /// are skipped honestly); the iterator answer is the lookup.
    fn entry_of(&self, app_id: &str) -> Option<AppEntry> {
        inventory(self.data_dirs.as_slice())
            .into_iter()
            .find(|entry| entry.app_id == app_id)
    }
}

/// The default application directories: `/usr/share/applications` plus
/// `$XDG_DATA_HOME/applications`, falling back to
/// `~/.local/share/applications` when `XDG_DATA_HOME` is unset or empty
/// (the XDG base-directory rule the spec pins).
pub fn default_data_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from(SYSTEM_APPLICATIONS)];
    let data_home = match std::env::var("XDG_DATA_HOME") {
        Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
        _ => match std::env::var("HOME") {
            Ok(home) if !home.trim().is_empty() => PathBuf::from(home).join(".local").join("share"),
            _ => return dirs,
        },
    };
    dirs.push(data_home.join("applications"));
    dirs
}

/// Walks the application roots and parses every `.desktop` entry. Unreadable
/// roots and unparsable files are skipped honestly (an inventory reports what
/// exists; it never fails the tool for one broken file).
fn inventory(data_dirs: &[PathBuf]) -> Vec<AppEntry> {
    let mut entries = Vec::new();
    for dir in data_dirs {
        for path in desktop_files(dir, 0) {
            if let Some(entry) = parse_desktop_entry(dir, &path) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by(|a, b| a.app_id.cmp(&b.app_id));
    entries.dedup_by(|a, b| a.app_id == b.app_id);
    entries
}

/// Collects `*.desktop` files under `root`, bounded in depth (subdirectories
/// carry namespaced ids: `kde/foo.desktop` inventories as `kde-foo.desktop`).
fn desktop_files(root: &Path, depth: usize) -> Vec<PathBuf> {
    const MAX_DEPTH: usize = 3;
    let mut found = Vec::new();
    let Ok(read) = std::fs::read_dir(root) else {
        return found;
    };
    for item in read.flatten() {
        let path = item.path();
        let Ok(file_type) = item.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if depth < MAX_DEPTH {
                found.extend(desktop_files(&path, depth + 1));
            }
        } else if path.extension().is_some_and(|ext| ext == "desktop") {
            found.push(path);
        }
    }
    found
}

/// Parses one `.desktop` file into the inventory DTO: the `[Desktop Entry]`
/// group's `Exec`, `Name`, `Icon` and `NoDisplay`. Entries without a name are
/// skipped (the DTO promises a display name); a missing `Exec` still lists —
/// the DTO honestly carries an empty `exec` and `launch` refuses it with
/// `no-exec` (the inventory reports what exists, not what launches).
fn parse_desktop_entry(root: &Path, path: &Path) -> Option<AppEntry> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut name: Option<String> = None;
    let mut exec: Option<String> = None;
    let mut icon: Option<String> = None;
    let mut no_display = false;
    let mut in_main_group = false;
    let mut saw_main_group = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            // A new group: only the first `[Desktop Entry]` group feeds the
            // inventory; `[Desktop Action ...]` groups never do.
            in_main_group = trimmed == "[Desktop Entry]" && !saw_main_group;
            if in_main_group {
                saw_main_group = true;
            }
            continue;
        }
        if !in_main_group {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        // Locale-qualified keys (Name[de]) are display variants; the plain
        // key is the canonical entry.
        if key == "Name" {
            name = Some(String::from(value));
        } else if key == "Exec" {
            exec = Some(String::from(value));
        } else if key == "Icon" {
            icon = Some(String::from(value));
        } else if key == "NoDisplay" {
            no_display = value.eq_ignore_ascii_case("true");
        } else if key == "Type" && value != "Application" {
            // Links and directories are not applications.
            return None;
        }
    }
    let name = name?;
    if name.is_empty() {
        return None;
    }
    let relative = path.strip_prefix(root).ok()?;
    let app_id = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("-");
    Some(AppEntry {
        app_id,
        name,
        exec: exec.unwrap_or_default(),
        icon,
        no_display,
    })
}

/// Splits a `.desktop` `Exec` line into argv per the freedesktop quoting
/// rules — NOT shell rules: outside quotes `\s \n \t \r \\` are the escapes;
/// inside double quotes only `\"` and `\\` are. Field codes (`%f %F %u %U`,
/// and the informational `%i %c %k` family) are dropped — launch passes no
/// files through them; `%%` collapses to a literal `%` anywhere. The user's
/// arguments from [`AppSpec`] are appended verbatim by the caller, never
/// substituted into field codes.
fn exec_argv(exec: &str) -> Vec<String> {
    let mut argv = Vec::new();
    let mut token = String::new();
    let mut in_quotes = false;
    let mut token_started = false;
    let mut chars = exec.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            ' ' | '\t' if !in_quotes => {
                if token_started {
                    argv.push(std::mem::take(&mut token));
                    token_started = false;
                }
            }
            '"' => {
                in_quotes = !in_quotes;
                token_started = true;
            }
            '\\' => {
                if let Some(&next) = chars.peek() {
                    let unescaped = if in_quotes {
                        match next {
                            '"' | '\\' => Some(next),
                            _ => None,
                        }
                    } else {
                        match next {
                            's' => Some(' '),
                            't' => Some('\t'),
                            'n' => Some('\n'),
                            'r' => Some('\r'),
                            '\\' => Some('\\'),
                            _ => None,
                        }
                    };
                    match unescaped {
                        Some(unescaped) => {
                            token.push(unescaped);
                            chars.next();
                            token_started = true;
                        }
                        // Unknown escape: keep the backslash literally (the
                        // freedesktop spec declares no other escapes).
                        None => {
                            token.push('\\');
                            token_started = true;
                        }
                    }
                } else {
                    token.push('\\');
                    token_started = true;
                }
            }
            '%' => {
                if let Some(&next) = chars.peek() {
                    if next == '%' {
                        token.push('%');
                        chars.next();
                        token_started = true;
                    } else if is_field_code(next) {
                        // A field code standing alone in its token is
                        // dropped; inside a longer token (rare) the code is
                        // removed in place, closing the accumulated token.
                        chars.next();
                        if !token.is_empty() {
                            argv.push(std::mem::take(&mut token));
                        }
                        token_started = false;
                    } else {
                        token.push('%');
                        token_started = true;
                    }
                } else {
                    token.push('%');
                    token_started = true;
                }
            }
            other => {
                token.push(other);
                token_started = true;
            }
        }
    }
    if token_started && !token.is_empty() {
        argv.push(token);
    }
    argv
}

/// Escaping note: the escape branch always consumes both characters of an
/// escape sequence (`\"` inside quotes is content, never a closer), so a bare
/// quote character can only be reached while unescaped — no lookahead guard
/// is needed on the quote branch.
/// The freedesktop field codes launch drops (no file/URL/icon/name/context
/// substitution: the user's typed arguments are appended, never woven in).
/// The freedesktop field codes launch drops (no file/URL/icon/name/context
/// substitution: the user's typed arguments are appended, never woven in).
fn is_field_code(ch: char) -> bool {
    matches!(
        ch,
        'f' | 'F' | 'u' | 'U' | 'i' | 'c' | 'k' | 'd' | 'D' | 'n' | 'N' | 'v' | 'm'
    )
}

/// Mints the transient unit name: `app-` + the sanitized desktop id + a
/// short hash of (nanos, pid, app_id), so concurrent launches of the same
/// app never collide and the unit stays observable (`ps.list`,
/// `journal_query`) by the name the outcome reports.
fn unit_name(app_id: &str) -> String {
    let sanitized: String = app_id
        .to_ascii_lowercase()
        .chars()
        .map(|ch| {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let sanitized = sanitized
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in nanos
        .to_le_bytes()
        .into_iter()
        .chain(std::process::id().to_le_bytes())
        .chain(app_id.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("app-{sanitized}-{hash:016x}.service")
}

/// Joins an optional drain task, ignoring its result: the drain outcomes are
/// diagnostics, never facts (the typed answer comes from the wait status).
async fn join_optional<T: Send + 'static>(task: Option<tokio::task::JoinHandle<T>>) {
    if let Some(task) = task {
        let _ = task.await;
    }
}

/// Runs one bounded child: a direct execve of `program` with `args` (never a
/// shell), `kill_on_drop` so a budget-elapse kills the child with the future,
/// and the payload-preserving error mapping. Returns stdout on success.
async fn run_bounded(
    program: &str,
    args: &[String],
    budget: std::time::Duration,
    on_elapse: ContractError,
) -> Result<String, ContractError> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // A timed-out child must die with the call (the apt doctrine).
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|e| ContractError::NativeFailure {
        code: NF_SPAWN,
        payload: format!("spawn {program}: {e}"),
    })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| ContractError::NativeFailure {
            code: NF_SPAWN,
            payload: format!("{program} spawned without a stdout pipe"),
        })?;
    // Both pipes drain on detached tasks (bounded), so a chatty child can
    // never wedge itself on a full pipe while we wait; on budget elapse the
    // tasks drop with the child (kill_on_drop) and the pipes close.
    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        use tokio::io::AsyncReadExt;
        let _ = stdout.read_to_end(&mut buf).await;
        // 64 KiB of stdout is plenty: systemd-run prints one line, xdg-open
        // prints nothing.
        buf.truncate(64 * 1024);
        buf
    });
    let stderr_task = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut buf = Vec::new();
            // Bounded drain: 8 KiB of diagnostic is plenty for a payload.
            let cap = 8 * 1024;
            use tokio::io::AsyncReadExt;
            let _ = stderr.read_to_end(&mut buf).await;
            buf.truncate(cap);
            String::from_utf8_lossy(&buf).into_owned()
        })
    });
    let waited = tokio::time::timeout(budget, child.wait()).await;
    match waited {
        Ok(Ok(status)) if status.success() => {
            let out = stdout_task.await.unwrap_or_default();
            join_optional(stderr_task).await;
            Ok(String::from_utf8_lossy(&out).into_owned())
        }
        Ok(Ok(status)) => {
            let stderr = match stderr_task {
                Some(task) => task.await.unwrap_or_default(),
                None => String::new(),
            };
            let _ = stdout_task.await;
            // The typed code distinguishes the refusal shape; the original
            // message always rides the payload.
            let code = if program == XDG_OPEN {
                NF_OPEN
            } else {
                NF_SPAWN
            };
            Err(ContractError::NativeFailure {
                code,
                payload: format!(
                    "{program} failed with {}: {}",
                    status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| String::from("signal")),
                    stderr.trim()
                ),
            })
        }
        Ok(Err(e)) => {
            join_optional(Some(stdout_task)).await;
            join_optional(stderr_task).await;
            Err(ContractError::NativeFailure {
                code: NF_SPAWN,
                payload: format!("wait {program}: {e}"),
            })
        }
        Err(_elapsed) => {
            // The child is dropped here: kill_on_drop ends it. The outcome
            // mapping (indeterminate vs TimeoutError) is the caller's `on_elapse`.
            Err(on_elapse)
        }
    }
}

#[async_trait]
impl AppLauncher for LaunchAdapter {
    fn report(&self) -> AdapterReport {
        let mut capabilities = Vec::new();
        if let Some(fact) = self.read_fact() {
            capabilities.push(fact);
        }
        if let Some(fact) = self.launch_fact() {
            capabilities.push(fact);
        }
        AdapterReport {
            adapter: String::from("launch"),
            subsystem: String::from("app-launch"),
            contract_version: String::from(CONTRACT_VERSION),
            capabilities,
            notes: vec![],
        }
    }

    async fn list(&self) -> Result<Vec<AppEntry>, ContractError> {
        Ok(inventory(self.data_dirs.as_slice()))
    }

    async fn launch(&self, spec: AppSpec) -> Result<LaunchOutcome, ContractError> {
        // R2 sentinel bindings first, before any inventory or native work.
        if spec.app_id.starts_with(SENTINEL_SLOW) {
            return Err(ContractError::TimeoutError);
        }
        if spec.app_id.starts_with(SENTINEL_POISON) {
            return Err(ContractError::NativeFailure {
                code: NF_OS_DENIED,
                payload: String::from("os-denied: launch refused by suite poison sentinel"),
            });
        }
        // Provider honesty before native work: a stand without `systemd-run`
        // never reaches for it (the tools are hidden above the seam; a direct
        // trait call still refuses typed).
        if !self.launch_backend {
            return Err(ContractError::AdapterUnavailable);
        }
        let Some(entry) = self.entry_of(&spec.app_id) else {
            return Err(ContractError::NativeFailure {
                code: NF_APP_NOT_FOUND,
                payload: format!("app-not-found: {}", spec.app_id),
            });
        };
        if entry.exec.trim().is_empty() {
            return Err(ContractError::NativeFailure {
                code: NF_NO_EXEC,
                payload: format!(
                    "no-exec: the desktop entry of {} carries no Exec",
                    spec.app_id
                ),
            });
        }
        let unit = unit_name(&spec.app_id);
        let mut argv = vec![
            String::from("--user"),
            format!("--unit={unit}"),
            String::from("--collect"),
        ];
        // The desktop Exec argv (field codes dropped), then the user's typed
        // arguments verbatim — metacharacters are data elements of the argv,
        // and no shell ever assembles or splits them.
        argv.extend(exec_argv(&entry.exec));
        argv.extend(spec.args.iter().cloned());
        let unit_for_payload = unit.clone();
        run_bounded(
            SYSTEMD_RUN,
            &argv,
            LAUNCH_BUDGET,
            ContractError::NativeFailure {
                code: NF_LAUNCH_INDETERMINATE,
                payload: format!(
                    "launch-indeterminate: the systemd-run budget elapsed; the transient unit \
                     {unit_for_payload} may or may not exist — re-check with ps.list or \
                     journal_query before retrying"
                ),
            },
        )
        .await?;
        Ok(LaunchOutcome { unit_name: unit })
    }

    async fn open(&self, request: OpenRequest) -> Result<(), ContractError> {
        // R2 sentinel bindings first.
        if request.target.starts_with(SENTINEL_SLOW) {
            return Err(ContractError::TimeoutError);
        }
        if request.target.starts_with(SENTINEL_POISON) {
            return Err(ContractError::NativeFailure {
                code: NF_OS_DENIED,
                payload: String::from("os-denied: open refused by suite poison sentinel"),
            });
        }
        if !self.opener_present {
            return Err(ContractError::AdapterUnavailable);
        }
        // One target, one argv element, no shell: the target reaches
        // xdg-open byte-for-byte.
        run_bounded(
            XDG_OPEN,
            std::slice::from_ref(&request.target),
            OPEN_BUDGET,
            ContractError::TimeoutError,
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_adapter(opener: bool) -> LaunchAdapter {
        LaunchAdapter::with_data_dirs(
            &[String::from("systemd-run")],
            vec![PathBuf::from("/nonexistent-omnibus-suite")],
        )
        .then_set_opener(opener)
    }

    impl LaunchAdapter {
        fn then_set_opener(mut self, opener: bool) -> Self {
            self.opener_present = opener;
            self
        }
    }

    fn fixture_dir(exec: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let applications = dir.path().join("applications");
        std::fs::create_dir_all(&applications).expect("applications dir");
        let file = applications.join("omnibus-suite-app.desktop");
        std::fs::write(
            &file,
            format!(
                "[Desktop Entry]\nType=Application\nName=Omnibus Suite App\nIcon=utilities-terminal\nExec={exec}\nNoDisplay=false\n"
            ),
        )
        .expect("desktop file");
        (dir, applications)
    }

    #[test]
    fn inventory_parses_the_fixture_desktop_entry() {
        let (_dir, applications) = fixture_dir("/usr/bin/true");
        let entries = inventory(&[applications]);
        assert_eq!(entries.len(), 1, "one fixture entry");
        let entry = &entries[0];
        assert_eq!(entry.app_id, "omnibus-suite-app.desktop");
        assert_eq!(entry.name, "Omnibus Suite App");
        assert_eq!(entry.exec, "/usr/bin/true");
        assert_eq!(entry.icon.as_deref(), Some("utilities-terminal"));
        assert!(!entry.no_display);
    }

    #[test]
    fn inventory_resolves_namespaced_ids_and_nodisplay() {
        let dir = tempfile::tempdir().expect("tempdir");
        let applications = dir.path().join("applications");
        let nested = applications.join("kde");
        std::fs::create_dir_all(&nested).expect("nested dir");
        std::fs::write(
            nested.join("suitesub.desktop"),
            "[Desktop Entry]\nName=Suite Sub\nExec=/usr/bin/true\nNoDisplay=true\n",
        )
        .expect("nested desktop file");
        let entries = inventory(&[applications]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].app_id, "kde-suitesub.desktop");
        assert!(entries[0].no_display);
    }

    #[test]
    fn inventory_skips_broken_and_non_application_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let applications = dir.path().join("applications");
        std::fs::create_dir_all(&applications).expect("applications dir");
        // No Name: skipped (the DTO promises a display name).
        std::fs::write(
            applications.join("noname.desktop"),
            "[Desktop Entry]\nExec=/usr/bin/true\n",
        )
        .expect("file");
        // A link, not an application: skipped.
        std::fs::write(
            applications.join("link.desktop"),
            "[Desktop Entry]\nType=Link\nName=A Link\nURL=https://suite.invalid/\n",
        )
        .expect("file");
        // Not a desktop file at all: skipped.
        std::fs::write(applications.join("readme.txt"), "hello\n").expect("file");
        // Actions groups do not leak into the inventory.
        std::fs::write(
            applications.join("withactions.desktop"),
            "[Desktop Entry]\nName=With Actions\nExec=/usr/bin/true\n\n[Desktop Action new]\nName=New\nExec=/usr/bin/true --new\n",
        )
        .expect("file");
        let entries = inventory(&[applications]);
        assert_eq!(entries.len(), 1, "only the healthy application entry");
        assert_eq!(entries[0].app_id, "withactions.desktop");
        assert_eq!(entries[0].exec, "/usr/bin/true");
    }

    /// THE metacharacter invariant, at the argv layer: `;`, `|` and backtick
    /// in the user's typed arguments are single argv elements handed to the
    /// target process verbatim — no shell splits, pipes or substitutes them.
    #[test]
    fn metacharacters_travel_verbatim_in_the_argv() {
        let mut argv = exec_argv("/usr/bin/echo");
        argv.extend(
            ["arg;a", "arg|b", "arg`c", "arg$d", "$(e)", "arg;|`f"]
                .iter()
                .map(|s| String::from(*s)),
        );
        // The exact elements, in order, byte-for-byte.
        assert_eq!(
            argv,
            vec![
                "/usr/bin/echo",
                "arg;a",
                "arg|b",
                "arg`c",
                "arg$d",
                "$(e)",
                "arg;|`f",
            ]
        );
    }

    #[test]
    fn exec_argv_drops_field_codes_and_unescapes_the_desktop_dialect() {
        assert_eq!(exec_argv("firefox %u"), vec![String::from("firefox")]);
        assert_eq!(
            exec_argv("code --new-window %F"),
            vec![String::from("code"), String::from("--new-window")]
        );
        assert_eq!(
            exec_argv("100%% sure"),
            vec![String::from("100%"), String::from("sure")]
        );
        assert_eq!(exec_argv("100%%sure"), vec![String::from("100%sure")]);
        assert_eq!(
            exec_argv("quoted\\sspace \"in quotes\""),
            vec![String::from("quoted space"), String::from("in quotes")]
        );
        assert_eq!(exec_argv(""), Vec::<String>::new());
    }

    #[test]
    fn unit_name_is_deterministic_per_launch_and_shape_valid() {
        let first = unit_name("omnibus-suite-app.desktop");
        let second = unit_name("omnibus-suite-app.desktop");
        assert_ne!(first, second, "concurrent launches never collide");
        assert!(first.starts_with("app-omnibus-suite-app-"));
        assert!(first.ends_with(".service"));
        // Sanitized ids stay within the systemd unit-name alphabet.
        let weird = unit_name("Weird/App_ID.desktop");
        assert!(weird.starts_with("app-weird-app-id-"));
        for ch in weird.chars() {
            assert!(
                ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '.',
                "unit name character {ch} is outside the systemd alphabet"
            );
        }
    }

    #[tokio::test]
    async fn sentinel_bindings_answer_the_suite_shapes() {
        let adapter = empty_adapter(true);
        let slow = adapter
            .launch(AppSpec {
                app_id: String::from(SENTINEL_SLOW),
                args: vec![],
            })
            .await;
        assert!(matches!(slow, Err(ContractError::TimeoutError)));
        let poison = adapter
            .launch(AppSpec {
                app_id: String::from(SENTINEL_POISON),
                args: vec![],
            })
            .await;
        assert!(matches!(poison, Err(ContractError::NativeFailure { .. })));
        let open_slow = adapter
            .open(OpenRequest {
                target: String::from(SENTINEL_SLOW),
            })
            .await;
        assert!(matches!(open_slow, Err(ContractError::TimeoutError)));
        let open_poison = adapter
            .open(OpenRequest {
                target: String::from(SENTINEL_POISON),
            })
            .await;
        assert!(matches!(
            open_poison,
            Err(ContractError::NativeFailure { .. })
        ));
    }

    #[tokio::test]
    async fn missing_app_is_an_app_not_found_native_failure() {
        let adapter = empty_adapter(true);
        let missing = adapter
            .launch(AppSpec {
                app_id: String::from("no-such-app.desktop"),
                args: vec![],
            })
            .await;
        assert!(matches!(
            missing,
            Err(ContractError::NativeFailure {
                code: NF_APP_NOT_FOUND,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn refusals_happen_before_any_native_work() {
        // No launch backend: launch refuses AdapterUnavailable even though the
        // inventory could resolve an entry.
        let (_dir, applications) = fixture_dir("/usr/bin/true");
        let no_backend = LaunchAdapter::with_data_dirs(&[], vec![applications.clone()]);
        let refused = no_backend
            .launch(AppSpec {
                app_id: String::from("omnibus-suite-app.desktop"),
                args: vec![],
            })
            .await;
        assert!(matches!(refused, Err(ContractError::AdapterUnavailable)));
        // No opener: open refuses AdapterUnavailable without spawning.
        let no_opener =
            LaunchAdapter::with_data_dirs(&[String::from("systemd-run")], vec![applications]);
        let open_refused = no_opener
            .open(OpenRequest {
                target: String::from("https://suite.invalid/"),
            })
            .await;
        assert!(matches!(
            open_refused,
            Err(ContractError::AdapterUnavailable)
        ));
    }

    #[test]
    fn report_facts_carry_the_r3_reason_for_a_missing_opener() {
        let with_opener =
            LaunchAdapter::from_backends(&[String::from("systemd-run"), String::from("xdg-open")]);
        let report = with_opener.report();
        assert_eq!(report.adapter, "launch");
        assert_eq!(report.subsystem, "app-launch");
        let names: Vec<&str> = report
            .capabilities
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(names.contains(&APP_READ));
        assert!(names.contains(&APP_LAUNCH));

        let without_opener = LaunchAdapter::from_backends(&[String::from("systemd-run")]);
        let report = without_opener.report();
        let launch_fact = report
            .capabilities
            .iter()
            .find(|f| f.name == APP_LAUNCH)
            .expect("launch fact survives a missing opener");
        assert!(
            launch_fact
                .note
                .as_deref()
                .is_some_and(|note| note.contains("app.open is hidden")),
            "the R3 hiding reason names the hidden tool: {:?}",
            launch_fact.note
        );

        // No launch backend: no app.launch fact at all — both write tools
        // hide through the ladder (absence of the entry = absence of the
        // capability).
        let without_backend = LaunchAdapter::from_backends(&[]);
        let report = without_backend.report();
        assert!(
            !report.capabilities.iter().any(|f| f.name == APP_LAUNCH),
            "a missing systemd-run is provider absence, not a runtime surprise"
        );
    }

    #[test]
    fn report_capability_names_are_registry_constants_only() {
        // CoN holds at the seam: the report only names registry entries.
        let adapter =
            LaunchAdapter::from_backends(&[String::from("systemd-run"), String::from("xdg-open")]);
        for fact in adapter.report().capabilities {
            assert!(omnibus_contracts::capabilities::ALL.contains(&fact.name.as_str()));
        }
    }
}
