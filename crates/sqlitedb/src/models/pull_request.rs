// Copyright Alexandre D. Díaz
use chrono::NaiveDateTime;
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::{pull_request, pull_request_module};
use crate::utils::date::get_sqlite_utc_now;

use super::pull_request_module as pr_module;
use super::{gh_repository, pull_request_history, system_event};

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = pull_request, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub name: String,
    pub version_odoo: i32,
    pub prid: i64,
    pub gh_repository_id: i64,
    pub created_at: Option<String>,
    pub ci_status: Option<String>,
    pub last_message_at: Option<String>,
    pub head_sha: Option<String>,
    pub author: Option<String>,
    pub is_draft: bool,
    /// JSON array of label names.
    pub labels: Option<String>,
    pub analyzed_sha: Option<String>,
    pub analysis_version: i32,
}

impl Model {
    pub fn label_names(&self) -> Vec<String> {
        self.labels
            .as_deref()
            .and_then(|l| serde_json::from_str(l).ok())
            .unwrap_or_default()
    }

    /// Labels saying a maintainer approved it (OCA: "approved", "ready to merge").
    pub fn is_approved(&self) -> bool {
        self.label_names().iter().any(|l| {
            let l = l.to_lowercase();
            l.contains("approved") || l.contains("ready to merge")
        })
    }

    /// Sort key of the PR to pick among several (smallest first): green CI,
    /// then pending, none, failing; approved; not a draft; most recently active.
    pub fn preference_key(&self) -> (u8, bool, bool, std::cmp::Reverse<Option<String>>) {
        let ci = match self.ci_status.as_deref() {
            Some("success") => 0,
            Some("pending") => 1,
            None => 2,
            _ => 3,
        };
        (
            ci,
            !self.is_approved(),
            self.is_draft,
            std::cmp::Reverse(self.last_message_at.clone()),
        )
    }
}

/// An open PR/MR as listed by the provider. `None`s overwrite: a value the
/// provider stopped reporting (CI removed) must not linger.
#[derive(Insertable, AsChangeset)]
#[diesel(table_name = pull_request, treat_none_as_null = true)]
pub struct NewPullRequest<'a> {
    pub name: &'a str,
    pub version_odoo: i32,
    pub prid: i64,
    pub gh_repository_id: i64,
    pub created_at: Option<&'a str>,
    pub ci_status: Option<&'a str>,
    pub last_message_at: Option<&'a str>,
    pub head_sha: Option<&'a str>,
    pub author: Option<&'a str>,
    pub is_draft: bool,
    pub labels: Option<&'a str>,
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
    pub repository_url: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    pub created_at: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    pub ci_status: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    pub last_message_at: Option<String>,
}

/// Every open migration PR/MR (one row per module it ports), across all
/// orgs/repos - for the
/// site-wide modules overview page (unlike the other getters here, which
/// scope to one module or one org).
pub fn get_all(conn: &mut SqliteConnection) -> Vec<PullRequestFullInfo> {
    diesel::sql_query(
        "SELECT pr.name, pr.version_odoo, pm.module_technical_name, pr.prid, \
         gh_repo.name as repository_name, gh_org.name as org_name, \
         gh_repo.url as repository_url, pr.created_at, pr.ci_status, pr.last_message_at \
         FROM pull_request as pr \
         INNER JOIN pull_request_module as pm ON pm.pull_request_id = pr.id AND pm.is_new \
         INNER JOIN gh_repository as gh_repo ON pr.gh_repository_id = gh_repo.id \
         INNER JOIN gh_organization as gh_org ON gh_repo.gh_organization_id = gh_org.id \
         ORDER BY gh_org.name ASC, pm.module_technical_name ASC, pr.version_odoo DESC",
    )
    .load::<PullRequestFullInfo>(conn)
    .expect("DB error in pull_request::get_all")
}

/// Days since a stored PR/MR date (`created_at` for age, `last_message_at`
/// for staleness), both stored as `%Y-%m-%d %H:%M:%S` (see `utils::date`);
/// rows inserted before a column existed have it as `None` until the next
/// collector run refreshes them.
pub fn days_since(date: Option<&str>) -> Option<i64> {
    let dt = NaiveDateTime::parse_from_str(date?, "%Y-%m-%d %H:%M:%S").ok()?;
    Some((chrono::Utc::now().naive_utc() - dt).num_days())
}

pub fn get_by_id(conn: &mut SqliteConnection, id: &i64) -> Option<Model> {
    pull_request::table
        .filter(pull_request::id.eq(id))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in pull_request::get_by_id")
}

