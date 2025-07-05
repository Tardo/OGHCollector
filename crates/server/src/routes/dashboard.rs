// Copyright 2025 Alexandre D. Díaz
use actix_web::{get, web, HttpRequest, Responder, Result};
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::{models, Pool};

#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleCountInfoResponse {
    pub count: u32,
    pub version_odoo: String,
}

#[get("/")]
pub async fn route(
    pool: web::Data<Pool>,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    let conn = web::block(move || pool.get()).await?.unwrap();

    let modules_count = models::module::count(&conn)
        .iter()
        .map(|x| ModuleCountInfoResponse {
            count: x.count,
            version_odoo: odoo_version_u8_to_string(&x.version_odoo),
        })
        .collect::<Vec<ModuleCountInfoResponse>>();
    tmpl_env.render(
        "pages/dashboard.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "dashboard",
                modules_count => modules_count,
            )
        ),
    )
}
