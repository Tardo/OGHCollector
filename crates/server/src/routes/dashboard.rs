// Copyright Alexandre D. Díaz
use actix_web::{get, web, HttpRequest, Responder, Result};
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::{models, Pool};

#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleCountInfoResponse {
    pub count: i64,
    pub version_odoo: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LastestCreatedInfo {
    pub id: i64,
    pub version: String,
    pub technical_name: String,
    pub org_name: String,
    pub create_date: String,
}

#[get("/")]
pub async fn route(
    pool: web::Data<Pool>,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    let (modules_count, modules_latest, modules_total, org_total) = web::block(move || {
        let mut conn = pool.get().unwrap();
        let count = models::module::count(&mut conn)
            .into_iter()
            .map(|x| ModuleCountInfoResponse {
                count: x.count,
                version_odoo: odoo_version_u8_to_string(&(x.version_odoo as u8)),
            })
            .collect::<Vec<ModuleCountInfoResponse>>();
        let latest = models::module::get_latest_modules_created(&mut conn)
            .into_iter()
            .map(|x| LastestCreatedInfo {
                id: x.id,
                version: odoo_version_u8_to_string(&(x.version_odoo as u8)),
                technical_name: x.technical_name,
                org_name: x.org_name,
                create_date: x.create_date,
            })
            .collect::<Vec<LastestCreatedInfo>>();
        let modules_total = models::module::count_distinct(&mut conn);
        let org_total = models::gh_organization::count(&mut conn);
        (count, latest, modules_total, org_total)
    })
    .await?;

    let version_total = modules_count.len();

    tmpl_env.render(
        "pages/dashboard.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "dashboard",
                modules_count => modules_count,
                modules_latest => modules_latest,
                modules_total => modules_total,
                org_total => org_total,
                version_total => version_total,
            )
        ),
    )
}
