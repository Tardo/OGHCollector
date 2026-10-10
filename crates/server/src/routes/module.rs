// Copyright Alexandre D. Díaz
use std::collections::{HashMap, HashSet};

use actix_web::{get, web, HttpRequest, HttpResponse, Responder, Result};
use diesel::sqlite::SqliteConnection;
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use oghutils::version::{odoo_version_string_to_u8, odoo_version_u8_to_string, parse_odoo_version};
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
    req: HttpRequest,
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
        context!(
            ..get_minijinja_context(&req),
            ..context!(module => module, history => history)
        ),
    )?;
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html.0))
}

#[derive(Debug, Deserialize)]
pub struct RouteModuleCompareRequest {
    from: Option<String>,
    to: Option<String>,
}

// The two versions to compare: what was asked for when it parses and the
// module has it, otherwise the module's two newest versions (oldest first).
fn pick_compare_versions(
    available_desc: &[i32],
    from: Option<&str>,
    to: Option<&str>,
) -> Option<(i32, i32)> {
    let pick = |raw: Option<&str>, fallback: Option<&i32>| {
        raw.and_then(parse_odoo_version)
            .map(|v| v as i32)
            .filter(|v| available_desc.contains(v))
            .or_else(|| fallback.copied())
    };
    let to = pick(to, available_desc.first())?;
    let from = pick(
        from,
        available_desc
            .iter()
            .find(|v| **v < to)
            .or(available_desc.get(1)),
    )?;
    Some((from, to))
}

/// Side-by-side "what changed" between two Odoo versions of one module.
#[get("/module/{org}/{technical_name}/compare")]
pub async fn route_compare(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    pool: web::Data<Pool>,
    path: web::Path<(String, String)>,
    info: web::Query<RouteModuleCompareRequest>,
) -> Result<impl Responder> {
    let (org, technical_name) = path.into_inner();
    let org_ctx = org.clone();
    let technical_name_ctx = technical_name.clone();
    let info = info.into_inner();
    let (name, versions, picked, diff) = web::block(move || {
        let mut conn = pool.get().unwrap();
        let mut modules = models::module::get_by_technical_name_organization_name(
            &mut conn,
            &technical_name,
            &org,
        );
        modules.sort_by_key(|m| std::cmp::Reverse(m.version_odoo));
        modules.dedup_by_key(|m| m.version_odoo);
        let available: Vec<i32> = modules.iter().map(|m| m.version_odoo).collect();
        let name = modules.first().map(|m| m.name.clone());
        let picked = pick_compare_versions(&available, info.from.as_deref(), info.to.as_deref());
        let diff = picked.and_then(|(from, to)| {
            let old = modules.iter().find(|m| m.version_odoo == from)?;
            let new = modules.iter().find(|m| m.version_odoo == to)?;
            Some(models::module_compare::compare(&mut conn, old, new))
        });
        let versions: Vec<String> = available
            .iter()
            .map(|v| odoo_version_u8_to_string(&(*v as u8)))
            .collect();
        let picked = picked.map(|(from, to)| {
            (
                odoo_version_u8_to_string(&(from as u8)),
                odoo_version_u8_to_string(&(to as u8)),
            )
        });
        (name, versions, picked, diff)
    })
    .await?;

    tmpl_env.render(
        "pages/module_compare.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "module_compare",
                org => org_ctx,
                technical_name => technical_name_ctx,
                module_name => name,
                versions => versions,
                from_version => picked.as_ref().map(|p| p.0.clone()),
                to_version => picked.map(|p| p.1),
                diff => diff,
            )
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::pick_compare_versions;

    #[test]
    fn compare_versions_default_to_the_two_newest_and_ignore_bad_input() {
        let available = [180, 170, 160];
        assert_eq!(
            pick_compare_versions(&available, None, None),
            Some((170, 180))
        );
        assert_eq!(
            pick_compare_versions(&available, Some("16.0"), Some("18.0")),
            Some((160, 180))
        );
        assert_eq!(
            pick_compare_versions(&available, Some("garbage"), Some("17.0")),
            Some((160, 170))
        );
        assert_eq!(
            pick_compare_versions(&available, None, Some("15.0")),
            Some((170, 180))
        );
        assert_eq!(pick_compare_versions(&[180], None, None), None);
    }
}

#[cfg(test)]
mod template_tests {
    use minijinja::{context, Environment};
    use serde_json::json;

    #[test]
    fn module_source_links_follow_the_scanned_directory() {
        let mut env = Environment::new();
        env.add_template(
            "module",
            include_str!("../../../../web/templates/partials/module_version_content.html"),
        )
        .unwrap();
        env.add_template(
            "overview",
            include_str!("../../../../web/templates/partials/modules_version_content.html"),
        )
        .unwrap();
        for (org, repo, path) in [
            ("OCA", "web", "web_responsive"),
            ("odoo", "odoo", "addons/web"),
            ("odoo", "odoo", "odoo/addons/base"),
        ] {
            let module = json!({
                "git": format!("https://github.com/{org}/{repo}.git"),
                "organization": org,
                "repository": repo,
                "odoo_version": "18.0",
                "source_path": path,
                "folder_size": 0,
                "security_warnings": [{"file": "models/test.py", "line": 42, "code": "TEST"}],
                "security_warning_count": 0,
                "migration_considerations": [],
                "dependencies": {"odoo": {}, "pip": [], "bin": []},
                "required_by": [],
                "license": "AGPL-3",
                "license_review": {
                    "effective_license": "AGPL-3",
                    "effective_license_source": "web",
                    "proprietary": [],
                    "licenses": {"AGPL-3": 1},
                    "findings": [{"severity": "warning", "code": "LICENSE_X", "message": "msg", "module": "web", "related": []}],
                },
                "views": [],
                "controllers": [],
                "models": [],
                "health": {"score": 62, "grade": "fair", "factors": [
                    {"name": "Security", "points": null, "detail": "No code analysis"},
                    {"name": "Activity", "points": 20, "detail": "Last human commit 8 months ago"},
                ]},
            });
            let html = env
                .get_template("module")
                .unwrap()
                .render(context!(module => module, history => Vec::<String>::new(), HEALTH_SCORE_ENABLED => true))
                .unwrap();
            assert!(html.contains(">62</span>") && html.contains("Security n/a"));
            assert!(html.contains(r#"title="Last human commit 8 months ago">Activity 20"#));
            assert!(html.contains(&format!("https://github.com/{org}/{repo}/tree/18.0/{path}")));
            assert!(html.contains(&format!(
                "https://github.com/{org}/{repo}/blob/18.0/{path}/models/test.py#L42"
            )));
            let group = json!({
                "odoo_version": "18.0",
                "pull_requests": [],
                "avg_days_open": null,
                "security_errors": [{
                    "organization": org,
                    "repository": repo,
                    "technical_name": "web",
                    "source_path": path,
                    "file": "models/test.py",
                    "line": 42,
                    "code": "TEST",
                    "message": "test"
                }],
                "security_warnings": [],
                "migration_warnings": [],
                "migration_infos": [],
            });
            let overview = env
                .get_template("overview")
                .unwrap()
                .render(context!(g => group))
                .unwrap();
            assert!(overview.contains(&format!(
                "https://github.com/{org}/{repo}/blob/18.0/{path}/models/test.py#L42"
            )));
        }
    }
}
