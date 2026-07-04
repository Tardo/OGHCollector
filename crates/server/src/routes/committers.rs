// Copyright Alexandre D. Díaz
use actix_web::{get, web, HttpRequest, Responder, Result};
use chrono::{Datelike, Utc};
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommittersPeriodGroup {
    pub key: String,
    pub label: String,
    pub committers: Vec<TopCommitterInfo>,
    pub total_committers: i64,
}

#[get("/committers")]
pub async fn route(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    pool: web::Data<Pool>,
) -> Result<impl Responder> {
    let now = Utc::now();
    let year = now.year();
    let month = now.month() as i32;

    let periods = web::block(move || {
        let mut conn = pool.get().unwrap();

        let month_entries = models::module_committer_period::rank_by_period(
            &mut conn,
            year,
            Some(month),
            TOP_LIMIT,
        );
        let year_entries =
            models::module_committer_period::rank_by_period(&mut conn, year, None, TOP_LIMIT);
        let all_entries = models::committer::rank_global(&mut conn, TOP_LIMIT);

        vec![
            CommittersPeriodGroup {
                key: "month".to_string(),
                label: "Month".to_string(),
                total_committers: month_entries
                    .first()
                    .map(|e| e.total_committers)
                    .unwrap_or(0),
                committers: month_entries
                    .into_iter()
                    .map(|e| TopCommitterInfo {
                        rank: e.rank,
                        name: e.name,
                        total_commits: e.total_commits,
                        modules_touched: e.modules_touched,
                    })
                    .collect(),
            },
            CommittersPeriodGroup {
                key: "year".to_string(),
                label: "Year".to_string(),
                total_committers: year_entries
                    .first()
                    .map(|e| e.total_committers)
                    .unwrap_or(0),
                committers: year_entries
                    .into_iter()
                    .map(|e| TopCommitterInfo {
                        rank: e.rank,
                        name: e.name,
                        total_commits: e.total_commits,
                        modules_touched: e.modules_touched,
                    })
                    .collect(),
            },
            CommittersPeriodGroup {
                key: "all".to_string(),
                label: "All time".to_string(),
                total_committers: all_entries.first().map(|e| e.total_committers).unwrap_or(0),
                committers: all_entries
                    .into_iter()
                    .map(|e| TopCommitterInfo {
                        rank: e.rank,
                        name: e.name,
                        total_commits: e.total_commits,
                        modules_touched: e.modules_touched,
                    })
                    .collect(),
            },
        ]
    })
    .await?;

    tmpl_env.render(
        "pages/committers.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "committers",
                periods => periods,
            )
        ),
    )
}
