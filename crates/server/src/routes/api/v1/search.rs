// Copyright 2025 Alexandre D. Díaz
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use actix_web::{web, get, Error as AWError, HttpResponse};

use sqlitedb::{Pool, models::{self, Connection}};
use oghutils::version::{odoo_version_u8_to_string, odoo_version_string_to_u8};

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


fn get_modules(conn: &Connection, module_name: &str) -> Vec<SearchGenericInfoResponse> {
    let modules = models::module::get_generic_info(&conn, &module_name);
    let mut res: Vec<SearchGenericInfoResponse> = Vec::new();
    for module in modules {
        let src_versions = module.versions.split(",").collect::<Vec<&str>>();
        let mut srcs: HashMap<String, Vec<String>> = HashMap::new();
        let versions = srcs.entry(module.src).or_insert(Vec::new());
        let mut src_versions_frmt = src_versions.iter().map(|&x| odoo_version_u8_to_string(&x.parse::<u8>().unwrap())).collect::<Vec<String>>();
        versions.append(&mut src_versions_frmt);
        res.push(SearchGenericInfoResponse {
            technical_name: module.technical_name.to_string().clone(),
            versions: srcs,
        })
    }
    res
}

fn get_modules_by_odoo_version_installable(conn: &Connection, module_name: &str, odoo_version: &u8, installable: &bool) -> Vec<SearchGenericInfoResponse> {
    let modules = models::module::get_generic_info_by_odoo_version_installable(&conn, &module_name, &odoo_version, &installable);
    let mut res: Vec<SearchGenericInfoResponse> = Vec::new();
    for module in modules {
        let src_versions = module.versions.split(",").collect::<Vec<&str>>();
        let mut srcs: HashMap<String, Vec<String>> = HashMap::new();
        let versions = srcs.entry(module.src).or_insert(Vec::new());
        let mut src_versions_frmt = src_versions.iter().map(|&x| odoo_version_u8_to_string(&x.parse::<u8>().unwrap())).collect::<Vec<String>>();
        versions.append(&mut src_versions_frmt);
        res.push(SearchGenericInfoResponse {
            technical_name: module.technical_name.to_string().clone(),
            versions: srcs,
        })
    }
    res
}

fn get_modules_by_odoo_version(conn: &Connection, module_name: &str, odoo_version: &u8) -> Vec<SearchGenericInfoResponse> {
    let modules = models::module::get_generic_info_by_odoo_version(&conn, &module_name, &odoo_version);
    let mut res: Vec<SearchGenericInfoResponse> = Vec::new();
    for module in modules {
        let src_versions = module.versions.split(",").collect::<Vec<&str>>();
        let mut srcs: HashMap<String, Vec<String>> = HashMap::new();
        let versions = srcs.entry(module.src).or_insert(Vec::new());
        let mut src_versions_frmt = src_versions.iter().map(|&x| odoo_version_u8_to_string(&x.parse::<u8>().unwrap())).collect::<Vec<String>>();
        versions.append(&mut src_versions_frmt);
        res.push(SearchGenericInfoResponse {
            technical_name: module.technical_name.to_string().clone(),
            versions: srcs,
        })
    }
    res
}

fn get_modules_by_installable(conn: &Connection, module_name: &str, installable: &bool) -> Vec<SearchGenericInfoResponse> {
    let modules = models::module::get_generic_info_by_installable(&conn, &module_name, &installable);
    let mut res: Vec<SearchGenericInfoResponse> = Vec::new();
    for module in modules {
        let src_versions = module.versions.split(",").collect::<Vec<&str>>();
        let mut srcs: HashMap<String, Vec<String>> = HashMap::new();
        let versions = srcs.entry(module.src).or_insert(Vec::new());
        let mut src_versions_frmt = src_versions.iter().map(|&x| odoo_version_u8_to_string(&x.parse::<u8>().unwrap())).collect::<Vec<String>>();
        versions.append(&mut src_versions_frmt);
        res.push(SearchGenericInfoResponse {
            technical_name: module.technical_name.to_string().clone(),
            versions: srcs,
        })
    }
    res
}

#[get("/search/{module_name}")]
pub async fn route(pool: web::Data<Pool>, path: web::Path<String>, info: web::Query<RouteSearchRequest>) -> Result<HttpResponse, AWError> {
    let conn = web::block(move || pool.get())
        .await?.unwrap();
    let module_name = path.into_inner();
    if info.odoo_version.is_some() && info.installable.is_some() {
        let version_odoo = info.odoo_version.clone().unwrap();
        let installable = info.installable.clone().unwrap();
        let result = web::block(move || {
            get_modules_by_odoo_version_installable(&conn, &module_name, &odoo_version_string_to_u8(&version_odoo), &installable)
        }).await?;
        return Ok(HttpResponse::Ok().json(result));
    } else if info.odoo_version.is_some() {
        let version_odoo = info.odoo_version.clone().unwrap();
        let result = web::block(move || {
            get_modules_by_odoo_version(&conn, &module_name, &odoo_version_string_to_u8(&version_odoo))
        }).await?;
        return Ok(HttpResponse::Ok().json(result));
    } else if info.installable.is_some() {
        let installable = info.installable.clone().unwrap();
        let result = web::block(move || {
            get_modules_by_installable(&conn, &module_name, &installable)
        }).await?;
        return Ok(HttpResponse::Ok().json(result));
    }
    let result = web::block(move || {
        get_modules(&conn, &module_name)
    }).await?;
    return Ok(HttpResponse::Ok().json(result));
}
