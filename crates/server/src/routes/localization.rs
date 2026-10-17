// Copyright Alexandre D. Díaz
use actix_web::{get, web, HttpRequest, HttpResponse, Result};
use minijinja::context;
use oghutils::version::odoo_version_u8_to_string;
use serde::Deserialize;
use sqlitedb::models;

use crate::workspace::AreaPool;

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

#[derive(Deserialize)]
pub struct LocalizationQuery {
    version: Option<String>,
}

/// The globe: countries with `l10n_XX` modules, per Odoo version.
#[get("/localization")]
pub async fn route_index(
    pool: AreaPool,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<HttpResponse> {
    let (countries, versions) = web::block(move || {
        let mut conn = pool.get().unwrap();
        (
            models::module::localization_countries_by_version(&mut conn),
            models::module::get_odoo_versions(&mut conn),
        )
    })
    .await?;
    // Newest first: the selector's default.
    let default_version = versions.first().copied();
    let versions: Vec<_> = versions
        .iter()
        .map(|v| odoo_version_u8_to_string(&(*v as u8)))
        .collect();
    let rendered = tmpl_env.render(
        "pages/localizations.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "localization",
                localization_countries => countries,
                localization_versions => versions,
                localization_default_version => default_version,
            )
        ),
    )?;
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(rendered.0))
}

/// One country's modules (`partials/localization_modules.html` context), for
/// its page and the globe's in-page list. `None` = no such country: 404.
async fn country_modules(
    pool: AreaPool,
    code: String,
    version: Option<&str>,
) -> Result<Option<Vec<minijinja::Value>>> {
    if code.len() != 2 || !code.bytes().all(|b| b.is_ascii_lowercase()) {
        return Ok(None);
    }
    let version = version
        .map(crate::utils::odoo_version)
        .transpose()?
        .map(i32::from);
    let modules = web::block(move || {
        let mut conn = pool.get().unwrap();
        models::module::list_localization(&mut conn, &code, version)
    })
    .await?;
    if modules.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        modules
            .into_iter()
            .map(|module| {
                let versions: Vec<_> = module
                    .versions_odoo
                    .iter()
                    .map(|version| odoo_version_u8_to_string(&(*version as u8)))
                    .collect();
                context!(
                    technical_name => module.technical_name,
                    name => module.name,
                    org_name => module.org_name,
                    versions => versions,
                )
            })
            .collect(),
    ))
}

#[get("/localization/{code}")]
pub async fn route(
    pool: AreaPool,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    path: web::Path<String>,
    query: web::Query<LocalizationQuery>,
) -> Result<HttpResponse> {
    let code = path.into_inner();
    let Some(modules) = country_modules(pool, code.clone(), query.version.as_deref()).await? else {
        return Ok(HttpResponse::NotFound().finish());
    };
    let rendered = tmpl_env.render(
        "pages/localization.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "localization",
                country_code => code.to_uppercase(),
                selected_version => query.version,
                modules => modules,
            )
        ),
    )?;
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(rendered.0))
}

/// Just the module list, fetched by the globe page when a country is chosen.
#[get("/localization/{code}/modules")]
pub async fn route_modules(
    pool: AreaPool,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    path: web::Path<String>,
    query: web::Query<LocalizationQuery>,
) -> Result<HttpResponse> {
    let Some(modules) = country_modules(pool, path.into_inner(), query.version.as_deref()).await?
    else {
        return Ok(HttpResponse::NotFound().finish());
    };
    let rendered = tmpl_env.render(
        "partials/localization_modules.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(modules => modules)
        ),
    )?;
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(rendered.0))
}
