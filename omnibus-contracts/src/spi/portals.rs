//! `ScreenCapture` — clipboard and screenshots (subsystem 5a); `Notify` —
//! desktop notifications (subsystem 5b). Clipboard writes are grant-gated even
//! though the subsystem reads as read-mostly (roadmap 1.5.5).

use async_trait::async_trait;

use crate::dto::portals::{
    ClipboardContent, NotifyRequest, NotifyResult, ScreenshotRequest, ScreenshotResult,
};
use crate::errors::ContractError;

/// Clipboard (read+write) and screenshot operations, backed by xdg-desktop-portal
/// and wl-clipboard/xclip on Linux. Capabilities (`portal.clipboard`,
/// `portal.screenshot`) are declared by the probe report when a backend exists.
#[async_trait]
pub trait ScreenCapture: Send + Sync {
    /// The adapter report (which backends answered).
    fn report(&self) -> crate::dto::probe::AdapterReport;

    /// `clip_read` — current clipboard content (text-focused).
    async fn clip_read(&self) -> Result<ClipboardContent, ContractError>;

    /// `clip_write` (write, granted) — replaces the clipboard content.
    async fn clip_write(&self, content: ClipboardContent) -> Result<(), ContractError>;

    /// `screenshot` — one screen capture as base64-encoded image data.
    async fn screenshot(
        &self,
        request: ScreenshotRequest,
    ) -> Result<ScreenshotResult, ContractError>;
}

/// Desktop notifications via the D-Bus notification service.
#[async_trait]
pub trait Notify: Send + Sync {
    /// The adapter report (notification backend liveness).
    fn report(&self) -> crate::dto::probe::AdapterReport;

    /// `notify` (write, granted) — shows one notification.
    async fn send(&self, request: NotifyRequest) -> Result<NotifyResult, ContractError>;
}
