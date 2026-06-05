// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::system_event;
use crate::utils::date::get_sqlite_utc_now;

use super::system_event_type;

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
}

pub fn get_messages_current_month(conn: &mut SqliteConnection) -> Vec<Model> {
    diesel::sql_query(
        "SELECT se.message, se.date, se.event_type_id, syset.name as event_type_name \
         FROM system_event as se \
         INNER JOIN system_event_type as syset ON syset.id = se.event_type_id \
         WHERE date(se.date) >= date('now', 'start of month') \
           AND date(se.date) <= date('now', 'start of month', '+1 month', '-1 day') \
         ORDER BY se.date DESC, se.id DESC LIMIT 1000",
    )
    .load::<Model>(conn)
    .expect("DB error in system_event::get_messages_current_month")
}

pub fn add(
    conn: &mut SqliteConnection,
    event_type_name: &str,
    message: &str,
) -> QueryResult<Model> {
    let date = get_sqlite_utc_now();
    let event_type =
        system_event_type::get_by_name(conn, event_type_name).expect("system_event_type not found");

    diesel::insert_into(system_event::table)
        .values(NewSystemEvent {
            message,
            date: &date,
            event_type_id: event_type.id,
        })
        .execute(conn)?;

    Ok(Model {
        message: message.to_string(),
        date,
        event_type_id: event_type.id,
        event_type_name: event_type.name,
    })
}

pub fn register_new_dependency_module(
    conn: &mut SqliteConnection,
    dep_name: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!("New dependency '<span class='dep_name'>{dep_name}</span>' added for '<span class='module_tech_name'>{module_technical_name}</span>' (<span class='module_name'>{module_name}</span>) [<span class='module_odoo_version'>{module_version_odoo}</span>]");
    add(conn, "dependency", &msg)
}

pub fn register_new_gh_organization(
    conn: &mut SqliteConnection,
    org_name: &str,
) -> QueryResult<Model> {
    let msg = format!("New organization '<span class='org_name'>{org_name}</span>' added");
    add(conn, "organization", &msg)
}

pub fn register_new_gh_repository(
    conn: &mut SqliteConnection,
    org_name: &str,
    repo_name: &str,
) -> QueryResult<Model> {
    let msg = format!("New repository '<span class='org_name'>{org_name}</span>/<span class='repo_name'>{repo_name}</span>' added");
    add(conn, "repository", &msg)
}

pub fn register_new_module_author(
    conn: &mut SqliteConnection,
    author_name: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!("New author '<span class='author_name'>{author_name}</span>' added for '<span class='module_tech_name'>{module_technical_name}</span>' (<span class='module_name'>{module_name}</span>) [<span class='module_odoo_version'>{module_version_odoo}</span>]");
    add(conn, "author", &msg)
}

pub fn register_new_module_maintainer(
    conn: &mut SqliteConnection,
    maintainer_name: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!("New maintainer '<span class='maintainer_name'>{maintainer_name}</span>' added for '<span class='module_tech_name'>{module_technical_name}</span>' (<span class='module_name'>{module_name}</span>) [<span class='module_odoo_version'>{module_version_odoo}</span>]");
    add(conn, "maintainer", &msg)
}

pub fn register_new_module_committer(
    conn: &mut SqliteConnection,
    committer_name: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!("New committer '<span class='committer_name'>{committer_name}</span>' added for '<span class='module_tech_name'>{module_technical_name}</span>' (<span class='module_name'>{module_name}</span>) [<span class='module_odoo_version'>{module_version_odoo}</span>]");
    add(conn, "committer", &msg)
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
    let msg = format!("New module '<span class='module_tech_name'>{module_technical_name}</span>' (<span class='module_name'>{module_name}</span>) [<span class='module_version'>{module_version}</span>] added in '<span class='org_name'>{org_name}</span>/<span class='repo_name'>{repo_name}</span>' [<span class='module_odoo_version'>{module_version_odoo}</span>]");
    add(conn, "module", &msg)
}

