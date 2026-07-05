// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::gh_repository;
use crate::utils::date::get_sqlite_utc_now;

use super::{gh_organization, system_event};

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = gh_repository, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub name: String,
    pub gh_organization_id: i64,
    pub create_date: String,
    pub update_date: String,
}

#[derive(Insertable)]
#[diesel(table_name = gh_repository)]
struct NewGhRepository<'a> {
    name: &'a str,
    gh_organization_id: i64,
    create_date: &'a str,
    update_date: &'a str,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct RepositoryInfo {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub organization: String,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub num_modules: i32,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
}

pub fn get_by_id(conn: &mut SqliteConnection, id: &i64) -> Option<Model> {
    gh_repository::table
        .filter(gh_repository::id.eq(id))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in gh_repository::get_by_id")
}

pub fn get_by_name(conn: &mut SqliteConnection, gh_org_id: &i64, name: &str) -> Option<Model> {
    gh_repository::table
        .filter(
            gh_repository::gh_organization_id
                .eq(gh_org_id)
                .and(gh_repository::name.eq(name)),
        )
        .first::<Model>(conn)
        .optional()
        .expect("DB error in gh_repository::get_by_name")
}

pub fn get_info_by_name(conn: &mut SqliteConnection, repo_name: &str) -> Vec<RepositoryInfo> {
    diesel::sql_query(
        "SELECT gh_repo.name, gh_org.name as organization, count(mod.id) as num_modules, \
         mod.version_odoo \
         FROM gh_repository as gh_repo \
         INNER JOIN gh_organization as gh_org ON gh_org.id = gh_repo.gh_organization_id \
         INNER JOIN module as mod ON mod.gh_repository_id = gh_repo.id \
         WHERE gh_repo.name = ? \
         GROUP BY gh_org.id, mod.version_odoo",
    )
    .bind::<diesel::sql_types::Text, _>(repo_name)
    .load::<RepositoryInfo>(conn)
    .expect("DB error in gh_repository::get_info_by_name")
}

/// Like `get_info_by_name`, but a substring (SQL LIKE) match intended for
/// discovery (e.g. "spain" -> "l10n-spain") rather than an exact lookup.
/// Grouped by repository id (not just org id) since several repositories
/// across different organizations can match the same substring.
pub fn search_by_name(conn: &mut SqliteConnection, name_substr: &str) -> Vec<RepositoryInfo> {
    diesel::sql_query(
        "SELECT gh_repo.name, gh_org.name as organization, count(mod.id) as num_modules, \
         mod.version_odoo \
         FROM gh_repository as gh_repo \
         INNER JOIN gh_organization as gh_org ON gh_org.id = gh_repo.gh_organization_id \
         INNER JOIN module as mod ON mod.gh_repository_id = gh_repo.id \
         WHERE gh_repo.name LIKE ? \
         GROUP BY gh_repo.id, mod.version_odoo \
         ORDER BY gh_org.name, gh_repo.name",
    )
    .bind::<diesel::sql_types::Text, _>(format!("%{name_substr}%"))
    .load::<RepositoryInfo>(conn)
    .expect("DB error in gh_repository::search_by_name")
}

pub fn add(conn: &mut SqliteConnection, gh_org_id: &i64, name: &str) -> QueryResult<Model> {
    let create_date = get_sqlite_utc_now();
    let inserted = diesel::insert_into(gh_repository::table)
        .values(NewGhRepository {
            name,
            gh_organization_id: *gh_org_id,
            create_date: &create_date,
            update_date: &create_date,
        })
        .on_conflict((gh_repository::name, gh_repository::gh_organization_id))
        .do_nothing()
        .execute(conn)?;

    if inserted == 0 {
        gh_repository::table
            .filter(
                gh_repository::gh_organization_id
                    .eq(gh_org_id)
                    .and(gh_repository::name.eq(name)),
            )
            .first::<Model>(conn)
    } else {
        let id = crate::models::last_insert_rowid(conn);
        let org = gh_organization::get_by_id(conn, gh_org_id).unwrap();
        let _ = system_event::register_new_gh_repository(conn, &org.name, name);
        Ok(Model {
            id,
            name: name.to_string(),
            gh_organization_id: *gh_org_id,
            create_date: create_date.clone(),
            update_date: create_date,
        })
    }
}
