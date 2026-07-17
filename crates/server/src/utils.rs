// Copyright Alexandre D. Díaz
use actix_web::HttpRequest;
use minijinja::{context, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::sync::LazyLock;

use crate::config::SERVER_CONFIG;

static PIP_NAMES_MAP: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    let path = "files/pip_names.txt";
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            log::error!("No se pudo abrir {path}: {e}");
            return HashMap::new();
        }
    };

    let reader = BufReader::new(file);
    let mut map = HashMap::new();

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l.trim().to_string(),
            Err(_) => continue,
        };

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut parts = line.splitn(2, ',');
        let Some(key_part) = parts.next() else {
            continue;
        };
        let Some(value_part) = parts.next() else {
            continue;
        };

        let key = key_part.trim().to_ascii_lowercase();
        let value = value_part.trim().to_string();

        map.insert(key, value);
    }

    map
});

pub fn normalize_python_dep(name: String) -> String {
    let lower = name.to_ascii_lowercase();
    PIP_NAMES_MAP.get(&lower).cloned().unwrap_or(name)
}

pub fn get_base_url(req: &HttpRequest) -> String {
    let conn_info = req.connection_info();
    format!("{}://{}", conn_info.scheme(), conn_info.host())
}

pub fn get_minijinja_context(req: &HttpRequest) -> Value {
    let base_url = get_base_url(req);
    context!(
        REQ_SCHEME => req.connection_info().scheme().to_string(),
        REQ_HOST => req.connection_info().host().to_string(),
        REQ_BASE_URL => base_url.clone(),
        REQ_URL => format!("{}{}", &base_url, req.path()),
        MCP_INFO_ENABLED => SERVER_CONFIG.get_mcp_info_enabled(),
        MCP_URL => SERVER_CONFIG.get_mcp_url().clone(),
        SEO_ENABLED => SERVER_CONFIG.get_seo_enabled(),
    )
}
