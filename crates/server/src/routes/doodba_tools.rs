// Copyright Alexandre D. Díaz
use crate::routes::api::v1::module::{process_modules_db, ModuleDependencyInfoResponse};
use actix_multipart::form::{text::Text, MultipartForm};
use actix_web::{get, post, web, Error as AWError, HttpRequest, HttpResponse, Responder, Result};
use minijinja::context;
use oghutils::version::odoo_version_string_to_u8;
use std::collections::HashMap;

use sqlitedb::{
    models::{self, module::ModuleRepositoryInfo, Connection},
    Pool,
};

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
    conn: &Connection,
    mods: &[Text<String>],
    odoo_version: &str,
) -> Vec<ModuleRepositoryInfo> {
    let odoo_ver = odoo_version_string_to_u8(odoo_version);
    let modules: Vec<String> = mods.iter().map(|x| x.as_str().to_string()).collect();
    let module_repos: Vec<ModuleRepositoryInfo> =
        models::module::get_module_repository(conn, &odoo_ver, modules.as_slice());
    module_repos
}

#[post("/doodba/converter/addons")]
pub async fn route_doodba_converter_addons(
    pool: web::Data<Pool>,
    form: MultipartForm<ConverterForm>,
) -> Result<HttpResponse, AWError> {
    let conn = web::block(move || pool.get()).await?.unwrap();
    let result =
        web::block(move || get_doodba_addons(&conn, &form.modules, &form.odoo_version)).await?;
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
    conn: &Connection,
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
    let conn = web::block(move || pool.get()).await?.unwrap();
    let result =
        web::block(move || get_doodba_addons_full(&conn, &form.modules, &form.odoo_version))
            .await?;
    Ok(HttpResponse::Ok().json(result))
}
