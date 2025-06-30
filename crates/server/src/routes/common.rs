// Copyright 2025 Alexandre D. Díaz
use actix_web::{web, get, post, HttpResponse, Result, Error as AWError};
use oghutils::version::odoo_version_u8_to_string;
use serde::{Deserialize, Serialize};

use sqlitedb::{Pool, models::{self, Connection, module::ModuleRepositoryInfo}};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OdooVersionInfo {
    pub key: u8,
    pub value: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleCountInfo {
    pub version: String,
    pub count: u32,
    pub org: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ContribRankInfo {
    pub version: String,
    pub count: u32,
    pub contrib: String,
    pub rank: u16,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommitterRankInfo {
    pub version: String,
    pub count: u32,
    pub committer: String,
    pub rank: u16,
}

#[derive(Deserialize)]
struct JSONRequestModuleDoodba {
    modules: Vec<String>,
}


fn get_odoo_versions(conn: &Connection) -> Vec<OdooVersionInfo> {
    let versions: Vec<OdooVersionInfo> = models::module::get_odoo_versions(&conn).iter().map(|x| OdooVersionInfo { key: x.clone(), value: odoo_version_u8_to_string(&x) }).collect();
    versions
}

fn get_odoo_module_count(conn: &Connection) -> Vec<ModuleCountInfo> {
    let modules_count: Vec<ModuleCountInfo> = models::module::count_organization(&conn).iter().map(|x| ModuleCountInfo { version: odoo_version_u8_to_string(&x.version_odoo), count: x.count.clone(), org: x.org_name.to_string() }).collect();
    modules_count
}

fn get_odoo_contributor_rank(conn: &Connection) -> Vec<ContribRankInfo> {
    let contrib_rank: Vec<ContribRankInfo> = models::module::rank_contributor(&conn).iter().map(|x| ContribRankInfo { version: odoo_version_u8_to_string(&x.version_odoo), count: x.count.clone(), contrib: x.contrib_name.to_string(), rank: x.rank.clone() }).collect();
    contrib_rank
}

fn get_odoo_committer_rank(conn: &Connection) -> Vec<CommitterRankInfo> {
    let committer_rank: Vec<CommitterRankInfo> = models::module::rank_committer(&conn).iter().map(|x| CommitterRankInfo { version: odoo_version_u8_to_string(&x.version_odoo), count: x.count.clone(), committer: x.committer_name.to_string(), rank: x.rank.clone() }).collect();
    committer_rank
}

fn get_doodba_addons(conn: &Connection, mods: &Vec<String>) -> Vec<ModuleRepositoryInfo> {
    let module_repos: Vec<ModuleRepositoryInfo> = models::module::get_module_repository(&conn, &mods);
    log::info!("{:?}", &module_repos);
    module_repos
}

#[get("/common/odoo/versions")]
pub async fn route_odoo_versions(pool: web::Data<Pool>) -> Result<HttpResponse, AWError> {
    let conn = web::block(move || pool.get())
        .await?.unwrap();
    let result = web::block(move || {
        get_odoo_versions(&conn)
    }).await?;
    return Ok(HttpResponse::Ok().json(result));
}

#[get("/common/odoo/module/count")]
pub async fn route_odoo_module_count(pool: web::Data<Pool>) -> Result<HttpResponse, AWError> {
    let conn = web::block(move || pool.get())
        .await?.unwrap();
    let result = web::block(move || {
        get_odoo_module_count(&conn)
    }).await?;
    return Ok(HttpResponse::Ok().json(result));
}

#[get("/common/odoo/contributor/rank")]
pub async fn route_odoo_contributor_rank(pool: web::Data<Pool>) -> Result<HttpResponse, AWError> {
    let conn = web::block(move || pool.get())
        .await?.unwrap();
    let result = web::block(move || {
        get_odoo_contributor_rank(&conn)
    }).await?;
    return Ok(HttpResponse::Ok().json(result));
}

#[get("/common/odoo/committer/rank")]
pub async fn route_odoo_committer_rank(pool: web::Data<Pool>) -> Result<HttpResponse, AWError> {
    let conn = web::block(move || pool.get())
        .await?.unwrap();
    let result = web::block(move || {
        get_odoo_committer_rank(&conn)
    }).await?;
    return Ok(HttpResponse::Ok().json(result));
}

#[post("/common/doodba/addons")]
pub async fn route_doodba_addons(pool: web::Data<Pool>, info: web::Json<JSONRequestModuleDoodba>) -> Result<HttpResponse, AWError> {
    let conn = web::block(move || pool.get())
        .await?.unwrap();
    let result = web::block(move || {
        get_doodba_addons(&conn, &info.modules)
    }).await?;
    return Ok(HttpResponse::Ok().json(result));
}