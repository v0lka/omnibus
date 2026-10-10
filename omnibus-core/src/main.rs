//! `omnibusd` — the Omnibus daemon binary (composition root).
//!
//! Roadmap 1.1.2 skeleton: the binary responds to `--version`. The MCP stdio facade
//! arrives with roadmap 1.3.1.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") => {
            println!("omnibusd {}", env!("CARGO_PKG_VERSION"));
        }
        Some("--help") | Some("-h") => {
            print_usage();
        }
        Some(other) => {
            eprintln!("omnibusd: unknown option: {other}");
            print_usage();
            std::process::exit(2);
        }
        None => {
            print_usage();
        }
    }
}

fn print_usage() {
    println!("omnibusd {} — Omnibus daemon", env!("CARGO_PKG_VERSION"));
    println!("usage: omnibusd [--version|--help]");
}
