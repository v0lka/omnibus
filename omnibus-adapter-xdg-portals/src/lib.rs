#![cfg_attr(test, allow(clippy::expect_used))] // unit tests may expect; the seam must not
//! xdg-portals adapter: screenshot, clipboard, notifications via desktop portals.
//!
//! Native backend: the xdg-desktop-portal service on the session bus plus the
//! freedesktop notification service. The portal request/response dance is
//! asynchronous: the Response signal on the predictable request object path is
//! subscribed before the call and awaited with a 5s inner timeout (reads
//! additionally ride the core's tool-call budget; writes are not core-timed —
//! see `tools::dispatch`); timeouts map to [`ContractError::TimeoutError`].
//! Native types (portal handles, variant dictionaries) never leave this crate.
//!
//! Honesty of the capability ladder (R3):
//! - screenshot: present when the portal publishes the Screenshot interface;
//!   a user denial is a typed [`ContractError::ScopeDenied`].
//! - clipboard: the portal Clipboard interface is session-gated — a usable
//!   clipboard channel would require a RemoteDesktop session, whose creation is
//!   out of scope for omnibus (no input-emulation surface, ADR-009). The usable
//!   backend is therefore the platform's own clipboard tooling (roadmap 1.5.5
//!   How): `wl-clipboard` (wl-copy/wl-paste) with an `xclip` fallback, detected
//!   by binary presence. The capability is reported present only when a backend
//!   exists; without one the fact is absent (the ladder hides `clip_read`/
//!   `clip_write`, a direct call answers [`ContractError::AdapterUnavailable`])
//!   and the notes explain the session-gated portal channel.
//! - notify: present when the notification service is on the bus.

mod wire;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use omnibus_contracts::capabilities::{PORTAL_CLIPBOARD, PORTAL_NOTIFY, PORTAL_SCREENSHOT};
use omnibus_contracts::dto::portals::{
    ClipboardContent, NotifyRequest, NotifyResult, ScreenshotRequest, ScreenshotResult, Urgency,
};
use omnibus_contracts::dto::probe::{AdapterReport, CapabilityFact};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::portals::{Notify, ScreenCapture};
use omnibus_contracts::version::CONTRACT_VERSION;
use zbus::connection::Builder;
use zbus::proxy::{Builder as ProxyBuilder, CacheProperties};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, Proxy};
// zbus does not re-export a StreamExt; SignalStream is a plain futures Stream.
use futures_util::StreamExt;

/// The desktop portal service on the session bus.
pub const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
/// The object the portal interfaces live on.
pub const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
/// The freedesktop notification service (owned by the desktop shell here).
pub const NOTIFY_DEST: &str = "org.freedesktop.Notifications";
pub const NOTIFY_PATH: &str = "/org/freedesktop/Notifications";

const IFACE_INTROSPECTABLE: &str = "org.freedesktop.DBus.Introspectable";
const IFACE_SCREENSHOT: &str = "org.freedesktop.portal.Screenshot";
const IFACE_CLIPBOARD: &str = "org.freedesktop.portal.Clipboard";
const IFACE_REQUEST: &str = "org.freedesktop.portal.Request";
const IFACE_NOTIFICATIONS: &str = "org.freedesktop.Notifications";

/// Inner timeout for the asynchronous portal Response dance.
const RESPONSE_BUDGET: Duration = Duration::from_secs(5);
/// Inner timeout for one synchronous portal/notification call.
const CALL_BUDGET: Duration = Duration::from_secs(5);
/// The screenshot read cap: one call never buffers more than this much of
/// the portal-produced file (the result then holds the bytes twice over —
/// raw plus base64). Sits far above any legitimate desktop capture (a busy
/// 4K PNG stays well under 10 MiB) and far below anything that could
/// pressure the daemon.
const MAX_SCREENSHOT_BYTES: u64 = 16 * 1024 * 1024;
/// The clipboard read cap: one `clip_read` never buffers more than this much
/// of the backend's stdout (the screenshot cap discipline, in the clipboard
/// wording — 2 MiB is far above any legitimate text clipboard and matches the
/// fs read ceiling).
const MAX_CLIPBOARD_BYTES: u64 = 2 * 1024 * 1024;
/// Inner budget for one clipboard backend subprocess (the same wall-clock
/// discipline as the synchronous portal calls).
const CLIPBOARD_BUDGET: Duration = Duration::from_secs(5);

/// The usable clipboard backend (roadmap 1.5.5 How): the portal Clipboard
/// channel is session-gated behind RemoteDesktop, so a real round trip rides
/// the platform's own clipboard tooling, detected by binary presence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardBackend {
    /// wl-clipboard: `wl-copy` writes, `wl-paste` reads (Wayland, and XWayland
    /// sessions where the tools are installed).
    WlClipboard { copy: PathBuf, paste: PathBuf },
    /// xclip: one binary reads (`-o`) and writes (`-i`) the CLIPBOARD
    /// selection (X11).
    Xclip { bin: PathBuf },
}

