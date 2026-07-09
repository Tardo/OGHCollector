// Copyright Alexandre D. Díaz
use array_tool::vec::Uniq;
use std::collections::HashMap;

use actix_web::{get, web, Error as AWError, HttpResponse};
use serde::{Deserialize, Serialize};

use diesel::sqlite::SqliteConnection;
use oghutils::version::{odoo_version_string_to_u8, odoo_version_u8_to_string};
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

// Grave security findings only ("error" severity): minor ones are log-lines
// in system_event by design, not part of the module's public record.
#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleSecurityWarningResponse {
    pub code: String,
    pub message: String,
    pub xml_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ModuleFullInfoResponse {
    pub technical_name: String,
    pub name: String,
    pub version: String,
    pub description: String,
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
    pub views: Vec<ModuleViewResponse>,
    pub models: Vec<ModuleModelResponse>,
    pub controllers: Vec<ModuleControllerResponse>,
    pub security_warnings: Vec<ModuleSecurityWarningResponse>,
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
    format!("https://github.com/{}/{}.git", org.name, repo.name)
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
) -> Vec<ModuleSecurityWarningResponse> {
    models::module_security_warning::get_by_module_version_id(conn, module_version_id)
        .into_iter()
        .filter(|w| w.severity == models::module_security_warning::SEVERITY_ERROR)
        .map(|w| ModuleSecurityWarningResponse {
            code: w.code,
            message: w.message,
            xml_id: w.xml_id,
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
        let (views, module_models, controllers, security_warnings, version) =
            match &resolved_version {
                Some(mv) => (
                    get_module_views(conn, &mv.id),
                    get_module_models(conn, &mv.id),
                    get_module_controllers(conn, &mv.id),
                    get_module_security_warnings(conn, &mv.id),
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
        res.push(ModuleFullInfoResponse {
            name: module.name.clone(),
            version,
            description: module.description.clone().unwrap_or_default(),
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
            views,
            models: module_models,
            controllers,
            security_warnings,
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
        .map(|x| format!("https://github.com/{}/{}.git", x.organization, x.repository))
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
    let version_odoo = odoo_version_string_to_u8(&odoo_version);
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
    let version_odoo = odoo_version_string_to_u8(&odoo_version);
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
