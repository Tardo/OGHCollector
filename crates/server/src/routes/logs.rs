// Copyright 2025 Alexandre D. Díaz
use actix_web::{get, web, HttpRequest, Responder, Result};
use minijinja::context;

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use sqlitedb::{models, Pool};

#[get("/logs")]
pub async fn route(
    pool: web::Data<Pool>,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    let conn = web::block(move || pool.get()).await?.unwrap();

    let logs = models::system_event::get_messages_current_month(&conn);
    tmpl_env.render(
        "pages/logs.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "logs",
                logs => logs
            )
        ),
    )
}
