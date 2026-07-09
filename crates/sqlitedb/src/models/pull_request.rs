// Copyright Alexandre D. Díaz
use chrono::NaiveDateTime;
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::pull_request;
use crate::utils::date::get_sqlite_utc_now;

use super::{gh_repository, pull_request_history, system_event};

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = pull_request, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub name: String,
    pub version_odoo: i32,
    pub module_technical_name: String,
    pub prid: i64,
    pub gh_repository_id: i64,
    pub created_at: Option<String>,
    pub ci_status: Option<String>,
}

#[derive(Insertable)]
#[diesel(table_name = pull_request)]
struct NewPullRequest<'a> {
    name: &'a str,
    version_odoo: i32,
    module_technical_name: &'a str,
    prid: i64,
    gh_repository_id: i64,
    created_at: Option<&'a str>,
    ci_status: Option<&'a str>,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct PullRequestFullInfo {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub name: String,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub module_technical_name: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub prid: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub repository_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub org_name: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    pub created_at: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    pub ci_status: Option<String>,
}

/// Every open migration PR/MR tracked, across all orgs/repos - for the
/// site-wide modules overview page (unlike the other getters here, which
/// scope to one module or one org).
pub fn get_all(conn: &mut SqliteConnection) -> Vec<PullRequestFullInfo> {
    diesel::sql_query(
        "SELECT pr.name, pr.version_odoo, pr.module_technical_name, pr.prid, \
         gh_repo.name as repository_name, gh_org.name as org_name, \
         pr.created_at, pr.ci_status \
         FROM pull_request as pr \
         INNER JOIN gh_repository as gh_repo ON pr.gh_repository_id = gh_repo.id \
         INNER JOIN gh_organization as gh_org ON gh_repo.gh_organization_id = gh_org.id \
         ORDER BY gh_org.name ASC, pr.module_technical_name ASC, pr.version_odoo DESC",
    )
    .load::<PullRequestFullInfo>(conn)
    .expect("DB error in pull_request::get_all")
}

/// Days since the PR/MR was opened, for staleness display in the PR lists.
/// `created_at` is stored as `%Y-%m-%d %H:%M:%S` (see `utils::date`); rows
/// inserted before that column existed have it as `None` until the next
/// collector run refreshes them.
pub fn age_days(created_at: Option<&str>) -> Option<i64> {
    let dt = NaiveDateTime::parse_from_str(created_at?, "%Y-%m-%d %H:%M:%S").ok()?;
    Some((chrono::Utc::now().naive_utc() - dt).num_days())
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
/// name can change if the PR is retargeted or renamed while still open, and
/// `ci_status` is expected to change across collector runs as checks complete).
#[allow(clippy::too_many_arguments)]
pub fn add(
    conn: &mut SqliteConnection,
    name: &str,
    module_technical_name: &str,
    prid: &i64,
    version_odoo: &u8,
    gh_repo_id: &i64,
    created_at: Option<&str>,
    ci_status: Option<&str>,
) -> QueryResult<Model> {
    let is_new = pull_request::table
        .filter(
            pull_request::gh_repository_id
                .eq(gh_repo_id)
                .and(pull_request::prid.eq(prid)),
        )
        .first::<Model>(conn)
        .optional()?
        .is_none();

    diesel::insert_into(pull_request::table)
        .values(NewPullRequest {
            name,
            version_odoo: *version_odoo as i32,
            module_technical_name,
            prid: *prid,
            gh_repository_id: *gh_repo_id,
            created_at,
            ci_status,
        })
        .on_conflict((pull_request::gh_repository_id, pull_request::prid))
        .do_update()
        .set((
            pull_request::name.eq(name),
            pull_request::version_odoo.eq(*version_odoo as i32),
            pull_request::module_technical_name.eq(module_technical_name),
            pull_request::created_at.eq(created_at),
            pull_request::ci_status.eq(ci_status),
        ))
        .execute(conn)?;

    let result = pull_request::table
        .filter(
            pull_request::gh_repository_id
                .eq(gh_repo_id)
                .and(pull_request::prid.eq(prid)),
        )
        .first::<Model>(conn)?;

    if is_new {
        let repo_name = gh_repository::get_by_id(conn, gh_repo_id)
            .map(|r| r.name)
            .unwrap_or_default();
        let _ = system_event::register_new_migration_pr(
            conn,
            module_technical_name,
            name,
            prid,
            &repo_name,
            version_odoo,
        );
    }

    Ok(result)
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
    let base_filter = pull_request::gh_repository_id
        .eq(gh_repo_id)
        .and(pull_request::version_odoo.eq(*version_odoo as i32));

    let removed: Vec<Model> = if prids.is_empty() {
        pull_request::table
            .filter(base_filter)
            .load::<Model>(conn)?
    } else {
        pull_request::table
            .filter(base_filter.and(pull_request::prid.ne_all(prids)))
            .load::<Model>(conn)?
    };

    if removed.is_empty() {
        return Ok(0);
    }

    let repo_name = gh_repository::get_by_id(conn, gh_repo_id)
        .map(|r| r.name)
        .unwrap_or_default();
    let closed_at = get_sqlite_utc_now();
    for pr in &removed {
        let _ = system_event::register_closed_migration_pr(
            conn,
            &pr.module_technical_name,
            &pr.name,
            &pr.prid,
            &repo_name,
            version_odoo,
        );
        // Only rows with a known open date are useful for the acceptance-time
        // stat; PRs collected before the `created_at` migration don't have one.
        if let Some(created_at) = &pr.created_at {
            let _ = pull_request_history::add(
                conn,
                &pr.module_technical_name,
                pr.version_odoo,
                *gh_repo_id,
                pr.prid,
                created_at,
                &closed_at,
            );
        }
    }

    if prids.is_empty() {
        diesel::delete(pull_request::table.filter(base_filter)).execute(conn)
    } else {
        diesel::delete(
            pull_request::table.filter(base_filter.and(pull_request::prid.ne_all(prids))),
        )
        .execute(conn)
    }
}

#[cfg(test)]
mod tests {
    use super::age_days;

    #[test]
    fn test_age_days_computes_days_since_creation() {
        let ten_days_ago = (chrono::Utc::now() - chrono::Duration::days(10))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        assert_eq!(age_days(Some(&ten_days_ago)), Some(10));
    }

    #[test]
    fn test_age_days_none_for_missing_or_bad_input() {
        assert_eq!(age_days(None), None);
        assert_eq!(age_days(Some("not a date")), None);
    }
}
