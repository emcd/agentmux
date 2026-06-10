//! `help` tool: return tool/command help and JSON schemas.

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    tool, tool_router,
};

use crate::mcp::help::help_tool;
use crate::mcp::params::HelpParams;
use crate::mcp::server::McpServer;
use crate::mcp::validation::validate_help_request;

#[tool_router(router = tool_router_help, vis = "pub(crate)")]
impl McpServer {
    #[tool(
        name = "help",
        description = "Return tool/command help and JSON schemas. Query omitted or `agentmux` for tool list, `list` for list meta-tool commands, or `list.principals`/`send`/`look`/`raww` for exact schemas."
    )]
    async fn tool_help(
        &self,
        Parameters(params): Parameters<HelpParams>,
    ) -> Result<CallToolResult, McpError> {
        validate_help_request(&params)?;
        Ok(CallToolResult::success(vec![Content::json(help_tool(
            params,
        )?)?]))
    }
}
