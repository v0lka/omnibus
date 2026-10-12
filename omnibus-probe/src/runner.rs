//! Subprocess/file runner abstraction: the detection logic stays pure and
//! unit-testable; the system runner shells out with a bounded timeout.

use async_trait::async_trait;
use std::sync::Arc;

/// One bounded observation of the host; `None` = missing binary, timeout, or
/// failure (the probe degrades honestly instead of guessing).
#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, prog: &str, args: &[&str]) -> Option<String>;
    fn file_exists(&self, path: &str) -> bool;
    fn env(&self, name: &str) -> Option<String>;
    fn read_file(&self, path: &str) -> Option<String>;
}

/// The real runner: tokio processes with a per-command budget, plain fs reads.
#[derive(Debug, Clone, Default)]
pub struct SystemRunner;

const COMMAND_BUDGET_MS: u64 = 3000;

#[async_trait]
impl CommandRunner for SystemRunner {
    async fn run(&self, prog: &str, args: &[&str]) -> Option<String> {
        let output = tokio::time::timeout(
            std::time::Duration::from_millis(COMMAND_BUDGET_MS),
            // kill_on_drop(true): when the budget fires the output future is
            // dropped and the child must die with it — without this a wedged
            // backend binary survives every probe (same defect class as the
            // journald adapter's journalctl spawn).
            tokio::process::Command::new(prog)
                .args(args)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .ok()?
        .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn file_exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn read_file(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
}

/// Shared runner handle.
pub type SharedRunner = Arc<dyn CommandRunner>;
