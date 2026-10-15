//! `omnibusd doctor` — the CLI doctor (roadmap 1.7.1): diagnosis like the MCP
//! tool, plus granular mutations a human confirms and executes one at a time.
//! Every mutation is its own audit event with provenance `doctor`; the agent
//! channel (MCP) can never execute them — that split is the channel separation
//! the spec demands.

use std::process::Stdio;
use std::time::Duration;

use omnibus_contracts::dto::caps::DoctorReport;
use tokio::io::AsyncReadExt;

/// `doctor --apply` runs each applied command under an explicit wall-clock
/// budget, and the command leads its own process group: a wedged child
/// (a hung sudo, a stuck systemctl) is killed together with every descendant
/// instead of hanging the CLI — or outliving the recorded failure.
const APPLY_BUDGET: Duration = Duration::from_secs(30);

/// Captured child output is capped per stream — the failure detail must not
/// buffer the child's entire output in memory. The cap bounds memory only,
/// never the child: `drain_capped` keeps draining (and discarding) past the
/// cap, so a chatty child never stalls on a full pipe and gets killed by the
/// budget — a would-have-succeeded run misreported as a timeout.
const APPLY_OUTPUT_CAP_BYTES: u64 = 64 * 1024;

/// The failure detail stored in the audit event is capped (the event is one
/// journal line, not a transcript).
const AUDIT_DETAIL_CAP_CHARS: usize = 200;

/// `omnibusd doctor [--apply <name>]`
pub async fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        None | Some("--diagnose") => diagnose().await,
        Some("--apply") => {
            let Some(name) = args.get(1) else {
                eprintln!("omnibusd doctor --apply: missing command name");
                return 2;
            };
            apply(name).await
        }
        Some(other) => {
            eprintln!("omnibusd doctor: unknown flag {other}");
            2
        }
    }
}

/// Belt-and-braces: core remedy strings now name `omnibusd` directly, but any
/// historical `omnibus grant` form reaching this CLI (older doctor entries in a
/// stored state file, hand-written remediation text) is still rewritten to the
/// shipped binary name for display, matching and execution.
fn display_command(raw: &str) -> String {
    if let Some(rest) = raw.strip_prefix("omnibus grant ") {
        format!("omnibusd grant {rest}")
    } else if raw == "omnibus grant" {
        String::from("omnibusd grant")
    } else {
        String::from(raw)
    }
}

/// Char-boundary-safe cap for text that lands in the audit event.
fn cap_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        String::from(text)
    } else {
        let mut out: String = text.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}

/// Builds the same wired core the daemon uses (read-only; adapters may be
/// absent — their absence is itself a doctor finding) and prints the report.
async fn diagnose() -> i32 {
    let (core, platform) = match super::wiring::build_core(&super::wiring::current_actor(Some(
        String::from("omnibus-doctor-cli"),
    )))
    .await
    {
        Ok(wired) => wired,
        Err(e) => {
            eprintln!("omnibusd doctor: {e}");
            return 1;
        }
    };
    let report = omnibus_core::doctor::diagnose(&core).await;
    println!(
        "omnibusd doctor — {} {} ({} / {})",
        platform.distro.name,
        platform.distro.release,
        platform.kernel,
        platform.desktop.as_deref().unwrap_or("no desktop session")
    );
    for finding in &report.findings {
        println!(
            "  [{}] {}: {}",
            finding.severity, finding.component, finding.detail
        );
        if let Some(remedy) = &finding.remedy {
            println!("      remedy: {remedy}");
        }
    }
    if report.ready_commands.is_empty() {
        println!("no pending human actions");
        return 0;
    }
    println!("granular actions (human-executed, each confirmed separately):");
    for (idx, command) in report.ready_commands.iter().enumerate() {
        println!(
            "  {}. [{}] {}",
            idx + 1,
            command.name,
            display_command(&command.command)
        );
    }
    println!("apply one with: omnibusd doctor --apply <name>");
    0
}

