// Copyright Alexandre D. Díaz
//! `mcp_info` (`GET /mcp`) is only registered (via `App::configure`) when `mcp_info_enabled` is set
//! in `server.yaml`; it shows visitors how to point popular MCP clients (Claude Code, Claude
//! Desktop, Cursor, VS Code, plus a generic `mcp-remote`-bridge fallback for local/stdio-only
//! clients) at the `mcp` crate's endpoint, whose URL is set via `mcp_url`. Both settings are also
//! exposed to every template via `utils::get_minijinja_context` (`MCP_INFO_ENABLED`/`MCP_URL`),
//! which is how `minimal_layout.html` decides whether to show the nav link.
use actix_web::{get, HttpRequest, Responder, Result};
use minijinja::context;

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

#[get("/mcp")]
pub async fn route(tmpl_env: MiniJinjaRenderer, req: HttpRequest) -> Result<impl Responder> {
    tmpl_env.render(
        "pages/mcp_info.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "mcp_info"
            )
        ),
    )
}
