//! The SPI trait registry (specs/architecture/spi-registry.md). Traits are declared
//! just-in-time together with their first implementation (ADR-011): MS1 declares the
//! eight subsystems plus `AuditSink`; `NetworkStatus` is declared here together with
//! `omnibus-adapter-nm`, its first implementation (roadmap task 2.2.3); `AppLauncher`
//! enters with `omnibus-adapter-launch` (roadmap task 2.7.1, v1.5);
//! `ProcessTelemetry` enters with `omnibus-adapter-proc` (roadmap task 2.8.1,
//! v1.5). `AppIntentsBridge` (MS3, macOS) stays undeclared — no speculative stubs.
//!
//! Every trait is dyn-compatible (ADR-001: `dyn Trait` SPI) and every method returns
//! a contract error; adapters map native failures to `NativeFailure{code}`.

pub mod a11y;
#[path = "app-launcher.rs"]
pub mod app_launcher;
pub mod audit_sink;
pub mod files;
pub mod journal;
pub mod kb;
pub mod net;
pub mod packages;
pub mod portals;
pub mod probe;
#[path = "process-telemetry.rs"]
pub mod process_telemetry;
pub mod services;
