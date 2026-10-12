//! AT-SPI2 a11y adapter: desktop tree, element addressing, typed writes, invocation.
//!
//! Native backend: AT-SPI2 over D-Bus (zbus). Addressing is role+name based and
//! never pixel/coordinate based (P2/P3): the tree is selected by application
//! name, elements are addressed by adapter-owned `node_ref` strings the core
//! treats as opaque. Typed writes use the Value interface property set and the
//! Action interface by action *name* — no keyboard emulation ever (ADR-009).
//!
//! Bus discovery: the session bus from `DBUS_SESSION_BUS_ADDRESS`; the a11y bus
//! address via `org.a11y.Bus`/GetAddress when present, else the session bus
//! directly. The registry lives at `org.a11y.atspi.Registry` with the tree root
//! at `/org/a11y/atspi/accessible/root`. Dead objects and vanished applications
//! map to [`ContractError::ElementStale`]; a missing a11y bus to
//! [`ContractError::AdapterUnavailable`]; a lost bus connection to
//! [`ContractError::NativeFailure`] with the original message preserved, after
//! a lazy re-connection attempt so the next call can succeed — the remedy for
//! transport loss is a fresh call, never "re-read the tree". Transport loss is
//! identified by the typed D-Bus error identity (the error NAME of a method
//! error reply, zbus's own closed-connection classes), never by matching
//! free-form error text; app-level errors such as UnknownMethod mean the
//! element or interface is gone and never trigger a re-connection.
//!
//! Every bus interaction is wall-clock bounded: connect-time probes under
//! [`CONNECT_BUDGET`], application discovery and single-node methods under
//! their call budgets ([`TREE_BUDGET`] and [`NODE_BUDGET`] respectively), and
//! the walk deadline under [`TREE_BUDGET`]. Walk-deadline expiry
//! stops descent at a node boundary (the in-flight node is kept, its unread
//! children marked truncated) and returns the materialized partial
//! snapshot with `truncated: true` instead of discarding it.
//!
//! Native types (zbus messages, AT-SPI object references) never leave this
//! crate — mapping to contract DTOs happens inside the adapter
//! (specs/architecture/spi-registry.md "DTO Discipline").

mod node_ref;
mod states;
mod walk;

pub use node_ref::{decode_node_ref, encode_node_ref};

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use omnibus_contracts::capabilities::{DESKTOP_TREE, DESKTOP_WRITE_TYPED};
use omnibus_contracts::dto::NamedValue;
use omnibus_contracts::dto::desktop::{
    A11yElement, A11yNode, A11yTree, InvokeRequest, SetValueRequest, TreeSelector, WriteOutcome,
};
use omnibus_contracts::dto::probe::{AdapterReport, CapabilityFact};
use omnibus_contracts::errors::ContractError;
use omnibus_contracts::spi::a11y::DesktopA11y;
use omnibus_contracts::suite::families::{SENTINEL_POISON, SENTINEL_SLOW};
use omnibus_contracts::version::CONTRACT_VERSION;
use zbus::proxy::{Builder as ProxyBuilder, CacheProperties};
use zbus::zvariant::OwnedObjectPath;
use zbus::{Connection, Proxy};

/// Well-known name of the AT-SPI registry on the a11y bus.
pub const REGISTRY_DEST: &str = "org.a11y.atspi.Registry";
/// Object path of the accessibility tree root (the desktop frame).
pub const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";

const A11Y_BUS_DEST: &str = "org.a11y.Bus";
const A11Y_BUS_PATH: &str = "/org/a11y/bus";
const DBUS_DEST: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const IFACE_ACCESSIBLE: &str = "org.a11y.atspi.Accessible";
const IFACE_ACTION: &str = "org.a11y.atspi.Action";
const IFACE_VALUE: &str = "org.a11y.atspi.Value";
const IFACE_EDITABLE_TEXT: &str = "org.a11y.atspi.EditableText";

/// Wall-clock budget for single-node operations.
const NODE_BUDGET: Duration = Duration::from_secs(NODE_BUDGET_SECS);
/// [`NODE_BUDGET`] in whole seconds (const arithmetic on durations is spelled
/// out in seconds so the budget derivation below stays checkable).
const NODE_BUDGET_SECS: u64 = 5;
/// The core-side `desktop.tree` wall-clock budget (`omnibus-core/src/limits.rs`,
/// `TREE_BUDGET`): the whole adapter call must finish strictly inside it, or a
/// partial snapshot materialized at the walk deadline would still be discarded
/// by the core's timeout — defeating the preserved-partial-snapshot design.
const CORE_TREE_BUDGET_SECS: u64 = 15;
/// Margin between the core starting its deadline (request dispatch) and the
/// adapter starting its own clocks, so "strictly inside" survives one task
/// spawn and a clock read.
const TREE_CALL_MARGIN_SECS: u64 = 1;
/// Hard bound for the whole `tree` call:
/// `CORE_TREE_BUDGET_SECS - TREE_CALL_MARGIN_SECS` (14s) — strictly inside the
/// core budget, so the adapter always wins the race against the core's
/// timeout and its answer (full or partial snapshot, or its own
/// `TimeoutError`) is the one the caller observes.
const TREE_CALL_BUDGET: Duration =
    Duration::from_secs(CORE_TREE_BUDGET_SECS - TREE_CALL_MARGIN_SECS);
/// Wall-clock deadline of the tree walk:
/// `TREE_CALL_BUDGET_SECS - NODE_BUDGET_SECS` (9s). Expiry stops descent at a
/// node boundary (the in-flight node is kept, its unread children marked
/// truncated); one in-flight node read window (a node's name, role and
/// children reads — with their error mapping — bounded jointly by
/// [`NODE_BUDGET`] — see `visit`) may still finish,
/// so `walk deadline + NODE_BUDGET == TREE_CALL_BUDGET` holds by construction:
/// a partial snapshot materialized at the deadline always fits inside the
/// whole-call bound and is returned, not discarded.
/// (Enforced by `tree_budgets_stay_inside_the_core_budget` in the tests.)
const TREE_BUDGET: Duration =
    Duration::from_secs(CORE_TREE_BUDGET_SECS - TREE_CALL_MARGIN_SECS - NODE_BUDGET_SECS);
/// Wall-clock budget for one connect-time phase (session bus, a11y-bus
/// address, registry probe): a wedged bus degrades the subsystem at startup
/// instead of hanging it.
const CONNECT_BUDGET: Duration = Duration::from_secs(5);
/// [`ContractError::NativeFailure`] code for D-Bus-level backend failures
/// (the original message travels in the payload).
const NATIVE_DBUS: i32 = 1;

/// One accessibility application discovered under the registry root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredApp {
    /// The accessible application name (often a desktop file id).
    pub name: String,
    /// PID of the owning connection, when the bus answers.
    pub pid: Option<u32>,
    /// Adapter-owned node reference of the application root.
    pub node_ref: String,
}

/// Live AT-SPI2 backend over the a11y bus (or the session bus fallback).
#[derive(Clone)]
pub struct AtspiAdapter {
    /// The live bus connection, cloned out per call and swapped in when a
    /// transport loss is detected (the mutex is never held across an await).
    conn: Arc<Mutex<Connection>>,
    /// True when the registry well-known name is owned on the connected bus.
    registry_reachable: Arc<AtomicBool>,
    /// True when the dedicated a11y bus answered (false = session-bus fallback).
    dedicated_bus: Arc<AtomicBool>,
}

