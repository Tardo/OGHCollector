// Copyright Alexandre D. Díaz
//! The real instances of a Doodba project: a source's `instance:` (`collector.yaml`; the 8th job
//! argument as JSON, `-` = none) makes `doodba::collect` ask each deployed environment (prod,
//! demo; the URLs `doodba::parse_deploy` read from the copier answers) over Odoo's JSON-RPC
//! (`/jsonrpc`): version, installed modules with their versions, active internal users, active
//! languages. That is what the database says, not the repository. Odoo has no JWT: the login and
//! an API key (Odoo >= 14: user preferences > Account security) take the password's place.
//! - `login`; `api_key` = a secret reference (`secrets::get`: env/Docker secret or
//!   `vault:<path>#<key>`), `{repo}` replaced by the repository's name first so each project has
//!   its own; `db` (optional, `{repo}` too; else the instance's database list must have exactly one).
//! - `hosts` is mandatory: the URL comes from a file anyone with push access to the project
//!   repository writes, and the key goes to it. Only `https` URLs on those domains (or their
//!   subdomains) are called, redirects are never followed.
//!
//! A failed query keeps the last good answer and sets `error` (an outage never reads as "nothing
//! installed"). The key never reaches logs, errors or the DB: reqwest errors go without URL, and
//! Odoo's own error text is cut short.
use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};

use sqlitedb::models::doodba_project::{Deploy, InstalledModule, Instance, Instances};

use crate::{gitclient, secrets};

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub hosts: Vec<String>,
    pub login: String,
    pub api_key: String,
    #[serde(default)]
    pub db: Option<String>,
}

/// The 8th job argument of a source without `instance:`.
pub const NONE: &str = "-";

impl Config {
    pub fn parse(arg: &str) -> Result<Option<Config>, String> {
        if arg == NONE {
            return Ok(None);
        }
        let config: Config = serde_json::from_str(arg).map_err(|e| format!("instance: {e}"))?;
        config.validate()?;
        Ok(Some(config))
    }

    pub fn validate(&self) -> Result<(), String> {
        let host = |h: &str| {
            !h.is_empty()
                && !h.starts_with('.')
                && h.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        };
        if self.hosts.is_empty() || !self.hosts.iter().all(|h| host(h)) {
            return Err("instance.hosts: domains the instances live on (example.com)".into());
        }
        if self.login.trim().is_empty() {
            return Err("instance.login is empty".into());
        }
        oghworkflow::secret_ref(&fill(&self.api_key, "repo"))
            .map_err(|e| format!("instance.api_key: {e}"))?;
        Ok(())
    }

    fn allows(&self, url: &Url) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        url.scheme() == "https"
            && self
                .hosts
                .iter()
                .any(|h| host == h || host.ends_with(&format!(".{h}")))
    }
}

fn fill(text: &str, repo: &str) -> String {
    text.replace("{repo}", repo)
}

/// The instances of a project: each deployed environment with a URL is asked; one that isn't
/// deployed (anymore) is dropped, one that can't be asked keeps its `stored` answer + `error`.
pub async fn refresh(
    config: &Config,
    repo: &str,
    deploy: &Deploy,
    stored: &Instances,
) -> Instances {
    let client = reqwest_client();
    let ask = |url: &Option<String>, on: bool, old: &Option<Instance>| {
        let url = url.clone().filter(|_| on);
        let old = old.clone();
        let client = &client;
        async move {
            let url = url?;
            Some(match query(client, config, repo, &url).await {
                Ok(instance) => instance,
                Err(err) => {
                    log::warn!("'{repo}': can't query {url}: {err}");
                    Instance {
                        url,
                        error: Some(err),
                        ..old.unwrap_or_default()
                    }
                }
            })
        }
    };
    Instances {
        prod: ask(&deploy.prod_url, deploy.prod, &stored.prod).await,
        demo: ask(&deploy.demo_url, deploy.demo, &stored.demo).await,
    }
}

fn reqwest_client() -> reqwest::Client {
    gitclient::http_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_else(|e| panic!("Problem creating the client: {e:?}"))
}

