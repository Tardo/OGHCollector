// Copyright  Alexandre D. Díaz
use actix_web::{get, web, HttpRequest, Responder, Result};
use minijinja::context;

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use sqlitedb::{
    models::{self, Connection},
    Pool,
};

use super::api::v1::module::{process_modules_db, ModuleFullInfoResponse};

fn get_module_infos(
    conn: &Connection,
    org: String,
    module_technical_name: String,
) -> Vec<ModuleFullInfoResponse> {
    let modules =
        models::module::get_by_technical_name_organization_name(conn, &module_technical_name, &org);
    process_modules_db(conn, &modules)
}

#[get("/module/{org}/{technical_name}")]
pub async fn route(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    pool: web::Data<Pool>,
    path: web::Path<(String, String)>,
) -> Result<impl Responder> {
    let conn = web::block(move || pool.get()).await?.unwrap();
    let (org, technical_name) = path.into_inner();
    tmpl_env.render(
        "pages/module.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "module",
                module_infos => get_module_infos(&conn, org, technical_name),
            )
        ),
    )
}
