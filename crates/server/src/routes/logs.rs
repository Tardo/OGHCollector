// Copyright Alexandre D. Díaz
use actix_web::{get, web, Error as AWError, HttpRequest, HttpResponse, Responder, Result};
use minijinja::context;
use serde::Deserialize;

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use sqlitedb::{models, Pool};

const DEFAULT_PAGE_SIZE: i64 = 200;
const MAX_PAGE_SIZE: i64 = 500;

#[derive(Debug, Deserialize)]
pub struct RouteLogsDataRequest {
    before_id: Option<i64>,
    date_from: Option<String>,
    date_to: Option<String>,
    limit: Option<i64>,
}

#[get("/logs/data")]
pub async fn route_data(
    pool: web::Data<Pool>,
    info: web::Query<RouteLogsDataRequest>,
) -> Result<HttpResponse, AWError> {
    let before_id = info.before_id.unwrap_or(i64::MAX);
    let limit = info
        .limit
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let date_from = info.date_from.clone();
    let date_to = info.date_to.clone();
    let logs = web::block(move || {
        let mut conn = pool.get().unwrap();
        models::system_event::get_messages_page(
            &mut conn,
            before_id,
            date_from.as_deref(),
            date_to.as_deref(),
            limit,
        )
    })
    .await?;
    // Live paginated data: the site-wide `Cache-Control: public, max-age=...`
    // default (see main.rs) must not apply here, or a browser/proxy cache
    // would keep serving the first page's snapshot instead of current logs.
    Ok(HttpResponse::Ok()
        .insert_header(("Cache-Control", "no-store"))
        .json(logs))
}

#[get("/logs")]
pub async fn route(tmpl_env: MiniJinjaRenderer, req: HttpRequest) -> Result<impl Responder> {
    let html = tmpl_env.render(
        "pages/logs.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(page_name => "logs")
        ),
    )?;
    Ok(HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .insert_header(("Cache-Control", "no-store"))
        .body(html.0))
}
