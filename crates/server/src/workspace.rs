// Copyright Alexandre D. Díaz
//! Portal workspaces (collector.yaml `workspaces:`). The `workspace` cookie,
//! set by the top-bar selector, picks a read pool that only sees that
//! workspace's repositories/modules (`sqlitedb::new_workspace_read_pool`).
//! Listing pages (dashboard, modules, search, atlas, committers, OSV...) take
//! `AreaPool`; module pages, dependency closures, tools, feeds and badges keep
//! `web::Data<Pool>` (everything); `OwnAreaPool` (overview, modules, migration) drops the
//! shared repositories too, leaving only the workspace's own, so a dependency on a module of another
//! workspace still resolves and its page still opens.
use actix_web::{dev::Payload, error::ErrorInternalServerError, web, FromRequest, HttpRequest};
use cached::{proc_macro::cached, stores::TimedCache};
use std::collections::HashMap;
use std::future::{ready, Ready};
use std::ops::Deref;
use std::sync::Mutex;

use crate::config::SERVER_CONFIG;
use sqlitedb::{models, Pool};

pub const COOKIE: &str = "workspace";

pub struct Workspaces {
    db_path: String,
    global: Pool,
    pools: Mutex<HashMap<(String, bool), Pool>>,
}

impl Workspaces {
    pub fn new(db_path: String, global: Pool) -> Self {
        Workspaces {
            db_path,
            global,
            pools: Mutex::new(HashMap::new()),
        }
    }

    pub fn names(&self) -> Vec<String> {
        names(&self.global)
    }

    /// Every (shared, own-only) pool pair a visitor can get, the whole-DB one first: the cache
    /// warm-up (`warmup.rs`) fills what their requests would, under the same keys.
    pub fn all_areas(&self) -> Vec<(AreaPool, AreaPool)> {
        std::iter::once(None)
            .chain(self.names().into_iter().map(Some))
            .map(|ws| {
                let ws = ws.as_deref();
                let key = ws.unwrap_or_default().to_string();
                let own_key = ws.map_or(String::new(), |w| format!("{w}\0own"));
                (
                    AreaPool {
                        pool: self.pool(ws, false),
                        key,
                    },
                    AreaPool {
                        pool: self.pool(ws, true),
                        key: own_key,
                    },
                )
            })
            .collect()
    }

    fn pool(&self, ws: Option<&str>, own: bool) -> Pool {
        let Some(ws) = ws else {
            return self.global.clone();
        };
        self.pools
            .lock()
            .unwrap()
            .entry((ws.to_string(), own))
            .or_insert_with(|| {
                sqlitedb::new_workspace_read_pool(
                    &self.db_path,
                    *SERVER_CONFIG.get_db_pool_max_size(),
                    ws,
                    own,
                )
            })
            .clone()
    }
}

// New workspaces show up after `cache_ttl`, like the rest of the cached pages.
#[cached(
    ty = "TimedCache<u8, Vec<String>>",
    key = "u8",
    create = r#"{ TimedCache::with_lifespan(std::time::Duration::from_secs(*SERVER_CONFIG.get_cache_ttl())) }"#,
    convert = r#"{ 0u8 }"#
)]
fn names(pool: &Pool) -> Vec<String> {
    models::gh_repository_workspace::names(&mut pool.get().unwrap())
}

/// Workspaces a user in `groups_header` may use: those of its groups plus `default`.
fn allowed_for(
    groups_header: &str,
    map: &HashMap<String, Vec<String>>,
    default: &str,
) -> Vec<String> {
    let mut out = vec![default.to_string()];
    for g in groups_header.split(',') {
        let g = g.trim().trim_start_matches('/').to_lowercase();
        out.extend(map.get(&g).into_iter().flatten().cloned());
    }
    out.sort();
    out.dedup();
    out
}