/// Batch lookup used by the migration plan tool: open migration PRs (the
/// module is new to that version) for a set of modules at a given target
/// version, regardless of organization (the caller doesn't know in advance
/// which org/repo a not-yet-merged module lives in). `(module, PR)` pairs.
pub fn get_by_technical_names_odoo_version(
    conn: &mut SqliteConnection,
    technical_names: &[String],
    version_odoo: &u8,
) -> Vec<(String, Model)> {
    if technical_names.is_empty() {
        return vec![];
    }
    pull_request::table
        .inner_join(pull_request_module::table)
        .filter(
            pull_request_module::module_technical_name
                .eq_any(technical_names)
                .and(pull_request_module::is_new.eq(true))
                .and(pull_request::version_odoo.eq(*version_odoo as i32)),
        )
        .select((
            pull_request_module::module_technical_name,
            Model::as_select(),
        ))
        .load::<(String, Model)>(conn)
        .expect("DB error in pull_request::get_by_technical_names_odoo_version")
}

/// Version-agnostic: tells the doodba migration plan tool which requested
/// modules have EVER had a migration PR/MR tracked, in any target version -
/// a module can be "registered" this way even if it has never actually
/// merged in any version yet (a first-time port with only an open PR so far).
pub fn get_repository_org_by_technical_names(
    conn: &mut SqliteConnection,
    technical_names: &[String],
) -> Vec<(String, String, String)> {
    use crate::schema::{gh_organization, gh_repository};
    if technical_names.is_empty() {
        return vec![];
    }
    pull_request::table
        .inner_join(pull_request_module::table)
        .inner_join(gh_repository::table.on(gh_repository::id.eq(pull_request::gh_repository_id)))
        .inner_join(
            gh_organization::table.on(gh_organization::id.eq(gh_repository::gh_organization_id)),
        )
        .filter(
            pull_request_module::module_technical_name
                .eq_any(technical_names)
                .and(pull_request_module::is_new.eq(true)),
        )
        .select((
            pull_request_module::module_technical_name,
            gh_repository::name,
            gh_organization::name,
        ))
        .load::<(String, String, String)>(conn)
        .expect("DB error in pull_request::get_repository_org_by_technical_names")
}

/// Every open PR/MR of an org that changes the module, with that module's row
/// (migration or not, analysis).
pub fn get_by_technical_name_organization_name(
    conn: &mut SqliteConnection,
    technical_name: &str,
    org_name: &str,
) -> Vec<(Model, pr_module::Model)> {
    use crate::schema::{gh_organization, gh_repository};
    pull_request::table
        .inner_join(pull_request_module::table)
        .inner_join(gh_repository::table.on(gh_repository::id.eq(pull_request::gh_repository_id)))
        .inner_join(
            gh_organization::table.on(gh_organization::id.eq(gh_repository::gh_organization_id)),
        )
        .filter(
            pull_request_module::module_technical_name
                .eq(technical_name)
                .and(gh_organization::name.eq(org_name)),
        )
        .select((Model::as_select(), pr_module::Model::as_select()))
        .load::<(Model, pr_module::Model)>(conn)
        .expect("DB error in pull_request::get_by_technical_name_organization_name")
}

/// How many open PRs/MRs of an org change the module (the module page's tab
/// badge; the list itself loads with the tab).
pub fn count_by_technical_name_organization_name(
    conn: &mut SqliteConnection,
    technical_name: &str,
    org_name: &str,
) -> i64 {
    use crate::schema::{gh_organization, gh_repository};
    pull_request::table
        .inner_join(pull_request_module::table)
        .inner_join(gh_repository::table.on(gh_repository::id.eq(pull_request::gh_repository_id)))
        .inner_join(
            gh_organization::table.on(gh_organization::id.eq(gh_repository::gh_organization_id)),
        )
        .filter(
            pull_request_module::module_technical_name
                .eq(technical_name)
                .and(gh_organization::name.eq(org_name)),
        )
        .count()
        .get_result(conn)
        .expect("DB error in pull_request::count_by_technical_name_organization_name")
}

pub fn get_by_repository_prid(
    conn: &mut SqliteConnection,
    gh_repo_id: &i64,
    prid: &i64,
) -> QueryResult<Option<Model>> {
    pull_request::table
        .filter(
            pull_request::gh_repository_id
                .eq(gh_repo_id)
                .and(pull_request::prid.eq(prid)),
        )
        .first::<Model>(conn)
        .optional()
}

/// Odoo version (= base branch) of the open PR/MR whose head is `sha`: what a
/// CI webhook (it only names the commit) refers to.
pub fn get_version_by_head_sha(conn: &mut SqliteConnection, sha: &str) -> Option<i32> {
    pull_request::table
        .filter(pull_request::head_sha.eq(sha))
        .select(pull_request::version_odoo)
        .first::<i32>(conn)
        .optional()
        .ok()
        .flatten()
}

pub fn set_analyzed(
    conn: &mut SqliteConnection,
    id: &i64,
    sha: &str,
    analysis_version: i32,
) -> QueryResult<usize> {
    diesel::update(pull_request::table.filter(pull_request::id.eq(id)))
        .set((
            pull_request::analyzed_sha.eq(sha),
            pull_request::analysis_version.eq(analysis_version),
        ))
        .execute(conn)
}

