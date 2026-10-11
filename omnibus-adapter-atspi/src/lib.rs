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
//! [`ContractError::AdapterUnavailable`].
//!
//! Native types (zbus messages, AT-SPI object references) never leave this
//! crate — mapping to contract DTOs happens inside the adapter
//! (specs/architecture/spi-registry.md "DTO Discipline").

mod node_ref;
mod states;
mod walk;

pub use node_ref::{decode_node_ref, encode_node_ref};

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
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

/// Wall-clock budget for one whole tree walk (the tool layer budgets 15s).
const TREE_BUDGET: Duration = Duration::from_secs(12);
/// Wall-clock budget for single-node operations.
const NODE_BUDGET: Duration = Duration::from_secs(5);

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
    conn: Connection,
    /// True when the registry well-known name is owned on the connected bus.
    registry_reachable: bool,
    /// True when the dedicated a11y bus answered (false = session-bus fallback).
    dedicated_bus: bool,
}

impl AtspiAdapter {
    /// Connects to the a11y bus (session-bus fallback) and probes the registry.
    ///
    /// Returns [`ContractError::AdapterUnavailable`] only when no session bus is
    /// reachable at all; a connected bus without a registry reports honestly
    /// through [`AtspiAdapter::registry_reachable`] and the adapter report.
    pub async fn connect() -> Result<Self, ContractError> {
        let session = Connection::session()
            .await
            .map_err(|_| ContractError::AdapterUnavailable)?;
        let mut conn = session.clone();
        let mut dedicated_bus = false;
        if let Some(bus) = dedicated_a11y_bus(&session).await {
            conn = bus;
            dedicated_bus = true;
        }
        let registry_reachable = name_has_owner(&conn, REGISTRY_DEST).await;
        Ok(Self {
            conn,
            registry_reachable,
            dedicated_bus,
        })
    }

    /// Whether the AT-SPI registry answers on the connected bus.
    pub fn registry_reachable(&self) -> bool {
        self.registry_reachable
    }

    /// Applications currently registered under the tree root.
    pub async fn discover_apps(&self) -> Result<Vec<DiscoveredApp>, ContractError> {
        if !self.registry_reachable {
            return Err(ContractError::AdapterUnavailable);
        }
        let children = self
            .contract(
                self.node_children(REGISTRY_DEST, ROOT_PATH).await,
                REGISTRY_DEST,
            )
            .await?;
        let mut apps = Vec::new();
        for (dest, path) in children {
            let name = self.node_name(&dest, &path).await.unwrap_or_default();
            let pid = connection_pid(&self.conn, &dest).await;
            apps.push(DiscoveredApp {
                name,
                pid,
                node_ref: node_ref::encode_node_ref(&dest, &path),
            });
        }
        Ok(apps)
    }

    /// Capability fact for one registry constant (CoN: names come from constants).
    fn capability_fact(&self, name: &str, note: String) -> CapabilityFact {
        CapabilityFact {
            name: String::from(name),
            ver: String::from(CONTRACT_VERSION),
            status: if self.registry_reachable {
                String::from("present")
            } else {
                String::from("degraded")
            },
            value: None,
            note: Some(note),
            remedy: None,
        }
    }

