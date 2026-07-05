// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::system_event;
use crate::utils::date::get_sqlite_utc_now;
use oghutils::version::odoo_version_u8_to_string;

use super::system_event_type;

// Allowed `severity` values. Kept as plain strings (matching the rest of this
// module's event-type-as-string convention) rather than a DB enum, since
// severity only ever drives display styling in the template.
const SEVERITY_INFO: &str = "info";
const SEVERITY_SUCCESS: &str = "success";
const SEVERITY_WARNING: &str = "warning";
const SEVERITY_ERROR: &str = "error";

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct Model {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub message: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub date: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub event_type_id: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub event_type_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub severity: String,
    #[diesel(sql_type = diesel::sql_types::Bool)]
    pub is_html: bool,
}

pub struct LogUpdateModuleInfo<'a> {
    pub module_technical_name: &'a str,
    pub module_name: &'a str,
    pub module_version: &'a str,
    pub org_name: &'a str,
    pub repo_name: &'a str,
    pub module_version_odoo: &'a str,
    pub module_changes: &'a Vec<(&'a str, &'a str, &'a str)>,
    pub last_commit_hash: &'a str,
    pub last_commit_author: &'a str,
    pub last_commit_date: &'a str,
    pub last_commit_name: &'a str,
    pub last_commit_partof: &'a str,
}

#[derive(Insertable)]
#[diesel(table_name = system_event)]
struct NewSystemEvent<'a> {
    message: &'a str,
    date: &'a str,
    event_type_id: i64,
    severity: &'a str,
    is_html: bool,
}

pub fn get_messages_current_month(conn: &mut SqliteConnection) -> Vec<Model> {
    diesel::sql_query(
        "SELECT se.message, se.date, se.event_type_id, syset.name as event_type_name, \
         se.severity, se.is_html \
         FROM system_event as se \
         INNER JOIN system_event_type as syset ON syset.id = se.event_type_id \
         WHERE date(se.date) >= date('now', 'start of month') \
           AND date(se.date) <= date('now', 'start of month', '+1 month', '-1 day') \
         ORDER BY se.date DESC, se.id DESC LIMIT 1000",
    )
    .load::<Model>(conn)
    .expect("DB error in system_event::get_messages_current_month")
}

/// Records a plain-text event. `event_type_name` is created on first use (see
/// `system_event_type::get_or_create`), so introducing a new kind of logged
/// action never requires a migration. `message` must be plain text: it is
/// rendered auto-escaped by the template, not as raw HTML.
pub fn add(
    conn: &mut SqliteConnection,
    event_type_name: &str,
    severity: &str,
    message: &str,
) -> QueryResult<Model> {
    let date = get_sqlite_utc_now();
    let event_type = system_event_type::get_or_create(conn, event_type_name);

    diesel::insert_into(system_event::table)
        .values(NewSystemEvent {
            message,
            date: &date,
            event_type_id: event_type.id,
            severity,
            is_html: false,
        })
        .execute(conn)?;

    Ok(Model {
        message: message.to_string(),
        date,
        event_type_id: event_type.id,
        event_type_name: event_type.name,
        severity: severity.to_string(),
        is_html: false,
    })
}

pub fn register_started_task_collector(
    conn: &mut SqliteConnection,
    source: &str,
    odoo_version: &str,
) -> QueryResult<Model> {
    let msg = format!("Scan started for '{source}' [{odoo_version}]");
    add(conn, "collector", SEVERITY_INFO, &msg)
}

pub fn register_new_dependency_module(
    conn: &mut SqliteConnection,
    dep_name: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!(
        "New dependency '{dep_name}' added for '{module_technical_name}' ({module_name}) [{module_version_odoo}]"
    );
    add(conn, "dependency", SEVERITY_SUCCESS, &msg)
}

pub fn register_new_gh_organization(
    conn: &mut SqliteConnection,
    org_name: &str,
) -> QueryResult<Model> {
    let msg = format!("New organization '{org_name}' added");
    add(conn, "organization", SEVERITY_SUCCESS, &msg)
}

pub fn register_new_gh_repository(
    conn: &mut SqliteConnection,
    org_name: &str,
    repo_name: &str,
) -> QueryResult<Model> {
    let msg = format!("New repository '{org_name}/{repo_name}' added");
    add(conn, "repository", SEVERITY_SUCCESS, &msg)
}

pub fn register_new_module_author(
    conn: &mut SqliteConnection,
    author_name: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!(
        "New author '{author_name}' added for '{module_technical_name}' ({module_name}) [{module_version_odoo}]"
    );
    add(conn, "author", SEVERITY_SUCCESS, &msg)
}

pub fn register_new_module_maintainer(
    conn: &mut SqliteConnection,
    maintainer_name: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!(
        "New maintainer '{maintainer_name}' added for '{module_technical_name}' ({module_name}) [{module_version_odoo}]"
    );
    add(conn, "maintainer", SEVERITY_SUCCESS, &msg)
}

pub fn register_new_module_committer(
    conn: &mut SqliteConnection,
    committer_name: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!(
        "New committer '{committer_name}' added for '{module_technical_name}' ({module_name}) [{module_version_odoo}]"
    );
    add(conn, "committer", SEVERITY_SUCCESS, &msg)
}