/// Applies one named doctor command after the human types an explicit
/// confirmation. This is the only mutation surface in the daemon binary.
async fn apply(name: &str) -> i32 {
    let (core, _platform) = match super::wiring::build_core(&super::wiring::current_actor(Some(
        String::from("omnibus-doctor-cli"),
    )))
    .await
    {
        Ok(wired) => wired,
        Err(e) => {
            eprintln!("omnibusd doctor: {e}");
            return 1;
        }
    };
    let report: DoctorReport = omnibus_core::doctor::diagnose(&core).await;
    let Some(command) = report
        .ready_commands
        .iter()
        .find(|c| c.name == name || c.command == name || display_command(&c.command) == name)
    else {
        eprintln!("omnibusd doctor: no pending command named {name:?}");
        return 2;
    };
    let command_text = display_command(&command.command);

    println!("about to execute: {command_text}");
    println!("type YES to confirm this single action: ");
    let mut answer = String::new();
    if std::io::stdin()
        .read_line(&mut answer)
        .map(|n| n == 0)
        .unwrap_or(true)
        || answer.trim() != "YES"
    {
        println!("aborted: nothing changed");
        return 1;
    }

    let (ok, detail) = run_confirmed(&command_text).await;
    audit_doctor_event(&command.name, ok, &detail).await;
    if ok {
        println!("done: {}", command.name);
        println!("start a new omnibusd instance to pick up the change (§7.2).");
        0
    } else {
        eprintln!("failed: {detail}");
        1
    }
}

/// Runs the confirmed command bounded (issue #37): piped, capped output; the
/// whole run under `APPLY_BUDGET`; a timed-out run loses its whole process
/// group (see [`run_bounded`]).
async fn run_confirmed(command: &str) -> (bool, String) {
    run_bounded(command, APPLY_BUDGET).await
}

