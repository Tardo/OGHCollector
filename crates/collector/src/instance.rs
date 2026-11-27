// Copyright Alexandre D. Díaz
//! The real instances of a Doodba project: a source's `doodba_project_scan: true`
//! (`collector.yaml`; the 8th job argument, `scan` or `-` = none) makes `doodba::collect` ask
//! each deployed environment (prod, demo) over Odoo's JSON-RPC (`/jsonrpc`): version, installed
//! modules with their versions, active internal users, active languages. That is what the
//! database says, not the repository. Everything but the credentials comes from the project's
//! own files (`doodba::parse_deploy`): the URLs from the copier answers' `domains_prod`/
//! `domains_test`, the database from `postgres_dbname` (none = the instance's database list must
//! have exactly one). Read-only calls only (`version`, `db.list`, `authenticate`, `search_read`,
//! `search_count`).
//! - `OGHCOLLECTOR_DOODBA_SCAN_LOGIN`: the Odoo user every instance has.
//! - `OGHCOLLECTOR_DOODBA_SCAN_PASSWORD`: a secret reference (`secrets::get`: env/Docker secret
//!   or `vault:<path>#<key>`, e.g. `vault:secret/data/{repo}#odoo_password`), `{repo}` replaced by
//!   the repository's name first so each project has its own. A password or an API key
//!   (Odoo >= 14) both work. Both are required when a source scans; `load_jobs` checks them.
//!
//! No host allowlist (a deliberate choice): the URL comes from a file anyone with push access to
//! the project repository writes, and that project's password goes to it. Only `https` URLs are
//! called and redirects are never followed. A failed query keeps the last good answer and sets
//! `error` (an outage never reads as "nothing installed"). An answer younger than `REUSE_FOR`
//! (any project, same URL: a project's branches share their instances) is reused, not asked
//! again. The password never reaches logs, errors or the DB: reqwest errors go without URL, and
//! Odoo's own error text is cut short.
use reqwest::Url;
use serde_json::{json, Value};
use std::collections::HashMap;

use sqlitedb::models::doodba_project::{Deploy, InstalledModule, Instance, Instances};

use crate::{gitclient, secrets};

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub login: String,
    pub password: String,
}

/// The 8th job argument of a source with `doodba_project_scan: true` / without it.
pub const SCAN: &str = "scan";
pub const NONE: &str = "-";

impl Config {
    /// The 8th job argument: `scan` = the credentials from the environment.
    pub fn parse(arg: &str) -> Result<Option<Config>, String> {
        match arg {
            NONE => Ok(None),
            SCAN => Config::from_env().map(Some),
            bad => Err(format!("doodba_project_scan: '{bad}' ({SCAN} or {NONE})")),
        }
    }

    pub fn from_env() -> Result<Config, String> {
        let var = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .ok_or(format!("{name} isn't set (doodba_project_scan)"))
        };
        let config = Config {
            login: var("OGHCOLLECTOR_DOODBA_SCAN_LOGIN")?,
            password: var("OGHCOLLECTOR_DOODBA_SCAN_PASSWORD")?,
        };
        oghworkflow::secret_ref(&fill(&config.password, "repo"))
            .map_err(|e| format!("OGHCOLLECTOR_DOODBA_SCAN_PASSWORD: {e}"))?;
        Ok(config)
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
    recent: &mut HashMap<String, Instance>,
) -> Instances {
    let client = reqwest_client();
    let mut ask = Asker {
        client: &client,
        config,
        repo,
        db: deploy.db.as_deref(),
        recent,
    };
    Instances {
        prod: ask.ask(&deploy.prod_url, deploy.prod, &stored.prod).await,
        demo: ask.ask(&deploy.demo_url, deploy.demo, &stored.demo).await,
    }
}

/// How long an instance's answer stands for every project naming its URL.
pub const REUSE_FOR: chrono::Duration = chrono::Duration::hours(1);

struct Asker<'a> {
    client: &'a reqwest::Client,
    config: &'a Config,
    repo: &'a str,
    db: Option<&'a str>,
    /// Answers newer than `REUSE_FOR` by URL (`doodba_project::recent_instances`, plus this job's).
    recent: &'a mut HashMap<String, Instance>,
}

impl Asker<'_> {
    async fn ask(
        &mut self,
        url: &Option<String>,
        on: bool,
        old: &Option<Instance>,
    ) -> Option<Instance> {
        let url = url.clone().filter(|_| on)?;
        if let Some(fresh) = self.recent.get(&url) {
            log::info!(
                "'{}': {url} answered at {:?}, not asked again",
                self.repo,
                fresh.checked_at
            );
            return Some(fresh.clone());
        }
        Some(
            match query(self.client, self.config, self.repo, self.db, &url).await {
                Ok(instance) => {
                    self.recent.insert(url, instance.clone());
                    instance
                }
                Err(err) => {
                    log::warn!("'{}': can't query {url}: {err}", self.repo);
                    Instance {
                        url,
                        error: Some(err),
                        ..old.clone().unwrap_or_default()
                    }
                }
            },
        )
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
    db: Option<&str>,
    url: &str,
) -> Result<Instance, String> {
    let base = Url::parse(url).map_err(|_| "not a URL".to_string())?;
    if base.scheme() != "https" {
        return Err(format!(
            "{} isn't an https URL",
            base.host_str().unwrap_or("?")
        ));
    }
    let endpoint = base.join("/jsonrpc").map_err(|e| e.to_string())?;
    fetch(client, config, repo, db, url, &endpoint).await
}

