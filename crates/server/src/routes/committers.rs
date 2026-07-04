// Copyright Alexandre D. Díaz
use actix_web::{get, web, HttpRequest, Responder, Result};
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use sqlitedb::{models, Pool};

const TOP_LIMIT: i64 = 20;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TopCommitterInfo {
    pub rank: i64,
    pub name: String,
    pub total_commits: i64,
    pub modules_touched: i64,
}

#[get("/committers")]
pub async fn route(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    pool: web::Data<Pool>,
) -> Result<impl Responder> {
    let (top_committers, total_committers) = web::block(move || {
        let mut conn = pool.get().unwrap();
        let entries = models::committer::rank_global(&mut conn, TOP_LIMIT);
        let total_committers = entries.first().map(|e| e.total_committers).unwrap_or(0);
        let top_committers: Vec<TopCommitterInfo> = entries
            .into_iter()
            .map(|e| TopCommitterInfo {
                rank: e.rank,
                name: e.name,
                total_commits: e.total_commits,
                modules_touched: e.modules_touched,
            })
            .collect();
        (top_committers, total_committers)
    })
    .await?;

    tmpl_env.render(
        "pages/committers.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "committers",
                top_committers => top_committers,
                total_committers => total_committers,
            )
        ),
    )
}
