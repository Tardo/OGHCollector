// Copyright Alexandre D. Díaz
use actix_web::{get, HttpRequest, Responder, Result};
use minijinja::context;

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

#[get("/api")]
pub async fn route(tmpl_env: MiniJinjaRenderer, req: HttpRequest) -> Result<impl Responder> {
    tmpl_env.render(
        "pages/api_doc.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "api"
            )
        ),
    )
}