impl ClipboardBackend {
    /// The agent-visible backend name for the capability note.
    pub fn name(&self) -> &'static str {
        match self {
            Self::WlClipboard { .. } => "wl-clipboard",
            Self::Xclip { .. } => "xclip",
        }
    }

    /// Detection by binary presence in the given directories, wl-clipboard
    /// first (the Wayland-native pair), xclip as the X11 fallback. Both
    /// wl-clipboard binaries must exist: a half-installed backend would fail
    /// exactly one direction of the round trip.
    pub fn detect_in(dirs: &[PathBuf]) -> Option<Self> {
        for dir in dirs {
            let copy = dir.join("wl-copy");
            let paste = dir.join("wl-paste");
            if copy.is_file() && paste.is_file() {
                return Some(Self::WlClipboard { copy, paste });
            }
        }
        for dir in dirs {
            let bin = dir.join("xclip");
            if bin.is_file() {
                return Some(Self::Xclip { bin });
            }
        }
        None
    }

    /// The standard detection set: `/usr/bin`, the same fixed location the
    /// probe scans (a PATH walk would make the capability fact depend on how
    /// the daemon was launched — the fact must be a machine fact).
    pub fn detect() -> Option<Self> {
        Self::detect_in(&[PathBuf::from("/usr/bin")])
    }

    /// Reads the clipboard as text through this backend.
    async fn read(&self) -> Result<ClipboardContent, ContractError> {
        let (program, args): (&Path, Vec<&str>) = match self {
            Self::WlClipboard { paste, .. } => (
                paste.as_path(),
                vec!["--no-newline", "--type", "text/plain"],
            ),
            Self::Xclip { bin } => (
                bin.as_path(),
                vec!["-selection", "clipboard", "-o", "-t", "text/plain"],
            ),
        };
        let out = run_bounded(
            program,
            &args,
            None,
            CLIPBOARD_BUDGET,
            MAX_CLIPBOARD_BYTES,
            true,
        )
        .await?;
        Ok(ClipboardContent {
            mime: String::from("text/plain"),
            text: String::from_utf8_lossy(&out.stdout).into_owned(),
            size_bytes: u64::try_from(out.stdout.len()).unwrap_or(u64::MAX),
        })
    }

    /// Writes text to the clipboard through this backend.
    async fn write(&self, text: &str) -> Result<(), ContractError> {
        let (program, args): (&Path, Vec<&str>) = match self {
            Self::WlClipboard { copy, .. } => (copy.as_path(), vec!["--type", "text/plain"]),
            Self::Xclip { bin } => (
                bin.as_path(),
                vec!["-selection", "clipboard", "-i", "-t", "text/plain"],
            ),
        };
        run_bounded(
            program,
            &args,
            Some(text.as_bytes().to_vec()),
            CLIPBOARD_BUDGET,
            MAX_CLIPBOARD_BYTES,
            false,
        )
        .await?;
        Ok(())
    }
}

/// One bounded backend run's captured stdout (read path only; the write path
/// is fire-and-forget — see [`run_bounded`]).
struct BackendOutput {
    stdout: Vec<u8>,
}

