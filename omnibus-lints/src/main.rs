//! CLI entry: run the four R4 rules against a repository (default: this workspace).
//!
//! Exit 0 = all rules green; exit 1 = at least one violation (the gate is red).

use std::path::PathBuf;
use std::process::ExitCode;

use omnibus_lints::run_all;

fn main() -> ExitCode {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."));

    let findings = run_all(&root);
    if findings.is_empty() {
        println!("omnibus-lints: OK (R4-COP, R4-NATIVE, R4-CON, R4-EDGES green)");
        ExitCode::SUCCESS
    } else {
        eprintln!("omnibus-lints: {} violation(s):", findings.len());
        for f in &findings {
            eprintln!("  [{}] {}:{} — {}", f.rule, f.path, f.line, f.message);
        }
        ExitCode::FAILURE
    }
}
