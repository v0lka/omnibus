//! The one bounded subprocess path of the flatpak/snap adapter (the apt
//! adapter's execution discipline, roadmap 2.9.1): every store invocation goes
//! through [`capture_bounded`] — stdout is captured under a hard byte cap,
//! stderr is drained under its own cap, and a stalled or overflowed child is
//! killed (`kill_on_drop`). A budget elapse answers [`ContractError::
//! TimeoutError`]: reads observe no native state, so "the adapter stopped
//! waiting" is the whole truth. Every invocation is a read: this crate has no
//! write path at all (the honest inventory limitation, specs/domains/
//! packages.md).

use std::process::Stdio;
use std::time::Duration;

use omnibus_contracts::errors::ContractError;
use tokio::io::AsyncReadExt;

/// `NativeFailure` code: a store CLI failed at runtime (spawn, nonzero exit,
/// IO). Adapter-scoped, part of the closed documented NF_* convention; the raw
/// exit status and stderr travel in the payload text, never as machine codes.
pub const NF_STORE: i32 = 1;
/// `NativeFailure` code: the named package is in neither live store.
pub const NF_PKG_NOT_FOUND: i32 = 2;

/// Hard cap on stdout kept in memory per call: a full `flatpak list` on an
/// app-rich machine fits easily; the cap bounds pathological output instead
/// of trusting it.
const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
/// Diagnostics cap for the concurrently drained stderr pipe.
const MAX_STDERR_BYTES: usize = 64 * 1024;

/// stdout captured under [`MAX_OUTPUT_BYTES`], with the honest truncation flag
/// (no read DTO carries a truncation channel today, so no caller consumes it —
/// the flag keeps the honest answer available instead of dropping the fact).
pub struct CapturedOutput {
    pub stdout: Vec<u8>,
    #[allow(dead_code)]
    pub truncated: bool,
}

/// Runs `program` with `args` under `budget`: stdout is captured under
/// [`MAX_OUTPUT_BYTES`], stderr is drained under its own cap, and a timed-out
/// or overflowed child is killed (`kill_on_drop`). The environment is
/// inherited (the parent's PATH keeps resolving the store binaries).
///
/// Spawns the bounded child. A transient `ETXTBSY` is retried within a tiny
/// window: on Linux `execve` fails with "Text file busy" while ANY writer
/// still holds the binary open — under parallel test writers a just-written
/// fixture script can hit it, and in production a store transaction can touch
/// the CLI binary the same way. Two extra attempts a millisecond apart keep
/// the honest error mapping (`NotFound` → `AdapterUnavailable`) without
/// masking a genuinely broken backend: a third failure answers
/// [`ContractError::NativeFailure`] with the original text.
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
                    code: NF_STORE,
                    payload: format!("spawn {program}: {e}"),
                });
            }
        }
    }
}

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

pub async fn capture_bounded(
    program: &str,
    args: &[String],
    budget: Duration,
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
            code: NF_STORE,
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
                    code: NF_STORE,
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
                    code: NF_STORE,
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
                code: NF_STORE,
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
    captured.map_err(|_| ContractError::TimeoutError)?
}
