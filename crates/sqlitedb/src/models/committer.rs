// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

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

// Bots/automation accounts excluded so the ranking reflects human contributors
// (kept in sync with module::rank_committer's exclusion list).
pub fn get_global_rank_by_name(conn: &mut SqliteConnection, name: &str) -> Option<GlobalRank> {
    diesel::sql_query(
        "SELECT rank, total_commits, total_committers FROM (\
           SELECT com.name as committer_name, SUM(mod_com.commits) as total_commits, \
                  RANK() OVER (ORDER BY SUM(mod_com.commits) DESC) as rank, \
                  COUNT(*) OVER () as total_committers \
           FROM module_committer as mod_com \
           INNER JOIN committer as com ON mod_com.committer_id = com.id \
           WHERE com.name NOT IN \
                 ('Odoo Translation Bot', 'OCA-git-bot', 'Weblate', 'oca-ci') \
           GROUP BY com.id \
         ) WHERE committer_name = ?",
    )
    .bind::<diesel::sql_types::Text, _>(name)
    .load::<GlobalRank>(conn)
    .expect("DB error in committer::get_global_rank_by_name")
    .into_iter()
    .next()
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
