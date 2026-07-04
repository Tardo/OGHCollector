// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::pull_request;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = pull_request, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub name: String,
    pub version_odoo: i32,
    pub module_technical_name: String,
    pub prid: i64,
    pub gh_repository_id: i64,
}

#[derive(Insertable)]
#[diesel(table_name = pull_request)]
struct NewPullRequest<'a> {
    name: &'a str,
    version_odoo: i32,
    module_technical_name: &'a str,
    prid: i64,
    gh_repository_id: i64,
}

pub fn get_by_id(conn: &mut SqliteConnection, id: &i64) -> Option<Model> {
    pull_request::table
        .filter(pull_request::id.eq(id))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in pull_request::get_by_id")
}

/// Batch lookup used by the migration plan tool: finds open migration PRs for a
/// set of modules at a given target version, regardless of organization (the
/// caller doesn't know in advance which org/repo a not-yet-merged module lives in).
pub fn get_by_technical_names_odoo_version(
    conn: &mut SqliteConnection,
    technical_names: &[String],
    version_odoo: &u8,
) -> Vec<Model> {
    if technical_names.is_empty() {
        return vec![];
    }
    pull_request::table
        .filter(
            pull_request::module_technical_name
                .eq_any(technical_names)
                .and(pull_request::version_odoo.eq(*version_odoo as i32)),
        )
        .load::<Model>(conn)
        .expect("DB error in pull_request::get_by_technical_names_odoo_version")
}

pub fn get_by_technical_name_organization_name(
    conn: &mut SqliteConnection,
    technical_name: &str,
    org_name: &str,
) -> Vec<Model> {
    use crate::schema::{gh_organization, gh_repository};
    pull_request::table
        .inner_join(gh_repository::table.on(gh_repository::id.eq(pull_request::gh_repository_id)))
        .inner_join(
            gh_organization::table.on(gh_organization::id.eq(gh_repository::gh_organization_id)),
        )
        .filter(
            pull_request::module_technical_name
                .eq(technical_name)
                .and(gh_organization::name.eq(org_name)),
        )
        .select(Model::as_select())
        .load::<Model>(conn)
        .expect("DB error in pull_request::get_by_technical_name_organization_name")
}

/// Inserts an open migration PR/MR, or refreshes it if already tracked (title/module
/// name can change if the PR is retargeted or renamed while still open).
pub fn add(
    conn: &mut SqliteConnection,
    name: &str,
    module_technical_name: &str,
    prid: &i64,
    version_odoo: &u8,
    gh_repo_id: &i64,
) -> QueryResult<Model> {
    diesel::insert_into(pull_request::table)
        .values(NewPullRequest {
            name,
            version_odoo: *version_odoo as i32,
            module_technical_name,
            prid: *prid,
            gh_repository_id: *gh_repo_id,
        })
        .on_conflict((pull_request::gh_repository_id, pull_request::prid))
        .do_update()
        .set((
            pull_request::name.eq(name),
            pull_request::version_odoo.eq(*version_odoo as i32),
            pull_request::module_technical_name.eq(module_technical_name),
        ))
        .execute(conn)?;

    pull_request::table
        .filter(
            pull_request::gh_repository_id
                .eq(gh_repo_id)
                .and(pull_request::prid.eq(prid)),
        )
        .first::<Model>(conn)
}

/// Removes PRs that are no longer open (merged/closed/renamed away from the
/// migration convention) for a given repo/version.
///
/// Unlike `module::delete_outdated`, an empty `prids` list is a valid terminal
/// state here (every previously open migration PR got merged or closed) and
/// must still clear out the stale rows rather than skip the delete.
pub fn delete_outdated(
    conn: &mut SqliteConnection,
    gh_repo_id: &i64,
    version_odoo: &u8,
    prids: &[i64],
) -> QueryResult<usize> {
    if prids.is_empty() {
        return diesel::delete(
            pull_request::table.filter(
                pull_request::gh_repository_id
                    .eq(gh_repo_id)
                    .and(pull_request::version_odoo.eq(*version_odoo as i32)),
            ),
        )
        .execute(conn);
    }
    diesel::delete(
        pull_request::table.filter(
            pull_request::gh_repository_id
                .eq(gh_repo_id)
                .and(pull_request::version_odoo.eq(*version_odoo as i32))
                .and(pull_request::prid.ne_all(prids)),
        ),
    )
    .execute(conn)
}
