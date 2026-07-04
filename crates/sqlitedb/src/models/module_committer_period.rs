// Copyright Alexandre D. Díaz
use std::collections::HashMap;

use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::module_committer_period;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module_committer_period, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_id: i64,
    pub committer_id: i64,
    pub year: i32,
    pub month: i32,
    pub commits: i32,
}

#[derive(Insertable)]
#[diesel(table_name = module_committer_period)]
struct NewModuleCommitterPeriod {
    module_id: i64,
    committer_id: i64,
    year: i32,
    month: i32,
    commits: i32,
}

/// Replaces every period row for this (module, committer) pair with `periods`.
/// The collector recomputes the full breakdown from `git log` on every run, so
/// delete+insert is simpler than diffing and keeps this in sync with
/// `module_committer::add`, which is called with the same data.
pub fn replace_for_committer(
    conn: &mut SqliteConnection,
    module_id: &i64,
    committer_id: &i64,
    periods: &HashMap<(i32, i32), u32>,
) -> QueryResult<()> {
    diesel::delete(
        module_committer_period::table.filter(
            module_committer_period::module_id
                .eq(module_id)
                .and(module_committer_period::committer_id.eq(committer_id)),
        ),
    )
    .execute(conn)?;

    let new_rows: Vec<NewModuleCommitterPeriod> = periods
        .iter()
        .map(|(&(year, month), &commits)| NewModuleCommitterPeriod {
            module_id: *module_id,
            committer_id: *committer_id,
            year,
            month,
            commits: commits as i32,
        })
        .collect();

    if !new_rows.is_empty() {
        diesel::insert_into(module_committer_period::table)
            .values(&new_rows)
            .execute(conn)?;
    }

    Ok(())
}

// Bots/automation accounts excluded so rankings reflect human contributors
// (kept in sync with committer::rank_global's exclusion list).
const BOT_COMMITTERS: &str = "'Odoo Translation Bot', 'OCA-git-bot', 'Weblate', 'oca-ci'";

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct PeriodRankEntry {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub name: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub total_commits: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub rank: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub total_committers: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub modules_touched: i64,
}

/// Ranks committers by commits within a period. `month` is only applied when
/// `year` is also set (a lone month with no year would be meaningless).
pub fn rank_by_period(
    conn: &mut SqliteConnection,
    year: i32,
    month: Option<i32>,
    limit: i64,
) -> Vec<PeriodRankEntry> {
    match month {
        Some(m) => diesel::sql_query(format!(
            "SELECT com.name as name, SUM(mcp.commits) as total_commits, \
                    RANK() OVER (ORDER BY SUM(mcp.commits) DESC) as rank, \
                    COUNT(*) OVER () as total_committers, \
                    COUNT(DISTINCT mcp.module_id) as modules_touched \
             FROM module_committer_period as mcp \
             INNER JOIN committer as com ON mcp.committer_id = com.id \
             WHERE com.name NOT IN ({BOT_COMMITTERS}) AND mcp.year = ? AND mcp.month = ? \
             GROUP BY com.id \
             ORDER BY total_commits DESC \
             LIMIT ?"
        ))
        .bind::<diesel::sql_types::Integer, _>(year)
        .bind::<diesel::sql_types::Integer, _>(m)
        .bind::<diesel::sql_types::BigInt, _>(limit)
        .load::<PeriodRankEntry>(conn)
        .expect("DB error in module_committer_period::rank_by_period"),
        None => diesel::sql_query(format!(
            "SELECT com.name as name, SUM(mcp.commits) as total_commits, \
                    RANK() OVER (ORDER BY SUM(mcp.commits) DESC) as rank, \
                    COUNT(*) OVER () as total_committers, \
                    COUNT(DISTINCT mcp.module_id) as modules_touched \
             FROM module_committer_period as mcp \
             INNER JOIN committer as com ON mcp.committer_id = com.id \
             WHERE com.name NOT IN ({BOT_COMMITTERS}) AND mcp.year = ? \
             GROUP BY com.id \
             ORDER BY total_commits DESC \
             LIMIT ?"
        ))
        .bind::<diesel::sql_types::Integer, _>(year)
        .bind::<diesel::sql_types::BigInt, _>(limit)
        .load::<PeriodRankEntry>(conn)
        .expect("DB error in module_committer_period::rank_by_period"),
    }
}
