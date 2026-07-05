// Copyright Alexandre D. Díaz
mod config;
mod tools;

use std::path::PathBuf;

use axum::Router;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};

use tools::OghMcp;

/// This is a hosted endpoint (unlike `server`'s HTTP daemon, its cwd is always the repo
/// root / container `/app`), so a relative default is fine here - but it's still explicit
/// and fails loudly rather than silently opening an empty/missing DB.
fn resolve_db_path() -> PathBuf {
    let raw = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("OGHCOLLECTOR_DB_PATH").ok())
        .unwrap_or_else(|| "data/data.db".to_string());
    std::fs::canonicalize(&raw).unwrap_or_else(|e| {
        let cwd = std::env::current_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        eprintln!(
            "oghmcp: cannot find SQLite DB at '{raw}' (resolved relative to cwd '{cwd}'): {e}. \
             Pass an absolute path as the first CLI argument or set OGHCOLLECTOR_DB_PATH."
        );
        std::process::exit(1);
    })
}

fn resolve_bind_addr() -> String {
    std::env::var("OGHCOLLECTOR_MCP_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8081".to_string())
}

/// rmcp rejects requests whose `Host` header doesn't match this list (DNS-rebinding
/// protection) - the built-in default only allows loopback hosts, so a real deployment
/// reachable by hostname/IP other than localhost must override it.
fn resolve_allowed_hosts() -> Vec<String> {
    match std::env::var("OGHCOLLECTOR_MCP_ALLOWED_HOSTS") {
        Ok(raw) => raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Err(_) => {
            log::warn!(
                "OGHCOLLECTOR_MCP_ALLOWED_HOSTS not set - only accepting Host: \
                 localhost/127.0.0.1/::1. Set it to a comma-separated list of your public \
                 hostname(s) for remote clients to connect."
            );
            vec![
                "localhost".to_string(),
                "127.0.0.1".to_string(),
                "::1".to_string(),
            ]
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    let db_path = resolve_db_path();
    log::info!("using SQLite DB at {}", db_path.display());
    let pool = sqlitedb::new_read_pool(&db_path.to_string_lossy(), 4);

    // Touch MCP_CONFIG now so a malformed mcp.yaml fails at startup rather than on the
    // first get_module call, and so the effective TTL is visible in the logs.
    log::info!(
        "get_module cache TTL: {}s",
        *config::MCP_CONFIG.get_cache_ttl()
    );

    let allowed_hosts = resolve_allowed_hosts();
    let service = StreamableHttpService::new(
        move || Ok(OghMcp::new(pool.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default().with_allowed_hosts(allowed_hosts),
    );

    let router = Router::new().nest_service("/mcp", service);
    let bind_addr = resolve_bind_addr();
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    log::info!("MCP endpoint listening on http://{bind_addr}/mcp");
    axum::serve(listener, router).await?;
    Ok(())
}
