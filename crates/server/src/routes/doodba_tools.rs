// Copyright Alexandre D. Díaz
use crate::routes::api::v1::module::{process_modules_db, ModuleDependencyInfoResponse};
use actix_multipart::form::{text::Text, MultipartForm};
use actix_web::{get, post, web, Error as AWError, HttpRequest, HttpResponse, Responder, Result};
use diesel::sqlite::SqliteConnection;
use minijinja::context;
use oghutils::version::{odoo_version_string_to_u8, odoo_version_u8_to_string};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

use sqlitedb::{models, Pool};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

#[derive(MultipartForm)]
struct ConverterForm {
    modules: Vec<Text<String>>,
    odoo_version: Text<String>,
}

#[derive(MultipartForm)]
struct DepResolverForm {
    modules: Vec<Text<String>>,
    odoo_version: Text<String>,
}

#[get("/doodba/converter")]
pub async fn route_doodba_converter(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    tmpl_env.render(
        "pages/doodba_tools/converter.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "doodba_converter",
            )
        ),
    )
}

fn get_doodba_addons(
    conn: &mut SqliteConnection,
    mods: &[Text<String>],
    odoo_version: &str,
) -> Vec<sqlitedb::models::module::ModuleRepositoryInfo> {
    let odoo_ver = odoo_version_string_to_u8(odoo_version);
    let modules: Vec<String> = mods.iter().map(|x| x.as_str().to_string()).collect();
    models::module::get_module_repository(conn, &odoo_ver, modules.as_slice())
}

#[post("/doodba/converter/addons")]
pub async fn route_doodba_converter_addons(
    pool: web::Data<Pool>,
    form: MultipartForm<ConverterForm>,
) -> Result<HttpResponse, AWError> {
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_doodba_addons(&mut conn, &form.modules, &form.odoo_version)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[get("/doodba/dependency-resolver")]
pub async fn route_doodba_dependency_resolver(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    tmpl_env.render(
        "pages/doodba_tools/dependency_resolver.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "doodba_dep_resolver",
            )
        ),
    )
}

fn get_doodba_addons_full(
    conn: &mut SqliteConnection,
    mods: &[Text<String>],
    odoo_version: &str,
) -> ModuleDependencyInfoResponse {
    let odoo_ver = odoo_version_string_to_u8(odoo_version);
    let modules: Vec<String> = mods.iter().map(|x| x.as_str().to_string()).collect();
    let modules = models::module::get_by_technical_name_odoo_version(conn, &modules, &odoo_ver);
    let modules_infos = process_modules_db(conn, &modules);

    let mut dependencies_info = ModuleDependencyInfoResponse {
        odoo: HashMap::new(),
        pip: Vec::new(),
        bin: Vec::new(),
    };
    for module_info in modules_infos {
        let main_odoo_deps = dependencies_info
            .odoo
            .entry(module_info.repository)
            .or_default();
        if !main_odoo_deps.contains(&module_info.technical_name) {
            main_odoo_deps.push(module_info.technical_name);
        }
        for (key, values) in module_info.dependencies.odoo {
            let new_key = match key.split_once('/') {
                Some((_, repo_name)) => repo_name.to_string(),
                None => key,
            };
            let vec_ref = dependencies_info.odoo.entry(new_key).or_default();
            let new_values: Vec<String> = values
                .into_iter()
                .filter(|v| !vec_ref.contains(v))
                .collect();
            vec_ref.extend(new_values);
        }
        for value in module_info.dependencies.pip {
            if !dependencies_info.pip.contains(&value) {
                dependencies_info.pip.push(value);
            }
        }
        for value in module_info.dependencies.bin {
            if !dependencies_info.bin.contains(&value) {
                dependencies_info.bin.push(value);
            }
        }
    }
    dependencies_info
}

