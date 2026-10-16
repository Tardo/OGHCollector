// Copyright Alexandre D. Díaz
//! Workflow secrets: what a node's `api_key_env` names and the `${...}` in its `url`
//! (`oghworkflow::secret_ref`/`url_parts`, the same rules the editor validates with), resolved
//! here, in the job, only when the node acts for real: dry-run and a cached AI answer never read
//! them. A name = the Docker secret `/run/secrets/<name in lowercase>`, else the env var
//! (`config::read_secret`). `vault:<path>#<key>` = HashiCorp Vault (OpenBao too): `GET
//! $VAULT_ADDR/v1/<path>` with `X-Vault-Token` (`VAULT_TOKEN`, or the `vault_token` Docker secret;
//! neither = a Vault Proxy with `use_auto_auth_token` adds it, and renews it), and
//! `X-Vault-Namespace` (`VAULT_NAMESPACE`); `VAULT_CACERT` = a PEM bundle trusted besides the
//! system's (only for Vault). The value is `data.data.<key>` (KV v2, whose API path has `data/`
//! after the mount) else `data.<key>` (KV v1, other engines). Values never reach templates, logs
//! or the ledger: errors name the reference, never a value or the token. Cached per process = per
//! job (each is a child process), failures too: a Vault outage costs one timeout per reference
//! and job, and the next job reads a rotated secret.
use std::collections::HashMap;
use std::env;
use std::sync::{LazyLock, Mutex};

use oghworkflow::{secret_ref, url_parts, SecretRef, UrlPart};
use serde_json::Value;

use crate::{config, gitclient};

type Cache = Mutex<HashMap<String, Result<String, String>>>;
static CACHE: LazyLock<Cache> = LazyLock::new(Cache::default);

/// The value of a secret reference.
pub async fn get(reference: &str) -> Result<String, String> {
    if let Some(known) = CACHE.lock().unwrap().get(reference) {
        return known.clone();
    }
    let value = match secret_ref(reference)? {
        SecretRef::Env(name) => {
            config::read_secret(&name.to_lowercase(), name).ok_or(format!("{name} isn't set"))
        }
        SecretRef::Vault { path, key } => vault(path, key).await,
    };
    CACHE
        .lock()
        .unwrap()
        .insert(reference.to_string(), value.clone());
    value
}

/// A `url` with its `${...}` secrets filled in.
pub async fn expand(url: &str) -> Result<String, String> {
    let mut out = String::new();
    for part in url_parts(url)? {
        match part {
            UrlPart::Text(text) => out.push_str(text),
            UrlPart::Secret(reference) => out.push_str(&get(reference).await?),
        }
    }
    Ok(out)
}

async fn vault(path: &str, key: &str) -> Result<String, String> {
    let at = format!("vault:{path}#{key}");
    let addr = env::var("VAULT_ADDR").map_err(|_| format!("{at}: VAULT_ADDR isn't set"))?;
    let mut client = gitclient::http_builder();
    if let Ok(file) = env::var("VAULT_CACERT") {
        let pem = std::fs::read(&file).map_err(|e| format!("VAULT_CACERT {file}: {e}"))?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|e| format!("VAULT_CACERT {file}: {e}"))?;
        client = client.tls_certs_merge(certs);
    }
    let client = client.build().map_err(|e| e.to_string())?;
    let token = config::read_secret("vault_token", "VAULT_TOKEN");
    let namespace = env::var("VAULT_NAMESPACE").ok();
    read_vault(&client, &addr, token, namespace, path, key)
        .await
        .map_err(|err| format!("{at}: {err}"))
}

