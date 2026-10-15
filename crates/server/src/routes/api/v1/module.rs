// Copyright Alexandre D. Díaz
//! Compare: `/module/{org}/{technical_name}/compare?from=&to=` (page, defaults to the two newest
//! versions) and `GET /v1/compare/{module_name}?from=&to=&org=&repo=` (JSON, `null` when missing),
//! both on `sqlitedb::models::module_compare`.
//!
//! Module health (`api/v1/module.rs::module_health`): synthetic 0-100 `health` field on every
//! `/v1/module/*` row, shown as the top band (score + one bar per factor) of the module-page
//! summary panel when `health_score_enabled` (default `true`, `HEALTH_SCORE_ENABLED` in the
//! template context). Four factors of 25 (security findings, months since the last *human* commit
//! on that branch via `module_committer_period` filtered by `sqlitedb::models::HUMAN_COMMITTER`,
//! the shared SQL predicate on `com.name` that also drives every committer ranking (suffix patterns
//! like `% bot`/`%-bot`/`%[bot]`/`Launchpad Translations on behalf of %`, not a list — bots come in
//! dozens of spellings) — `last_commit_date` is useless here, ~70% of 16.0+ modules' last commit is
//! Weblate/bot —, "Required by", Odoo series count up to this version). Thresholds were checked
//! against whether 16.0/17.0 modules were later migrated (see the comment on `module_health`).
//! Factors without data are `null` and left out of the denominator; a grave finding caps the grade
//! at "fair".
use array_tool::vec::Uniq;
use std::collections::HashMap;

use actix_web::{get, web, Error as AWError, HttpResponse};
use chrono::Datelike;
use serde::{Deserialize, Serialize};

use diesel::sqlite::SqliteConnection;
use oghutils::license::LicenseReport;
use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::{models, Pool};

use crate::utils::normalize_python_dep;

#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleDependencyInfoResponse {
    pub odoo: HashMap<String, Vec<String>>,
    pub pip: Vec<String>,
    pub bin: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleViewResponse {
    pub xml_id: String,
    pub name: String,
    pub model: String,
    pub inherit_xml_id: Option<String>,
    pub is_new: bool,
    pub view_type: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleModelFieldResponse {
    pub name: String,
    pub field_type: String,
    pub relation: Option<String>,
    pub attrs: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleModelMethodResponse {
    pub name: String,
    pub decorators: Vec<String>,
    pub signature: String,
    pub docstring: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleModelResponse {
    pub model_name: String,
    pub class_name: String,
    pub inherit_from: Vec<String>,
    pub is_new_model: bool,
    pub docstring: Option<String>,
    pub attrs: Option<serde_json::Value>,
    pub fields: Vec<ModuleModelFieldResponse>,
    pub methods: Vec<ModuleModelMethodResponse>,
}

// An HTTP endpoint the module exposes. `auth` is the resolved value (Odoo
// defaults applied) or None for pure overrides of inherited routes; `csrf`
// None means the framework default (enabled).
#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleControllerResponse {
    pub class_name: String,
    pub name: String,
    pub routes: Vec<String>,
    pub auth: Option<String>,
    pub http_type: String,
    pub methods: Vec<String>,
    pub csrf: Option<bool>,
    pub website: bool,
    pub uses_sudo: bool,
    pub signature: String,
    pub docstring: Option<String>,
}

// Grave security findings only ("error" severity): minor ones are counted
// separately; their details remain in the system_event log.
#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleSecurityWarningResponse {
    pub code: String,
    pub message: String,
    pub xml_id: Option<String>,
    pub file: Option<String>,
    pub line: Option<i32>,
}

// Every migration consideration, both severities - unlike security warnings
// these aren't graded grave/minor, they're all meant to be read by whoever
// is about to migrate the module.
#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleMigrationConsiderationResponse {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub context: Option<String>,
    pub file: Option<String>,
    pub line: Option<i32>,
}

// One part of `ModuleHealth`, worth `HEALTH_FACTOR_MAX` points; `points` is
// None when there's no data for it, and it's then left out of the score.
#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleHealthFactor {
    pub name: String,
    pub points: Option<u8>,
    pub detail: String,
}