pub fn register_delete_module_author(
    conn: &mut SqliteConnection,
    author_name: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!("'<span class='author_name'>{author_name}</span>' has been removed as author of the module '<span class='module_tech_name'>{module_technical_name}</span>' (<span class='module_name'>{module_name}</span>) [<span class='module_odoo_version'>{module_version_odoo}</span>]");
    add(conn, "author", &msg)
}

pub fn register_delete_module_dependency(
    conn: &mut SqliteConnection,
    dep_name: &str,
    dep_type: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!("'<span class='dep_name'>{dep_name}</span>' has been removed as {dep_type} dependency of the module '<span class='module_tech_name'>{module_technical_name}</span>' (<span class='module_name'>{module_name}</span>) [<span class='module_odoo_version'>{module_version_odoo}</span>]");
    add(conn, "dependency", &msg)
}

pub fn register_delete_module_maintainer(
    conn: &mut SqliteConnection,
    maintainer_name: &str,
    module_technical_name: &str,
    module_name: &str,
    module_version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!("'<span class='maintainer_name'>{maintainer_name}</span>' has been removed as maintainer of the module '<span class='module_tech_name'>{module_technical_name}</span>' (<span class='module_name'>{module_name}</span>) [<span class='module_odoo_version'>{module_version_odoo}</span>]");
    add(conn, "maintainer", &msg)
}

pub fn register_update_module(
    conn: &mut SqliteConnection,
    module_info: &LogUpdateModuleInfo,
) -> QueryResult<Model> {
    let mut changes = String::new();
    for change in module_info.module_changes {
        changes += &format!(
            "<li>{}: <span class='value_old'>{}</span> -> <span class='value_new'>{}</span></li>",
            change.0, change.1, change.2
        );
    }
    let mut msg = format!(
        "Module '<span class='module_tech_name'>{}</span>' (<span class='module_name'>{}</span>) [<span class='module_version'>{}</span>] in '<span class='org_name'>{}</span>/<span class='repo_name'>{}</span>' [<span class='module_odoo_version'>{}</span>] updated:<ul class='module_changes'>{}</ul><div class='commit_info'><a class='git_commit' href='https://github.com/{}/{}/commit/{}'>{}</a> - <span class='git_author'>{}</span> - <span class='git_date'>{}</span>",
        module_info.module_technical_name,
        module_info.module_name,
        module_info.module_version,
        module_info.org_name,
        module_info.repo_name,
        module_info.module_version_odoo,
        changes,
        module_info.org_name,
        module_info.repo_name,
        module_info.last_commit_hash,
        module_info.last_commit_name,
        module_info.last_commit_author,
        module_info.last_commit_date,
    );
    if !module_info.last_commit_partof.is_empty() {
        msg += &format!(
            " - PR: <a class='git_pr' href='https://github.com/{}'>{}</a>",
            module_info.last_commit_partof.replace('#', "/pull/"),
            module_info.last_commit_partof,
        );
    }
    msg += "</div>";
    add(conn, "module", &msg)
}

pub fn register_finished_task_collector(
    conn: &mut SqliteConnection,
    scan_seconds: &str,
    number_modules: &str,
    org_name: &str,
    odoo_version: &str,
) -> QueryResult<Model> {
    let msg = format!("Scan finished in {scan_seconds} seconds: <span class='number_modules'>{number_modules}</span> modules collected in '<span class='org_name'>{org_name}</span>' [<span class='odoo_version'>{odoo_version}</span>]");
    add(conn, "internal", &msg)
}

pub fn register_problem_module_version(
    conn: &mut SqliteConnection,
    module_technical_name: &str,
    module_name: &str,
    repo_name: &str,
    manifest_version_odoo: &str,
    version_odoo: &str,
) -> QueryResult<Model> {
    let msg = format!("<span class='problem'>PROBLEM DETECTED: '<span class='module_tech_name'>{module_technical_name}</span>' (<span class='module_name'>{module_name}</span>) from <span class='module_repo'>{repo_name}</span> has an incorrect Odoo version: <span class='value_wrong'>{manifest_version_odoo}</span> should be <span class='value_good'>{version_odoo}</span></span>");
    add(conn, "issue", &msg)
}
