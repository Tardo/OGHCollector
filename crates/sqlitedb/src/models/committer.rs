// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::models::HUMAN_COMMITTER;
use crate::schema::committer;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = committer, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub name: String,
}

#[derive(Insertable)]
#[diesel(table_name = committer)]
struct NewCommitter<'a> {
    name: &'a str,
}

pub fn get_by_id(conn: &mut SqliteConnection, id: &i64) -> Option<Model> {
    committer::table
        .filter(committer::id.eq(id))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in committer::get_by_id")
}

pub fn get_by_name(conn: &mut SqliteConnection, name: &str) -> Option<Model> {
    committer::table
        .filter(committer::name.eq(name))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in committer::get_by_name")
}

#[derive(QueryableByName, Debug, Serialize)]
pub struct CommitterSummary {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub name: String,
    /// Not a bot (`HUMAN_COMMITTER`).
    #[diesel(sql_type = diesel::sql_types::Bool)]
    pub human: bool,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub commits: i64,
    /// Distinct technical names, whatever the Odoo version.
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub modules: i64,
    /// First and last month with commits, as `year * 12 + month - 1`.
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub first_month: i32,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub last_month: i32,
}

/// Every committer with dated activity, bots included (flagged), in one pass:
/// the committers page ranks, filters and totals these rows in Rust instead of
/// running one grouped scan per list.
pub fn activity_summary(conn: &mut SqliteConnection) -> Vec<CommitterSummary> {
    diesel::sql_query(format!(
        "SELECT com.name, {HUMAN_COMMITTER} AS human, SUM(mcp.commits) AS commits, \
         COUNT(DISTINCT mod.technical_name) AS modules, \
         MIN(mcp.year * 12 + mcp.month - 1) AS first_month, \
         MAX(mcp.year * 12 + mcp.month - 1) AS last_month \
         FROM module_committer_period AS mcp \
         JOIN committer AS com ON com.id = mcp.committer_id \
         JOIN module AS mod ON mod.id = mcp.module_id \
         GROUP BY com.id"
    ))
    .load::<CommitterSummary>(conn)
    .expect("DB error in committer::activity_summary")
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct CommitterListInfo {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub name: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub total_commits: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub modules_touched: i64,
}

pub fn list(conn: &mut SqliteConnection) -> Vec<CommitterListInfo> {
    diesel::sql_query(
        "SELECT com.name as name, SUM(mc.commits) as total_commits, \
         COUNT(DISTINCT mc.module_id) as modules_touched \
         FROM committer as com \
         INNER JOIN module_committer as mc ON mc.committer_id = com.id \
         INNER JOIN module as mod ON mod.id = mc.module_id \
         GROUP BY com.id \
         ORDER BY total_commits DESC",
    )
    .load::<CommitterListInfo>(conn)
    .expect("DB error in committer::list")
}

pub fn add(conn: &mut SqliteConnection, name: &str) -> QueryResult<Model> {
    let inserted = diesel::insert_into(committer::table)
        .values(NewCommitter { name })
        .on_conflict(committer::name)
        .do_nothing()
        .execute(conn)?;

    if inserted == 0 {
        committer::table
            .filter(committer::name.eq(name))
            .first::<Model>(conn)
    } else {
        let id = crate::models::last_insert_rowid(conn);
        Ok(Model {
            id,
            name: name.to_string(),
        })
    }
}