/// Runs the command bounded: the child leads its own process group
/// (`process_group(0)`), so the timeout kill reaches every descendant that
/// stays in the child's process group. That group is the point: the commands
/// doctor applies are compound `sudo … && sudo …` pipelines, and a killed
/// `sh` leader would leave the other segments running — the mutation could
/// still complete after the failure was audited, making the "killed" claim a
/// lie. The coverage is not absolute: a command escaping into its own session
/// (e.g. `sudo` built or configured with `use_pty` runs its payload in a new
/// session the group kill cannot reach) or a mutation delegated to systemd
/// (an already-submitted restart job completes out-of-band regardless) is
/// not covered. `kill_on_drop` stays as belt-and-braces for the leader itself.
async fn run_bounded(command: &str, budget: Duration) -> (bool, String) {
    let spawned = tokio::process::Command::new("/usr/bin/sh")
        .arg("-c")
        .arg(command)
        .process_group(0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    match spawned {
        Err(e) => (
            false,
            cap_text(&format!("spawn: {e}"), AUDIT_DETAIL_CAP_CHARS),
        ),
        Ok(mut child) => {
            let child_id = child.id();
            let stdout_pipe = child.stdout.take();
            let stderr_pipe = child.stderr.take();
            let run = async {
                tokio::join!(
                    drain_capped(stdout_pipe),
                    drain_capped(stderr_pipe),
                    child.wait()
                )
            };
            match tokio::time::timeout(budget, run).await {
                Ok((stdout, stderr, status)) => match status {
                    Ok(status) if status.success() => (true, String::from("ok")),
                    Ok(status) => {
                        let text = if stderr.is_empty() { stdout } else { stderr };
                        let detail = format!(
                            "exit {:?}: {}",
                            status.code(),
                            String::from_utf8_lossy(&text).trim()
                        );
                        (false, cap_text(&detail, AUDIT_DETAIL_CAP_CHARS))
                    }
                    Err(e) => (
                        false,
                        cap_text(&format!("wait: {e}"), AUDIT_DETAIL_CAP_CHARS),
                    ),
                },
                Err(_) => {
                    // The timeout dropped the run future, whose drop killed
                    // the `sh` leader (`kill_on_drop`); the group kill is what
                    // reaches its surviving descendants. An already-empty
                    // group is the same success (ESRCH). A `None` pid means
                    // no kill can even be attempted — it must take the
                    // honest "may still be running" wording below, never the
                    // "killed" claim.
                    let detail = match child_id
                        .map_or(Err(String::from("child pid unknown")), kill_process_group)
                    {
                        Ok(()) => format!(
                            "timed out after {}s; the child process group was killed (members \
                             that escaped into their own session, e.g. sudo with use_pty, or \
                             mutations delegated to systemd may still complete)",
                            budget.as_secs()
                        ),
                        Err(err) => format!(
                            "timed out after {}s; the process group kill failed: {err} — the command may still be running",
                            budget.as_secs()
                        ),
                    };
                    (false, detail)
                }
            }
        }
    }
}

/// SIGKILLs the process group led by `pid`. The child was spawned with
/// `process_group(0)`, so its group holds every descendant that stayed in the
/// child's process group — the doctor's `sudo`/`systemctl`/`pacman` commands
/// normally do — and reaches nothing outside it: a command that escaped into
/// its own session (e.g. `sudo` built or configured with `use_pty` runs its
/// payload in a new session) and a mutation delegated to systemd (an
/// already-submitted job completes out-of-band regardless) are not covered.
/// A safe wrapper ([`nix::sys::signal::kill`]): the workspace
/// forbids `unsafe`, which the raw `libc::kill` call would need.
fn kill_process_group(pid: u32) -> Result<(), String> {
    use nix::sys::signal::Signal;
    use nix::unistd::Pid;
    let group = Pid::from_raw(-(pid as i32));
    match nix::sys::signal::kill(group, Signal::SIGKILL) {
        Ok(()) => Ok(()),
        // No member left: the group died with the leader.
        Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

/// Reads a child stream to EOF as a counting reader: every byte read is
/// counted, only the first [`APPLY_OUTPUT_CAP_BYTES`] are buffered, everything
/// past the cap drains to the discard sink. Reads must never stop at the cap —
/// a stalled reader fills the pipe and blocks the child mid-run until
/// `APPLY_BUDGET` kills it, misreporting a would-have-succeeded run as a
/// timeout — so the cap bounds memory, not the child.
async fn drain_capped<R: tokio::io::AsyncRead + Unpin>(pipe: Option<R>) -> Vec<u8> {
    let mut buf = Vec::new();
    let Some(mut pipe) = pipe else {
        return buf;
    };
    let cap = APPLY_OUTPUT_CAP_BYTES as usize;
    let mut chunk = [0u8; 8 * 1024];
    let mut kept: usize = 0;
    let mut counted: u64 = 0;
    loop {
        match pipe.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                counted = counted.saturating_add(u64::from(n as u32));
                if kept < cap {
                    let take = (cap - kept).min(n);
                    buf.extend_from_slice(&chunk[..take]);
                    kept += take;
                }
                // Past the cap: drain to the discard sink, buffer nothing.
            }
        }
    }
    debug_assert!(counted >= kept as u64, "every kept byte was counted");
    buf
}

/// One applied doctor command's audit event, assembled at the producer: the
/// single source for BOTH the delivery call and its shape pin (the unit test
/// below) — the tool, the op (`doctor-apply:<name>`), the params, the result
/// and the `doctor:<name>` provenance are defined here, never hand-copied
/// into tests.
struct DoctorAuditEvent {
    tool: &'static str,
    op: String,
    arguments: serde_json::Value,
    result: String,
    scope_ref: Option<String>,
}

fn doctor_audit_event(op: &str, ok: bool, detail: &str) -> DoctorAuditEvent {
    DoctorAuditEvent {
        tool: "doctor",
        op: format!("doctor-apply:{op}"),
        arguments: serde_json::json!({ "op": op }),
        result: if ok {
            String::from("ok")
        } else {
            format!("fail:{detail}")
        },
        scope_ref: Some(format!("doctor:{op}")),
    }
}

/// The doctor mutation is audited through the same pipeline the daemon uses
/// (journald primary, file fallback), with provenance `doctor` in `scope_ref`.
/// A delivery failure on both sinks is surfaced loudly (audit totality) without
/// changing the command's outcome.
async fn audit_doctor_event(op: &str, ok: bool, detail: &str) {
    let event = doctor_audit_event(op, ok, detail);
    let delivered = super::wiring::audit_cli_event(
        "cli",
        "omnibus-doctor-cli",
        event.tool,
        &event.op,
        &event.arguments,
        &event.result,
        event.scope_ref,
    )
    .await;
    if !delivered {
        eprintln!(
            "omnibusd doctor: WARNING: doctor audit could not be delivered; \
             the fallback sink also failed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_commands_are_rewritten_to_the_installed_binary() {
        assert_eq!(
            display_command("omnibus grant files.scope.write"),
            "omnibusd grant files.scope.write"
        );
        assert_eq!(
            display_command("omnibus grant pkg.write"),
            "omnibusd grant pkg.write"
        );
    }

    #[test]
    fn other_commands_pass_through_untouched() {
        assert_eq!(
            display_command("sudo systemctl restart systemd-journald"),
            "sudo systemctl restart systemd-journald"
        );
        assert_eq!(display_command(""), "");
        // only the exact grant form is rewritten
        assert_eq!(
            display_command("omnibus granting none"),
            "omnibus granting none"
        );
    }

    #[test]
    fn cap_text_truncates_on_a_char_boundary() {
        assert_eq!(cap_text("short", 10), "short");
        let long = "日".repeat(300);
        let capped = cap_text(&long, AUDIT_DETAIL_CAP_CHARS);
        assert_eq!(capped.chars().count(), AUDIT_DETAIL_CAP_CHARS + 1);
        assert!(capped.ends_with('…'));
    }

    /// The producer's audit shape, pinned at the only code that emits the
    /// event (`apply` → `audit_doctor_event` → this builder): tool `doctor`,
    /// op `doctor-apply:<name>`, the op in the params, and the provenance in
    /// `scope_ref` as `doctor:<name>`. The core-side fallback-sink test
    /// (`omnibus-core/tests/doctor_optin.rs`) proves the pipeline renders the
    /// shape; THIS pin proves the CLI produces it — the acceptance AC lives
    /// or dies with both halves, and neither hand-copies the other's strings
    /// without the producer being checked.
    #[test]
    fn doctor_audit_event_pins_the_producer_shape() {
        let event = doctor_audit_event("enable journal persistence", true, "");
        assert_eq!(event.tool, "doctor");
        assert_eq!(event.op, "doctor-apply:enable journal persistence");
        assert_eq!(
            event.arguments,
            serde_json::json!({ "op": "enable journal persistence" })
        );
        assert_eq!(event.result, "ok");
        assert_eq!(
            event.scope_ref.as_deref(),
            Some("doctor:enable journal persistence")
        );

        // A failed apply carries the failure detail in the result, provenance
        // unchanged.
        let failed = doctor_audit_event("enable journal persistence", false, "exit 1: nope");
        assert_eq!(failed.result, "fail:exit 1: nope");
        assert_eq!(
            failed.scope_ref.as_deref(),
            Some("doctor:enable journal persistence")
        );
    }

    /// The honest "killed" claim: the timeout must end the command's WHOLE
    /// process group, not only the `sh` leader — a compound command's surviving
    /// descendant could otherwise complete the mutation after the failure was
    /// audited. A backgrounded sleeper shares the group; the kill is proven by
    /// its /proc entry disappearing.
    #[tokio::test]
    async fn a_timed_out_run_kills_the_whole_process_group() {
        let dir = tempfile::tempdir().expect("tempdir");
        let pid_file = dir.path().join("sleeper.pid");
        let command = format!(
            "sleep 30 & echo $! > {}; sleep 30; wait",
            pid_file.display()
        );
        let (ok, detail) = run_bounded(&command, Duration::from_millis(300)).await;
        assert!(!ok, "the run must report the timeout");
        assert!(
            detail.contains("process group was killed"),
            "the detail must state the group kill honestly: {detail}"
        );
        // Under extreme load `echo $!` may land late — poll briefly for the
        // pid file before reading, so a slow write cannot fail the run
        // spuriously.
        let mut waited = Duration::ZERO;
        while !pid_file.exists() {
            assert!(
                waited < Duration::from_secs(2),
                "the backgrounded sleeper never recorded its pid"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            waited += Duration::from_millis(10);
        }
        let sleeper: u32 = std::fs::read_to_string(&pid_file)
            .expect("the pid file reads")
            .trim()
            .parse()
            .expect("a pid");
        // SIGKILL is prompt; a bounded poll keeps the test deterministic. The
        // sleeper's leader (`sh`) is dead, so the reaped process cannot linger.
        let mut waited = Duration::ZERO;
        while std::path::Path::new(&format!("/proc/{sleeper}")).exists() {
            assert!(
                waited < Duration::from_secs(5),
                "the group member {sleeper} outlived the process-group kill"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
            waited += Duration::from_millis(10);
        }
    }
}
