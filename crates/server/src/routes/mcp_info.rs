// Copyright Alexandre D. Díaz
//! `mcp_info` (`GET /mcp`) is only registered (via `App::configure`) when `mcp_info_enabled` is set
//! in `server.yaml`; it shows visitors how to point popular MCP clients (Claude Code, Claude
//! Desktop, Cursor, VS Code, plus a generic `mcp-remote`-bridge fallback for local/stdio-only
//! clients) at the `mcp` crate's endpoint, whose URL is set via `mcp_url`. Both settings are also
//! exposed to every template via `utils::get_minijinja_context` (`MCP_INFO_ENABLED`/`MCP_URL`),
//! which is how `minimal_layout.html` decides whether to show the nav link.
use actix_web::{get, HttpRequest, HttpResponse, Responder, Result};
use minijinja::context;

use crate::config::SERVER_CONFIG;
use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

/// The visitor's API key (`oghutils::mcp_key`), minted from the user id the auth proxy sent; only
/// with `auth_enabled` and a key secret, else `None` (the page then shows no key).
fn api_key(req: &HttpRequest) -> Option<String> {
    if !SERVER_CONFIG.get_auth_enabled() {
        return None;
    }
    let secret = oghutils::mcp_key::secret()?;
    let id = req
        .headers()
        .get(SERVER_CONFIG.get_auth_user_header())?
        .to_str()
        .ok()?;
    let key = oghutils::mcp_key::mint(&secret, id.trim());
    if key.is_none() {
        log::warn!(
            "MCP key: the user header isn't a numeric id (oauth2-proxy needs --user-id-claim=sub)"
        );
    }
    key
}

#[get("/mcp")]
pub async fn route(tmpl_env: MiniJinjaRenderer, req: HttpRequest) -> Result<HttpResponse> {
    let key = api_key(&req);
    let html = tmpl_env.render(
        "pages/mcp_info.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "mcp_info",
                MCP_KEY => key.clone()
            )
        ),
    )?;
    let mut res = html.respond_to(&req).map_into_boxed_body();
    if key.is_some() {
        // A bearer secret: never cached, by the browser or a proxy.
        res.headers_mut().insert(
            actix_web::http::header::CACHE_CONTROL,
            actix_web::http::header::HeaderValue::from_static("no-store"),
        );
    }
    Ok(res)
}