impl AtspiAdapter {
    /// Connects to the a11y bus (session-bus fallback) and probes the registry.
    ///
    /// Every probe phase runs inside [`CONNECT_BUDGET`]: a wedged bus degrades
    /// to [`ContractError::AdapterUnavailable`] at startup instead of hanging
    /// it. `AdapterUnavailable` is returned only when no session bus is
    /// reachable at all; a connected bus without a registry reports honestly
    /// through [`AtspiAdapter::registry_reachable`] and the adapter report.
    pub async fn connect() -> Result<Self, ContractError> {
        let (conn, registry_reachable, dedicated_bus) = Self::open_connections().await?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            registry_reachable: Arc::new(AtomicBool::new(registry_reachable)),
            dedicated_bus: Arc::new(AtomicBool::new(dedicated_bus)),
        })
    }

    /// Opens the bus connections and probes the registry; shared by
    /// [`AtspiAdapter::connect`] and the lazy re-connection path.
    async fn open_connections() -> Result<(Connection, bool, bool), ContractError> {
        let session = tokio::time::timeout(CONNECT_BUDGET, session_connection())
            .await
            .map_err(|_| ContractError::AdapterUnavailable)?
            .map_err(|_| ContractError::AdapterUnavailable)?;
        let mut conn = session.clone();
        let mut dedicated_bus = false;
        if let Some(bus) = tokio::time::timeout(CONNECT_BUDGET, dedicated_a11y_bus(&session))
            .await
            .unwrap_or(None)
        {
            conn = bus;
            dedicated_bus = true;
        }
        // A probe that cannot be answered leaves the registry reported absent:
        // honest degradation, never a hang.
        let registry_reachable =
            tokio::time::timeout(CONNECT_BUDGET, name_has_owner(&conn, REGISTRY_DEST))
                .await
                .unwrap_or_else(|_| Ok(false))
                .unwrap_or(false);
        Ok((conn, registry_reachable, dedicated_bus))
    }

    /// Re-establishes the bus connection after a transport-level failure and
    /// refreshes the reachability flags; the next call uses the fresh
    /// connection. Errors here fail the triggering call honestly.
    async fn try_reconnect(&self) -> Result<(), ContractError> {
        let (conn, registry_reachable, dedicated_bus) = Self::open_connections().await?;
        *self
            .conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = conn;
        self.registry_reachable
            .store(registry_reachable, Ordering::Relaxed);
        self.dedicated_bus.store(dedicated_bus, Ordering::Relaxed);
        Ok(())
    }

    /// Whether the AT-SPI registry answers on the connected bus.
    pub fn registry_reachable(&self) -> bool {
        self.registry_reachable.load(Ordering::Relaxed)
    }

    /// Applications currently registered under the tree root.
    ///
    /// Self-bounded like the SPI methods: on a wedged bus this answers as
    /// [`ContractError::TimeoutError`] instead of hanging discovery.
    pub async fn discover_apps(&self) -> Result<Vec<DiscoveredApp>, ContractError> {
        if !self.registry_reachable() {
            return Err(ContractError::AdapterUnavailable);
        }
        tokio::time::timeout(TREE_BUDGET, async {
            let children = self
                .contract(
                    self.node_children(REGISTRY_DEST, ROOT_PATH).await,
                    REGISTRY_DEST,
                )
                .await?;
            let mut apps = Vec::new();
            for (dest, path) in children {
                // An application that died during discovery is skipped instead
                // of reported as an empty-named app; other failures propagate.
                let name = match self
                    .contract(self.node_name(&dest, &path).await, &dest)
                    .await
                {
                    Ok(name) => name,
                    Err(ContractError::ElementStale) => continue,
                    Err(error) => return Err(error),
                };
                let pid = connection_pid(&self.conn_snapshot(), &dest).await;
                apps.push(DiscoveredApp {
                    name,
                    pid,
                    node_ref: node_ref::encode_node_ref(&dest, &path),
                });
            }
            Ok(apps)
        })
        .await
        .map_err(|_| ContractError::TimeoutError)?
    }

    /// Capability fact for one registry constant (CoN: names come from constants).
    fn capability_fact(&self, name: &str, note: String) -> CapabilityFact {
        CapabilityFact {
            name: String::from(name),
            ver: String::from(CONTRACT_VERSION),
            status: if self.registry_reachable() {
                String::from("present")
            } else {
                String::from("degraded")
            },
            value: None,
            note: Some(note),
            remedy: None,
        }
    }

    /// Cheap clone of the live connection (zbus shares the real socket
    /// internally); a poisoned lock still yields the last good connection.
    fn conn_snapshot(&self) -> Connection {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// A plain (non-caching) proxy: AT-SPI objects change constantly, caching
    /// would only add round-trips.
    async fn proxy<'c>(
        conn: &'c Connection,
        dest: &'c str,
        path: &'c str,
        iface: &'c str,
    ) -> zbus::Result<Proxy<'c>> {
        ProxyBuilder::new(conn)
            .destination(dest)?
            .path(path)?
            .interface(iface)?
            .cache_properties(CacheProperties::No)
            .build()
            .await
    }

    async fn node_name(&self, dest: &str, path: &str) -> zbus::Result<String> {
        self.node_string_property(dest, path, "Name").await
    }

    async fn node_string_property(
        &self,
        dest: &str,
        path: &str,
        property: &str,
    ) -> zbus::Result<String> {
        let conn = self.conn_snapshot();
        let proxy = Self::proxy(&conn, dest, path, IFACE_ACCESSIBLE).await?;
        proxy.get_property::<String>(property).await
    }

    async fn node_role(&self, dest: &str, path: &str) -> zbus::Result<String> {
        let conn = self.conn_snapshot();
        let proxy = Self::proxy(&conn, dest, path, IFACE_ACCESSIBLE).await?;
        let message = proxy.call_method("GetRoleName", &()).await?;
        message.body().deserialize::<String>()
    }

    async fn node_children(&self, dest: &str, path: &str) -> zbus::Result<Vec<(String, String)>> {
        let conn = self.conn_snapshot();
        let proxy = Self::proxy(&conn, dest, path, IFACE_ACCESSIBLE).await?;
        let message = proxy.call_method("GetChildren", &()).await?;
        let raw: Vec<(String, OwnedObjectPath)> = message.body().deserialize()?;
        Ok(raw
            .into_iter()
            .map(|(child_dest, child_path)| (child_dest, child_path.to_string()))
            .collect())
    }

    /// Child references of a node as adapter-owned `node_ref` strings.
    async fn node_child_refs(&self, dest: &str, path: &str) -> zbus::Result<Vec<String>> {
        let refs = self.node_children(dest, path).await?;
        Ok(refs
            .into_iter()
            .map(|(child_dest, child_path)| node_ref::encode_node_ref(&child_dest, &child_path))
            .collect())
    }

    async fn node_interfaces(&self, dest: &str, path: &str) -> zbus::Result<Vec<String>> {
        let conn = self.conn_snapshot();
        let proxy = Self::proxy(&conn, dest, path, IFACE_ACCESSIBLE).await?;
        let message = proxy.call_method("GetInterfaces", &()).await?;
        message.body().deserialize::<Vec<String>>()
    }

    async fn node_states(&self, dest: &str, path: &str) -> zbus::Result<Vec<String>> {
        let conn = self.conn_snapshot();
        let proxy = Self::proxy(&conn, dest, path, IFACE_ACCESSIBLE).await?;
        let message = proxy.call_method("GetState", &()).await?;
        let words: Vec<u32> = message.body().deserialize()?;
        Ok(states::state_names(&words))
    }

    async fn node_attributes(&self, dest: &str, path: &str) -> zbus::Result<Vec<NamedValue>> {
        let conn = self.conn_snapshot();
        let proxy = Self::proxy(&conn, dest, path, IFACE_ACCESSIBLE).await?;
        let message = proxy.call_method("GetAttributes", &()).await?;
        let raw: HashMap<String, String> = message.body().deserialize()?;
        let mut attributes: Vec<NamedValue> = raw
            .into_iter()
            .map(|(name, value)| NamedValue { name, value })
            .collect();
        attributes.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(attributes)
    }

    /// Action names of a node, index-aligned with the Action interface.
    async fn action_names(&self, dest: &str, path: &str) -> zbus::Result<Vec<String>> {
        let conn = self.conn_snapshot();
        let proxy = Self::proxy(&conn, dest, path, IFACE_ACTION).await?;
        let message = proxy.call_method("GetActions", &()).await?;
        let raw: Vec<(String, String, String)> = message.body().deserialize()?;
        Ok(raw.into_iter().map(|(name, _, _)| name).collect())
    }

    /// Gates on `dest` still being owned by a live bus name. A `false` answer
    /// is real staleness (the application is gone); a failing ownership probe
    /// is a transport loss, never staleness.
    async fn ensure_owner(&self, dest: &str) -> Result<(), ContractError> {
        match name_has_owner(&self.conn_snapshot(), dest).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(ContractError::ElementStale),
            Err(error) => Err(self.map_transport_loss(error).await),
        }
    }

    /// Converts one raw bus result into a contract result in the same breath.
    async fn contract<T>(&self, result: zbus::Result<T>, dest: &str) -> Result<T, ContractError> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => Err(self.map_bus_error(error, dest).await),
        }
    }

    /// A transport-level failure: re-establish the connection lazily and fail
    /// this call honestly with the original message preserved. The remedy is a
    /// fresh call (or an adapter rebuild), never "re-read the tree".
    ///
    /// The reconnection is bounded by ONE [`CONNECT_BUDGET`] for the whole
    /// attempt, not per phase. [`Self::open_connections`] arms
    /// [`CONNECT_BUDGET`] on each of its three phases (session bus, a11y-bus
    /// address, registry probe — 3 x 5s = 15s worst case), and 15s would
    /// outrun [`TREE_CALL_BUDGET`] (14s): on a wedged bus the whole-call
    /// timeout would fire first and the caller would see the plain outer
    /// `TimeoutError` instead of this honest `NativeFailure`. One 5s bound
    /// fits inside 14s with the margin to spare — provided the reconnection
    /// starts early enough. Timing note, stated honestly: the full window is
    /// guaranteed only while the reconnect begins before
    /// `TREE_CALL_BUDGET − CONNECT_BUDGET`; a transport loss that surfaces
    /// late in a tree walk (the failed read itself consumed most of the
    /// budget) starts the reconnect near the whole-call bound, and the outer
    /// `TimeoutError` may fire first — still typed, still honest, but the
    /// original-message `NativeFailure` payload is lost with the snapshot. On
    /// elapse the reconnection is abandoned mid-flight (dropped
    /// at the next await; the mutex is never held across an await, so the
    /// cancellation is safe) — the stale connection stays swapped in and the
    /// next call re-detects the loss — while the error below still carries the
    /// original payload. Node-path note: a single-node caller's outer
    /// `NODE_BUDGET` may elapse first (both are honest outcomes — the deadline
    /// genuinely elapsed).
    async fn map_transport_loss<E: fmt::Display>(&self, error: E) -> ContractError {
        // The outcome is deliberately discarded: reconnect or not, this call
        // fails honestly with the original message either way.
        let _ = tokio::time::timeout(CONNECT_BUDGET, self.try_reconnect()).await;
        ContractError::NativeFailure {
            code: NATIVE_DBUS,
            payload: error.to_string(),
        }
    }

    /// Maps a raw D-Bus failure onto the closed contract taxonomy.
    ///
    /// Branching is on the typed D-Bus error identity first ([`classify_bus_error`]):
    /// a method error reply carries the error NAME, and zbus's own
    /// closed-connection classes are enum variants. Genuine transport loss
    /// (`Disconnected` / `NoValidConnection` / I/O on the socket) triggers one
    /// bounded re-connection and fails honestly with the message preserved;
    /// an element or interface that is gone (`UnknownMethod` /
    /// `NoSuchInterface`) is [`ContractError::ElementStale`] with no
    /// re-connection — a healthy bus must never be swapped out because an
    /// application answered with an app-level error. Everything else falls
    /// through to the ownership gate and a last-resort object-level check;
    /// free-form message text is never the primary signal
    /// (specs/architecture/error-taxonomy.md anti-patterns).
    async fn map_bus_error(&self, error: zbus::Error, dest: &str) -> ContractError {
        match classify_bus_error(&error) {
            BusFailureClass::TransportLoss => return self.map_transport_loss(error).await,
            BusFailureClass::ElementGone => return ContractError::ElementStale,
            BusFailureClass::Untyped => {}
        }
        match name_has_owner(&self.conn_snapshot(), dest).await {
            Ok(true) => {}
            Ok(false) => return ContractError::ElementStale,
            // The ownership probe failed: the transport, not the element.
            Err(gate_error) => return self.map_transport_loss(gate_error).await,
        }
        if is_stale_signature(&error.to_string()) {
            return ContractError::ElementStale;
        }
        ContractError::NativeFailure {
            code: NATIVE_DBUS,
            payload: error.to_string(),
        }
    }

    /// Depth-first materialization of one node with budget accounting.
    ///
    /// A dead subtree shrinks the snapshot (marked truncated) instead of failing
    /// the whole tree, and the walk deadline cuts at a node boundary (the
    /// in-flight node is kept, its unread children marked truncated), so the
    /// already-materialized partial snapshot still returns — a snapshot is
    /// most useful when it reports what
    /// it saw. The node's own reads — name, role, the children listing, and any
    /// error mapping they trigger (an ownership probe, one bounded
    /// re-connection) — share ONE [`NODE_BUDGET`] window (the single in-flight
    /// window the budget arithmetic allows); an elapsed window leaves the node
    /// truncated — a bare stub when the identity half elapses, the
    /// materialized identity with no children when the children half does —
    /// instead of failing the tree.
    fn visit<'s>(
        &'s self,
        dest: String,
        path: String,
        depth: u32,
        budget: &'s mut walk::WalkBudget,
        deadline: Instant,
    ) -> Pin<Box<dyn Future<Output = Result<A11yNode, ContractError>> + Send + 's>> {
        Box::pin(async move {
            budget.record_node();
            // The node's own reads share ONE `NODE_BUDGET` window, split into a
            // name/role phase and a children phase so a window elapsed during
            // the children phase keeps the already-materialized identity. Two
            // sequential FULL windows could straddle the walk deadline by
            // `2 x NODE_BUDGET` and let the whole-call timeout discard the
            // materialized snapshot — the invariant
            // `walk deadline + NODE_BUDGET == TREE_CALL_BUDGET` holds for a
            // single in-flight window per node, which is why the children
            // listing (and the error mapping it can trigger: an ownership
            // probe, one `CONNECT_BUDGET` re-connection) is bounded by the
            // same window instead of running unbounded on a degraded bus.
            let window_deadline = tokio::time::Instant::now() + NODE_BUDGET;
            let pair = tokio::time::timeout_at(window_deadline, async {
                let name = self
                    .contract(self.node_name(&dest, &path).await, &dest)
                    .await?;
                let role = self
                    .contract(self.node_role(&dest, &path).await, &dest)
                    .await?;
                Ok((name, role))
            })
            .await;
            let (name, role) = match pair {
                Ok(result) => result?,
                Err(_elapsed) => {
                    budget.mark_truncated();
                    return Ok(A11yNode {
                        node_ref: node_ref::encode_node_ref(&dest, &path),
                        role: String::from("unknown"),
                        name: String::new(),
                        pid: None,
                        children: Vec::new(),
                    });
                }
            };
            let mut children = Vec::new();
            if !budget.may_descend(depth + 1) || Instant::now() >= deadline {
                budget.mark_truncated();
            } else {
                // The children listing shares the identity reads' window: a
                // wedged bus must not push one node past
                // `walk deadline + NODE_BUDGET`, where the whole-call timeout
                // discards the materialized snapshot. On window elapse the
                // node keeps its identity, truncated — on `ElementStale` the
                // dead subtree shrinks the snapshot the same way.
                let refs = tokio::time::timeout_at(window_deadline, async {
                    match self.node_children(&dest, &path).await {
                        Ok(refs) => Ok(Some(refs)),
                        Err(error) => match self.map_bus_error(error, &dest).await {
                            ContractError::ElementStale => Ok(None),
                            other => Err(other),
                        },
                    }
                })
                .await;
                match refs {
                    Ok(Ok(Some(refs))) => {
                        for (child_dest, child_path) in refs {
                            if !budget.may_descend(depth + 1) || Instant::now() >= deadline {
                                budget.mark_truncated();
                                break;
                            }
                            match self
                                .visit(child_dest, child_path, depth + 1, budget, deadline)
                                .await
                            {
                                Ok(child) => children.push(child),
                                Err(ContractError::ElementStale) => budget.mark_truncated(),
                                Err(error) => return Err(error),
                            }
                        }
                    }
                    Ok(Ok(None)) | Err(_) => budget.mark_truncated(),
                    Ok(Err(error)) => return Err(error),
                }
            }
            Ok(A11yNode {
                node_ref: node_ref::encode_node_ref(&dest, &path),
                role,
                name,
                pid: None,
                children,
            })
        })
    }

    /// Flat detail of one node; every field read is strict — a failure at any
    /// point maps through the taxonomy instead of silently degrading the
    /// snapshot to empty values.
    async fn element_inner(&self, dest: &str, path: &str) -> Result<A11yElement, ContractError> {
        let interfaces = self
            .contract(self.node_interfaces(dest, path).await, dest)
            .await?;
        let mut attributes = self
            .contract(self.node_attributes(dest, path).await, dest)
            .await?;
        // A missing Description property means the element has none; any other
        // read failure is a real failure, never a silently absent attribute.
        match self.node_string_property(dest, path, "Description").await {
            Ok(description) if !description.is_empty() => {
                attributes.push(NamedValue {
                    name: String::from("description"),
                    value: description,
                });
            }
            Ok(_) => {}
            Err(error) if !is_missing_property_signature(&error.to_string()) => {
                return Err(self.map_bus_error(error, dest).await);
            }
            Err(_) => {}
        }
        let actions = if interfaces.iter().any(|iface| iface == IFACE_ACTION) {
            self.contract(self.action_names(dest, path).await, dest)
                .await?
        } else {
            Vec::new()
        };
        Ok(A11yElement {
            node_ref: node_ref::encode_node_ref(dest, path),
            role: self
                .contract(self.node_role(dest, path).await, dest)
                .await?,
            name: self
                .contract(self.node_name(dest, path).await, dest)
                .await?,
            attributes,
            states: self
                .contract(self.node_states(dest, path).await, dest)
                .await?,
            actions,
            child_refs: self
                .contract(self.node_child_refs(dest, path).await, dest)
                .await?,
        })
    }

    /// Reads one well-known attribute. A missing property means the attribute
    /// is genuinely absent (`Unsupported`); any other failure propagates with
    /// its real class (stale/native) instead of collapsing to `Unsupported`.
    async fn read_well_known(
        &self,
        dest: &str,
        attribute: &str,
        read: impl Future<Output = zbus::Result<String>> + Send,
    ) -> Result<String, ContractError> {
        match read.await {
            Ok(value) => Ok(value),
            Err(error) if is_missing_property_signature(&error.to_string()) => {
                Err(ContractError::Unsupported {
                    reason: format!("attribute {attribute} is not present on this element"),
                })
            }
            Err(error) => Err(self.map_bus_error(error, dest).await),
        }
    }
}