pub async fn query(
    client: &reqwest::Client,
    config: &Config,
    repo: &str,
    url: &str,
) -> Result<Instance, String> {
    let base = Url::parse(url).map_err(|_| "not a URL".to_string())?;
    if !config.allows(&base) {
        return Err(format!(
            "{} isn't an https URL on instance.hosts",
            base.host_str().unwrap_or("?")
        ));
    }
    let endpoint = base.join("/jsonrpc").map_err(|e| e.to_string())?;
    fetch(client, config, repo, url, &endpoint).await
}

/// `query` past the host check (the tests' server isn't https).
async fn fetch(
    client: &reqwest::Client,
    config: &Config,
    repo: &str,
    url: &str,
    endpoint: &Url,
) -> Result<Instance, String> {
    let key = secrets::get(&fill(&config.api_key, repo)).await?;
    let rpc = |service: &'static str, method: &'static str, args: Value| {
        call(client, endpoint, service, method, args)
    };
    let server_version = rpc("common", "version", json!([]))
        .await?
        .get("server_version")
        .and_then(Value::as_str)
        .map(str::to_string);
    let db = match &config.db {
        Some(db) => fill(db, repo),
        None => {
            let list = rpc("db", "list", json!([])).await.map_err(|e| {
                format!("{e} (set instance.db when the instance hides its database list)")
            })?;
            match list.as_array().map(Vec::as_slice) {
                Some([only]) => only.as_str().unwrap_or_default().to_string(),
                _ => return Err("can't tell the database: set instance.db".into()),
            }
        }
    };
    let uid = rpc("common", "authenticate", json!([db, config.login, key, {}]))
        .await?
        .as_i64()
        .ok_or("login or API key refused")?;
    let object = |model: &'static str, method: &'static str, args: Value, kwargs: Value| {
        rpc(
            "object",
            "execute_kw",
            json!([db, uid, key, model, method, args, kwargs]),
        )
    };
    let modules = object(
        "ir.module.module",
        "search_read",
        json!([[["state", "in", ["installed", "to upgrade", "to remove"]]]]),
        json!({"fields": ["name", "installed_version", "state"], "order": "name"}),
    )
    .await?;
    // Counts and languages are extras: a user that may not read them still gets the modules.
    let users = object(
        "res.users",
        "search_count",
        json!([[["share", "=", false]]]),
        json!({}),
    )
    .await
    .ok()
    .and_then(|n| n.as_i64());
    let languages = object(
        "res.lang",
        "search_read",
        json!([[]]),
        json!({"fields": ["code"]}),
    )
    .await
    .ok()
    .map(|langs| {
        langs
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l["code"].as_str().map(str::to_string))
            .collect()
    })
    .unwrap_or_default();
    Ok(Instance {
        url: url.to_string(),
        checked_at: Some(sqlitedb::utils::date::get_sqlite_utc_now()),
        error: None,
        db: Some(db),
        server_version,
        modules: modules
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| {
                Some(InstalledModule {
                    name: m["name"].as_str()?.to_string(),
                    version: m["installed_version"].as_str().map(str::to_string),
                    state: m["state"].as_str().unwrap_or("installed").to_string(),
                })
            })
            .collect(),
        users,
        languages,
    })
}

