//! The MCP facade over rmcp (specs/contracts/mcp-client-daemon.md): stdio
//! transport, one session per connection, tool list reflecting the fresh probe,
//! typed errors with remedies, no consent surface — no MCP method accepts a
//! consent token or field, and an in-band "the user confirmed" is always ignored.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, InitializeRequestParams, InitializeResult,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};

use crate::Core;
use crate::error_map::render;

/// The facade state: the wired core plus the client name captured at initialize.
pub struct OmnibusHandler {
    core: Arc<Core>,
    client_name: std::sync::OnceLock<String>,
}

impl OmnibusHandler {
    pub fn new(core: Core) -> Self {
        Self {
            core: Arc::new(core),
            client_name: std::sync::OnceLock::new(),
        }
    }

    /// The captured MCP client identity (audit `actor` correlation); falls back
    /// to a stable placeholder when initialize never carried a name.
    pub fn actor_name(&self) -> String {
        self.client_name
            .get()
            .cloned()
            .unwrap_or_else(|| String::from("mcp-client"))
    }

    /// Serves one stdio session to completion (the daemon dies with the client
    /// process; there is no supervisor and no reconnect surface).
    pub async fn serve_stdio(self) -> Result<(), String> {
        let running = self
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| format!("mcp serve failed: {e}"))?;
        // Any quit reason ends a stdio child normally: the client is gone.
        let _quit = running
            .waiting()
            .await
            .map_err(|e| format!("mcp session task failed: {e}"))?;
        Ok(())
    }
}

/// Renders a contract error as the tool-level failure text: the closed taxonomy
/// code name plus the doctor/grant remedy, visible to the calling agent.
fn error_text(error: &omnibus_contracts::errors::ContractError, tool: &str) -> String {
    let capability = crate::error_map::capability_for_tool(tool);
    render(error, capability)
}

#[allow(clippy::needless_pass_by_value)]
fn tool_error(message: String) -> CallToolResult {
    // Tool-level error (is_error=true): the request was routable, the tool ran
    // (or refused) — the agent must see the message with the remedy.
    CallToolResult::error(vec![ContentBlock::text(message)])
}

impl ServerHandler for OmnibusHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new(
                "omnibusd",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "capabilities() is the machine truth about this machine; \
                 hidden tools mean absent capabilities — run doctor for remedies",
            )
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        // Actor capture (audit correlation): client name from MCP initialize.
        let _ = self.client_name.set(request.client_info.name.clone());
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let specs = crate::tools::visible_tools(&self.core);
        let tools = specs
            .iter()
            .map(|spec| {
                Tool::new(
                    spec.name.clone(),
                    spec.description,
                    Arc::new(
                        crate::tools::schema_json(spec)
                            .as_object()
                            .cloned()
                            .unwrap_or_default(),
                    ),
                )
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, McpError> {
        let arguments = request.arguments.clone().unwrap_or_default();
        let result = match crate::tools::dispatch(&self.core, &request.name, arguments).await {
            Ok(outcome) => {
                // A degraded audit sink never fails the tool call; the degradation
                // is visible through capabilities()/doctor (audit.md).
                let json = serde_json::to_string_pretty(&outcome.value)
                    .unwrap_or_else(|_| String::from("null"));
                CallToolResult::success(vec![ContentBlock::text(json)])
            }
            Err(error) => {
                // Every contract error surfaces as a typed tool-level error; the
                // code name and the remedy travel in the message content.
                tool_error(error_text(&error, &request.name))
            }
        };
        Ok(result.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_error_text_carries_the_code_name_and_remedy() {
        let text = error_text(
            &omnibus_contracts::errors::ContractError::ScopeDenied,
            "pkg_install",
        );
        assert!(text.contains("ScopeDenied"));
        assert!(text.contains("omnibus grant"));

        let hidden = error_text(
            &omnibus_contracts::errors::ContractError::CapabilityRequired,
            "net_status",
        );
        assert!(hidden.contains("CapabilityRequired"));
        assert!(hidden.contains("doctor"));
    }

    #[test]
    fn tool_level_error_marks_is_error() {
        let result = tool_error(String::from("CapabilityRequired: …"));
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.content.len(), 1);
    }
}