/// Spawns one clipboard-backend subprocess under the wall-clock budget.
///
/// Two shapes, because the two directions of the round trip behave
/// differently: `wl-copy`/`xclip -i` fork a resident child that serves later
/// paste requests and INHERIT the spawned process's pipes — capturing their
/// output would never see EOF, so the write path runs fire-and-forget
/// (`capture = false`: stdin piped, stdout/stderr to null, the direct child's
/// exit status checked). The read path (`wl-paste`/`xclip -o`) exits after
/// printing, and its stdout is captured under a hard byte cap (the screenshot
/// `take` discipline — a backend that streams more than the cap loses the
/// tail and the run is a typed cap refusal). Every failure is a contract
/// error; the budget kills a stalled child (`kill_on_drop`).
async fn run_bounded(
    program: &Path,
    args: &[&str],
    stdin: Option<Vec<u8>>,
    budget: Duration,
    cap: u64,
    capture: bool,
) -> Result<BackendOutput, ContractError> {
    use tokio::io::AsyncReadExt as _;
    use tokio::io::AsyncWriteExt as _;

    let map_err = |what: String, e: std::io::Error| match e.kind() {
        std::io::ErrorKind::NotFound => ContractError::AdapterUnavailable,
        _ => ContractError::NativeFailure {
            code: 1,
            payload: format!("{what}: {e}"),
        },
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        // A timed-out backend child must die with the call.
        .kill_on_drop(true);
    // A spawn can transiently fail with ETXTBSY ("Text file busy", raw OS
    // error 26): the kernel denies exec while any writer still holds the
    // file, and such a writer is always brief — a package upgrade replacing
    // the backend binary mid-run, or (in tests) a fixture written just
    // before the exec. A short bounded retry settles the race; anything
    // else, or a denial that outlives the retries, is the honest contract
    // error.
    let what = format!("clipboard backend {}", program.display());
    let retry_pause = Duration::from_millis(25);
    let mut attempts_left = 3usize;
    let mut child = loop {
        match command.spawn() {
            Ok(child) => break child,
            Err(e) if e.raw_os_error() == Some(26) && attempts_left > 0 => {
                attempts_left -= 1;
                tokio::time::sleep(retry_pause).await;
            }
            Err(e) => return Err(map_err(what, e)),
        }
    };

    let stdin_handle = if stdin.is_some() {
        Some(
            child
                .stdin
                .take()
                .ok_or_else(|| ContractError::NativeFailure {
                    code: 1,
                    payload: format!("clipboard backend {} has no stdin pipe", program.display()),
                })?,
        )
    } else {
        None
    };
    let mut stdout_handle = if capture {
        Some(
            child
                .stdout
                .take()
                .ok_or_else(|| ContractError::NativeFailure {
                    code: 1,
                    payload: format!("clipboard backend {} has no stdout pipe", program.display()),
                })?,
        )
    } else {
        None
    };
    let mut stderr_handle = if capture { child.stderr.take() } else { None };

    let total = async {
        if let Some(input) = stdin {
            // The pipe was configured with the process above; if the OS ever
            // fails to hand it over, the seam answers a contract error instead
            // of panicking.
            let Some(mut handle) = stdin_handle else {
                return Err(ContractError::NativeFailure {
                    code: 1,
                    payload: format!("clipboard backend {} has no stdin pipe", program.display()),
                });
            };
            handle
                .write_all(&input)
                .await
                .map_err(|e| map_err(String::from("clipboard stdin"), e))?;
            handle
                .shutdown()
                .await
                .map_err(|e| map_err(String::from("clipboard stdin"), e))?;
        }
        let stdout = if let Some(handle) = stdout_handle.take() {
            let mut buf: Vec<u8> = Vec::new();
            // +1 turns an oversized clipboard into a typed cap refusal
            // instead of a silently bigger buffer.
            let took = {
                // `take` bounds the read; the owned pipe survives this block
                // and drops (SIGPIPE for a still-writing backend) when the
                // arm ends, still before `wait()` below.
                let mut limited = handle.take(cap + 1);
                limited
                    .read_to_end(&mut buf)
                    .await
                    .map_err(|e| map_err(String::from("clipboard stdout"), e))?
            };
            if u64::try_from(took).unwrap_or(u64::MAX) > cap {
                return Err(ContractError::Unsupported {
                    reason: format!(
                        "clipboard: {took} bytes exceeds the {} KiB per-call cap",
                        cap / 1024
                    ),
                });
            }
            buf
        } else {
            Vec::new()
        };
        let mut stderr: Vec<u8> = Vec::new();
        if let Some(handle) = stderr_handle.as_mut() {
            let _ = handle.take(64 * 1024).read_to_end(&mut stderr).await;
        }
        drop(stderr_handle);
        let status = child
            .wait()
            .await
            .map_err(|e| map_err(format!("clipboard backend {}", program.display()), e))?;
        if !status.success() {
            return Err(ContractError::NativeFailure {
                code: 1,
                payload: format!(
                    "clipboard backend {} failed ({}): {}",
                    program.display(),
                    status,
                    String::from_utf8_lossy(&stderr).trim()
                ),
            });
        }
        Ok(BackendOutput { stdout })
    };
    tokio::time::timeout(budget, total)
        .await
        .map_err(|_| ContractError::TimeoutError)?
}

/// Live portal backend over the session bus.
#[derive(Clone)]
pub struct PortalsAdapter {
    conn: Connection,
    /// The portal service answers on the session bus.
    portal_present: bool,
    /// The portal object publishes the Screenshot interface.
    screenshot_iface: bool,
    /// The portal object publishes the Clipboard interface.
    clipboard_iface: bool,
    /// The notification service answers on the session bus.
    notify_available: bool,
    /// The usable clipboard backend (wl-clipboard/xclip), detected by binary
    /// presence in `/usr/bin` — `None` means clip_read/clip_write are hidden.
    clipboard_backend: Option<ClipboardBackend>,
}

impl PortalsAdapter {
    /// Connects to the session bus and probes the portal and notification
    /// services. Fails with [`ContractError::AdapterUnavailable`] only when no
    /// session bus is reachable at all; a bus that answers but a portal that
    /// never replies degrades to "absent" capability facts instead.
    pub async fn connect() -> Result<Self, ContractError> {
        // The probes below run on this connection; `method_timeout` bounds
        // every method call issued on it (zbus defaults to no timeout), so a
        // session bus that never answers Introspect/NameHasOwner degrades to
        // "portal absent" instead of hanging daemon startup forever.
        let conn = Builder::session()
            .map_err(|_| ContractError::AdapterUnavailable)?
            .method_timeout(CALL_BUDGET)
            .build()
            .await
            .map_err(|_| ContractError::AdapterUnavailable)?;
        let portal_present = name_has_owner(&conn, PORTAL_DEST).await;
        let notify_available = name_has_owner(&conn, NOTIFY_DEST).await;
        let (screenshot_iface, clipboard_iface) = if portal_present {
            match introspect(&conn, PORTAL_DEST, PORTAL_PATH).await {
                Some(xml) => (
                    xml.contains(IFACE_SCREENSHOT),
                    xml.contains(IFACE_CLIPBOARD),
                ),
                None => (false, false),
            }
        } else {
            (false, false)
        };
        Ok(Self {
            conn,
            portal_present,
            screenshot_iface,
            clipboard_iface,
            notify_available,
            clipboard_backend: ClipboardBackend::detect(),
        })
    }

    /// Whether the portal service is on the bus.
    pub fn portal_present(&self) -> bool {
        self.portal_present
    }

    /// The detected clipboard backend, if any (roadmap 1.5.5: wl-clipboard or
    /// xclip, by binary presence).
    pub fn clipboard_backend(&self) -> Option<&ClipboardBackend> {
        self.clipboard_backend.as_ref()
    }

    /// Whether the Screenshot portal interface is published.
    pub fn screenshot_available(&self) -> bool {
        self.portal_present && self.screenshot_iface
    }

    /// Whether the Clipboard portal interface is published (still session-gated).
    pub fn clipboard_present(&self) -> bool {
        self.portal_present && self.clipboard_iface
    }

    /// Whether the notification service is on the bus.
    pub fn notify_available(&self) -> bool {
        self.notify_available
    }

    /// A plain (non-caching) proxy on the connected session bus.
    async fn proxy<'a>(
        &'a self,
        dest: &'a str,
        path: &'a str,
        iface: &'a str,
    ) -> Result<Proxy<'a>, ContractError> {
        let builder = ProxyBuilder::new(&self.conn)
            .destination(dest)
            .and_then(|builder| builder.path(path))
            .and_then(|builder| builder.interface(iface))
            .map_err(Self::native)?;
        builder
            .cache_properties(CacheProperties::No)
            .build()
            .await
            .map_err(Self::native)
    }

    fn native(error: impl std::fmt::Display) -> ContractError {
        ContractError::NativeFailure {
            code: 1,
            payload: error.to_string(),
        }
    }

    /// Capability fact for one registry constant (CoN: names come from constants).
    fn capability_fact(&self, name: &str, status: &str, note: &str) -> CapabilityFact {
        CapabilityFact {
            name: String::from(name),
            ver: String::from(CONTRACT_VERSION),
            status: String::from(status),
            value: None,
            note: Some(String::from(note)),
            remedy: None,
        }
    }

    fn screenshot_fact(&self) -> CapabilityFact {
        if self.screenshot_available() {
            self.capability_fact(
                PORTAL_SCREENSHOT,
                "present",
                "the portal Screenshot interface answers on the session bus",
            )
        } else {
            self.capability_fact(
                PORTAL_SCREENSHOT,
                "degraded",
                "the portal Screenshot interface is not published on this machine",
            )
        }
    }

    /// The clipboard capability fact — present only when a usable backend
    /// exists (roadmap 1.5.5: the capability follows the backend, never the
    /// session-gated portal channel). `None` means the fact is absent, so the
    /// ladder hides `clip_read`/`clip_write` (AC: a stand without a backend
    /// hides the tools; the capabilities document explains via the notes).
    fn clipboard_fact(&self) -> Option<CapabilityFact> {
        let backend = self.clipboard_backend.as_ref()?;
        let note = match backend {
            ClipboardBackend::WlClipboard { .. } => {
                "clipboard read/write via wl-clipboard (wl-copy/wl-paste); the portal \
                 Clipboard channel stays unused (session-gated behind RemoteDesktop)"
            }
            ClipboardBackend::Xclip { .. } => {
                "clipboard read/write via xclip (CLIPBOARD selection); the portal \
                 Clipboard channel stays unused (session-gated behind RemoteDesktop)"
            }
        };
        Some(self.capability_fact(PORTAL_CLIPBOARD, "present", note))
    }

    fn notify_fact(&self) -> CapabilityFact {
        if self.notify_available {
            self.capability_fact(
                PORTAL_NOTIFY,
                "present",
                "the notification service answers on the session bus",
            )
        } else {
            self.capability_fact(
                PORTAL_NOTIFY,
                "degraded",
                "no notification service on the session bus",
            )
        }
    }

    fn base_report(&self) -> AdapterReport {
        let mut notes = Vec::new();
        notes.push(if self.portal_present {
            String::from("portal service present on the session bus")
        } else {
            String::from("no portal service on the session bus")
        });
        notes.push(if self.notify_available {
            String::from("notification service present")
        } else {
            String::from("no notification service on the session bus")
        });
        notes.push(match self.clipboard_backend.as_ref() {
            Some(backend) => format!("clipboard backend: {}", backend.name()),
            None => String::from(
                "no clipboard backend (wl-clipboard/xclip absent): clip_read/clip_write are \
                 hidden; the portal Clipboard channel is session-gated behind RemoteDesktop \
                 and is never created",
            ),
        });
        let mut capabilities = vec![self.screenshot_fact(), self.notify_fact()];
        if let Some(clipboard) = self.clipboard_fact() {
            capabilities.push(clipboard);
        }
        AdapterReport {
            adapter: String::from("xdg-portals"),
            subsystem: String::from("portals"),
            contract_version: String::from(CONTRACT_VERSION),
            capabilities,
            notes,
        }
    }

    /// The screenshot portal dance: subscribe to the Response signal on the
    /// predictable request path, then call Screenshot with a non-interactive
    /// request, then await the response and read the returned file URI.
    async fn screenshot_inner(
        &self,
        _request: ScreenshotRequest,
    ) -> Result<ScreenshotResult, ContractError> {
        let unique = self
            .conn
            .unique_name()
            .map(|name| name.to_string())
            .ok_or(ContractError::AdapterUnavailable)?;
        let token = wire::new_handle_token();
        let request_object = wire::request_path(&wire::sender_part(&unique), &token);
        // Subscribe BEFORE the call: the portal may answer within milliseconds.
        let request_proxy = self
            .proxy(PORTAL_DEST, &request_object, IFACE_REQUEST)
            .await?;
        let mut responses = request_proxy
            .receive_signal("Response")
            .await
            .map_err(Self::native)?;
        let screenshot_proxy = self
            .proxy(PORTAL_DEST, PORTAL_PATH, IFACE_SCREENSHOT)
            .await?;
        let options: HashMap<&str, Value> = HashMap::from([
            ("handle_token", Value::from(token.as_str())),
            ("interactive", Value::from(false)),
        ]);
        let call = tokio::time::timeout(
            CALL_BUDGET,
            screenshot_proxy.call_method("Screenshot", &("", options)),
        )
        .await
        .map_err(|_| ContractError::TimeoutError)?
        .map_err(Self::native)?;
        let _handle: OwnedObjectPath = call.body().deserialize().map_err(Self::native)?;
        let message = tokio::time::timeout(RESPONSE_BUDGET, responses.next())
            .await
            .map_err(|_| ContractError::TimeoutError)?
            .ok_or_else(|| Self::native("the portal response stream closed"))?;
        let (code, results): (u32, HashMap<String, OwnedValue>) =
            message.body().deserialize().map_err(Self::native)?;
        match code {
            0 => {}
            1 => return Err(ContractError::ScopeDenied),
            other => {
                return Err(ContractError::NativeFailure {
                    code: i32::try_from(other).unwrap_or(1),
                    payload: format!("the portal ended the screenshot request ({other})"),
                });
            }
        }
        let uri = wire::uri_from_results(&results)
            .ok_or_else(|| Self::native("the portal response carries no uri"))?;
        let path = wire::file_uri_to_path(&uri)
            .ok_or_else(|| Self::native(format!("unreadable screenshot uri {uri}")))?;
        let bytes = Self::read_bounded(&path).await?;
        Ok(ScreenshotResult {
            mime: String::from("image/png"),
            image_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            size_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        })
    }

    /// Reads the portal-produced screenshot under the bounded-capture
    /// discipline the rest of this changeset reads bytes with:
    /// - a non-regular file is refused (`Unsupported`) — a FIFO or device
    ///   would park the open or stream forever;
    /// - the size is judged on the opened handle (fs-xdg's "judge the
    ///   handle, not the path" idiom), then bounded a second time by a hard
    ///   `take` cap, so even a file growing between the two checks cannot
    ///   inflate the buffer past [`MAX_SCREENSHOT_BYTES`];
    /// - oversize is a typed cap refusal, mirroring `fs_write`'s.
    ///
    /// The per-component `O_NOFOLLOW` scope walk of `fs_read` is deliberately
    /// not replicated: the URI here is minted by the portal on the session
    /// bus — a system component, never an untrusted MCP client — so the
    /// out-of-scope-redirect threat that walk closes does not exist here.
    async fn read_bounded(path: &str) -> Result<Vec<u8>, ContractError> {
        use tokio::io::AsyncReadExt as _;
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|e| Self::native(format!("cannot read {path}: {e}")))?;
        // Judge the handle, not the path.
        let meta = file
            .metadata()
            .await
            .map_err(|e| Self::native(format!("cannot read {path}: {e}")))?;
        if !meta.is_file() {
            return Err(ContractError::Unsupported {
                reason: format!("screenshot: not a regular file: {path}"),
            });
        }
        if meta.len() > MAX_SCREENSHOT_BYTES {
            return Err(screenshot_cap_error(meta.len()));
        }
        let mut bytes = Vec::new();
        let took = file
            // +1 turns a concurrent size growth into the same honest cap
            // refusal instead of a silently bigger buffer.
            .take(MAX_SCREENSHOT_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| Self::native(format!("cannot read {path}: {e}")))?;
        if took as u64 > MAX_SCREENSHOT_BYTES {
            return Err(screenshot_cap_error(took as u64));
        }
        Ok(bytes)
    }

    /// Shows one notification through the freedesktop notification service.
    async fn notify_inner(&self, request: NotifyRequest) -> Result<NotifyResult, ContractError> {
        let proxy = self
            .proxy(NOTIFY_DEST, NOTIFY_PATH, IFACE_NOTIFICATIONS)
            .await?;
        let urgency: u8 = match request.urgency {
            Some(Urgency::Low) => 0,
            None | Some(Urgency::Normal) => 1,
            Some(Urgency::Critical) => 2,
        };
        let hints: HashMap<&str, Value> = HashMap::from([("urgency", Value::U8(urgency))]);
        let call = tokio::time::timeout(
            CALL_BUDGET,
            proxy.call_method(
                "Notify",
                &(
                    "omnibusd",
                    0u32,
                    "",
                    request.title.as_str(),
                    request.body.as_str(),
                    Vec::<String>::new(),
                    hints,
                    -1i32,
                ),
            ),
        )
        .await
        .map_err(|_| ContractError::TimeoutError)?
        .map_err(Self::native)?;
        // A successful call means the service accepted the notification; the
        // returned id is its handle (0 would mean no notification was shown).
        let (id,): (u32,) = call.body().deserialize().map_err(Self::native)?;
        Ok(NotifyResult { delivered: id > 0 })
    }
}