/// `query` past the host check (the tests' server isn't https).
async fn fetch(
    client: &reqwest::Client,
    config: &Config,
    repo: &str,
    db: Option<&str>,
    url: &str,
    endpoint: &Url,
) -> Result<Instance, String> {
    let key = secrets::get(&fill(&config.password, repo)).await?;
    let rpc = |service: &'static str, method: &'static str, args: Value| {
        call(client, endpoint, service, method, args)
    };
    let server_version = rpc("common", "version", json!([]))
        .await?
        .get("server_version")
        .and_then(Value::as_str)
        .map(str::to_string);
    let db = match db {
        Some(db) => db.to_string(),
        None => {
            let list = rpc("db", "list", json!([])).await.map_err(|e| {
                format!("{e} (no postgres_dbname in the copier answers to name it)")
            })?;
            match list.as_array().map(Vec::as_slice) {
                Some([only]) => only.as_str().unwrap_or_default().to_string(),
                _ => {
                    return Err(
                        "can't tell the database: no postgres_dbname in the copier answers".into(),
                    )
                }
            }
        }
    };
    let uid = rpc("common", "authenticate", json!([db, config.login, key, {}]))
        .await?
        .as_i64()
        .ok_or("login or password refused")?;
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

    fn config() -> Config {
        Config {
            login: "ogh".into(),
            password: "OGH_TEST_INSTANCE_KEY".into(),
        }
    }

    #[tokio::test]
    async fn a_fresh_answer_for_the_same_url_is_reused() {
        let url = "https://erp.invalid".to_string();
        let fresh = Instance {
            url: url.clone(),
            server_version: Some("18.0".into()),
            ..Default::default()
        };
        let mut recent = HashMap::from([(url.clone(), fresh.clone())]);
        let deploy = Deploy {
            prod: true,
            prod_url: Some(url),
            demo_url: Some("https://demo.invalid".into()),
            ..Default::default()
        };
        // Demo is off: not asked. Prod would fail (no such host): it isn't asked either.
        let got = refresh(
            &config(),
            "client-a",
            &deploy,
            &Instances::default(),
            &mut recent,
        )
        .await;
        assert_eq!(got.prod, Some(fresh));
        assert_eq!(got.demo, None);
    }

    #[test]
    fn the_scan_switch_takes_its_credentials_from_the_environment() {
        assert_eq!(Config::parse("-"), Ok(None));
        assert!(Config::parse("yes").is_err());
        // The only test that touches these env vars.
        std::env::remove_var("OGHCOLLECTOR_DOODBA_SCAN_LOGIN");
        assert!(Config::parse("scan").unwrap_err().contains("_LOGIN"));
        std::env::set_var("OGHCOLLECTOR_DOODBA_SCAN_LOGIN", "ogh");
        std::env::set_var("OGHCOLLECTOR_DOODBA_SCAN_PASSWORD", "GH_TOKEN");
        assert!(Config::parse("scan")
            .unwrap_err()
            .contains("collector's own"));
        let password = "vault:secret/data/{repo}#odoo_password";
        std::env::set_var("OGHCOLLECTOR_DOODBA_SCAN_PASSWORD", password);
        let c = Config::parse("scan").unwrap().unwrap();
        assert_eq!(c.login, "ogh");
        assert_eq!(
            fill(&c.password, "client-a"),
            "vault:secret/data/client-a#odoo_password"
        );
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
                    (Some("db"), Some("list")) => json!(["client-a"]),
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
        let got = fetch(&client, &config(), "client-a", None, &url, &endpoint)
            .await
            .unwrap();
        assert_eq!(got.db.as_deref(), Some("client-a"));
        assert_eq!(got.server_version.as_deref(), Some("18.0"));
        assert_eq!(got.users, Some(5));
        assert!(got.languages.is_empty(), "denied extras are skipped");
        assert_eq!(got.modules.len(), 2);
        assert_eq!(got.modules[1].version, None);
        assert_eq!(got.modules[1].state, "to upgrade");
        let wrong = fetch(
            &client,
            &config(),
            "client-a",
            Some("other"),
            &url,
            &endpoint,
        )
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
        let refused = query(&client, &config(), "client-a", None, &url)
            .await
            .unwrap_err();
        assert!(
            refused.contains("https") && !refused.contains("s3cr3t"),
            "{refused}"
        );
    }
}