/// One JSON-RPC call: its `result`, or why not (Odoo's message, short).
async fn call(
    client: &reqwest::Client,
    endpoint: &Url,
    service: &str,
    method: &str,
    args: Value,
) -> Result<Value, String> {
    let body = json!({"jsonrpc": "2.0", "method": "call", "id": 1,
        "params": {"service": service, "method": method, "args": args}});
    let res = client
        .post(endpoint.clone())
        .json(&body)
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    let status = res.status();
    let answer: Value = res
        .json()
        .await
        .map_err(|_| format!("{service}.{method}: answered {status}, not JSON-RPC"))?;
    if let Some(error) = answer.get("error") {
        let why = error["data"]["message"]
            .as_str()
            .or(error["message"].as_str())
            .unwrap_or("error");
        return Err(format!(
            "{service}.{method}: {}",
            why.chars().take(200).collect::<String>()
        ));
    }
    answer.get("result").cloned().ok_or(format!(
        "{service}.{method}: answered {status} without a result"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(hosts: &[&str]) -> Config {
        Config {
            hosts: hosts.iter().map(|h| h.to_string()).collect(),
            login: "ogh".into(),
            api_key: "OGH_TEST_INSTANCE_KEY".into(),
            db: Some("{repo}".into()),
        }
    }

    #[test]
    fn config_validates_and_only_allows_https_on_its_hosts() {
        assert_eq!(Config::parse("-"), Ok(None));
        assert!(Config::parse(r#"{"hosts":[],"login":"a","api_key":"K"}"#).is_err());
        assert!(
            Config::parse(r#"{"hosts":["a.b"],"login":"a","api_key":"GH_TOKEN"}"#)
                .unwrap_err()
                .contains("collector's own")
        );
        let ok = r#"{"hosts":["a.b"],"login":"a","api_key":"vault:kv/data/odoo/{repo}#key"}"#;
        assert!(Config::parse(ok).unwrap().is_some());
        let c = config(&["example.com"]);
        let allowed = |u: &str| c.allows(&Url::parse(u).unwrap());
        assert!(allowed("https://erp.example.com/web"));
        assert!(allowed("https://example.com"));
        assert!(!allowed("http://example.com"));
        assert!(!allowed("https://evilexample.com"));
        assert!(!allowed("https://example.com.evil.net"));
    }

    #[tokio::test]
    async fn queries_an_instance_and_keeps_the_key_out_of_errors() {
        std::env::set_var("OGH_TEST_INSTANCE_KEY", "s3cr3t-key");
        let app = axum::Router::new().route(
            "/jsonrpc",
            axum::routing::post(|axum::Json(body): axum::Json<Value>| async move {
                let p = &body["params"];
                let args = &p["args"];
                let result = match (p["service"].as_str(), p["method"].as_str()) {
                    (Some("common"), Some("version")) => json!({"server_version": "18.0"}),
                    (Some("common"), Some("authenticate")) => {
                        if args[2] == "s3cr3t-key" && args[1] == "ogh" && args[0] == "client-a" {
                            json!(7)
                        } else {
                            json!(false)
                        }
                    }
                    (Some("object"), Some("execute_kw")) => match args[3].as_str() {
                        Some("ir.module.module") => json!([
                            {"name": "base", "installed_version": "18.0.1.3", "state": "installed"},
                            {"name": "sale_x", "installed_version": false, "state": "to upgrade"}
                        ]),
                        Some("res.users") => json!(5),
                        _ => {
                            return axum::Json(
                                json!({"error": {"message": "x", "data": {"message": "Access Denied"}}}),
                            )
                        }
                    },
                    _ => json!(null),
                };
                axum::Json(json!({"jsonrpc": "2.0", "id": 1, "result": result}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest_client();
        let endpoint = Url::parse(&format!("{url}/jsonrpc")).unwrap();
        let got = fetch(&client, &config(&[]), "client-a", &url, &endpoint)
            .await
            .unwrap();
        assert_eq!(got.db.as_deref(), Some("client-a"));
        assert_eq!(got.server_version.as_deref(), Some("18.0"));
        assert_eq!(got.users, Some(5));
        assert!(got.languages.is_empty(), "denied extras are skipped");
        assert_eq!(got.modules.len(), 2);
        assert_eq!(got.modules[1].version, None);
        assert_eq!(got.modules[1].state, "to upgrade");
        let wrong = fetch(&client, &config(&[]), "other", &url, &endpoint)
            .await
            .unwrap_err();
        assert!(
            wrong.contains("refused") && !wrong.contains("s3cr3t"),
            "{wrong}"
        );
        let version = call(&client, &endpoint, "common", "version", json!([]))
            .await
            .unwrap();
        assert_eq!(version["server_version"], "18.0");
        let uid = call(
            &client,
            &endpoint,
            "common",
            "authenticate",
            json!(["client-a", "ogh", "s3cr3t-key", {}]),
        )
        .await
        .unwrap();
        assert_eq!(uid, 7);
        let denied = call(
            &client,
            &endpoint,
            "object",
            "execute_kw",
            json!([
                "client-a",
                7,
                "s3cr3t-key",
                "res.lang",
                "search_read",
                [[]],
                {}
            ]),
        )
        .await
        .unwrap_err();
        assert!(
            denied.contains("Access Denied") && !denied.contains("s3cr3t"),
            "{denied}"
        );
        let refused = query(&client, &config(&["127.0.0.1"]), "client-a", &url)
            .await
            .unwrap_err();
        assert!(
            refused.contains("instance.hosts") && !refused.contains("s3cr3t"),
            "{refused}"
        );
    }
}