/// `None` = auth off (everything allowed).
fn allowed(req: &HttpRequest) -> Option<Vec<String>> {
    if !SERVER_CONFIG.get_auth_enabled() {
        return None;
    }
    let groups = req
        .headers()
        .get(SERVER_CONFIG.get_auth_groups_header())
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    Some(allowed_for(
        groups,
        SERVER_CONFIG.get_auth_groups(),
        SERVER_CONFIG.get_default_workspace(),
    ))
}

/// Workspaces the top-bar selector may offer.
pub fn visible_names(req: &HttpRequest) -> Vec<String> {
    let names = req
        .app_data::<web::Data<Workspaces>>()
        .map(|w| w.names())
        .unwrap_or_default();
    match allowed(req) {
        Some(a) => names.into_iter().filter(|n| a.contains(n)).collect(),
        None => names,
    }
}

/// The visitor's workspace, if the cookie names one that exists.
pub fn current(req: &HttpRequest) -> Option<String> {
    let cookie = req.cookie(COOKIE).map(|c| c.value().to_string());
    let workspaces = req.app_data::<web::Data<Workspaces>>()?;
    let cookie = cookie.filter(|ws| workspaces.names().contains(ws));
    match allowed(req) {
        None => cookie,
        Some(a) => Some(
            cookie
                .filter(|ws| a.contains(ws))
                .unwrap_or_else(|| SERVER_CONFIG.get_default_workspace().to_string()),
        ),
    }
}

/// Read pool of the visitor's workspace (every repository without one).
pub struct AreaPool {
    pool: Pool,
    key: String,
}

impl AreaPool {
    /// For cache keys of data read through this pool.
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl Deref for AreaPool {
    type Target = Pool;
    fn deref(&self) -> &Pool {
        &self.pool
    }
}

impl FromRequest for AreaPool {
    type Error = actix_web::Error;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        // Same as a missing `web::Data<Pool>`: a 500, not a panic.
        let Some(workspaces) = req.app_data::<web::Data<Workspaces>>() else {
            return ready(Err(ErrorInternalServerError("Workspaces not configured")));
        };
        let workspace = current(req);
        ready(Ok(AreaPool {
            pool: workspaces.pool(workspace.as_deref(), false),
            key: workspace.unwrap_or_default(),
        }))
    }
}

/// Like `AreaPool`, but inside a workspace only its own repositories, not the
/// shared ones (modules and migration: "what is mine"). No workspace = all.
pub struct OwnAreaPool(AreaPool);

impl Deref for OwnAreaPool {
    type Target = AreaPool;
    fn deref(&self) -> &AreaPool {
        &self.0
    }
}

impl FromRequest for OwnAreaPool {
    type Error = actix_web::Error;
    type Future = Ready<Result<Self, Self::Error>>;

    fn from_request(req: &HttpRequest, _: &mut Payload) -> Self::Future {
        let Some(workspaces) = req.app_data::<web::Data<Workspaces>>() else {
            return ready(Err(ErrorInternalServerError("Workspaces not configured")));
        };
        let workspace = current(req);
        // Own-only views differ from the shared ones: distinct cache key.
        let key = workspace
            .as_ref()
            .map_or(String::new(), |w| format!("{w}\0own"));
        ready(Ok(OwnAreaPool(AreaPool {
            pool: workspaces.pool(workspace.as_deref(), true),
            key,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::allowed_for;
    use std::collections::HashMap;

    #[test]
    fn groups_map_to_workspaces_plus_default() {
        let map = HashMap::from([
            ("oca".to_string(), vec!["oca".to_string()]),
            (
                "staff".to_string(),
                vec!["interno".to_string(), "oca".to_string()],
            ),
        ]);
        assert_eq!(allowed_for("", &map, "default"), ["default"]);
        assert_eq!(
            allowed_for("/Staff, /nope", &map, "default"),
            ["default", "interno", "oca"]
        );
        assert_eq!(allowed_for("oca", &map, "default"), ["default", "oca"]);
    }
}
