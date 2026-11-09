#![cfg_attr(test, allow(clippy::expect_used))] // unit tests may expect; the seam must not
//! `omnibusd` — the Omnibus daemon (composition root).
//!
//! Modes (specs/contracts/mcp-client-daemon.md, config-and-grants.md):
//! - `omnibusd`            serve one stdio MCP session (probe → serve → shutdown)
//! - `omnibusd grant <capability> [--path <dir>]...`   record a grant in the
//!   user scope manifest (the human runs this; agents have no grant surface —
//!   never-list); parameterized scopes require at least one `--path`
//! - `omnibusd doctor [--apply <name>]`  CLI doctor: diagnosis, and granular
//!   mutations only when a human confirms each one interactively
//! - `omnibusd --version`

mod doctor_cli;
mod grant;
mod wiring;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| exit_err(format!("tokio runtime: {e}"), 1));
    let exit = runtime.block_on(run(args));
    if exit != 0 {
        std::process::exit(exit);
    }
}

async fn run(args: Vec<String>) -> i32 {
    match args.first().map(String::as_str) {
        None => serve().await,
        Some("--serve") => serve().await,
        Some("--version") | Some("-V") => {
            println!("omnibusd {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some("--help") | Some("-h") => {
            print_usage();
            0
        }
        Some("grant") => grant::run(args.get(1..).unwrap_or(&[])).await,
        Some("doctor") => doctor_cli::run(args.get(1..).unwrap_or(&[])).await,
        Some(other) => {
            eprintln!("omnibusd: unknown option: {other}");
            print_usage();
            2
        }
    }
}

/// The daemon mode: one stdio MCP session; the process lives and dies with the
/// client (topology-and-lifecycle.md — no supervisor, no reconnect surface).
async fn serve() -> i32 {
    let (core, platform) = match wiring::build_core(&wiring::current_actor(None)).await {
        Ok(wired) => wired,
        Err(e) => {
            eprintln!("omnibusd: {e}");
            return 1;
        }
    };
    if let Some(facts) = &platform.manjaro {
        eprintln!(
            "omnibusd: distro={} release={} manjaro(delayed_repos={}, mhwd={}, pamac={})",
            platform.distro.id,
            platform.distro.release,
            facts.delayed_repos,
            facts.mhwd_present,
            facts.pamac_present,
        );
    } else {
        eprintln!(
            "omnibusd: distro={} release={}",
            platform.distro.id, platform.distro.release
        );
    }
    let handler = omnibus_core::facade::OmnibusHandler::new(core);
    match handler.serve_stdio().await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("omnibusd: {e}");
            1
        }
    }
}

fn print_usage() {
    println!("omnibusd {} — Omnibus daemon", env!("CARGO_PKG_VERSION"));
    println!("usage:");
    println!("  omnibusd                      serve one stdio MCP session");
    println!(
        "  omnibusd grant <capability> [--path <dir>]...
                                record a grant in the user scope manifest; \
         parameterized scopes (files.scope.write) require at least one --path"
    );
    println!("  omnibusd doctor [--apply <name>]  CLI doctor (human-executed mutations)");
    println!("  omnibusd --version");
}

fn exit_err(message: String, code: i32) -> ! {
    eprintln!("omnibusd: {message}");
    std::process::exit(code);
}
