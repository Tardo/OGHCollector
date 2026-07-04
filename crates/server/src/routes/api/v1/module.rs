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
}

fn construct_module_dependencies_info(
    conn: &mut SqliteConnection,
    module: &models::module::Model,
    mod_depends_dict: &mut HashMap<String, Vec<String>>,
    pip_depends: &mut Vec<String>,
    bin_depends: &mut Vec<String>,
) {
    let mut pip_depends_list: Vec<String> =
        models::dependency::get_module_external_dependency_names(conn, &module.id, "python");
    pip_depends_list = pip_depends_list
        .into_iter()
        .map(normalize_python_dep)
        .collect();
    pip_depends.append(&mut pip_depends_list);
    let mut bin_depends_list: Vec<String> =
        models::dependency::get_module_external_dependency_names(conn, &module.id, "bin");
    bin_depends.append(&mut bin_depends_list);
    let mod_depends = models::dependency::get_module_dependency_info(conn, &module.id);
    for mod_dep in mod_depends {
        let repo_depends = mod_depends_dict
            .entry(format!("{}/{}", &mod_dep.org, &mod_dep.repo))
            .or_default();
        let technical_name = mod_dep.module_name.clone();
        if !repo_depends.contains(&technical_name) {
            repo_depends.push(technical_name);
            let dep_module = models::module::get_by_id(conn, &mod_dep.module_id).unwrap();
            construct_module_dependencies_info(
                conn,
                &dep_module,
                mod_depends_dict,
                pip_depends,
                bin_depends,
            );
        }
    }
}

fn get_module_git(conn: &mut SqliteConnection, module: &models::module::Model) -> String {
    let repo = models::gh_repository::get_by_id(conn, &module.gh_repository_id).unwrap();
    let org = models::gh_organization::get_by_id(conn, &repo.gh_organization_id).unwrap();
    format!("https://github.com/{}/{}.git", &org.name, &repo.name)
}

pub fn process_modules_db(
    conn: &mut SqliteConnection,
    modules: &[models::module::Model],
) -> Vec<ModuleFullInfoResponse> {
    let mut res: Vec<ModuleFullInfoResponse> = Vec::new();
    for module in modules {
        let mut pip_dependencies: Vec<String> = Vec::new();
        let mut bin_dependencies: Vec<String> = Vec::new();
        let mut odoo_dependencies: HashMap<String, Vec<String>> = HashMap::new();
        construct_module_dependencies_info(
            conn,
            module,
            &mut odoo_dependencies,
            &mut pip_dependencies,
            &mut bin_dependencies,
        );
        let dependencies = ModuleDependencyInfoResponse {
            odoo: odoo_dependencies,
            pip: pip_dependencies.unique(),
            bin: bin_dependencies.unique(),
        };
        let authors = models::module_author::get_names_by_module_id(conn, &module.id);
        let maintainers = models::module_maintainer::get_names_by_module_id(conn, &module.id);
        let committers = models::module_committer::get_names_by_module_id(conn, &module.id);
        let repo = models::gh_repository::get_by_id(conn, &module.gh_repository_id).unwrap();
        let org = models::gh_organization::get_by_id(conn, &repo.gh_organization_id).unwrap();
        let git = get_module_git(conn, module);
        res.push(ModuleFullInfoResponse {
            name: module.name.clone(),
            version: module.version_module.clone(),
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
            format!(
                "https://github.com/{}/{}.git",
                &x.organization, &x.repository
            )
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
    let version_odoo = odoo_version_string_to_u8(&odoo_version);

    if info.org.is_some() && info.repo.is_some() {
        let org_name = info.org.clone().unwrap();
        let repo_name = info.repo.clone().unwrap();
        let result = web::block(move || {
            let mut conn = pool.get().unwrap();
            let modules = models::module::get_by_technical_name_odoo_version_organization_name_repository_name(
                &mut conn, &module_name, &version_odoo, &org_name, &repo_name,
            );
            process_modules_db(&mut conn, &modules)
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
            process_modules_db(&mut conn, &modules)
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
            process_modules_db(&mut conn, &modules)
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
        process_modules_db(&mut conn, &modules)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}
