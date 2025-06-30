// Copyright 2025 Alexandre D. Díaz
use minijinja::context;
use actix_web::{web, get, HttpRequest, Responder, Result};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

use crate::utils::get_minijinja_context;
use crate::minijinja_renderer::MiniJinjaRenderer;

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::{Pool, models};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OSVInfo {
    pub osv_id: String,
    pub details: String,
    pub fixed_in: String,
}


#[get("/osv")]
pub async fn route(pool: web::Data<Pool>, tmpl_env: MiniJinjaRenderer, req: HttpRequest) -> Result<impl Responder> {
    let conn = web::block(move || pool.get())
        .await?.unwrap();
    let osv_infos = models::dependency_osv::get_osv_info(&conn);

    let mut res: HashMap<String, HashMap<String, HashMap<String, Vec<OSVInfo>>>> = HashMap::new();
    for osv_info in osv_infos {
        let version_odoo = odoo_version_u8_to_string(&osv_info.version_odoo);
        let module_name = format!("{} ({})", &osv_info.module_technical_name, &osv_info.module_name);
        let by_ver = res.entry(version_odoo).or_insert(HashMap::new());
        let by_mod = by_ver.entry(module_name).or_insert(HashMap::new());
        let by_pack = by_mod.entry(osv_info.name).or_insert(Vec::new());
        by_pack.push(OSVInfo {
            osv_id: osv_info.osv_id,
            details: osv_info.details,
            fixed_in: osv_info.fixed_in
        });
    }

    return tmpl_env.render("pages/osv.html", context!(
        ..get_minijinja_context(&req),
        ..context!(
            page_name => "osv",
            osv_info => res,
        )
    ))
}
