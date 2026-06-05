// Copyright Alexandre D. Díaz
use actix_web::{get, web, HttpRequest, Responder, Result};
use diesel::sqlite::SqliteConnection;
use minijinja::context;

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use sqlitedb::{models, Pool};

use super::api::v1::module::{process_modules_db, ModuleFullInfoResponse};

fn get_module_infos(
    conn: &mut SqliteConnection,
    org: &str,
    module_technical_name: &str,
) -> Vec<ModuleFullInfoResponse> {
    let modules =
        models::module::get_by_technical_name_organization_name(conn, module_technical_name, org);
    process_modules_db(conn, &modules)
}

#[get("/module/{org}/{technical_name}")]
pub async fn route(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    pool: web::Data<Pool>,
    path: web::Path<(String, String)>,
) -> Result<impl Responder> {
    let (org, technical_name) = path.into_inner();
    let module_infos = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_module_infos(&mut conn, &org, &technical_name)
    })
    .await?;

    tmpl_env.render(
        "pages/module.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "module",
                module_infos => module_infos,
            )
        ),
    )
}
