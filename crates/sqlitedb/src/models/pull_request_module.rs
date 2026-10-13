// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::pull_request_module;

/// A module an open PR/MR changes (see the `add_pull_request_module` migration).
#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = pull_request_module, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub pull_request_id: i64,
    pub module_technical_name: String,
    /// Not on the base branch: a migration/port (or a brand-new module).
    pub is_new: bool,
    /// JSON analysis of the module at the PR head, `None` when the PR changes
    /// no code of it.
    pub analysis: Option<String>,
}

#[derive(Insertable)]
#[diesel(table_name = pull_request_module)]
pub struct NewPullRequestModule<'a> {
    pub pull_request_id: i64,
    pub module_technical_name: &'a str,
    pub is_new: bool,
    pub analysis: Option<&'a str>,
}

pub fn get_by_pull_request_ids(
    conn: &mut SqliteConnection,
    ids: &[i64],
) -> QueryResult<Vec<Model>> {
    pull_request_module::table
        .filter(pull_request_module::pull_request_id.eq_any(ids))
        .load::<Model>(conn)
}

/// Replaces a PR's module rows, returning the previous ones.
pub fn replace_for_pull_request(
    conn: &mut SqliteConnection,
    pull_request_id: i64,
    rows: &[NewPullRequestModule],
) -> QueryResult<Vec<Model>> {
    let before = get_by_pull_request_ids(conn, &[pull_request_id])?;
    delete_by_pull_request_ids(conn, &[pull_request_id])?;
    diesel::insert_into(pull_request_module::table)
        .values(rows)
        .execute(conn)?;
    Ok(before)
}

/// Foreign keys are off on the app's connections: the CASCADE never fires,
/// so whoever deletes PRs deletes their rows here too.
pub fn delete_by_pull_request_ids(conn: &mut SqliteConnection, ids: &[i64]) -> QueryResult<usize> {
    diesel::delete(
        pull_request_module::table.filter(pull_request_module::pull_request_id.eq_any(ids)),
    )
    .execute(conn)
}