#[post("/doodba/dependency-resolver/addons")]
pub async fn route_doodba_dependency_resolver_addons(
    pool: web::Data<Pool>,
    form: MultipartForm<DepResolverForm>,
) -> Result<HttpResponse, AWError> {
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_doodba_addons_full(&mut conn, &form.modules, &form.odoo_version)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[derive(MultipartForm)]
struct MigrationPlanForm {
    modules: Vec<Text<String>>,
    from_version: Text<String>,
    to_version: Option<Text<String>>,
}

// A module that isn't merged yet at a given target version, but has an open
// migration PR/MR already visible upstream (see [[pull_request]] model).
#[derive(Debug, Serialize, Clone)]
pub struct MigrationPendingModuleInfo {
    pub technical_name: String,
    pub repository_name: String,
    pub organization: String,
    pub prid: i64,
    pub title: String,
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct MigrationPlanStepResponse {
    pub version: String,
    pub merged: Vec<models::module::ModuleRepositoryInfo>,
    pub pending: Vec<MigrationPendingModuleInfo>,
    pub missing: Vec<String>,
}

#[get("/doodba/migration")]
pub async fn route_doodba_migration_plan(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    tmpl_env.render(
        "pages/doodba_tools/migration_plan.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "doodba_migration_plan",
            )
        ),
    )
}

fn get_pending_modules(
    conn: &mut SqliteConnection,
    technical_names: &[String],
    version_odoo: &u8,
) -> Vec<MigrationPendingModuleInfo> {
    let prs = models::pull_request::get_by_technical_names_odoo_version(
        conn,
        technical_names,
        version_odoo,
    );
    let mut seen = HashSet::new();
    prs.into_iter()
        .filter(|pr| seen.insert(pr.module_technical_name.clone()))
        .map(|pr| {
            let repo = models::gh_repository::get_by_id(conn, &pr.gh_repository_id).unwrap();
            let org = models::gh_organization::get_by_id(conn, &repo.gh_organization_id).unwrap();
            MigrationPendingModuleInfo {
                url: format!(
                    "https://github.com/{}/{}/pull/{}",
                    &org.name, &repo.name, pr.prid
                ),
                technical_name: pr.module_technical_name,
                repository_name: repo.name,
                organization: org.name,
                prid: pr.prid,
                title: pr.name,
            }
        })
        .collect()
}

/// For each Odoo version between `from_version` and `to_version` (inclusive,
/// only versions actually present in the DB), classifies every requested module
/// as merged / pending-PR / missing. `from_version` itself is included as a
/// baseline step so the current addons.yaml can be sanity-checked too.
fn get_migration_plan(
    conn: &mut SqliteConnection,
    mods: &[Text<String>],
    from_version: &str,
    to_version: Option<&str>,
) -> Vec<MigrationPlanStepResponse> {
    let modules: Vec<String> = mods.iter().map(|x| x.as_str().to_string()).collect();
    let from_ver = odoo_version_string_to_u8(from_version) as i32;
    let to_ver = to_version
        .filter(|v| !v.is_empty())
        .map(|v| odoo_version_string_to_u8(v) as i32);

    let mut steps: Vec<i32> = models::module::get_odoo_versions(conn)
        .into_iter()
        .filter(|v| *v >= from_ver && to_ver.is_none_or(|to| *v <= to))
        .collect();
    steps.sort_unstable();

    steps
        .into_iter()
        .map(|version_odoo| {
            let version_u8 = version_odoo as u8;
            let merged = models::module::get_module_repository(conn, &version_u8, &modules);
            let merged_names: HashSet<&str> =
                merged.iter().map(|m| m.technical_name.as_str()).collect();
            let remaining: Vec<String> = modules
                .iter()
                .filter(|m| !merged_names.contains(m.as_str()))
                .cloned()
                .collect();
            let pending = get_pending_modules(conn, &remaining, &version_u8);
            let pending_names: HashSet<&str> =
                pending.iter().map(|p| p.technical_name.as_str()).collect();
            let missing: Vec<String> = remaining
                .into_iter()
                .filter(|m| !pending_names.contains(m.as_str()))
                .collect();
            MigrationPlanStepResponse {
                version: odoo_version_u8_to_string(&version_u8),
                merged,
                pending,
                missing,
            }
        })
        .collect()
}

#[post("/doodba/migration/plan")]
pub async fn route_doodba_migration_plan_addons(
    pool: web::Data<Pool>,
    form: MultipartForm<MigrationPlanForm>,
) -> Result<HttpResponse, AWError> {
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_migration_plan(
            &mut conn,
            &form.modules,
            &form.from_version,
            form.to_version.as_ref().map(|t| t.as_str()),
        )
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}
