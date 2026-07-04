// Copyright Alexandre D. Díaz
use std::collections::HashSet;

use actix_web::{get, web, HttpRequest, Responder, Result};
use diesel::sqlite::SqliteConnection;
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::{models, Pool};

use super::api::v1::module::{process_modules_db, ModuleFullInfoResponse};

// A migration PR/MR still open for this module: the module isn't merged yet for
// that Odoo version, but work towards it is already visible upstream.
#[derive(Debug, Deserialize, Serialize)]
pub struct ModulePullRequestInfo {
    pub title: String,
    pub prid: i64,
    pub odoo_version: String,
    pub repository: String,
    pub organization: String,
    pub url: String,
}

fn get_module_pull_requests(
    conn: &mut SqliteConnection,
    org: &str,
    module_technical_name: &str,
    merged_versions: &HashSet<i32>,
) -> Vec<ModulePullRequestInfo> {
    models::pull_request::get_by_technical_name_organization_name(conn, module_technical_name, org)
        .into_iter()
        .filter(|pr| !merged_versions.contains(&pr.version_odoo))
        .map(|pr| {
            let repo = models::gh_repository::get_by_id(conn, &pr.gh_repository_id).unwrap();
            let org_model =
                models::gh_organization::get_by_id(conn, &repo.gh_organization_id).unwrap();
            ModulePullRequestInfo {
                url: format!(
                    "https://github.com/{}/{}/pull/{}",
                    &org_model.name, &repo.name, pr.prid
                ),
                title: pr.name,
                prid: pr.prid,
                odoo_version: odoo_version_u8_to_string(&(pr.version_odoo as u8)),
                repository: repo.name,
                organization: org_model.name,
            }
        })
        .collect()
}

#[get("/module/{org}/{technical_name}")]
pub async fn route(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    pool: web::Data<Pool>,
    path: web::Path<(String, String)>,
) -> Result<impl Responder> {
    let (org, technical_name) = path.into_inner();
    let org_ctx = org.clone();
    let technical_name_ctx = technical_name.clone();
    let (module_infos, pull_requests) = web::block(move || {
        let mut conn = pool.get().unwrap();
        let modules = models::module::get_by_technical_name_organization_name(
            &mut conn,
            &technical_name,
            &org,
        );
        let merged_versions: HashSet<i32> = modules.iter().map(|m| m.version_odoo).collect();
        let module_infos: Vec<ModuleFullInfoResponse> = process_modules_db(&mut conn, &modules);
        let pull_requests =
            get_module_pull_requests(&mut conn, &org, &technical_name, &merged_versions);
        (module_infos, pull_requests)
    })
    .await?;

    tmpl_env.render(
        "pages/module.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "module",
                org => org_ctx,
                technical_name => technical_name_ctx,
                module_infos => module_infos,
                pull_requests => pull_requests,
            )
        ),
    )
}
