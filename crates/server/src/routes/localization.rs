// Copyright Alexandre D. Díaz
use actix_web::{get, web, HttpRequest, HttpResponse, Result};
use minijinja::context;
use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::{models, Pool};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

#[get("/localization/{code}")]
pub async fn route(
    pool: web::Data<Pool>,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    path: web::Path<String>,
) -> Result<HttpResponse> {
    let code = path.into_inner();
    if code.len() != 2 || !code.bytes().all(|b| b.is_ascii_lowercase()) {
        return Ok(HttpResponse::NotFound().finish());
    }
    let modules = web::block({
        let code = code.clone();
        move || {
            let mut conn = pool.get().unwrap();
            models::module::list_localization(&mut conn, &code)
        }
    })
    .await?;
    if modules.is_empty() {
        return Ok(HttpResponse::NotFound().finish());
    }
    let modules: Vec<_> = modules
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
        .collect();
    let rendered = tmpl_env.render(
        "pages/localization.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "localization",
                country_code => code.to_uppercase(),
                modules => modules,
            )
        ),
    )?;
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(rendered.0))
}
