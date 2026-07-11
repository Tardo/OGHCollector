// Copyright Alexandre D. Díaz
use std::collections::{HashMap, HashSet};

use actix_web::{get, web, HttpRequest, HttpResponse, Responder, Result};
use diesel::sqlite::SqliteConnection;
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use oghutils::version::{odoo_version_string_to_u8, odoo_version_u8_to_string};
use sqlitedb::{models, Pool};

use super::api::v1::module::{process_modules_db, ModuleFullInfoResponse};

#[derive(Debug, Deserialize)]
pub struct RouteModulePageRequest {
    version: Option<String>,
}

// A version_module ever seen for a module, for the version-history dropdown
// on the module detail page. Fetched eagerly (not via a client-side request)
// since the page is server-rendered and the history is small per module.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleVersionHistoryInfo {
    pub version_module: String,
    pub create_date: String,
    pub is_latest: bool,
}

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
    pub age_days: Option<i64>,
    pub last_message_days: Option<i64>,
    pub ci_status: Option<String>,
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
                    org_model.name, repo.name, pr.prid
                ),
                age_days: models::pull_request::days_since(pr.created_at.as_deref()),
                last_message_days: models::pull_request::days_since(pr.last_message_at.as_deref()),
                ci_status: pr.ci_status,
                title: pr.name,
                prid: pr.prid,
                odoo_version: odoo_version_u8_to_string(&(pr.version_odoo as u8)),
                repository: repo.name,
                organization: org_model.name,
            }
        })
        .collect()
}

// Keyed by Odoo version (e.g. "17.0") for the template's per-tab
// version-history dropdown; shared by `route` (active tab, eager) and
// `route_tab` (one lazily-loaded tab).
fn build_module_context(
    conn: &mut SqliteConnection,
    modules: &[models::module::Model],
    version_module: Option<&str>,
) -> (
    Vec<ModuleFullInfoResponse>,
    HashMap<String, Vec<ModuleVersionHistoryInfo>>,
) {
    let module_versions: HashMap<String, Vec<ModuleVersionHistoryInfo>> = modules
        .iter()
        .map(|m| {
            let odoo_ver = odoo_version_u8_to_string(&(m.version_odoo as u8));
            let mut history: Vec<ModuleVersionHistoryInfo> =
                models::module_version::get_by_module_id(conn, &m.id)
                    .into_iter()
                    .map(|v| ModuleVersionHistoryInfo {
                        is_latest: v.version_module == m.version_module,
                        version_module: v.version_module,
                        create_date: v.create_date,
                    })
                    .collect();
            history.reverse(); // newest first
            (odoo_ver, history)
        })
        .collect();
    let module_infos = process_modules_db(conn, modules, version_module);
    (module_infos, module_versions)
}

#[get("/module/{org}/{technical_name}")]
pub async fn route(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    pool: web::Data<Pool>,
    path: web::Path<(String, String)>,
    info: web::Query<RouteModulePageRequest>,
) -> Result<impl Responder> {
    let (org, technical_name) = path.into_inner();
    let org_ctx = org.clone();
    let technical_name_ctx = technical_name.clone();
    let version_module = info.version.clone();
    let (module_infos, module_versions, pull_requests, odoo_versions) = web::block(move || {
        let mut conn = pool.get().unwrap();
        let modules = models::module::get_by_technical_name_organization_name(
            &mut conn,
            &technical_name,
            &org,
        );
        let merged_versions: HashSet<i32> = modules.iter().map(|m| m.version_odoo).collect();

        let mut distinct_versions: Vec<i32> = merged_versions.iter().copied().collect();
        distinct_versions.sort_unstable();
        let odoo_versions: Vec<String> = distinct_versions
            .iter()
            .rev() // newest first, matches the tab order
            .map(|v| odoo_version_u8_to_string(&(*v as u8)))
            .collect();

        // Only the newest Odoo version is rendered eagerly here - the rest
        // are fetched lazily by the client on first tab activation (see
        // route_tab below), to avoid computing dependency trees, code
        // analysis and required_by (up to 500 rows) for versions nobody
        // may ever look at.
        let active_modules: Vec<models::module::Model> = match distinct_versions.last() {
            Some(&newest) => modules
                .iter()
                .filter(|m| m.version_odoo == newest)
                .cloned()
                .collect(),
            None => Vec::new(),
        };
        let (module_infos, module_versions) =
            build_module_context(&mut conn, &active_modules, version_module.as_deref());
        let pull_requests =
            get_module_pull_requests(&mut conn, &org, &technical_name, &merged_versions);
        (module_infos, module_versions, pull_requests, odoo_versions)
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
                module_versions => module_versions,
                pull_requests => pull_requests,
                odoo_versions => odoo_versions,
            )
        ),
    )
}

// Renders the content of a single Odoo-version tab (`partials/module_version_content.html`)
// for `module.mjs` to inject on demand when that tab is first shown - see
// the comment on `route` above for why this isn't computed eagerly for
// every version.
#[get("/module/{org}/{technical_name}/tab/{odoo_version}")]
pub async fn route_tab(
    tmpl_env: MiniJinjaRenderer,
    pool: web::Data<Pool>,
    path: web::Path<(String, String, String)>,
    info: web::Query<RouteModulePageRequest>,
) -> Result<HttpResponse> {
    let (org, technical_name, odoo_version) = path.into_inner();
    let version_odoo = odoo_version_string_to_u8(&odoo_version);
    let version_module = info.version.clone();
    let (module_infos, module_versions) = web::block(move || {
        let mut conn = pool.get().unwrap();
        let modules = models::module::get_by_technical_name_odoo_version_organization_name(
            &mut conn,
            &technical_name,
            &version_odoo,
            &org,
        );
        build_module_context(&mut conn, &modules, version_module.as_deref())
    })
    .await?;

    let Some(module) = module_infos.into_iter().next() else {
        return Ok(HttpResponse::NotFound().finish());
    };
    let history = module_versions
        .get(&module.odoo_version)
        .cloned()
        .unwrap_or_default();
    let html = tmpl_env.render(
        "partials/module_version_content.html",
        context!(module => module, history => history),
    )?;
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html.0))
}
