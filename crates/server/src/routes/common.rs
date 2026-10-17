// Copyright Alexandre D. Díaz
use actix_web::{get, web, Error as AWError, HttpResponse, Result};
use base64::Engine;
use diesel::sqlite::SqliteConnection;
use oghutils::version::odoo_version_u8_to_string;
use serde::{Deserialize, Serialize};

use sqlitedb::{models, Pool};

use crate::workspace::AreaPool;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OdooVersionInfo {
    pub key: i32,
    pub value: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleListInfo {
    pub versions: Vec<String>,
    pub technical_name: String,
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub org_name: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommitterListInfo {
    pub name: String,
    pub total_commits: i64,
    pub modules_touched: i64,
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
            name: x.name,
            description: x.description,
            category: x.category,
            org_name: x.org_name,
        })
        .collect()
}

fn get_odoo_committer_list(conn: &mut SqliteConnection) -> Vec<CommitterListInfo> {
    models::committer::list(conn)
        .into_iter()
        .map(|x| CommitterListInfo {
            name: x.name,
            total_commits: x.total_commits,
            modules_touched: x.modules_touched,
        })
        .collect()
}

#[get("/common/odoo/versions")]
pub async fn route_odoo_versions(pool: AreaPool) -> Result<HttpResponse, AWError> {
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_odoo_versions(&mut conn)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[get("/common/odoo/module/list")]
pub async fn route_odoo_module_list(pool: AreaPool) -> Result<HttpResponse, AWError> {
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_odoo_module_list(&mut conn)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

// Served as an actual image, not embedded as base64 in the bulk module list
// above - that list can hold thousands of entries, and only the handful of
// results rendered in the search dropdown ever need their icon loaded.
#[get("/common/odoo/module/{org_name}/{technical_name}/icon")]
pub async fn route_odoo_module_icon(
    path: web::Path<(String, String)>,
    pool: web::Data<Pool>,
) -> Result<HttpResponse, AWError> {
    let (org_name, technical_name) = path.into_inner();
    let icon_b64 = web::block(move || {
        let mut conn = pool.get().unwrap();
        models::module::get_icon(&mut conn, &org_name, &technical_name)
    })
    .await?;
    let icon_bytes =
        icon_b64.and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok());
    match icon_bytes {
        Some(bytes) => Ok(HttpResponse::Ok().content_type("image/png").body(bytes)),
        None => Ok(HttpResponse::NotFound().finish()),
    }
}

#[get("/common/odoo/committer/list")]
pub async fn route_odoo_committer_list(pool: AreaPool) -> Result<HttpResponse, AWError> {
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_odoo_committer_list(&mut conn)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}
