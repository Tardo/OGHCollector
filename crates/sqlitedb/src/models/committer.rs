// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::models::BOT_COMMITTERS;
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

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct GlobalRank {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub rank: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub total_commits: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub total_committers: i64,
}

pub fn get_global_rank_by_name(conn: &mut SqliteConnection, name: &str) -> Option<GlobalRank> {
    diesel::sql_query(format!(
        "SELECT rank, total_commits, total_committers FROM (\
           SELECT com.name as committer_name, SUM(mod_com.commits) as total_commits, \
                  RANK() OVER (ORDER BY SUM(mod_com.commits) DESC) as rank, \
                  COUNT(*) OVER () as total_committers \
           FROM module_committer as mod_com \
           INNER JOIN committer as com ON mod_com.committer_id = com.id \
           WHERE com.name NOT IN ({BOT_COMMITTERS}) \
           GROUP BY com.id \
         ) WHERE committer_name = ?"
    ))
    .bind::<diesel::sql_types::Text, _>(name)
    .load::<GlobalRank>(conn)
    .expect("DB error in committer::get_global_rank_by_name")
    .into_iter()
    .next()
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct GlobalRankEntry {
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

pub fn rank_global(conn: &mut SqliteConnection, limit: i64) -> Vec<GlobalRankEntry> {
    diesel::sql_query(format!(
        "SELECT com.name as name, SUM(mod_com.commits) as total_commits, \
                RANK() OVER (ORDER BY SUM(mod_com.commits) DESC) as rank, \
                COUNT(*) OVER () as total_committers, \
                COUNT(DISTINCT mod_com.module_id) as modules_touched \
         FROM module_committer as mod_com \
         INNER JOIN committer as com ON mod_com.committer_id = com.id \
         WHERE com.name NOT IN ({BOT_COMMITTERS}) \
         GROUP BY com.id \
         ORDER BY total_commits DESC \
         LIMIT ?"
    ))
    .bind::<diesel::sql_types::BigInt, _>(limit)
    .load::<GlobalRankEntry>(conn)
    .expect("DB error in committer::rank_global")
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
