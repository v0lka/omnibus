//! xdg-portals adapter: screenshot, clipboard, notifications via desktop portals.
//!
//! Native backend: the xdg-desktop-portal service on the session bus plus the
//! freedesktop notification service. The portal request/response dance is
//! asynchronous: the Response signal on the predictable request object path is
//! subscribed before the call and awaited with a 5s inner timeout (the core
//! budget still applies on top); timeouts map to [`ContractError::TimeoutError`].
//! Native types (portal handles, variant dictionaries) never leave this crate.
//!
//! Honesty of the capability ladder (R3):
//! - screenshot: present when the portal publishes the Screenshot interface;
//!   a user denial is a typed [`ContractError::ScopeDenied`].
//! - clipboard: the portal Clipboard interface is session-gated — a usable
//!   clipboard channel requires a RemoteDesktop session, whose creation is out
//!   of scope for omnibus (no input-emulation surface, ADR-009). The capability
//!   is therefore reported degraded and clipboard calls answer
//!   [`ContractError::AdapterUnavailable`] rather than pretending.
//! - notify: present when the notification service is on the bus.

mod wire;

use std::collections::HashMap;
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
}

impl PortalsAdapter {
    /// Connects to the session bus and probes the portal and notification
    /// services. Fails with [`ContractError::AdapterUnavailable`] only when no
    /// session bus is reachable at all.
    pub async fn connect() -> Result<Self, ContractError> {
        let conn = Connection::session()
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
        })
    }

    /// Whether the portal service is on the bus.
    pub fn portal_present(&self) -> bool {
        self.portal_present
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

    fn clipboard_fact(&self) -> CapabilityFact {
        if self.clipboard_present() {
            self.capability_fact(
                PORTAL_CLIPBOARD,
                "degraded",
                "the portal Clipboard interface is published but session-gated: a usable \
                 clipboard channel requires a RemoteDesktop session, which omnibus does not \
                 create (no input-emulation surface)",
            )
        } else {
            self.capability_fact(
                PORTAL_CLIPBOARD,
                "degraded",
                "no portal Clipboard interface on this machine",
            )
        }
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
        AdapterReport {
            adapter: String::from("xdg-portals"),
            subsystem: String::from("portals"),
            contract_version: String::from(CONTRACT_VERSION),
            capabilities: vec![
                self.screenshot_fact(),
                self.clipboard_fact(),
                self.notify_fact(),
            ],
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
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| Self::native(format!("cannot read {path}: {e}")))?;
        Ok(ScreenshotResult {
            mime: String::from("image/png"),
            image_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            size_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        })
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
        // The portal Clipboard channel is session-gated (see module docs):
        // without a RemoteDesktop session there is no reachable clipboard
        // backend, so the honest answer is the absence of an active adapter.
        let _ = self.clipboard_present();
        Err(ContractError::AdapterUnavailable)
    }

    async fn clip_write(&self, _content: ClipboardContent) -> Result<(), ContractError> {
        // Same session gate as clip_read; writes are additionally grant-gated
        // in the core, never here.
        Err(ContractError::AdapterUnavailable)
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
        };
        let report = ScreenCapture::report(&adapter);
        let statuses: Vec<(String, String)> = report
            .capabilities
            .iter()
            .map(|fact| (fact.name.clone(), fact.status.clone()))
            .collect();
        assert!(statuses.iter().all(|(_, status)| status == "degraded"));
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
        };
        let report = ScreenCapture::report(&adapter);
        let screenshot = report
            .capabilities
            .iter()
            .find(|fact| fact.name == PORTAL_SCREENSHOT)
            .map(|fact| fact.status.clone())
            .unwrap_or_default();
        assert_eq!(screenshot, "present");
        // Clipboard stays degraded even with the interface published: it is
        // session-gated behind RemoteDesktop.
        let clipboard = report
            .capabilities
            .iter()
            .find(|fact| fact.name == PORTAL_CLIPBOARD)
            .map(|fact| fact.status.clone())
            .unwrap_or_default();
        assert_eq!(clipboard, "degraded");
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
}