    /// A plain (non-caching) proxy: AT-SPI objects change constantly, caching
    /// would only add round-trips.
    async fn proxy<'a>(
        &'a self,
        dest: &'a str,
        path: &'a str,
        iface: &'a str,
    ) -> zbus::Result<Proxy<'a>> {
        ProxyBuilder::new(&self.conn)
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
        let proxy = self.proxy(dest, path, IFACE_ACCESSIBLE).await?;
        proxy.get_property::<String>(property).await
    }

    async fn node_role(&self, dest: &str, path: &str) -> zbus::Result<String> {
        let proxy = self.proxy(dest, path, IFACE_ACCESSIBLE).await?;
        let message = proxy.call_method("GetRoleName", &()).await?;
        message.body().deserialize::<String>()
    }

    async fn node_children(&self, dest: &str, path: &str) -> zbus::Result<Vec<(String, String)>> {
        let proxy = self.proxy(dest, path, IFACE_ACCESSIBLE).await?;
        let message = proxy.call_method("GetChildren", &()).await?;
        let raw: Vec<(String, OwnedObjectPath)> = message.body().deserialize()?;
        Ok(raw
            .into_iter()
            .map(|(child_dest, child_path)| (child_dest, child_path.to_string()))
            .collect())
    }

    async fn node_interfaces(&self, dest: &str, path: &str) -> zbus::Result<Vec<String>> {
        let proxy = self.proxy(dest, path, IFACE_ACCESSIBLE).await?;
        let message = proxy.call_method("GetInterfaces", &()).await?;
        message.body().deserialize::<Vec<String>>()
    }

    async fn node_states(&self, dest: &str, path: &str) -> zbus::Result<Vec<String>> {
        let proxy = self.proxy(dest, path, IFACE_ACCESSIBLE).await?;
        let message = proxy.call_method("GetState", &()).await?;
        let words: Vec<u32> = message.body().deserialize()?;
        Ok(states::state_names(&words))
    }

    async fn node_attributes(&self, dest: &str, path: &str) -> zbus::Result<Vec<NamedValue>> {
        let proxy = self.proxy(dest, path, IFACE_ACCESSIBLE).await?;
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
        let proxy = self.proxy(dest, path, IFACE_ACTION).await?;
        let message = proxy.call_method("GetActions", &()).await?;
        let raw: Vec<(String, String, String)> = message.body().deserialize()?;
        Ok(raw.into_iter().map(|(name, _, _)| name).collect())
    }

    async fn has_owner(&self, name: &str) -> bool {
        name_has_owner(&self.conn, name).await
    }

    /// Converts one raw bus result into a contract result in the same breath.
    async fn contract<T>(&self, result: zbus::Result<T>, dest: &str) -> Result<T, ContractError> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => Err(self.map_bus_error(error, dest).await),
        }
    }

    /// Maps a raw D-Bus failure onto the closed contract taxonomy.
    ///
    /// A vanished destination or a dead object path means the referenced element
    /// no longer exists: the agent re-reads the tree (ElementStale). Everything
    /// else is a native backend failure with the original message preserved.
    async fn map_bus_error(&self, error: zbus::Error, dest: &str) -> ContractError {
        if !self.has_owner(dest).await || is_stale_signature(&error.to_string()) {
            return ContractError::ElementStale;
        }
        ContractError::NativeFailure {
            code: 1,
            payload: error.to_string(),
        }
    }

    /// Depth-first materialization of one node with budget accounting.
    ///
    /// A dead subtree shrinks the snapshot (marked truncated) instead of failing
    /// the whole tree: a snapshot is most useful when it reports what it saw.
    fn visit<'s>(
        &'s self,
        dest: String,
        path: String,
        depth: u32,
        budget: &'s mut walk::WalkBudget,
        deadline: Instant,
    ) -> Pin<Box<dyn Future<Output = Result<A11yNode, ContractError>> + Send + 's>> {
        Box::pin(async move {
            if Instant::now() >= deadline {
                return Err(ContractError::TimeoutError);
            }
            budget.record_node();
            let name = self.node_name(&dest, &path).await.unwrap_or_default();
            let role = self.node_role(&dest, &path).await.unwrap_or_default();
            let mut children = Vec::new();
            if budget.may_descend(depth + 1) {
                match self.node_children(&dest, &path).await {
                    Ok(refs) => {
                        for (child_dest, child_path) in refs {
                            if !budget.may_descend(depth + 1) {
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
                    Err(error) => match self.map_bus_error(error, &dest).await {
                        ContractError::ElementStale => budget.mark_truncated(),
                        other => return Err(other),
                    },
                }
            } else {
                budget.mark_truncated();
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

    async fn element_inner(&self, dest: &str, path: &str) -> Result<A11yElement, ContractError> {
        let interfaces = self
            .contract(self.node_interfaces(dest, path).await, dest)
            .await?;
        let mut attributes = self
            .contract(self.node_attributes(dest, path).await, dest)
            .await?;
        if let Ok(description) = self.node_string_property(dest, path, "Description").await
            && !description.is_empty()
        {
            attributes.push(NamedValue {
                name: String::from("description"),
                value: description,
            });
        }
        let actions = if interfaces.iter().any(|iface| iface == IFACE_ACTION) {
            self.contract(self.action_names(dest, path).await, dest)
                .await?
        } else {
            Vec::new()
        };
        let child_refs = match self.node_children(dest, path).await {
            Ok(refs) => refs
                .into_iter()
                .map(|(d, p)| node_ref::encode_node_ref(&d, &p))
                .collect(),
            Err(_) => Vec::new(),
        };
        Ok(A11yElement {
            node_ref: node_ref::encode_node_ref(dest, path),
            role: self.node_role(dest, path).await.unwrap_or_default(),
            name: self.node_name(dest, path).await.unwrap_or_default(),
            attributes,
            states: self.node_states(dest, path).await.unwrap_or_default(),
            actions,
            child_refs,
        })
    }
}

/// Contract-suite failure injection points (families.rs sentinels): the suite
/// binds these selector names to configured failure classes, so a live backend
/// can exercise the poison and timeout branches of the harness.
fn injected_failure(app: &str) -> Option<ContractError> {
    match app {
        x if x == SENTINEL_SLOW => Some(ContractError::TimeoutError),
        x if x == SENTINEL_POISON => Some(ContractError::NativeFailure {
            code: 1,
            payload: String::from("contract-suite poison selector (families sentinel)"),
        }),
        _ => None,
    }
}

/// Case-insensitive application resolution: the full accessible name or its
/// basename (e.g. `org.gnome.Nautilus` answers to `nautilus`). A requested pid
/// disambiguates among same-name applications.
fn resolve_app<'a>(
    apps: &'a [DiscoveredApp],
    selector: &TreeSelector,
) -> Option<&'a DiscoveredApp> {
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
        Some(pid) => matches.into_iter().find(|app| app.pid == Some(pid)),
        None => matches.into_iter().next(),
    }
}

/// Error-text signatures that mean "the referenced element is gone".
///
/// Matching runs on a normalized form (lowercase, alphanumerics only) so both
/// the human message ("Invalid object path") and the D-Bus error name
/// (`org.freedesktop.DBus.Error.UnknownObject`) hit the same needle.
fn is_stale_signature(message: &str) -> bool {
    let normalized: String = message
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    [
        "unknownobject",
        "invalidobjectpath",
        "nosuchobject",
        "serviceunknown",
        "notactivatable",
        "namehasnoowner",
        "unknownmethod",
        "nosuchinterface",
        "disconnected",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

/// The dedicated a11y bus connection, when `org.a11y.Bus` publishes an address.
async fn dedicated_a11y_bus(session: &Connection) -> Option<Connection> {
    if !name_has_owner(session, A11Y_BUS_DEST).await {
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
        .build()
        .await;
    connection.ok()
}

/// `NameHasOwner` over the raw bus interface; false on any failure.
async fn name_has_owner(conn: &Connection, name: &str) -> bool {
    let Ok(proxy) = Proxy::new(conn, DBUS_DEST, DBUS_PATH, DBUS_DEST).await else {
        return false;
    };
    let Ok(message) = proxy.call_method("NameHasOwner", &(name)).await else {
        return false;
    };
    message.body().deserialize::<bool>().unwrap_or(false)
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
        notes.push(if self.dedicated_bus {
            String::from("a11y bus via the org.a11y.Bus address")
        } else {
            String::from("a11y bus absent; the session bus is used directly")
        });
        notes.push(if self.registry_reachable {
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
        let walked = tokio::time::timeout(TREE_BUDGET, async {
            if !self.registry_reachable {
                return Err(ContractError::AdapterUnavailable);
            }
            if let Some(failure) = injected_failure(&selector.app) {
                return Err(failure);
            }
            let apps = self.discover_apps().await?;
            let Some(app) = resolve_app(&apps, &selector) else {
                return Err(ContractError::NativeFailure {
                    code: 1,
                    payload: format!(
                        "application {} is not present in the accessibility tree",
                        selector.app
                    ),
                });
            };
            let Some((dest, path)) = node_ref::decode_node_ref(&app.node_ref) else {
                return Err(ContractError::ElementStale);
            };
            let mut budget = walk::WalkBudget::new(walk::MAX_DEPTH, walk::MAX_NODES);
            let deadline = Instant::now() + TREE_BUDGET;
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
        if !self.registry_reachable {
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
        if !self.registry_reachable {
            return Err(ContractError::AdapterUnavailable);
        }
        if !self.has_owner(&dest).await {
            return Err(ContractError::ElementStale);
        }
        let named = match attribute.to_ascii_lowercase().as_str() {
            "name" => self.node_name(&dest, &path).await.ok(),
            "role" => self.node_role(&dest, &path).await.ok(),
            "description" => self
                .node_string_property(&dest, &path, "Description")
                .await
                .ok(),
            "accessible-id" => self
                .node_string_property(&dest, &path, "AccessibleId")
                .await
                .ok(),
            "help-text" => self
                .node_string_property(&dest, &path, "HelpText")
                .await
                .ok(),
            _ => {
                let attributes = self
                    .contract(self.node_attributes(&dest, &path).await, &dest)
                    .await?;
                attributes
                    .into_iter()
                    .find(|pair| pair.name.eq_ignore_ascii_case(&attribute))
                    .map(|pair| pair.value)
            }
        };
        named.ok_or(ContractError::Unsupported {
            reason: format!("attribute {attribute} is not present on this element"),
        })
    }

    async fn set_value(&self, request: SetValueRequest) -> Result<WriteOutcome, ContractError> {
        let Some((dest, path)) = node_ref::decode_node_ref(&request.node_ref) else {
            return Err(ContractError::ElementStale);
        };
        if !self.registry_reachable {
            return Err(ContractError::AdapterUnavailable);
        }
        if !self.has_owner(&dest).await {
            return Err(ContractError::ElementStale);
        }
        let outcome = tokio::time::timeout(NODE_BUDGET, async {
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
            if has_value && let Ok(numeric) = request.value.trim().parse::<f64>() {
                let proxy = self.proxy(&dest, &path, IFACE_VALUE).await;
                if let Ok(proxy) = proxy {
                    // The typed write path: the Value property set, no keystrokes.
                    if proxy.set_property("CurrentValue", numeric).await.is_ok()
                        || proxy.set_property("Value", numeric).await.is_ok()
                    {
                        return Ok(WriteOutcome {
                            node_ref: request.node_ref.clone(),
                            applied: true,
                        });
                    }
                }
            }
            if has_editable {
                let applied = match self.proxy(&dest, &path, IFACE_EDITABLE_TEXT).await {
                    Ok(proxy) => proxy
                        .call_method("SetTextContents", &(request.value.as_str()))
                        .await
                        .and_then(|message| message.body().deserialize::<bool>())
                        .unwrap_or(false),
                    Err(_) => false,
                };
                return Ok(WriteOutcome {
                    node_ref: request.node_ref.clone(),
                    applied,
                });
            }
            Err(ContractError::Unsupported {
                reason: String::from(
                    "the element holds a numeric Value interface; the request value is not numeric",
                ),
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
        if !self.registry_reachable {
            return Err(ContractError::AdapterUnavailable);
        }
        if !self.has_owner(&dest).await {
            return Err(ContractError::ElementStale);
        }
        let outcome = tokio::time::timeout(NODE_BUDGET, async {
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
            let applied = match self.proxy(&dest, &path, IFACE_ACTION).await {
                Ok(proxy) => {
                    let call = proxy
                        .call_method("DoAction", &(i32::try_from(index).unwrap_or(i32::MAX)))
                        .await;
                    let message = self.contract(call, &dest).await?;
                    message.body().deserialize::<bool>().unwrap_or(false)
                }
                Err(e) => return Err(self.map_bus_error(e, &dest).await),
            };
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
        assert_eq!(by_full.map(|app| app.pid), Some(Some(4242)));
        let by_base = resolve_app(
            &apps,
            &TreeSelector {
                app: String::from("nautilus"),
                pid: None,
            },
        );
        assert_eq!(by_base.map(|app| app.pid), Some(Some(4242)));
        let by_pid = resolve_app(
            &apps,
            &TreeSelector {
                app: String::from("gnome-text-editor"),
                pid: Some(3),
            },
        );
        assert_eq!(by_pid, None);
        let missing = resolve_app(
            &apps,
            &TreeSelector {
                app: String::from("absent"),
                pid: None,
            },
        );
        assert_eq!(missing, None);
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
