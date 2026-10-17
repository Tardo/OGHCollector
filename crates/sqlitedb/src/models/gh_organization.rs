// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::gh_organization;

use super::system_event;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = gh_organization, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub name: String,
}

#[derive(Insertable)]
#[diesel(table_name = gh_organization)]
struct NewGhOrganization<'a> {
    name: &'a str,
}

pub fn get_by_id(conn: &mut SqliteConnection, id: &i64) -> Option<Model> {
    gh_organization::table
        .filter(gh_organization::id.eq(id))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in gh_organization::get_by_id")
}

pub fn get_by_name(conn: &mut SqliteConnection, name: &str) -> Option<Model> {
    gh_organization::table
        .filter(gh_organization::name.eq(name))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in gh_organization::get_by_name")
}

pub fn count(conn: &mut SqliteConnection) -> i64 {
    gh_organization::table
        .count()
        .get_result(conn)
        .expect("DB error in gh_organization::count")
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct OrganizationSummary {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub name: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub repositories: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub modules: i64,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub min_version: i32,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub max_version: i32,
}

/// Organizations with modules, the largest first.
pub fn summary(conn: &mut SqliteConnection) -> Vec<OrganizationSummary> {
    diesel::sql_query(
        "SELECT org.name, count(DISTINCT repo.id) as repositories, \
         count(DISTINCT mod.technical_name) as modules, \
         MIN(mod.version_odoo) as min_version, MAX(mod.version_odoo) as max_version \
         FROM gh_organization as org \
         INNER JOIN gh_repository as repo ON repo.gh_organization_id = org.id \
         INNER JOIN module as mod ON mod.gh_repository_id = repo.id \
         GROUP BY org.id ORDER BY modules DESC",
    )
    .load::<OrganizationSummary>(conn)
    .expect("DB error in gh_organization::summary")
}

pub fn add(conn: &mut SqliteConnection, name: &str) -> QueryResult<Model> {
    let inserted = diesel::insert_into(gh_organization::table)
        .values(NewGhOrganization { name })
        .on_conflict(gh_organization::name)
        .do_nothing()
        .execute(conn)?;

    if inserted == 0 {
        gh_organization::table
            .filter(gh_organization::name.eq(name))
            .first::<Model>(conn)
    } else {
        let id = crate::models::last_insert_rowid(conn);
        let _ = system_event::register_new_gh_organization(conn, name);
        Ok(Model {
            id,
            name: name.to_string(),
        })
    }
}
