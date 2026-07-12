// Copyright Alexandre D. Díaz
use actix_web::{get, HttpRequest, Responder, Result};
use minijinja::context;

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

// Static page: favorites/packs data lives only in the browser's
// localStorage (see web/js/utils/favorites-store.mjs), so there is nothing
// to fetch from the DB here.
#[get("/favorites")]
pub async fn route(tmpl_env: MiniJinjaRenderer, req: HttpRequest) -> Result<impl Responder> {
    tmpl_env.render(
        "pages/favorites.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "favorites"
            )
        ),
    )
}
