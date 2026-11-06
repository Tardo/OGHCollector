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

/// The visitor's workspace, if the cookie names one that exists.
pub fn current(req: &HttpRequest) -> Option<String> {
    let ws = req.cookie(COOKIE)?.value().to_string();
    let workspaces = req.app_data::<web::Data<Workspaces>>()?;
    workspaces.names().contains(&ws).then_some(ws)
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