/// The typed over-cap refusal for a screenshot read (the `fs_write`
/// `write_cap_error` shape, in the screenshot wording).
fn screenshot_cap_error(bytes: u64) -> ContractError {
    ContractError::Unsupported {
        reason: format!(
            "screenshot: {bytes} bytes exceeds the {} MiB per-call capture cap",
            MAX_SCREENSHOT_BYTES / (1024 * 1024)
        ),
    }
}

/// `NameHasOwner` over the raw bus interface; false on any failure.
async fn name_has_owner(conn: &Connection, name: &str) -> bool {
    let Ok(proxy) = Proxy::new(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await
    else {
        return false;
    };
    let Ok(message) = proxy.call_method("NameHasOwner", &(name)).await else {
        return false;
    };
    message.body().deserialize::<bool>().unwrap_or(false)
}

/// D-Bus introspection XML of one object, when it answers.
async fn introspect(conn: &Connection, dest: &str, path: &str) -> Option<String> {
    let proxy = Proxy::new(conn, dest, path, IFACE_INTROSPECTABLE)
        .await
        .ok()?;
    let message = proxy.call_method("Introspect", &()).await.ok()?;
    message.body().deserialize::<String>().ok()
}

#[async_trait]
impl ScreenCapture for PortalsAdapter {
    fn report(&self) -> AdapterReport {
        self.base_report()
    }

    async fn clip_read(&self) -> Result<ClipboardContent, ContractError> {
        // The usable backend is wl-clipboard/xclip by binary presence; without
        // one there is no reachable clipboard backend (the portal channel is
        // session-gated and never created), so the honest answer is the
        // absence of an active adapter — and the ladder hides the tool.
        match self.clipboard_backend.as_ref() {
            Some(backend) => backend.read().await,
            None => Err(ContractError::AdapterUnavailable),
        }
    }

    async fn clip_write(&self, content: ClipboardContent) -> Result<(), ContractError> {
        // Same backend gate as clip_read; writes are additionally grant-gated
        // in the core, never here. Only the text payload rides the backend:
        // the text/plain channel is what both toolchains expose.
        match self.clipboard_backend.as_ref() {
            Some(backend) => backend.write(&content.text).await,
            None => Err(ContractError::AdapterUnavailable),
        }
    }

    async fn screenshot(
        &self,
        request: ScreenshotRequest,
    ) -> Result<ScreenshotResult, ContractError> {
        if !self.screenshot_available() {
            return Err(ContractError::AdapterUnavailable);
        }
        let result = tokio::time::timeout(RESPONSE_BUDGET * 2, self.screenshot_inner(request))
            .await
            .map_err(|_| ContractError::TimeoutError)??;
        Ok(result)
    }
}

#[async_trait]
impl Notify for PortalsAdapter {
    fn report(&self) -> AdapterReport {
        self.base_report()
    }

    async fn send(&self, request: NotifyRequest) -> Result<NotifyResult, ContractError> {
        if !self.notify_available {
            return Err(ContractError::AdapterUnavailable);
        }
        let result = tokio::time::timeout(CALL_BUDGET * 2, self.notify_inner(request))
            .await
            .map_err(|_| ContractError::TimeoutError)??;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_contracts::dto::portals::{ImageFormat, ScreenshotResult as _ScreenshotResult};

    /// The report/facts tests only build the struct; they need a Connection
    /// value but never put a message on the wire. Headless machines skip.
    async fn live_connection() -> Option<Connection> {
        Connection::session().await.ok()
    }

    #[tokio::test]
    async fn facts_report_honest_statuses() {
        let Some(conn) = live_connection().await else {
            eprintln!("skip: no session bus on this machine");
            return;
        };
        let adapter = PortalsAdapter {
            conn,
            portal_present: false,
            screenshot_iface: false,
            clipboard_iface: false,
            notify_available: false,
            clipboard_backend: None,
        };
        let report = ScreenCapture::report(&adapter);
        let statuses: Vec<(String, String)> = report
            .capabilities
            .iter()
            .map(|fact| (fact.name.clone(), fact.status.clone()))
            .collect();
        assert!(statuses.iter().all(|(_, status)| status == "degraded"));
        // Without a clipboard backend the fact is ABSENT (the ladder hides
        // clip_read/clip_write — AC 1.5.5), explained in the notes.
        assert!(
            report
                .capabilities
                .iter()
                .all(|fact| fact.name != PORTAL_CLIPBOARD),
            "no backend: the clipboard fact must be absent, not degraded"
        );
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("no clipboard backend")),
            "the notes explain the hidden clipboard tools"
        );
        // CoN rule: names come from the registry constants.
        assert!(
            report
                .capabilities
                .iter()
                .all(|fact| omnibus_contracts::capabilities::ALL.contains(&fact.name.as_str()))
        );
    }

    #[tokio::test]
    async fn present_interfaces_report_present() {
        let Some(conn) = live_connection().await else {
            eprintln!("skip: no session bus on this machine");
            return;
        };
        let adapter = PortalsAdapter {
            conn,
            portal_present: true,
            screenshot_iface: true,
            clipboard_iface: true,
            notify_available: true,
            clipboard_backend: None,
        };
        let report = ScreenCapture::report(&adapter);
        let screenshot = report
            .capabilities
            .iter()
            .find(|fact| fact.name == PORTAL_SCREENSHOT)
            .map(|fact| fact.status.clone())
            .unwrap_or_default();
        assert_eq!(screenshot, "present");
        // A published portal Clipboard interface is still no backend: the
        // channel is session-gated behind RemoteDesktop, so without
        // wl-clipboard/xclip the fact is absent and the tools stay hidden.
        assert!(
            !report
                .capabilities
                .iter()
                .any(|fact| fact.name == PORTAL_CLIPBOARD),
            "session-gated portal channel is never reported as a usable backend"
        );
    }

    /// AC 1.5.5: the capability follows the backend — with wl-clipboard
    /// detected, the fact is present and names the backend.
    #[tokio::test]
    async fn backend_presence_makes_the_clipboard_fact_present() {
        let Some(conn) = live_connection().await else {
            eprintln!("skip: no session bus on this machine");
            return;
        };
        let adapter = PortalsAdapter {
            conn,
            portal_present: false,
            screenshot_iface: false,
            clipboard_iface: false,
            notify_available: false,
            clipboard_backend: Some(ClipboardBackend::WlClipboard {
                copy: PathBuf::from("/usr/bin/wl-copy"),
                paste: PathBuf::from("/usr/bin/wl-paste"),
            }),
        };
        let report = ScreenCapture::report(&adapter);
        let clipboard = report
            .capabilities
            .iter()
            .find(|fact| fact.name == PORTAL_CLIPBOARD)
            .expect("clipboard fact present with a backend");
        assert_eq!(clipboard.status, "present");
        assert!(
            clipboard
                .note
                .as_deref()
                .is_some_and(|note| note.contains("wl-clipboard")),
            "the note names the backend, got {:?}",
            clipboard.note
        );
    }

    #[tokio::test]
    async fn absent_backends_answer_adapter_unavailable() {
        let Some(conn) = live_connection().await else {
            eprintln!("skip: no session bus on this machine");
            return;
        };
        let adapter = PortalsAdapter {
            conn,
            portal_present: false,
            screenshot_iface: false,
            clipboard_iface: false,
            notify_available: false,
            clipboard_backend: None,
        };
        use omnibus_contracts::spi::portals::{Notify as _, ScreenCapture as _};
        let shot = adapter
            .screenshot(ScreenshotRequest {
                format: ImageFormat::Png,
            })
            .await;
        assert!(matches!(shot, Err(ContractError::AdapterUnavailable)));
        let read = adapter.clip_read().await;
        assert!(matches!(read, Err(ContractError::AdapterUnavailable)));
        let written = adapter
            .clip_write(ClipboardContent {
                mime: String::from("text/plain"),
                text: String::from("x"),
                size_bytes: 1,
            })
            .await;
        assert!(matches!(written, Err(ContractError::AdapterUnavailable)));
        let sent = adapter
            .send(NotifyRequest {
                title: String::from("t"),
                body: String::from("b"),
                urgency: None,
            })
            .await;
        assert!(matches!(sent, Err(ContractError::AdapterUnavailable)));
    }

    /// DTO codec from a fixture payload (no D-Bus): the result the adapter
    /// builds from portal bytes must round-trip serde untouched.
    #[test]
    fn screenshot_result_roundtrips_from_fixture() {
        let result = ScreenshotResult {
            mime: String::from("image/png"),
            image_base64: String::from("aGVsbG8="),
            size_bytes: 5,
        };
        let text = serde_json::to_string(&result).unwrap_or_default();
        let fallback = ScreenshotResult {
            mime: String::new(),
            image_base64: String::new(),
            size_bytes: 0,
        };
        let back: _ScreenshotResult = serde_json::from_str(&text).unwrap_or(fallback);
        assert_eq!(back, result);
    }

    /// A unique scratch path under the session temp dir; the read_bounded
    /// tests never touch a bus, so they run on headless machines too.
    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("omnibus-portals-{}-{name}", std::process::id()))
    }

    #[tokio::test]
    async fn screenshot_read_serves_a_regular_file_within_the_cap() {
        let path = scratch("ok.png");
        std::fs::write(&path, b"pngbytes").expect("seed scratch file");
        let bytes = PortalsAdapter::read_bounded(path.to_str().expect("utf8 temp path"))
            .await
            .expect("in-cap read must succeed");
        assert_eq!(bytes, b"pngbytes".to_vec());
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn screenshot_read_refuses_non_regular_files() {
        // A directory: on Linux open() succeeds, so the refusal must come
        // from the handle check, not from the open itself.
        let dir = scratch("dir.png");
        std::fs::create_dir_all(&dir).expect("seed scratch dir");
        let refused = PortalsAdapter::read_bounded(dir.to_str().expect("utf8 temp path")).await;
        assert!(
            matches!(refused, Err(ContractError::Unsupported { .. })),
            "a non-regular file must be refused: {refused:?}"
        );
        let _ = std::fs::remove_dir(&dir);
    }

    #[tokio::test]
    async fn screenshot_read_refuses_files_over_the_cap() {
        let path = scratch("huge.png");
        // Sparse: the oversize lives in the metadata, not on disk.
        let file = std::fs::File::create(&path).expect("seed scratch file");
        file.set_len(MAX_SCREENSHOT_BYTES + 1).expect("oversize");
        drop(file);
        let refused = PortalsAdapter::read_bounded(path.to_str().expect("utf8 temp path")).await;
        match refused {
            Err(ContractError::Unsupported { reason }) => {
                assert!(reason.contains("capture cap"), "{reason}");
            }
            other => panic!("over-cap read must be refused, got {other:?}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    // ---- wl-clipboard / xclip fallback backend (roadmap 1.5.5) ----

    /// An executable fixture script standing in for a backend binary; the
    /// backend tests never touch a bus, so they run on headless machines too.
    fn fixture_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write fixture script");
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make the fixture executable");
        path
    }

    /// The wl pair over a state file: wl-copy stores stdin, wl-paste emits it.
    fn wl_fixture(dir: &Path) -> (PathBuf, PathBuf) {
        let state = dir.join("state.txt");
        let state_str = state.to_str().expect("utf8 temp path").to_owned();
        let paste = fixture_script(
            dir,
            "wl-paste",
            &format!("#!/bin/sh\ncat \"{state_str}\" 2>/dev/null || true\n"),
        );
        let copy = fixture_script(
            dir,
            "wl-copy",
            &format!("#!/bin/sh\ncat > \"{state_str}\"\n"),
        );
        (copy, paste)
    }

    /// AC 1.5.5: the round trip rides the fallback backend — write then read
    /// preserves the text, and the content carries the text/plain mime.
    #[tokio::test]
    async fn clipboard_round_trip_via_the_wlclipboard_backend() {
        let dir = scratch("wl-backend");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let (copy, paste) = wl_fixture(&dir);
        let backend = ClipboardBackend::WlClipboard { copy, paste };
        backend
            .write("omnibus-clipboard-marker")
            .await
            .expect("fixture write");
        let content = backend.read().await.expect("fixture read");
        assert_eq!(content.mime, "text/plain");
        assert_eq!(content.text, "omnibus-clipboard-marker");
        assert_eq!(
            content.size_bytes,
            u64::try_from(content.text.len()).unwrap_or(u64::MAX)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The X11 fallback: one binary, `-o` reads the stored selection.
    #[tokio::test]
    async fn clipboard_round_trip_via_the_xclip_backend() {
        let dir = scratch("xclip-backend");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let state = dir.join("state.txt");
        let state_str = state.to_str().expect("utf8 temp path").to_owned();
        let bin = fixture_script(
            dir.as_path(),
            "xclip",
            &format!(
                "#!/bin/sh\nfor arg in \"$@\"; do \
                 [ \"$arg\" = \"-o\" ] && exec cat \"{state_str}\"; done\n\
                 cat > \"{state_str}\"\n"
            ),
        );
        let backend = ClipboardBackend::Xclip { bin };
        backend.write("xclip-marker").await.expect("fixture write");
        let content = backend.read().await.expect("fixture read");
        assert_eq!(content.text, "xclip-marker");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An empty clipboard is an honest empty content, never an error.
    #[tokio::test]
    async fn empty_clipboard_reads_as_empty_content() {
        let dir = scratch("wl-empty");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let (copy, paste) = wl_fixture(&dir);
        let backend = ClipboardBackend::WlClipboard { copy, paste };
        let content = backend.read().await.expect("empty read succeeds");
        assert_eq!(content.text, "");
        assert_eq!(content.size_bytes, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A backend that fails the read (wl-paste: the clipboard holds no text)
    /// answers a typed NativeFailure carrying the backend's own stderr.
    #[tokio::test]
    async fn failing_backend_answers_native_failure() {
        let dir = scratch("wl-fail");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let paste = fixture_script(
            dir.as_path(),
            "wl-paste",
            "#!/bin/sh\necho 'no text type on clipboard' >&2\nexit 1\n",
        );
        let copy = fixture_script(dir.as_path(), "wl-copy", "#!/bin/sh\ncat >/dev/null\n");
        let backend = ClipboardBackend::WlClipboard { copy, paste };
        let refused = backend.read().await;
        match refused {
            Err(ContractError::NativeFailure { code: 1, payload }) => {
                assert!(payload.contains("clipboard backend"), "{payload}");
                assert!(payload.contains("no text type"), "{payload}");
            }
            other => panic!("a failing backend must be a NativeFailure, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The read cap: an oversized backend stream is a typed refusal (the
    /// screenshot cap discipline), never an unbounded buffer.
    #[tokio::test]
    async fn oversized_clipboard_read_is_a_typed_cap_refusal() {
        let dir = scratch("wl-cap");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        // 3 MiB of zeros: past the 2 MiB cap (/dev/zero exists on Linux).
        let paste = fixture_script(
            dir.as_path(),
            "wl-paste",
            "#!/bin/sh\nhead -c 3000000 /dev/zero\n",
        );
        let copy = fixture_script(dir.as_path(), "wl-copy", "#!/bin/sh\ncat >/dev/null\n");
        let backend = ClipboardBackend::WlClipboard { copy, paste };
        let refused = backend.read().await;
        match refused {
            Err(ContractError::Unsupported { reason }) => {
                assert!(reason.contains("per-call cap"), "{reason}");
            }
            other => panic!("over-cap clipboard read must be refused, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A binary that vanished between detection and the call (or a broken
    /// install) answers AdapterUnavailable, not a panic.
    #[tokio::test]
    async fn missing_backend_binary_answers_adapter_unavailable() {
        let backend = ClipboardBackend::Xclip {
            bin: PathBuf::from("/nonexistent/omnibus-xclip"),
        };
        let read = backend.read().await;
        assert!(matches!(read, Err(ContractError::AdapterUnavailable)));
    }

    /// Detection: wl-clipboard requires BOTH binaries (a half-installed pair
    /// would fail one direction of the round trip); xclip is the fallback;
    /// nothing installed is an honest None.
    #[test]
    fn detection_prefers_wlclipboard_and_falls_back_to_xclip() {
        let dir = scratch("detect");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        assert_eq!(
            ClipboardBackend::detect_in(std::slice::from_ref(&dir)),
            None,
            "nothing installed: honest absence"
        );
        let xclip = fixture_script(dir.as_path(), "xclip", "#!/bin/sh\ntrue\n");
        assert_eq!(
            ClipboardBackend::detect_in(std::slice::from_ref(&dir)),
            Some(ClipboardBackend::Xclip { bin: xclip }),
            "a lone xclip is the X11 fallback"
        );
        let (copy, paste) = wl_fixture(&dir);
        assert_eq!(
            ClipboardBackend::detect_in(std::slice::from_ref(&dir)),
            Some(ClipboardBackend::WlClipboard {
                copy: copy.clone(),
                paste: paste.clone()
            }),
            "the complete wl pair wins over xclip"
        );
        std::fs::remove_file(&paste).expect("drop wl-paste");
        let xclip = dir.join("xclip");
        assert_eq!(
            ClipboardBackend::detect_in(std::slice::from_ref(&dir)),
            Some(ClipboardBackend::Xclip { bin: xclip }),
            "a half-installed wl pair falls back to xclip"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