/// Contract-suite failure injection points (families.rs sentinels): the suite
/// binds these selector names to configured failure classes, so a live backend
/// can exercise the poison and timeout branches of the harness.
fn injected_failure(app: &str) -> Option<ContractError> {
    match app {
        x if x == SENTINEL_SLOW => Some(ContractError::TimeoutError),
        x if x == SENTINEL_POISON => Some(ContractError::NativeFailure {
            code: NATIVE_DBUS,
            payload: String::from("contract-suite poison selector (families sentinel)"),
        }),
        _ => None,
    }
}

/// Case-insensitive application resolution: the full accessible name or its
/// basename (e.g. `org.gnome.Nautilus` answers to `nautilus`). A requested pid
/// disambiguates among same-name applications; an ambiguous selector without
/// one is a typed error listing the candidates so the agent can pick one and
/// retry (specs/domains/desktop-a11y.md, Behavior and Invariants).
fn resolve_app<'a>(
    apps: &'a [DiscoveredApp],
    selector: &TreeSelector,
) -> Result<Option<&'a DiscoveredApp>, ContractError> {
    let wanted = selector.app.trim();
    let matches: Vec<&DiscoveredApp> = apps
        .iter()
        .filter(|app| {
            app.name.eq_ignore_ascii_case(wanted)
                || app
                    .name
                    .rsplit('.')
                    .next()
                    .is_some_and(|base| base.eq_ignore_ascii_case(wanted))
        })
        .collect();
    match selector.pid {
        Some(pid) => Ok(matches.into_iter().find(|app| app.pid == Some(pid))),
        None => {
            if matches.len() > 1 {
                // Taxonomy wording check: `Unsupported` is documented as
                // "structurally impossible; the reason names alternatives" —
                // for an ambiguous selector it is the caller's input the
                // backend cannot serve, and `reason` listing the candidates
                // is exactly that alternatives channel. It is the least-wrong
                // code for ambiguous input without widening the closed
                // eight-code set (no contracts change).
                let candidates = matches
                    .iter()
                    .map(|app| match app.pid {
                        Some(pid) => format!("{} (pid {pid})", app.name),
                        None => app.name.clone(),
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(ContractError::Unsupported {
                    reason: format!(
                        "application selector {wanted} matches multiple applications; \
                         pick one by pid and retry: {candidates}"
                    ),
                });
            }
            Ok(matches.into_iter().next())
        }
    }
}

/// Error-text signatures that mean "the referenced element is gone".
///
/// Last-resort check only, for failures with no typed identity to branch on
/// ([`classify_bus_error`] answered [`BusFailureClass::Untyped`]). Matching
/// runs on a normalized form (lowercase, alphanumerics only) so both the human
/// message ("Invalid object path") and the D-Bus error name
/// (`org.freedesktop.DBus.Error.UnknownObject`) hit the same needle. Only
/// object-level absence counts: transport and interface-level failures are
/// pinned down by the typed classes first.
fn is_stale_signature(message: &str) -> bool {
    let normalized = normalize_signature(message);
    [
        "unknownobject",
        "invalidobjectpath",
        "nosuchobject",
        "serviceunknown",
        "notactivatable",
        "namehasnoowner",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

/// The D-Bus error names the adapter branches on (the typed identity a method
/// error reply carries). Branching happens on these names — never on
/// free-form message text (specs/architecture/error-taxonomy.md anti-patterns).
mod dbus_error_names {
    /// The bus (or the peer) dropped the connection: the transport is gone.
    pub const DISCONNECTED: &str = "org.freedesktop.DBus.Error.Disconnected";
    /// The bus could not route the call to a live connection: transport loss.
    pub const NO_VALID_CONNECTION: &str = "org.freedesktop.DBus.Error.NoValidConnection";
    /// The object has no such method: the element or its interface is gone.
    pub const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
    /// Name suffix of an interface-absent reply; the exact namespace varies
    /// across implementations (e.g. `org.a11y.atspi.Error.NoSuchInterface`).
    pub const NO_SUCH_INTERFACE_SUFFIX: &str = "NoSuchInterface";
}

/// How a raw D-Bus failure maps onto the closed taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BusFailureClass {
    /// Transport loss: one bounded re-connection, then `NativeFailure`.
    TransportLoss,
    /// The element or its interface is gone: `ElementStale`, no re-connection
    /// — the remedy is a re-read of the tree, not a swapped connection.
    ElementGone,
    /// No typed identity to branch on: fall through to the ownership gate.
    Untyped,
}

/// Classifies a D-Bus error NAME (pure; offline-tested). Only the typed
/// identity counts: an application answering UnknownMethod on a healthy bus
/// must never look like a lost connection.
fn classify_error_name(name: &str) -> BusFailureClass {
    match name {
        dbus_error_names::DISCONNECTED | dbus_error_names::NO_VALID_CONNECTION => {
            BusFailureClass::TransportLoss
        }
        dbus_error_names::UNKNOWN_METHOD => BusFailureClass::ElementGone,
        other if other.ends_with(dbus_error_names::NO_SUCH_INTERFACE_SUFFIX) => {
            BusFailureClass::ElementGone
        }
        _ => BusFailureClass::Untyped,
    }
}

/// The typed identity of a raw zbus failure.
fn classify_bus_error(error: &zbus::Error) -> BusFailureClass {
    match error {
        // zbus's own closed-connection classes: I/O on the socket or a failed
        // (re)connect — the transport, never the element.
        zbus::Error::InputOutput(_) | zbus::Error::Connection(..) => BusFailureClass::TransportLoss,
        // A method error reply: the D-Bus error NAME is the identity.
        zbus::Error::MethodError(name, _, _) => classify_error_name(name.as_str()),
        _ => BusFailureClass::Untyped,
    }
}

/// Signatures of a property the element does not have: the attribute is
/// genuinely absent ([`ContractError::Unsupported`]), not a backend failure.
fn is_missing_property_signature(message: &str) -> bool {
    let normalized = normalize_signature(message);
    ["unknownproperty", "nosuchproperty"]
        .iter()
        .any(|needle| normalized.contains(needle))
}

/// Lowercases and strips every non-alphanumeric byte so needle matching is
/// insensitive to message phrasing.
fn normalize_signature(message: &str) -> String {
    message
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

/// The session-bus connection with a hard per-method timeout (the zbus default
/// is no timeout at all, so a wedged bus would otherwise answer never).
async fn session_connection() -> zbus::Result<Connection> {
    zbus::connection::Builder::session()?
        .method_timeout(NODE_BUDGET)
        .build()
        .await
}

/// The dedicated a11y bus connection, when `org.a11y.Bus` publishes an address.
///
/// `None` means "fall back to the session bus": any probe failure counts,
/// including a wedged `org.a11y.Bus` (bounded by the caller's
/// [`CONNECT_BUDGET`]).
async fn dedicated_a11y_bus(session: &Connection) -> Option<Connection> {
    if !name_has_owner(session, A11Y_BUS_DEST)
        .await
        .unwrap_or(false)
    {
        return None;
    }
    let Ok(proxy) = Proxy::new(session, A11Y_BUS_DEST, A11Y_BUS_PATH, A11Y_BUS_DEST).await else {
        return None;
    };
    let message = proxy.call_method("GetAddress", &()).await.ok()?;
    let address: String = message.body().deserialize().ok()?;
    let parsed: zbus::address::Address = address.parse().ok()?;
    let connection = zbus::connection::Builder::address(parsed)
        .ok()?
        .method_timeout(NODE_BUDGET)
        .build()
        .await;
    connection.ok()
}

/// `NameHasOwner` over the raw bus interface; the error is honest (transport).
async fn name_has_owner(conn: &Connection, name: &str) -> zbus::Result<bool> {
    let proxy = Proxy::new(conn, DBUS_DEST, DBUS_PATH, DBUS_DEST).await?;
    let message = proxy.call_method("NameHasOwner", &(name)).await?;
    message.body().deserialize::<bool>()
}

/// PID of the connection owning `name`, when the bus answers.
async fn connection_pid(conn: &Connection, name: &str) -> Option<u32> {
    let proxy = Proxy::new(conn, DBUS_DEST, DBUS_PATH, DBUS_DEST)
        .await
        .ok()?;
    let message = proxy
        .call_method("GetConnectionUnixProcessID", &(name))
        .await
        .ok()?;
    message.body().deserialize::<u32>().ok()
}

#[async_trait]
impl DesktopA11y for AtspiAdapter {
    fn report(&self) -> AdapterReport {
        let mut notes = Vec::new();
        notes.push(if self.dedicated_bus.load(Ordering::Relaxed) {
            String::from("a11y bus via the org.a11y.Bus address")
        } else {
            String::from("a11y bus absent; the session bus is used directly")
        });
        notes.push(if self.registry_reachable() {
            String::from("registry reachable")
        } else {
            String::from("registry absent: no accessibility tree on this machine")
        });
        AdapterReport {
            adapter: String::from("atspi"),
            subsystem: String::from("a11y"),
            contract_version: String::from(CONTRACT_VERSION),
            capabilities: vec![
                self.capability_fact(
                    DESKTOP_TREE,
                    String::from("role+name addressing only; walk capped at depth 12 / 2000 nodes"),
                ),
                self.capability_fact(
                    DESKTOP_WRITE_TYPED,
                    String::from(
                        "Value property set and named-action invocation; no keyboard emulation",
                    ),
                ),
            ],
            notes,
        }
    }

    async fn tree(&self, selector: TreeSelector) -> Result<A11yTree, ContractError> {
        let walked = tokio::time::timeout(TREE_CALL_BUDGET, async {
            if !self.registry_reachable() {
                return Err(ContractError::AdapterUnavailable);
            }
            if let Some(failure) = injected_failure(&selector.app) {
                return Err(failure);
            }
            // The walk deadline is fixed from the call start, not from the
            // end of application discovery: walk deadline + at most one
            // in-flight node read window (name, role and children bounded
            // jointly — see `visit`) then always fits inside the whole-call
            // bound (see the budget constants).
            let deadline = Instant::now() + TREE_BUDGET;
            let apps = self.discover_apps().await?;
            // A selector the registry cannot resolve means the application is
            // not present in the tree: the backend did not fail — the agent
            // re-reads the tree (ElementStale), never NativeFailure.
            let app = resolve_app(&apps, &selector)?.ok_or(ContractError::ElementStale)?;
            let Some((dest, path)) = node_ref::decode_node_ref(&app.node_ref) else {
                return Err(ContractError::ElementStale);
            };
            let mut budget = walk::WalkBudget::new(walk::MAX_DEPTH, walk::MAX_NODES);
            let mut root = self.visit(dest, path, 1, &mut budget, deadline).await?;
            root.pid = app.pid;
            Ok(A11yTree {
                app: selector.app.clone(),
                pid: app.pid,
                root,
                node_count: u32::try_from(budget.nodes).unwrap_or(u32::MAX),
                truncated: budget.truncated,
            })
        })
        .await
        .map_err(|_| ContractError::TimeoutError)??;
        Ok(walked)
    }

    async fn element(&self, node_ref: String) -> Result<A11yElement, ContractError> {
        let Some((dest, path)) = node_ref::decode_node_ref(&node_ref) else {
            return Err(ContractError::ElementStale);
        };
        if !self.registry_reachable() {
            return Err(ContractError::AdapterUnavailable);
        }
        let element = tokio::time::timeout(NODE_BUDGET, self.element_inner(&dest, &path))
            .await
            .map_err(|_| ContractError::TimeoutError)??;
        Ok(element)
    }

    async fn read_attr(
        &self,
        node_ref: String,
        attribute: String,
    ) -> Result<String, ContractError> {
        let Some((dest, path)) = node_ref::decode_node_ref(&node_ref) else {
            return Err(ContractError::ElementStale);
        };
        if !self.registry_reachable() {
            return Err(ContractError::AdapterUnavailable);
        }
        // The gate and every read arm run inside the budget: on a wedged bus
        // this answers as TimeoutError instead of hanging.
        let named = tokio::time::timeout(NODE_BUDGET, async {
            self.ensure_owner(&dest).await?;
            match attribute.to_ascii_lowercase().as_str() {
                "name" => {
                    self.read_well_known(&dest, "name", self.node_name(&dest, &path))
                        .await
                }
                "role" => {
                    self.read_well_known(&dest, "role", self.node_role(&dest, &path))
                        .await
                }
                "description" => {
                    self.read_well_known(
                        &dest,
                        "description",
                        self.node_string_property(&dest, &path, "Description"),
                    )
                    .await
                }
                "accessible-id" => {
                    self.read_well_known(
                        &dest,
                        "accessible-id",
                        self.node_string_property(&dest, &path, "AccessibleId"),
                    )
                    .await
                }
                "help-text" => {
                    self.read_well_known(
                        &dest,
                        "help-text",
                        self.node_string_property(&dest, &path, "HelpText"),
                    )
                    .await
                }
                _ => {
                    let attributes = self
                        .contract(self.node_attributes(&dest, &path).await, &dest)
                        .await?;
                    attributes
                        .into_iter()
                        .find(|pair| pair.name.eq_ignore_ascii_case(&attribute))
                        .map(|pair| pair.value)
                        .ok_or_else(|| ContractError::Unsupported {
                            reason: format!("attribute {attribute} is not present on this element"),
                        })
                }
            }
        })
        .await
        .map_err(|_| ContractError::TimeoutError)??;
        Ok(named)
    }

    async fn set_value(&self, request: SetValueRequest) -> Result<WriteOutcome, ContractError> {
        let Some((dest, path)) = node_ref::decode_node_ref(&request.node_ref) else {
            return Err(ContractError::ElementStale);
        };
        if !self.registry_reachable() {
            return Err(ContractError::AdapterUnavailable);
        }
        let outcome = tokio::time::timeout(NODE_BUDGET, async {
            self.ensure_owner(&dest).await?;
            let interfaces = self
                .contract(self.node_interfaces(&dest, &path).await, &dest)
                .await?;
            let has_value = interfaces.iter().any(|iface| iface == IFACE_VALUE);
            let has_editable = interfaces.iter().any(|iface| iface == IFACE_EDITABLE_TEXT);
            if !has_value && !has_editable {
                return Ok(WriteOutcome {
                    node_ref: request.node_ref.clone(),
                    applied: false,
                });
            }
            if has_value {
                match request.value.trim().parse::<f64>() {
                    // The typed write path: the Value property set, no
                    // keystrokes. `CurrentValue` is the property
                    // `org.a11y.atspi.Value` exposes. A failed set is never
                    // masked as `Unsupported` — that code is reserved for the
                    // unparseable-value case below — but when the element
                    // also exposes EditableText (Chromium/GTK inputs ship a
                    // read-only Value next to editable text) the write falls
                    // through to SetTextContents instead of failing hard.
                    Ok(numeric) => {
                        let conn = self.conn_snapshot();
                        let proxy = match Self::proxy(&conn, &dest, &path, IFACE_VALUE).await {
                            Ok(proxy) => proxy,
                            Err(error) => return Err(self.map_bus_error(error, &dest).await),
                        };
                        let set = proxy
                            .set_property("CurrentValue", numeric)
                            .await
                            .map_err(zbus::Error::from);
                        match self.contract(set, &dest).await {
                            // The Value set is the observed outcome: real success.
                            Ok(()) => {
                                return Ok(WriteOutcome {
                                    node_ref: request.node_ref.clone(),
                                    applied: true,
                                });
                            }
                            // A read-only Value is not the end of the write:
                            // fall through to EditableText and return the
                            // failure only if that write fails too. Success
                            // is never claimed for the unobserved Value
                            // outcome.
                            Err(_) if has_editable => {}
                            // No editable interface to fall back on: the
                            // Value-set failure is the honest outcome.
                            Err(value_error) => return Err(value_error),
                        }
                    }
                    // A numeric Value interface without a numeric request value
                    // is only writable through EditableText; without it the
                    // write is structurally impossible.
                    Err(_) if !has_editable => {
                        return Err(ContractError::Unsupported {
                            reason: String::from(
                                "the element holds a numeric Value interface; \
                                 the request value is not numeric",
                            ),
                        });
                    }
                    Err(_) => {}
                }
            }
            let conn = self.conn_snapshot();
            let proxy = match Self::proxy(&conn, &dest, &path, IFACE_EDITABLE_TEXT).await {
                Ok(proxy) => proxy,
                Err(error) => return Err(self.map_bus_error(error, &dest).await),
            };
            let call = proxy
                .call_method("SetTextContents", &(request.value.as_str()))
                .await;
            let message = self.contract(call, &dest).await?;
            // A reply that cannot be decoded is a backend failure, not a
            // declined write: `applied: false` only for a genuine false reply.
            let applied = message.body().deserialize::<bool>().map_err(|error| {
                ContractError::NativeFailure {
                    code: NATIVE_DBUS,
                    payload: error.to_string(),
                }
            })?;
            Ok(WriteOutcome {
                node_ref: request.node_ref.clone(),
                applied,
            })
        })
        .await
        .map_err(|_| ContractError::TimeoutError)??;
        Ok(outcome)
    }

    async fn invoke(&self, request: InvokeRequest) -> Result<WriteOutcome, ContractError> {
        let Some((dest, path)) = node_ref::decode_node_ref(&request.node_ref) else {
            return Err(ContractError::ElementStale);
        };
        if !self.registry_reachable() {
            return Err(ContractError::AdapterUnavailable);
        }
        let outcome = tokio::time::timeout(NODE_BUDGET, async {
            self.ensure_owner(&dest).await?;
            let interfaces = self
                .contract(self.node_interfaces(&dest, &path).await, &dest)
                .await?;
            if !interfaces.iter().any(|iface| iface == IFACE_ACTION) {
                return Ok(WriteOutcome {
                    node_ref: request.node_ref.clone(),
                    applied: false,
                });
            }
            let actions = self
                .contract(self.action_names(&dest, &path).await, &dest)
                .await?;
            let index = match request.action.as_deref() {
                // No name given: the default action is index 0.
                None => {
                    if actions.is_empty() {
                        return Ok(WriteOutcome {
                            node_ref: request.node_ref.clone(),
                            applied: false,
                        });
                    }
                    0usize
                }
                Some(wanted) => {
                    match actions
                        .iter()
                        .position(|name| name.eq_ignore_ascii_case(wanted))
                    {
                        Some(index) => index,
                        None => {
                            return Ok(WriteOutcome {
                                node_ref: request.node_ref.clone(),
                                applied: false,
                            });
                        }
                    }
                }
            };
            let conn = self.conn_snapshot();
            let proxy = match Self::proxy(&conn, &dest, &path, IFACE_ACTION).await {
                Ok(proxy) => proxy,
                Err(error) => return Err(self.map_bus_error(error, &dest).await),
            };
            let call = proxy
                .call_method("DoAction", &(i32::try_from(index).unwrap_or(i32::MAX)))
                .await;
            let message = self.contract(call, &dest).await?;
            // A codec failure after a possibly-executed DoAction is a backend
            // failure, not a declined invocation.
            let applied = message.body().deserialize::<bool>().map_err(|error| {
                ContractError::NativeFailure {
                    code: NATIVE_DBUS,
                    payload: error.to_string(),
                }
            })?;
            Ok(WriteOutcome {
                node_ref: request.node_ref.clone(),
                applied,
            })
        })
        .await
        .map_err(|_| ContractError::TimeoutError)??;
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_app_matches_full_name_and_basename() {
        let apps = vec![
            DiscoveredApp {
                name: String::from("org.gnome.Nautilus"),
                pid: Some(4242),
                node_ref: String::from(":1.40|/org/a11y/atspi/accessible/root"),
            },
            DiscoveredApp {
                name: String::from("gnome-text-editor"),
                pid: Some(4711),
                node_ref: String::from(":1.45|/org/a11y/atspi/accessible/root"),
            },
        ];
        let by_full = resolve_app(
            &apps,
            &TreeSelector {
                app: String::from("ORG.GNOME.NAUTILUS"),
                pid: None,
            },
        );
        assert_eq!(by_full.ok().flatten().map(|app| app.pid), Some(Some(4242)));
        let by_base = resolve_app(
            &apps,
            &TreeSelector {
                app: String::from("nautilus"),
                pid: None,
            },
        );
        assert_eq!(by_base.ok().flatten().map(|app| app.pid), Some(Some(4242)));
        let by_pid = resolve_app(
            &apps,
            &TreeSelector {
                app: String::from("gnome-text-editor"),
                pid: Some(3),
            },
        );
        assert_eq!(by_pid.ok().flatten().map(|app| app.pid), None);
        let missing = resolve_app(
            &apps,
            &TreeSelector {
                app: String::from("absent"),
                pid: None,
            },
        );
        assert_eq!(missing.ok().flatten().map(|app| app.pid), None);
    }

    #[test]
    fn ambiguous_selector_errors_listing_candidates_until_pid_picks() {
        let apps = vec![
            DiscoveredApp {
                name: String::from("gnome-text-editor"),
                pid: Some(4711),
                node_ref: String::from(":1.45|/org/a11y/atspi/accessible/root"),
            },
            DiscoveredApp {
                name: String::from("gnome-text-editor"),
                pid: Some(5150),
                node_ref: String::from(":1.46|/org/a11y/atspi/accessible/root"),
            },
        ];
        let ambiguous = resolve_app(
            &apps,
            &TreeSelector {
                app: String::from("gnome-text-editor"),
                pid: None,
            },
        );
        assert!(matches!(ambiguous, Err(ContractError::Unsupported { .. })));
        if let Err(ContractError::Unsupported { reason }) = ambiguous {
            assert!(reason.contains("gnome-text-editor (pid 4711)"), "{reason}");
            assert!(reason.contains("gnome-text-editor (pid 5150)"), "{reason}");
        }
        let by_pid = resolve_app(
            &apps,
            &TreeSelector {
                app: String::from("gnome-text-editor"),
                pid: Some(5150),
            },
        );
        assert_eq!(by_pid.ok().flatten().map(|app| app.pid), Some(Some(5150)));
    }

    #[test]
    fn sentinel_selectors_map_to_their_failure_class() {
        assert!(matches!(
            injected_failure(SENTINEL_SLOW),
            Some(ContractError::TimeoutError)
        ));
        assert!(matches!(
            injected_failure(SENTINEL_POISON),
            Some(ContractError::NativeFailure { .. })
        ));
        assert!(injected_failure("gnome-shell").is_none());
    }

    #[test]
    fn stale_signatures_cover_the_observed_dead_object_errors() {
        assert!(is_stale_signature("Invalid object path: /gone"));
        assert!(is_stale_signature("The name is not activatable"));
        assert!(is_stale_signature(
            "org.freedesktop.DBus.Error.ServiceUnknown"
        ));
        assert!(is_stale_signature(
            "org.freedesktop.DBus.Error.UnknownObject"
        ));
        assert!(!is_stale_signature("access denied: not permitted"));
    }

    #[test]
    fn transport_and_interface_level_failures_are_never_stale() {
        // A dead transport or a missing method/interface is not element
        // staleness: the remedy is a fresh call, never a re-read of the tree.
        assert!(!is_stale_signature(
            "Disconnected: The connection is closed"
        ));
        assert!(!is_stale_signature(
            "org.freedesktop.DBus.Error.UnknownMethod"
        ));
        assert!(!is_stale_signature(
            "org.freedesktop.DBus.Error.NoSuchInterface"
        ));
        // Transport-loss identities are never stale either: they are the
        // typed TransportLoss class, mapped to NativeFailure after one
        // bounded re-connection.
        assert!(!is_stale_signature(
            "org.freedesktop.DBus.Error.NoValidConnection"
        ));
    }

    /// A fixture method-error reply carrying `name`: the offline stand-in for
    /// what the bus produces on the wire.
    fn method_error(name: &'static str) -> zbus::Error {
        let Ok(name) = zbus::names::OwnedErrorName::try_from(name) else {
            panic!("fixture error name must be a valid D-Bus error name");
        };
        let Ok(reply) = zbus::Message::method_call("/org/a11y/atspi/test", "Ping")
            .and_then(|builder| builder.build(&()))
        else {
            panic!("fixture reply message must build");
        };
        zbus::Error::MethodError(name, None, reply)
    }

    #[test]
    fn typed_error_names_classify_the_failure_mode() {
        // Transport loss: exactly the two bus-level identities — the old
        // text matcher also caught UnknownMethod/NoSuchInterface here and
        // misrouted app-level errors into reconnection storms.
        assert_eq!(
            classify_error_name(dbus_error_names::DISCONNECTED),
            BusFailureClass::TransportLoss
        );
        assert_eq!(
            classify_error_name(dbus_error_names::NO_VALID_CONNECTION),
            BusFailureClass::TransportLoss
        );
        // UnknownMethod / NoSuchInterface are NOT transport loss: the element
        // or interface is gone and the remedy is a re-read of the tree, with
        // no reconnection of a healthy bus.
        assert_eq!(
            classify_error_name(dbus_error_names::UNKNOWN_METHOD),
            BusFailureClass::ElementGone
        );
        assert_eq!(
            classify_error_name("org.a11y.atspi.Error.NoSuchInterface"),
            BusFailureClass::ElementGone
        );
        // Everything else is untyped: the ownership gate decides.
        assert_eq!(
            classify_error_name("org.freedesktop.DBus.Error.UnknownObject"),
            BusFailureClass::Untyped
        );
        assert_eq!(
            classify_error_name("org.freedesktop.DBus.Error.AccessDenied"),
            BusFailureClass::Untyped
        );
    }

    #[test]
    fn typed_zbus_errors_classify_the_failure_mode() {
        // zbus's own closed-connection classes: the transport, not the element.
        let io = zbus::Error::InputOutput(Arc::new(std::io::Error::other("connection closed")));
        assert_eq!(classify_bus_error(&io), BusFailureClass::TransportLoss);
        // A method error reply branches on the carried error NAME, never on
        // the free-form message text.
        assert_eq!(
            classify_bus_error(&method_error("org.freedesktop.DBus.Error.Disconnected")),
            BusFailureClass::TransportLoss
        );
        assert_eq!(
            classify_bus_error(&method_error(
                "org.freedesktop.DBus.Error.NoValidConnection"
            )),
            BusFailureClass::TransportLoss
        );
        assert_eq!(
            classify_bus_error(&method_error("org.freedesktop.DBus.Error.UnknownMethod")),
            BusFailureClass::ElementGone
        );
        assert_eq!(
            classify_bus_error(&method_error("org.a11y.atspi.Error.NoSuchInterface")),
            BusFailureClass::ElementGone
        );
        assert_eq!(
            classify_bus_error(&method_error("org.freedesktop.DBus.Error.UnknownObject")),
            BusFailureClass::Untyped
        );
        assert_eq!(
            classify_bus_error(&zbus::Error::InvalidReply),
            BusFailureClass::Untyped
        );
    }

    #[test]
    fn tree_budgets_stay_inside_the_core_budget() {
        // The core budgets desktop.tree at 15s (omnibus-core/src/limits.rs);
        // the adapter's whole call must finish strictly inside it, and the
        // walk deadline plus at most one in-flight node read must fill the
        // whole-call bound exactly: a partial snapshot materialized at the
        // deadline is returned, never discarded by a timeout race.
        assert_eq!(
            TREE_CALL_BUDGET,
            Duration::from_secs(CORE_TREE_BUDGET_SECS - TREE_CALL_MARGIN_SECS)
        );
        assert_eq!(TREE_BUDGET + NODE_BUDGET, TREE_CALL_BUDGET);
        assert!(TREE_CALL_BUDGET < Duration::from_secs(CORE_TREE_BUDGET_SECS));
        // The lazy reconnection inside `map_transport_loss` is bounded by one
        // whole-attempt CONNECT_BUDGET: three per-phase budgets (3 x 5s = 15s)
        // would outrun the 14s whole-call bound and mask the honest
        // NativeFailure behind the outer TimeoutError. The assertion below
        // only checks the inequality; the mechanism that actually enforces it
        // is the inner `tokio::time::timeout(CONNECT_BUDGET,
        // self.try_reconnect())` in `map_transport_loss`.
        assert!(CONNECT_BUDGET <= TREE_CALL_BUDGET);
    }

    #[test]
    fn missing_properties_are_absent_not_broken() {
        assert!(is_missing_property_signature(
            "org.freedesktop.DBus.Error.UnknownProperty: No such property 'HelpText'"
        ));
        assert!(!is_missing_property_signature(
            "org.freedesktop.DBus.Error.AccessDenied"
        ));
    }

    /// DTO codecs from a fixture payload (no D-Bus): the adapter fills the tree
    /// DTO from native data; serde must round-trip the exact shape.
    #[test]
    fn a11y_tree_dto_roundtrips_from_fixture() {
        let tree = A11yTree {
            app: String::from("fixture-app"),
            pid: Some(7),
            root: A11yNode {
                node_ref: String::from(":1.1|/org/a11y/atspi/accessible/root"),
                role: String::from("application"),
                name: String::from("fixture-app"),
                pid: Some(7),
                children: vec![A11yNode {
                    node_ref: String::from(":1.1|/org/a11y/atspi/accessible/1"),
                    role: String::from("push button"),
                    name: String::from("OK"),
                    pid: None,
                    children: vec![],
                }],
            },
            node_count: 2,
            truncated: true,
        };
        let text = serde_json::to_string(&tree).unwrap_or_default();
        let fallback = A11yTree {
            app: String::new(),
            pid: None,
            root: A11yNode {
                node_ref: String::new(),
                role: String::new(),
                name: String::new(),
                pid: None,
                children: vec![],
            },
            node_count: 0,
            truncated: false,
        };
        let back: A11yTree = serde_json::from_str(&text).unwrap_or(fallback);
        assert_eq!(back, tree);
        assert!(text.contains("node_ref"));
    }

    #[test]
    fn malformed_node_refs_are_stale_without_any_bus_traffic() {
        // decode_node_ref is the first line of defense in every operation.
        assert!(decode_node_ref("no-separator").is_none());
        assert!(decode_node_ref(":1.1|not-a-path").is_none());
    }
}
