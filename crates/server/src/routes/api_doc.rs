use minijinja::context;
use actix_web::{get, HttpRequest, Responder, Result};

use crate::utils::get_minijinja_context;
use crate::minijinja_renderer::MiniJinjaRenderer;


#[get("/api")]
pub async fn route(tmpl_env: MiniJinjaRenderer, req: HttpRequest) -> Result<impl Responder> {
    return tmpl_env.render("pages/api_doc.html", context!(
        ..get_minijinja_context(&req),
        ..context!(
            page_name => "api"
        )
    ))
}