/// `key` of what Vault at `addr` answers for `path`.
async fn read_vault(
    client: &reqwest::Client,
    addr: &str,
    token: Option<String>,
    namespace: Option<String>,
    path: &str,
    key: &str,
) -> Result<String, String> {
    let mut req = client.get(format!("{}/v1/{path}", addr.trim_end_matches('/')));
    if let Some(token) = token {
        req = req.header("X-Vault-Token", token);
    }
    if let Some(namespace) = namespace.filter(|n| !n.is_empty()) {
        req = req.header("X-Vault-Namespace", namespace);
    }
    let res = req.send().await.map_err(|e| e.without_url().to_string())?;
    let status = res.status();
    let body: Value = res.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let why: Vec<&str> = body["errors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let hint = match status == reqwest::StatusCode::NOT_FOUND && !path.contains("/data/") {
            true => " (a KV v2 path has data/ after the mount: secret/data/...)",
            false => "",
        };
        return Err(format!("Vault answered {status} {}{hint}", why.join("; ")));
    }
    let data = &body["data"];
    data["data"][key]
        .as_str()
        .or(data[key].as_str())
        .map(str::to_string)
        .ok_or(format!("no text '{key}' there"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn vault_answers_kv_v1_and_v2_with_its_token_only() {
        let app = axum::Router::new().fallback(
            |headers: axum::http::HeaderMap, uri: axum::http::Uri| async move {
                let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
                if header("x-vault-token") != Some("t0k") {
                    let body = serde_json::json!({"errors": ["permission denied"]});
                    return (axum::http::StatusCode::FORBIDDEN, axum::Json(body));
                }
                let body = match (uri.path(), header("x-vault-namespace")) {
                    ("/v1/secret/data/ogh", None) => {
                        serde_json::json!({"data": {"data": {"key": "v2"}, "metadata": {}}})
                    }
                    ("/v1/kv/ogh", Some("team")) => serde_json::json!({"data": {"key": "v1"}}),
                    _ => {
                        let body = serde_json::json!({"errors": []});
                        return (axum::http::StatusCode::NOT_FOUND, axum::Json(body));
                    }
                };
                (axum::http::StatusCode::OK, axum::Json(body))
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = gitclient::http_client();
        let read = |token: &str, namespace: Option<&str>, path: &'static str, key: &'static str| {
            let (client, addr) = (client.clone(), addr.clone());
            let (token, namespace) = (Some(token.to_string()), namespace.map(str::to_string));
            async move { read_vault(&client, &addr, token, namespace, path, key).await }
        };

        assert_eq!(
            read("t0k", None, "secret/data/ogh", "key").await,
            Ok("v2".into())
        );
        assert_eq!(
            read("t0k", Some("team"), "kv/ogh", "key").await,
            Ok("v1".into())
        );
        let missing = read("t0k", None, "secret/data/ogh", "nope")
            .await
            .unwrap_err();
        assert!(missing.contains("no text 'nope'"), "{missing}");
        let v1_path = read("t0k", None, "secret/ogh", "key").await.unwrap_err();
        assert!(
            v1_path.contains("404") && v1_path.contains("secret/data/"),
            "{v1_path}"
        );
        let denied = read("wrong", None, "secret/data/ogh", "key")
            .await
            .unwrap_err();
        assert!(
            denied.contains("403") && denied.contains("permission denied"),
            "{denied}"
        );
        assert!(!denied.contains("wrong"), "the token stays out: {denied}");
    }

    #[tokio::test]
    async fn references_resolve_once_per_job() {
        std::env::set_var("OGH_TEST_SECRET_ONCE", "first");
        assert_eq!(get("OGH_TEST_SECRET_ONCE").await, Ok("first".into()));
        std::env::set_var("OGH_TEST_SECRET_ONCE", "second");
        assert_eq!(get("OGH_TEST_SECRET_ONCE").await, Ok("first".into()));
        assert_eq!(
            expand("http://a:1/n/${OGH_TEST_SECRET_ONCE}?x=1").await,
            Ok("http://a:1/n/first?x=1".into())
        );
        assert!(get("OGH_TEST_SECRET_UNSET")
            .await
            .unwrap_err()
            .contains("isn't set"));
        assert!(get("GH_TOKEN")
            .await
            .unwrap_err()
            .contains("collector's own"));
    }
}
