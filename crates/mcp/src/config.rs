// Copyright Alexandre D. Díaz
use config::Config;
use lazy_static::lazy_static;

/// Mirrors `server::config::OGHServerConfig` in spirit, but scoped to what `mcp` actually
/// needs. File source is `./mcp.yaml` (or `.json`, etc.) relative to cwd, same convention as
/// `server`'s `./server.yaml`; the `OGHCOLLECTOR_MCP_` env prefix keeps `OGHCOLLECTOR_MCP_CACHE_TTL`
/// working exactly as before this file existed.
#[derive(Debug)]
pub struct OGHMcpConfig {
    cache_ttl: u64,
}

impl OGHMcpConfig {
    pub fn new() -> OGHMcpConfig {
        let settings = Config::builder()
            .add_source(config::File::with_name("./mcp").required(false))
            .add_source(config::Environment::with_prefix("OGHCOLLECTOR_MCP"))
            .build()
            .unwrap();

        let cache_ttl = settings.get_int("cache_ttl").unwrap_or(3600) as u64;
        OGHMcpConfig { cache_ttl }
    }

    pub fn get_cache_ttl(&self) -> &u64 {
        &self.cache_ttl
    }
}

lazy_static! {
    pub static ref MCP_CONFIG: OGHMcpConfig = OGHMcpConfig::new();
}