pub fn register_new_module(
    conn: &mut SqliteConnection,
    module_technical_name: &str,
    module_name: &str,
    module_version: &str,
    org_name: &str,
    repo_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!(
        "New module '{module_technical_name}' ({module_name}) [{module_version}] added in '{org_name}/{repo_name}' [{module_version_odoo}]"
    );
    add(conn, "module", SEVERITY_SUCCESS, &msg)
}

pub fn register_delete_module_author(
    conn: &mut SqliteConnection,
    author_name: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!(
        "'{author_name}' removed as author of '{module_technical_name}' ({module_name}) [{module_version_odoo}]"
    );
    add(conn, "author", SEVERITY_WARNING, &msg)
}

pub fn register_delete_module_dependency(
    conn: &mut SqliteConnection,
    dep_name: &str,
    dep_type: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!(
        "'{dep_name}' removed as {dep_type} dependency of '{module_technical_name}' ({module_name}) [{module_version_odoo}]"
    );
    add(conn, "dependency", SEVERITY_WARNING, &msg)
}

pub fn register_delete_module_maintainer(
    conn: &mut SqliteConnection,
    maintainer_name: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!(
        "'{maintainer_name}' removed as maintainer of '{module_technical_name}' ({module_name}) [{module_version_odoo}]"
    );
    add(conn, "maintainer", SEVERITY_WARNING, &msg)
}

pub fn register_update_module(
    conn: &mut SqliteConnection,
    module_info: &LogUpdateModuleInfo,
) -> QueryResult<Model> {
    let changes = module_info
        .module_changes
        .iter()
        .map(|change| format!("{}: {} -> {}", change.0, change.1, change.2))
        .collect::<Vec<_>>()
        .join(", ");
    let short_hash = &module_info.last_commit_hash[..module_info.last_commit_hash.len().min(7)];
    let mut msg = format!(
        "Module '{}' ({}) [{}] in '{}/{}' [{}] updated: {}. Commit \"{}\" ({}) by {} on {}",
        module_info.module_technical_name,
        module_info.module_name,
        module_info.module_version,
        module_info.org_name,
        module_info.repo_name,
        module_info.module_version_odoo,
        changes,
        module_info.last_commit_name,
        short_hash,
        module_info.last_commit_author,
        module_info.last_commit_date,
    );
    // Not linkified: the commit/PR URL scheme depends on whether the repo is
    // hosted on GitHub or GitLab, which gh_repository doesn't track today.
    if !module_info.last_commit_partof.is_empty() {
        msg += &format!(" (PR {})", module_info.last_commit_partof);
    }
    add(conn, "module", SEVERITY_INFO, &msg)
}

pub fn register_finished_task_collector(
    conn: &mut SqliteConnection,
    scan_seconds: &str,
    number_modules: &str,
    org_name: &str,
    odoo_version: &str,
) -> QueryResult<Model> {
    let msg = format!(
        "Scan finished in {scan_seconds} seconds: {number_modules} modules collected in '{org_name}' [{odoo_version}]"
    );
    add(conn, "collector", SEVERITY_SUCCESS, &msg)
}

pub fn register_problem_module_version(
    conn: &mut SqliteConnection,
    module_technical_name: &str,
    module_name: &str,
    repo_name: &str,
    manifest_version_odoo: &str,
    version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!(
        "PROBLEM DETECTED: '{module_technical_name}' ({module_name}) from {repo_name} has an incorrect Odoo version: {manifest_version_odoo} should be {version_odoo}"
    );
    add(conn, "issue", SEVERITY_ERROR, &msg)
}

pub fn register_new_migration_pr(
    conn: &mut SqliteConnection,
    module_technical_name: &str,
    pr_name: &str,
    prid: &i64,
    repo_name: &str,
    version_odoo: &u8,
) -> QueryResult<Model> {
    let version_odoo_str = odoo_version_u8_to_string(version_odoo);
    let msg = format!(
        "New migration PR #{prid} \"{pr_name}\" opened for '{module_technical_name}' in '{repo_name}' [{version_odoo_str}]"
    );
    add(conn, "migration_pr", SEVERITY_INFO, &msg)
}

pub fn register_closed_migration_pr(
    conn: &mut SqliteConnection,
    module_technical_name: &str,
    pr_name: &str,
    prid: &i64,
    repo_name: &str,
    version_odoo: &u8,
) -> QueryResult<Model> {
    let version_odoo_str = odoo_version_u8_to_string(version_odoo);
    let msg = format!(
        "Migration PR #{prid} \"{pr_name}\" for '{module_technical_name}' in '{repo_name}' [{version_odoo_str}] is no longer open (merged/closed)"
    );
    add(conn, "migration_pr", SEVERITY_INFO, &msg)
}

pub fn register_new_osv_vulnerability(
    conn: &mut SqliteConnection,
    dep_name: &str,
    osv_id: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!(
        "Vulnerability {osv_id} found in dependency '{dep_name}' used by '{module_technical_name}' ({module_name}) [{module_version_odoo}]"
    );
    add(conn, "vulnerability", SEVERITY_ERROR, &msg)
}
