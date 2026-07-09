// Copyright Alexandre D. Díaz
use std::collections::HashMap;

use actix_web::{get, web, Error as AWError, HttpResponse};
use serde::{Deserialize, Serialize};

use diesel::sqlite::SqliteConnection;
use oghutils::version::{odoo_version_string_to_u8, odoo_version_u8_to_string};
use sqlitedb::{models, Pool};

#[derive(Debug, Deserialize, Serialize)]
pub struct SearchGenericInfoResponse {
    pub technical_name: String,
    pub versions: HashMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct RouteSearchRequest {
    odoo_version: Option<String>,
    installable: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct RouteSearchCriteriaRequest {
    odoo_version: String,
    term: Option<String>,
    category: Option<String>,
    depends_on: Option<String>,
    limit: Option<u32>,
}

fn build_response(
    modules: Vec<sqlitedb::models::module::ModuleGenericInfo>,
) -> Vec<SearchGenericInfoResponse> {
    let mut res = Vec::new();
    for module in modules {
        let src_versions: Vec<&str> = module.versions.split(',').collect();
        let mut srcs: HashMap<String, Vec<String>> = HashMap::new();
        let versions = srcs.entry(module.src).or_default();
        let mut frmt = src_versions
            .iter()
            .filter_map(|&x| x.trim().parse::<u8>().ok())
            .map(|v| odoo_version_u8_to_string(&v))
            .collect::<Vec<String>>();
        versions.append(&mut frmt);
        res.push(SearchGenericInfoResponse {
            technical_name: module.technical_name,
            versions: srcs,
        });
    }
    res
}

fn get_modules(conn: &mut SqliteConnection, module_name: &str) -> Vec<SearchGenericInfoResponse> {
    build_response(models::module::get_generic_info(conn, module_name))
}

fn get_modules_by_odoo_version_installable(
    conn: &mut SqliteConnection,
    module_name: &str,
    odoo_version: &u8,
    installable: &bool,
) -> Vec<SearchGenericInfoResponse> {
    build_response(
        models::module::get_generic_info_by_odoo_version_installable(
            conn,
            module_name,
            odoo_version,
            installable,
        ),
    )
}

fn get_modules_by_odoo_version(
    conn: &mut SqliteConnection,
    module_name: &str,
    odoo_version: &u8,
) -> Vec<SearchGenericInfoResponse> {
    build_response(models::module::get_generic_info_by_odoo_version(
        conn,
        module_name,
        odoo_version,
    ))
}

fn get_modules_by_installable(
    conn: &mut SqliteConnection,
    module_name: &str,
    installable: &bool,
) -> Vec<SearchGenericInfoResponse> {
    build_response(models::module::get_generic_info_by_installable(
        conn,
        module_name,
        installable,
    ))
}

/// Cross-repository discovery by free-text term, category and/or reverse
/// Odoo dependency ("which modules depend on X"); same query (and limit cap)
/// as the MCP `list_modules_by_criteria` tool.
#[get("/search")]
pub async fn route_criteria(
    pool: web::Data<Pool>,
    info: web::Query<RouteSearchCriteriaRequest>,
) -> Result<HttpResponse, AWError> {
    let params = info.into_inner();
    let version_odoo = odoo_version_string_to_u8(&params.odoo_version);
    let limit = params.limit.unwrap_or(50).min(200) as i64;
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        models::module::search_by_criteria(
            &mut conn,
            &version_odoo,
            params.term.as_deref(),
            params.category.as_deref(),
            params.depends_on.as_deref(),
            limit,
        )
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[get("/search/{module_name}")]
pub async fn route(
    pool: web::Data<Pool>,
    path: web::Path<String>,
    info: web::Query<RouteSearchRequest>,
) -> Result<HttpResponse, AWError> {
    let module_name = path.into_inner();
    if let (Some(version_odoo), Some(installable)) = (info.odoo_version.clone(), info.installable) {
        let result = web::block(move || {
            let mut conn = pool.get().unwrap();
            get_modules_by_odoo_version_installable(
                &mut conn,
                &module_name,
                &odoo_version_string_to_u8(&version_odoo),
                &installable,
            )
        })
        .await?;
        return Ok(HttpResponse::Ok().json(result));
    } else if info.odoo_version.is_some() {
        let version_odoo = info.odoo_version.clone().unwrap();
        let result = web::block(move || {
            let mut conn = pool.get().unwrap();
            get_modules_by_odoo_version(
                &mut conn,
                &module_name,
                &odoo_version_string_to_u8(&version_odoo),
            )
        })
        .await?;
        return Ok(HttpResponse::Ok().json(result));
    } else if let Some(installable) = info.installable {
        let result = web::block(move || {
            let mut conn = pool.get().unwrap();
            get_modules_by_installable(&mut conn, &module_name, &installable)
        })
        .await?;
        return Ok(HttpResponse::Ok().json(result));
    }
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_modules(&mut conn, &module_name)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}
