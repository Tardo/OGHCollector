// Copyright Alexandre D. Díaz
use actix_web::{get, web, Error as AWError, HttpResponse, Result};
use diesel::sqlite::SqliteConnection;
use oghutils::version::odoo_version_u8_to_string;
use serde::{Deserialize, Serialize};

use sqlitedb::{models, Pool};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OdooVersionInfo {
    pub key: i32,
    pub value: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleCountInfo {
    pub version: String,
    pub count: i64,
    pub org: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleListInfo {
    pub versions: Vec<String>,
    pub technical_name: String,
    pub org_name: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ContribRankInfo {
    pub version: String,
    pub count: i64,
    pub contrib: String,
    pub rank: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommitterRankInfo {
    pub version: String,
    pub count: i64,
    pub committer: String,
    pub rank: i64,
}

fn get_odoo_versions(conn: &mut SqliteConnection) -> Vec<OdooVersionInfo> {
    models::module::get_odoo_versions(conn)
        .into_iter()
        .map(|x| OdooVersionInfo {
            key: x,
            value: odoo_version_u8_to_string(&(x as u8)),
        })
        .collect()
}

fn get_odoo_module_count(conn: &mut SqliteConnection) -> Vec<ModuleCountInfo> {
    models::module::count_organization(conn)
        .into_iter()
        .map(|x| ModuleCountInfo {
            version: odoo_version_u8_to_string(&(x.version_odoo as u8)),
            count: x.count,
            org: x.org_name,
        })
        .collect()
}

fn get_odoo_module_list(conn: &mut SqliteConnection) -> Vec<ModuleListInfo> {
    models::module::list(conn)
        .into_iter()
        .map(|x| ModuleListInfo {
            versions: x
                .versions_odoo
                .iter()
                .map(|v| odoo_version_u8_to_string(&(*v as u8)))
                .collect(),
            technical_name: x.technical_name,
            org_name: x.org_name,
        })
        .collect()
}

fn get_odoo_contributor_rank(conn: &mut SqliteConnection) -> Vec<ContribRankInfo> {
    models::module::rank_contributor(conn)
        .into_iter()
        .map(|x| ContribRankInfo {
            version: odoo_version_u8_to_string(&(x.version_odoo as u8)),
            count: x.count,
            contrib: x.contrib_name,
            rank: x.rank,
        })
        .collect()
}

fn get_odoo_committer_rank(conn: &mut SqliteConnection) -> Vec<CommitterRankInfo> {
    models::module::rank_committer(conn)
        .into_iter()
        .map(|x| CommitterRankInfo {
            version: odoo_version_u8_to_string(&(x.version_odoo as u8)),
            count: x.count,
            committer: x.committer_name,
            rank: x.rank,
        })
        .collect()
}

#[get("/common/odoo/versions")]
pub async fn route_odoo_versions(pool: web::Data<Pool>) -> Result<HttpResponse, AWError> {
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_odoo_versions(&mut conn)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[get("/common/odoo/module/list")]
pub async fn route_odoo_module_list(pool: web::Data<Pool>) -> Result<HttpResponse, AWError> {
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_odoo_module_list(&mut conn)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[get("/common/odoo/module/count")]
pub async fn route_odoo_module_count(pool: web::Data<Pool>) -> Result<HttpResponse, AWError> {
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_odoo_module_count(&mut conn)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[get("/common/odoo/contributor/rank")]
pub async fn route_odoo_contributor_rank(pool: web::Data<Pool>) -> Result<HttpResponse, AWError> {
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_odoo_contributor_rank(&mut conn)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[get("/common/odoo/committer/rank")]
pub async fn route_odoo_committer_rank(pool: web::Data<Pool>) -> Result<HttpResponse, AWError> {
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_odoo_committer_rank(&mut conn)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}
