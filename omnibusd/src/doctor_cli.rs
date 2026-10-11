//! `omnibusd doctor` — the CLI doctor (roadmap 1.7.1): diagnosis like the MCP
//! tool, plus granular mutations a human confirms and executes one at a time.
//! Every mutation is its own audit event with provenance `doctor`; the agent
//! channel (MCP) can never execute them — that split is the channel separation
//! the spec demands.

use omnibus_contracts::dto::audit::AuditEvent;
use omnibus_contracts::dto::caps::DoctorReport;
use omnibus_core::audit::FileSink;

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
        println!("  {}. [{}] {}", idx + 1, command.name, command.command);
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
        .find(|c| c.name == name || c.command == name)
    else {
        eprintln!("omnibusd doctor: no pending command named {name:?}");
        return 2;
    };

    println!("about to execute: {}", command.command);
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

    let output = tokio::process::Command::new("/usr/bin/sh")
        .arg("-c")
        .arg(&command.command)
        .output()
        .await;
    let (ok, detail) = match output {
        Ok(out) if out.status.success() => (true, String::from("ok")),
        Ok(out) => (
            false,
            format!(
                "exit {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ),
        Err(e) => (false, format!("spawn: {e}")),
    };
    audit_doctor_event(&command.name, ok, &detail);
    if ok {
        println!("done: {}", command.name);
        println!("start a new omnibusd instance to pick up the change (§7.2).");
        0
    } else {
        eprintln!("failed: {detail}");
        1
    }
}

fn audit_doctor_event(op: &str, ok: bool, detail: &str) {
    let event = AuditEvent {
        ts_epoch_ms: omnibus_contracts::dto::Milliseconds::now_epoch().0,
        session: String::from("cli"),
        actor: super::wiring::current_actor(Some(String::from("omnibus-doctor-cli"))),
        tool: String::from("doctor"),
        adapter: String::from("core"),
        op: format!("doctor-apply:{op}"),
        params_hash: omnibus_core::audit::params_hash(&serde_json::json!({ "op": op })),
        result: if ok {
            String::from("ok")
        } else {
            format!("fail:{detail}")
        },
        scope_ref: None,
    };
    let fallback = super::wiring::audit_fallback_path();
    if let Some(parent) = fallback.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    FileSink::new(fallback).append(&event);
}
