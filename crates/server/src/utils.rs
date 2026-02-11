// Copyright Alexandre D. Díaz
use actix_web::HttpRequest;
use minijinja::{context, Value};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::sync::LazyLock;

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

pub fn get_minijinja_context(req: &HttpRequest) -> Value {
    let scheme = req.connection_info().scheme().to_string();
    let host = req.connection_info().host().to_string();
    context!(
        REQ_SCHEME => scheme.clone(),
        REQ_HOST => host.clone(),
        REQ_BASE_URL => format!("{}://{}", &scheme, &host),
    )
}
