// Copyright 2025 Alexandre D. Díaz
use array_tool::vec::Uniq;
use std::collections::HashMap;

use actix_web::{get, web, Error as AWError, HttpResponse};
use serde::{Deserialize, Serialize};

use oghutils::version::{odoo_version_string_to_u8, odoo_version_u8_to_string};
use sqlitedb::{
    models::{self, Connection},
    Pool,
};

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
    pub dependencies: ModuleDependencyInfoResponse,
    pub update_date: String,
    pub git: String,
    pub folder_size: u64,
    pub repository: String,
    pub organization: String,
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

fn construct_module_dependecies_info(
    conn: &Connection,
    module: &models::module::Model,
    mod_depends_dict: &mut HashMap<String, Vec<String>>,
    pip_depends: &mut Vec<String>,
    bin_depends: &mut Vec<String>,
) {
    let mut pip_depends_list: Vec<String> =
        models::dependency::get_module_external_dependency_names(conn, &module.id, "python");
    pip_depends.append(&mut pip_depends_list);
    let mut bin_depends_list: Vec<String> =
        models::dependency::get_module_external_dependency_names(conn, &module.id, "bin");
    bin_depends.append(&mut bin_depends_list);
    let mod_depends = models::dependency::get_module_dependency_info(conn, &module.id);
    for mod_dep in mod_depends {
        let repo_depends = mod_depends_dict
            .entry(format!("{}/{}", &mod_dep.org, &mod_dep.repo))
            .or_default();
        let technical_name = mod_dep.technical_name.clone();
        if !repo_depends.contains(&technical_name) {
            repo_depends.push(mod_dep.technical_name.clone());
            construct_module_dependecies_info(
                conn,
                module,
                mod_depends_dict,
                pip_depends,
                bin_depends,
            );
        }
    }
}

fn get_module_git(conn: &Connection, module: &models::module::Model) -> String {
    let repo = models::gh_repository::get_by_id(conn, &module.gh_repository_id.0).unwrap();
    let org = models::gh_organization::get_by_id(conn, &repo.gh_organization_id.0).unwrap();
    format!("https://github.com/{}/{}.git", &org.name, &repo.name).to_string()
}

pub fn process_modules_db(
    conn: &Connection,
    modules: &Vec<models::module::Model>,
) -> Vec<ModuleFullInfoResponse> {
    let mut res: Vec<ModuleFullInfoResponse> = Vec::new();
    for module in modules {
        let mut pip_dependencies: Vec<String> = Vec::new();
        let mut bin_dependencies: Vec<String> = Vec::new();
        let mut odoo_dependencies: HashMap<String, Vec<String>> = HashMap::new();
        construct_module_dependecies_info(
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
        let repo = models::gh_repository::get_by_id(conn, &module.gh_repository_id.0).unwrap();
        let org = models::gh_organization::get_by_id(conn, &repo.gh_organization_id.0).unwrap();
        res.push(ModuleFullInfoResponse {
            name: module.name.clone(),
            version: module.version_module.clone(),
            description: module.description.clone(),
            authors,
            website: module.website.clone(),
            license: module.license.clone(),
            category: module.category.clone(),
            auto_install: module.auto_install,
            technical_name: module.technical_name.clone(),
            application: module.application,
            installable: module.installable,
            maintainers,
            dependencies,
            update_date: module.update_date.clone(),
            git: get_module_git(conn, module),
            folder_size: module.folder_size,
            repository: module.gh_repository_id.1.clone(),
            organization: org.name.clone(),
        });
    }
    res
}

fn get_module_generic_info(
    conn: &Connection,
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
        .map(|x| odoo_version_u8_to_string(&x.version_odoo))
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
    let conn = web::block(move || pool.get()).await?.unwrap();
    let module_name = path.into_inner();
    let result = web::block(move || get_module_generic_info(&conn, &module_name)).await?;
    Ok(HttpResponse::Ok().json(result))
}

#[get("/module/{module_name}/{odoo_version}")]
pub async fn route_odoo_version(
    pool: web::Data<Pool>,
    path: web::Path<(String, String)>,
    info: web::Query<RouteModuleRequest>,
) -> Result<HttpResponse, AWError> {
    let conn = web::block(move || pool.get()).await?.unwrap();
    let (module_name, odoo_version) = path.into_inner();
    let version_odoo = odoo_version_string_to_u8(&odoo_version);

    if info.org.is_some() && info.repo.is_some() {
        let org_name = info.org.clone().unwrap();
        let repo_name = info.repo.clone().unwrap();
        let result = web::block(move || {
            let modules = models::module::get_by_technical_name_odoo_version_organization_name_repository_name(&conn, &module_name, &version_odoo, &org_name, &repo_name);
            process_modules_db(&conn, &modules)
        }).await?;
        return Ok(HttpResponse::Ok().json(result));
    } else if info.org.is_some() {
        let org_name = info.org.clone().unwrap();
        let result = web::block(move || {
            let modules = models::module::get_by_technical_name_odoo_version_organization_name(
                &conn,
                &module_name,
                &version_odoo,
                &org_name,
            );
            process_modules_db(&conn, &modules)
        })
        .await?;
        return Ok(HttpResponse::Ok().json(result));
    } else if info.repo.is_some() {
        let repo_name = info.repo.clone().unwrap();
        let result = web::block(move || {
            let modules = models::module::get_by_technical_name_odoo_version_repository_name(
                &conn,
                &module_name,
                &version_odoo,
                &repo_name,
            );
            process_modules_db(&conn, &modules)
        })
        .await?;
        return Ok(HttpResponse::Ok().json(result));
    }
    let result = web::block(move || {
        let modules = models::module::get_by_technical_name_odoo_version(
            &conn,
            &[module_name],
            &version_odoo,
        );
        process_modules_db(&conn, &modules)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}