/// Synthetic summary of facts already shown on the module page (security
/// findings, human commit activity, reverse dependencies, Odoo series), so
/// they don't have to be weighed by hand. A heuristic, not an audit.
#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleHealth {
    /// 0-100 over the factors that have data.
    pub score: u8,
    /// "good" / "fair" / "poor"; never "good" with a grave security finding.
    pub grade: String,
    pub factors: Vec<ModuleHealthFactor>,
}

pub const HEALTH_FACTOR_MAX: u8 = 25;

// Explained on /about (pages/about.html): keep its table in sync.
// Step thresholds checked (2026-10) against whether 16.0/17.0 modules were
// later migrated: that rate climbs with the Odoo series count (53% for 1 ->
// 92% for 7+) and with "Required by" up to ~5 (68% for 0 -> 95%), then
// flattens. Activity and security bands (and the 75/50 grade cutoffs) are
// unvalidated: migration says nothing about upkeep/risk and there's no other
// outcome to check them against.
fn module_health(
    security: Option<(usize, usize)>,
    months_idle: Option<Option<i32>>,
    required_by: usize,
    series: usize,
    first_series: &str,
) -> ModuleHealth {
    let factor = |name: &str, points: Option<u8>, detail: String| ModuleHealthFactor {
        name: name.to_string(),
        points,
        detail,
    };
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let factors = vec![
        match security {
            None => factor("Security", None, "No code analysis".to_string()),
            Some((0, 0)) => factor("Security", Some(25), "No findings".to_string()),
            Some((0, minor)) => factor(
                "Security",
                Some(25 - (3 * minor).min(10) as u8),
                format!("{minor} minor finding{}", plural(minor)),
            ),
            Some((grave, minor)) => factor(
                "Security",
                Some(10 - 5 * (grave - 1).min(2) as u8),
                format!("{grave} grave, {minor} minor"),
            ),
        },
        match months_idle {
            None => factor("Activity", None, "No commit data".to_string()),
            Some(None) => factor("Activity", Some(0), "Only bot commits".to_string()),
            Some(Some(m)) => factor(
                "Activity",
                Some(match m {
                    ..=6 => 25,
                    7..=12 => 20,
                    13..=24 => 12,
                    25..=36 => 6,
                    _ => 0,
                }),
                match m {
                    ..=0 => "Human commit this month".to_string(),
                    _ => format!("Last human commit {m} month{} ago", plural(m as usize)),
                },
            ),
        },
        factor(
            "Required by",
            Some(match required_by {
                0 => 5,
                1 => 15,
                2..=4 => 20,
                _ => 25,
            }),
            if required_by == 1 {
                "1 module depends on it".to_string()
            } else {
                format!("{required_by} modules depend on it")
            },
        ),
        factor(
            "Maturity",
            Some(match series {
                ..=1 => 5,
                2 => 12,
                3 => 18,
                _ => 25,
            }),
            format!("In {series} Odoo series since {first_series}"),
        ),
    ];
    let (got, max) = factors
        .iter()
        .filter_map(|f| f.points)
        .fold((0u32, 0u32), |(g, m), p| {
            (g + p as u32, m + HEALTH_FACTOR_MAX as u32)
        });
    let score = (got * 100).checked_div(max).unwrap_or(0) as u8;
    let has_grave = matches!(security, Some((g, _)) if g > 0);
    let grade = match score {
        75.. if !has_grave => "good",
        50.. => "fair",
        _ => "poor",
    };
    ModuleHealth {
        score,
        grade: grade.to_string(),
        factors,
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleFullInfoResponse {
    pub technical_name: String,
    pub name: String,
    pub version: String,
    pub description: String,
    /// base64-encoded `static/description/icon.png`; empty when the module has none.
    pub icon: String,
    pub authors: Vec<String>,
    pub website: String,
    pub license: String,
    pub category: String,
    pub auto_install: bool,
    pub application: bool,
    pub installable: bool,
    pub maintainers: Vec<String>,
    pub committers: Vec<String>,
    pub dependencies: ModuleDependencyInfoResponse,
    pub update_date: String,
    pub git: String,
    pub folder_size: u64,
    pub repository: String,
    pub organization: String,
    pub odoo_version: String,
    pub source_path: String,
    pub views: Vec<ModuleViewResponse>,
    pub models: Vec<ModuleModelResponse>,
    pub controllers: Vec<ModuleControllerResponse>,
    pub security_warnings: Vec<ModuleSecurityWarningResponse>,
    pub security_warning_count: usize,
    pub migration_considerations: Vec<ModuleMigrationConsiderationResponse>,
    /// Modules (same Odoo version, any repository) that declare this module
    /// as an Odoo dependency.
    pub required_by: Vec<models::module::ModuleCriteriaInfo>,
    /// This module's license checked against its whole dependency closure.
    pub license_review: LicenseReport,
    pub health: ModuleHealth,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleGenericInfoResponse {
    pub name: String,
    pub technical_name: String,
    pub odoo_versions: Vec<String>,
    pub repos: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct RouteModuleRequest {
    org: Option<String>,
    repo: Option<String>,
    version: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleVersionInfoResponse {
    pub version_module: String,
    pub create_date: String,
    pub update_date: String,
    pub is_latest: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleVersionHistoryResponse {
    pub organization: String,
    pub repository: String,
    pub versions: Vec<ModuleVersionInfoResponse>,
}

fn get_module_git(conn: &mut SqliteConnection, module: &models::module::Model) -> String {
    let repo = models::gh_repository::get_by_id(conn, &module.gh_repository_id).unwrap();
    let org = models::gh_organization::get_by_id(conn, &repo.gh_organization_id).unwrap();
    oghutils::git::clone_url(repo.url.as_deref(), &org.name, &repo.name)
}

fn get_module_views(
    conn: &mut SqliteConnection,
    module_version_id: &i64,
) -> Vec<ModuleViewResponse> {
    models::module_view::get_by_module_version_id(conn, module_version_id)
        .into_iter()
        .map(|v| ModuleViewResponse {
            is_new: v.inherit_xml_id.is_none(),
            xml_id: v.xml_id,
            name: v.name.unwrap_or_default(),
            model: v.model.unwrap_or_default(),
            inherit_xml_id: v.inherit_xml_id,
            view_type: v.view_type,
        })
        .collect()
}

fn get_module_controllers(
    conn: &mut SqliteConnection,
    module_version_id: &i64,
) -> Vec<ModuleControllerResponse> {
    models::module_controller::get_by_module_version_id(conn, module_version_id)
        .into_iter()
        .map(|c| ModuleControllerResponse {
            routes: c.routes_vec(),
            methods: c.methods_vec(),
            class_name: c.class_name,
            name: c.name,
            auth: c.auth,
            http_type: c.http_type,
            csrf: c.csrf,
            website: c.website,
            uses_sudo: c.uses_sudo,
            signature: c.signature,
            docstring: c.docstring,
        })
        .collect()
}

fn get_module_security_warnings(
    conn: &mut SqliteConnection,
    module_version_id: &i64,
) -> (Vec<ModuleSecurityWarningResponse>, usize) {
    let mut errors = Vec::new();
    let mut warnings = 0;
    for w in models::module_security_warning::get_by_module_version_id(conn, module_version_id) {
        if w.severity == models::module_security_warning::SEVERITY_WARNING {
            warnings += 1;
        } else if w.severity == models::module_security_warning::SEVERITY_ERROR {
            errors.push(ModuleSecurityWarningResponse {
                code: w.code,
                message: w.message,
                xml_id: w.xml_id,
                file: w.file,
                line: w.line,
            });
        }
    }
    (errors, warnings)
}

fn get_module_migration_considerations(
    conn: &mut SqliteConnection,
    module_version_id: &i64,
) -> Vec<ModuleMigrationConsiderationResponse> {
    models::module_migration_note::get_by_module_version_id(conn, module_version_id)
        .into_iter()
        .map(|n| ModuleMigrationConsiderationResponse {
            severity: n.severity,
            code: n.code,
            message: n.message,
            context: n.context,
            file: n.file,
            line: n.line,
        })
        .collect()
}

fn get_module_models(
    conn: &mut SqliteConnection,
    module_version_id: &i64,
) -> Vec<ModuleModelResponse> {
    models::module_model::get_by_module_version_id(conn, module_version_id)
        .into_iter()
        .map(|m| {
            let fields = models::module_model_field::get_by_module_model_id(conn, &m.id)
                .into_iter()
                .map(|f| {
                    let attrs = f.attrs_value();
                    ModuleModelFieldResponse {
                        name: f.name,
                        field_type: f.field_type,
                        relation: f.relation,
                        attrs,
                    }
                })
                .collect();
            let methods = models::module_model_method::get_by_module_model_id(conn, &m.id)
                .into_iter()
                .map(|meth| ModuleModelMethodResponse {
                    decorators: meth.decorators_vec(),
                    name: meth.name,
                    signature: meth.signature,
                    docstring: meth.docstring,
                })
                .collect();
            let attrs = m.attrs_value();
            ModuleModelResponse {
                model_name: m.model_name,
                class_name: m.class_name,
                inherit_from: m
                    .inherit_from
                    .map(|s| s.split(',').map(|x| x.to_string()).collect())
                    .unwrap_or_default(),
                is_new_model: m.is_new_model,
                docstring: m.docstring,
                attrs,
                fields,
                methods,
            }
        })
        .collect()
}

pub fn process_modules_db(
    conn: &mut SqliteConnection,
    modules: &[models::module::Model],
    version_module: Option<&str>,
) -> Vec<ModuleFullInfoResponse> {
    let mut res: Vec<ModuleFullInfoResponse> = Vec::new();
    for module in modules {
        let full_deps = models::dependency::get_full_dependency_info(conn, module);
        let license_review = full_deps.license_review(&[module.id]);
        let dependencies = ModuleDependencyInfoResponse {
            odoo: full_deps.odoo,
            pip: full_deps
                .pip
                .into_iter()
                .map(normalize_python_dep)
                .collect::<Vec<_>>()
                .unique(),
            bin: full_deps.bin.unique(),
        };
        // ponytail: hard cap of 500; core modules (e.g. `web`) have huge
        // fan-in — paginate if a page ever truly needs the full list.
        let required_by = models::module::search_by_criteria(
            conn,
            &(module.version_odoo as u8),
            None,
            None,
            Some(&module.technical_name),
            500,
        );
        let authors = models::module_author::get_names_by_module_id(conn, &module.id);
        let maintainers = models::module_maintainer::get_names_by_module_id(conn, &module.id);
        let committers = models::module_committer::get_names_by_module_id(conn, &module.id);
        let repo = models::gh_repository::get_by_id(conn, &module.gh_repository_id).unwrap();
        let org = models::gh_organization::get_by_id(conn, &repo.gh_organization_id).unwrap();
        let git = get_module_git(conn, module);
        // None (default) resolves to the latest version; an explicit request
        // for a version that doesn't exist for this module comes back with
        // empty views/models rather than silently falling back to "latest".
        let resolved_version = match version_module {
            Some(v) => models::module_version::get_by_module_id_version_module(conn, &module.id, v),
            None => models::module_version::resolve_current(conn, module),
        };
        let (security_warnings, security_warning_count) = resolved_version
            .as_ref()
            .map(|mv| get_module_security_warnings(conn, &mv.id))
            .unwrap_or_default();
        let (views, module_models, controllers, migration_considerations, version) =
            match &resolved_version {
                Some(mv) => (
                    get_module_views(conn, &mv.id),
                    get_module_models(conn, &mv.id),
                    get_module_controllers(conn, &mv.id),
                    get_module_migration_considerations(conn, &mv.id),
                    mv.version_module.clone(),
                ),
                None => (
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    version_module
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| module.version_module.clone()),
                ),
            };
        let activity = models::module_committer_period::get_human_activity(conn, &module.id);
        let now = chrono::Utc::now();
        let months_idle = (activity.rows > 0).then(|| {
            activity
                .last_month
                .map(|m| (now.year() * 12 + now.month0() as i32 - m).max(0))
        });
        let series: Vec<i32> =
            models::module::get_odoo_versions_by_technical_name_organization_name(
                conn,
                &module.technical_name,
                &org.name,
            )
            .into_iter()
            .filter(|v| *v <= module.version_odoo)
            .collect();
        let health = module_health(
            resolved_version
                .as_ref()
                .map(|_| (security_warnings.len(), security_warning_count)),
            months_idle,
            required_by.len(),
            series.len(),
            &odoo_version_u8_to_string(&(*series.first().unwrap_or(&module.version_odoo) as u8)),
        );
        res.push(ModuleFullInfoResponse {
            name: module.name.clone(),
            version,
            description: module.description.clone().unwrap_or_default(),
            icon: module.icon.clone().unwrap_or_default(),
            authors,
            website: module.website.clone().unwrap_or_default(),
            license: module.license.clone().unwrap_or_default(),
            category: module.category.clone().unwrap_or_default(),
            auto_install: module.auto_install,
            technical_name: module.technical_name.clone(),
            application: module.application,
            installable: module.installable,
            maintainers,
            committers,
            dependencies,
            update_date: module.update_date.clone(),
            git,
            folder_size: module.folder_size as u64,
            repository: repo.name.clone(),
            organization: org.name.clone(),
            odoo_version: odoo_version_u8_to_string(&(module.version_odoo as u8)),
            source_path: module.source_path.clone(),
            views,
            models: module_models,
            controllers,
            security_warnings,
            security_warning_count,
            migration_considerations,
            required_by,
            license_review,
            health,
        });
    }
    res
}

fn get_module_generic_info(
    conn: &mut SqliteConnection,
    module_name: &str,
) -> Option<ModuleGenericInfoResponse> {
    let modules = models::module::get_info(conn, module_name);
    if modules.is_empty() {
        return None;
    }
    let name = &modules[0].name;
    let technical_name = &modules[0].technical_name;
    let odoo_versions = modules
        .iter()
        .map(|x| odoo_version_u8_to_string(&(x.version_odoo as u8)))
        .collect::<Vec<String>>();
    let repos = modules
        .iter()
        .map(|x| {
            oghutils::git::clone_url(x.repository_url.as_deref(), &x.organization, &x.repository)
        })
        .collect::<Vec<String>>();
    Some(ModuleGenericInfoResponse {
        name: name.clone(),
        technical_name: technical_name.clone(),
        odoo_versions,
        repos: repos.unique(),
    })
}

#[get("/module/{module_name}")]
pub async fn route(
    pool: web::Data<Pool>,
    path: web::Path<String>,
) -> Result<HttpResponse, AWError> {
    let module_name = path.into_inner();
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_module_generic_info(&mut conn, &module_name)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[get("/module/{module_name}/{odoo_version}")]
pub async fn route_odoo_version(
    pool: web::Data<Pool>,
    path: web::Path<(String, String)>,
    info: web::Query<RouteModuleRequest>,
) -> Result<HttpResponse, AWError> {
    let (module_name, odoo_version) = path.into_inner();
    let version_odoo = crate::utils::odoo_version(&odoo_version)?;
    let version_module = info.version.clone();

    if info.org.is_some() && info.repo.is_some() {
        let org_name = info.org.clone().unwrap();
        let repo_name = info.repo.clone().unwrap();
        let result = web::block(move || {
            let mut conn = pool.get().unwrap();
            let modules = models::module::get_by_technical_name_odoo_version_organization_name_repository_name(
                &mut conn, &module_name, &version_odoo, &org_name, &repo_name,
            );
            process_modules_db(&mut conn, &modules, version_module.as_deref())
        })
        .await?;
        return Ok(HttpResponse::Ok().json(result));
    } else if info.org.is_some() {
        let org_name = info.org.clone().unwrap();
        let result = web::block(move || {
            let mut conn = pool.get().unwrap();
            let modules = models::module::get_by_technical_name_odoo_version_organization_name(
                &mut conn,
                &module_name,
                &version_odoo,
                &org_name,
            );
            process_modules_db(&mut conn, &modules, version_module.as_deref())
        })
        .await?;
        return Ok(HttpResponse::Ok().json(result));
    } else if info.repo.is_some() {
        let repo_name = info.repo.clone().unwrap();
        let result = web::block(move || {
            let mut conn = pool.get().unwrap();
            let modules = models::module::get_by_technical_name_odoo_version_repository_name(
                &mut conn,
                &module_name,
                &version_odoo,
                &repo_name,
            );
            process_modules_db(&mut conn, &modules, version_module.as_deref())
        })
        .await?;
        return Ok(HttpResponse::Ok().json(result));
    }
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        let modules = models::module::get_by_technical_name_odoo_version(
            &mut conn,
            &[module_name],
            &version_odoo,
        );
        process_modules_db(&mut conn, &modules, version_module.as_deref())
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[get("/module/{module_name}/{odoo_version}/versions")]
pub async fn route_versions(
    pool: web::Data<Pool>,
    path: web::Path<(String, String)>,
    info: web::Query<RouteModuleRequest>,
) -> Result<HttpResponse, AWError> {
    let (module_name, odoo_version) = path.into_inner();
    let version_odoo = crate::utils::odoo_version(&odoo_version)?;
    let org = info.org.clone();
    let repo = info.repo.clone();

    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        let modules = match (&org, &repo) {
            (Some(org), Some(repo)) => {
                models::module::get_by_technical_name_odoo_version_organization_name_repository_name(
                    &mut conn, &module_name, &version_odoo, org, repo,
                )
            }
            (Some(org), None) => models::module::get_by_technical_name_odoo_version_organization_name(
                &mut conn,
                &module_name,
                &version_odoo,
                org,
            ),
            (None, Some(repo)) => models::module::get_by_technical_name_odoo_version_repository_name(
                &mut conn,
                &module_name,
                &version_odoo,
                repo,
            ),
            (None, None) => models::module::get_by_technical_name_odoo_version(
                &mut conn,
                std::slice::from_ref(&module_name),
                &version_odoo,
            ),
        };
        modules
            .iter()
            .map(|m| {
                let repo_model = models::gh_repository::get_by_id(&mut conn, &m.gh_repository_id).unwrap();
                let org_model =
                    models::gh_organization::get_by_id(&mut conn, &repo_model.gh_organization_id)
                        .unwrap();
                let versions = models::module_version::get_by_module_id(&mut conn, &m.id)
                    .into_iter()
                    .map(|v| ModuleVersionInfoResponse {
                        is_latest: v.version_module == m.version_module,
                        version_module: v.version_module,
                        create_date: v.create_date,
                        update_date: v.update_date,
                    })
                    .collect();
                ModuleVersionHistoryResponse {
                    organization: org_model.name,
                    repository: repo_model.name,
                    versions,
                }
            })
            .collect::<Vec<_>>()
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[derive(Debug, Deserialize)]
pub struct RouteModuleCompareRequest {
    from: String,
    to: String,
    org: Option<String>,
    repo: Option<String>,
}

fn find_module(
    conn: &mut SqliteConnection,
    technical_name: &str,
    version_odoo: &u8,
    org: Option<&str>,
    repo: Option<&str>,
) -> Option<models::module::Model> {
    let modules = match (org, repo) {
        (Some(org), Some(repo)) => {
            models::module::get_by_technical_name_odoo_version_organization_name_repository_name(
                conn,
                technical_name,
                version_odoo,
                org,
                repo,
            )
        }
        (Some(org), None) => models::module::get_by_technical_name_odoo_version_organization_name(
            conn,
            technical_name,
            version_odoo,
            org,
        ),
        (None, Some(repo)) => models::module::get_by_technical_name_odoo_version_repository_name(
            conn,
            technical_name,
            version_odoo,
            repo,
        ),
        (None, None) => models::module::get_by_technical_name_odoo_version(
            conn,
            &[technical_name.to_string()],
            version_odoo,
        ),
    };
    modules.into_iter().next()
}

/// What changed in a module between two Odoo versions (manifest, direct
/// dependencies, models/fields/methods, views, HTTP routes). `null` when the
/// module doesn't exist at either version.
#[get("/compare/{module_name}")]
pub async fn route_compare(
    pool: web::Data<Pool>,
    path: web::Path<String>,
    info: web::Query<RouteModuleCompareRequest>,
) -> Result<HttpResponse, AWError> {
    let module_name = path.into_inner();
    let info = info.into_inner();
    let (from, to) = (
        crate::utils::odoo_version(&info.from)?,
        crate::utils::odoo_version(&info.to)?,
    );
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        let (org, repo) = (info.org.as_deref(), info.repo.as_deref());
        let old = find_module(&mut conn, &module_name, &from, org, repo)?;
        let new = find_module(&mut conn, &module_name, &to, org, repo)?;
        Some(models::module_compare::compare(&mut conn, &old, &new))
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[cfg(test)]
mod tests {
    use super::module_health;

    #[test]
    fn health_skips_missing_data_and_grave_findings_cap_the_grade() {
        let h = module_health(Some((0, 0)), Some(Some(2)), 12, 5, "12.0");
        assert_eq!((h.score, h.grade.as_str()), (100, "good"));
        // No analysis/commit data: scored over the other two factors only.
        let h = module_health(None, None, 12, 5, "12.0");
        assert_eq!(h.score, 100);
        let h = module_health(Some((1, 0)), Some(Some(0)), 50, 8, "8.0");
        assert_eq!((h.score, h.grade.as_str()), (85, "fair"));
        let h = module_health(Some((3, 4)), Some(None), 0, 1, "18.0");
        assert_eq!((h.score, h.grade.as_str()), (10, "poor"));
    }
}