/// Inserts an open PR/MR or refreshes it (title, CI, head, labels change
/// while it's open). Returns the stored row from before (`None` = first seen)
/// and after, for the caller to tell what changed.
pub fn add(
    conn: &mut SqliteConnection,
    pr: &NewPullRequest,
) -> QueryResult<(Option<Model>, Model)> {
    let before = get_by_repository_prid(conn, &pr.gh_repository_id, &pr.prid)?;
    diesel::insert_into(pull_request::table)
        .values(pr)
        .on_conflict((pull_request::gh_repository_id, pull_request::prid))
        .do_update()
        .set(pr)
        .execute(conn)?;
    let after = get_by_repository_prid(conn, &pr.gh_repository_id, &pr.prid)?
        .ok_or(diesel::result::Error::NotFound)?;
    Ok((before, after))
}

/// Drops PRs (and their module rows) without logging anything: still open,
/// only no longer tracked.
pub fn delete_by_ids(conn: &mut SqliteConnection, ids: &[i64]) -> QueryResult<usize> {
    pr_module::delete_by_pull_request_ids(conn, ids)?;
    diesel::delete(pull_request::table.filter(pull_request::id.eq_any(ids))).execute(conn)
}

/// PRs that would be removed by `delete_outdated` for a given repo/version -
/// a read-only lookup so the caller (which has provider API access, unlike
/// this crate) can check each one's merged status first.
pub fn find_outdated(
    conn: &mut SqliteConnection,
    gh_repo_id: &i64,
    version_odoo: &u8,
    prids: &[i64],
) -> QueryResult<Vec<Model>> {
    let base_filter = pull_request::gh_repository_id
        .eq(gh_repo_id)
        .and(pull_request::version_odoo.eq(*version_odoo as i32));
    if prids.is_empty() {
        pull_request::table.filter(base_filter).load::<Model>(conn)
    } else {
        pull_request::table
            .filter(base_filter.and(pull_request::prid.ne_all(prids)))
            .load::<Model>(conn)
    }
}

/// Removes PRs that are no longer open (merged/closed/retargeted) for a given
/// repo/version, with their module rows. `merged_prids` (a subset of the
/// outdated set, resolved by the caller via the provider API) tells merges
/// from closes; only merged migration PRs get a `pull_request_history` row
/// (the "avg. time open before merge" stat).
///
/// Unlike `module::delete_outdated`, an empty `prids` list is a valid terminal
/// state here (every previously open PR got merged or closed) and must still
/// clear out the stale rows rather than skip the delete.
pub fn delete_outdated(
    conn: &mut SqliteConnection,
    gh_repo_id: &i64,
    version_odoo: &u8,
    prids: &[i64],
    merged_prids: &[i64],
) -> QueryResult<usize> {
    let removed = find_outdated(conn, gh_repo_id, version_odoo, prids)?;

    if removed.is_empty() {
        return Ok(0);
    }

    let repo_name = gh_repository::get_by_id(conn, gh_repo_id)
        .map(|r| r.name)
        .unwrap_or_default();
    let closed_at = get_sqlite_utc_now();
    let removed_ids: Vec<i64> = removed.iter().map(|pr| pr.id).collect();
    let modules = pr_module::get_by_pull_request_ids(conn, &removed_ids)?;
    for pr in &removed {
        let merged = merged_prids.contains(&pr.prid);
        let ported: Vec<&str> = modules
            .iter()
            .filter(|m| m.pull_request_id == pr.id && m.is_new)
            .map(|m| m.module_technical_name.as_str())
            .collect();
        if ported.is_empty() {
            let _ = system_event::register_closed_pull_request(
                conn,
                &pr.name,
                &pr.prid,
                &repo_name,
                version_odoo,
                merged,
            );
        }
        for module_technical_name in ported {
            let _ = system_event::register_closed_migration_pr(
                conn,
                module_technical_name,
                &pr.name,
                &pr.prid,
                &repo_name,
                version_odoo,
                merged,
            );
            // Only merged rows with a known open date are useful for the
            // acceptance-time stat: closed-without-merge PRs would skew it
            // toward rejections instead of real acceptance time.
            if let (true, Some(created_at)) = (merged, &pr.created_at) {
                let _ = pull_request_history::add(
                    conn,
                    module_technical_name,
                    pr.version_odoo,
                    *gh_repo_id,
                    pr.prid,
                    created_at,
                    &closed_at,
                );
            }
        }
    }

    pr_module::delete_by_pull_request_ids(conn, &removed_ids)?;
    diesel::delete(pull_request::table.filter(pull_request::id.eq_any(removed_ids))).execute(conn)
}

#[cfg(test)]
mod tests {
    use super::days_since;

    #[test]
    fn test_days_since_computes_days_since_a_date() {
        let ten_days_ago = (chrono::Utc::now() - chrono::Duration::days(10))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        assert_eq!(days_since(Some(&ten_days_ago)), Some(10));
    }

    #[test]
    fn test_days_since_none_for_missing_or_bad_input() {
        assert_eq!(days_since(None), None);
        assert_eq!(days_since(Some("not a date")), None);
    }
}
