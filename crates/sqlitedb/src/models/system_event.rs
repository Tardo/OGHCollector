// Copyright 2025 Alexandre D. Díaz
use cached::proc_macro::cached;
use serde::{Deserialize, Serialize};
use rusqlite::{Result, ToSql, params};

use crate::models::system_event_type;
use crate::utils::date::get_sqlite_utc_now;

pub type Connection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub static TABLE_NAME: &str = "system_event";


#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Model {
    pub message: String,
    pub date: String,
    pub event_type_id: (i64, String),
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
    pub last_commit_partof: &'a str
}

pub fn create_table(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        format!("CREATE TABLE IF NOT EXISTS {0} (
            id integer primary key,
            message text not null,
            date text not null,
            event_type_id integer not null references {1}(id),
            CONSTRAINT fk_event_type
                FOREIGN KEY (event_type_id)
                REFERENCES {1}(id)
                ON DELETE CASCADE
        )", &TABLE_NAME, &system_event_type::TABLE_NAME).as_str(),
        params![],
    )
}

fn query(conn: &Connection, extra_sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Model>, rusqlite::Error> {
    let sql: String = format!("SELECT se.message, se.date, se.event_type_id, syset.name \
    FROM {} as se \
    INNER JOIN {} as syset \
    ON syset.id = se.event_type_id
    {}", &TABLE_NAME, &system_event_type::TABLE_NAME, &extra_sql);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params, 
        |row| {
            Ok(Model {
                message: row.get(0)?,
                date: row.get(1)?,
                event_type_id: (row.get(2)?, row.get(3)?)
            })
    })?;
    let iter = rows.map(|x| x.unwrap());
    let records = iter.collect::<Vec<Model>>();
    Ok(records)
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("") }"#
)]
pub fn get_messages_current_month(conn: &Connection) -> Vec<Model> {
    
    query(conn, 
        "WHERE date(se.date) >= date('now', 'start of month') AND date(se.date) <= date('now', 'start of month', '+1 month', '-1 day') ORDER BY se.date DESC, se.id DESC LIMIT 1000", params![]).unwrap()
}

pub fn add(conn: &Connection, event_type_name: &str, message: &str) -> Result<Model, rusqlite::Error> {
    let date: String = get_sqlite_utc_now();
    let system_event_type = system_event_type::get_by_name(conn, event_type_name).unwrap();
    conn.execute(
        format!("INSERT INTO {}(message, date, event_type_id) VALUES (?1, ?2, ?3)", &TABLE_NAME).as_str(),
        params![&message, &date, &system_event_type.id],
    )?;
    Ok(Model { message: message.to_string(), date, event_type_id: (system_event_type.id, system_event_type.name) })
}

pub fn register_new_dependency_module(conn: &Connection, dep_name: &str, module_technical_name: &str, module_name: &str, module_version_odoo: &str) -> Result<Model, rusqlite::Error> {
    let msg = format!("New dependency '<span class='dep_name'>{}</span>' added for '<span class='module_tech_name'>{}</span>' (<span class='module_name'>{}</span>) [<span class='module_odoo_version'>{}</span>]", &dep_name, &module_technical_name, &module_name, &module_version_odoo);
    add(conn, "dependency", &msg,)
}
pub fn register_new_gh_organization(conn: &Connection, org_name: &str) -> Result<Model, rusqlite::Error> {
    let msg = format!("New organization '<span class='org_name'>{}</span>' added", &org_name);
    add(conn, "organization", &msg)
}

pub fn register_new_gh_repository(conn: &Connection, org_name: &str, repo_name: &str) -> Result<Model, rusqlite::Error> {
    let msg = format!("New repository '<span class='org_name'>{}</span>/<span class='repo_name'>{}</span>' added", &org_name, &repo_name);
    add(conn, "repository", &msg)
}

pub fn register_new_module_author(conn: &Connection, author_name: &str, module_technical_name: &str, module_name: &str, module_version_odoo: &str) -> Result<Model, rusqlite::Error> {
    let msg = format!("New author '<span class='author_name'>{}</span>' added for '<span class='module_tech_name'>{}</span>' (<span class='module_name'>{}</span>) [<span class='module_odoo_version'>{}</span>]", &author_name, &module_technical_name, &module_name, &module_version_odoo);
    add(conn, "author", &msg)
}

pub fn register_new_module_maintainer(conn: &Connection, maintainer_name: &str, module_technical_name: &str, module_name: &str, module_version_odoo: &str) -> Result<Model, rusqlite::Error> {
    let msg = format!("New maintainer '<span class='maintainer_name'>{}</span>' added for '<span class='module_tech_name'>{}</span>' (<span class='module_name'>{}</span>) [<span class='module_odoo_version'>{}</span>]", &maintainer_name, &module_technical_name, &module_name, &module_version_odoo);
    add(conn, "maintainer", &msg)
}

pub fn register_new_module_committer(conn: &Connection, committer_name: &str, module_technical_name: &str, module_name: &str, module_version_odoo: &str) -> Result<Model, rusqlite::Error> {
    let msg = format!("New committer '<span class='committer_name'>{}</span>' added for '<span class='module_tech_name'>{}</span>' (<span class='module_name'>{}</span>) [<span class='module_odoo_version'>{}</span>]", &committer_name, &module_technical_name, &module_name, &module_version_odoo);
    add(conn, "committer", &msg)
}

pub fn register_new_module(conn: &Connection, module_technical_name: &str, module_name: &str, module_version: &str, org_name: &str, repo_name: &str, module_version_odoo: &str) -> Result<Model, rusqlite::Error> {
    let msg = format!("New module '<span class='module_tech_name'>{}</span>' (<span class='module_name'>{}</span>) [<span class='module_version'>{}</span>] added in '<span class='org_name'>{}</span>/<span class='repo_name'>{}</span>' [<span class='module_odoo_version'>{}</span>]", &module_technical_name, &module_name, &module_version, &org_name, &repo_name, &module_version_odoo);
    add(conn, "module", &msg)
}

pub fn register_delete_module_author(conn: &Connection, author_name: &str, module_technical_name: &str, module_name: &str, module_version_odoo: &str) -> Result<Model, rusqlite::Error> {
    let msg = format!("'<span class='author_name'>{}</span>' has been removed as author of the module '<span class='module_tech_name'>{}</span>' (<span class='module_name'>{}</span>) [<span class='module_odoo_version'>{}</span>]", &author_name, &module_technical_name, &module_name, &module_version_odoo);
    add(conn, "author", &msg)
}

pub fn register_delete_module_dependency(conn: &Connection, dep_name: &str, dep_type: &str, module_technical_name: &str, module_name: &str, module_version_odoo: &str) -> Result<Model, rusqlite::Error> {
    let msg = format!("'<span class='dep_name'>{}</span>' has been removed as {} dependency of the module '<span class='module_tech_name'>{}</span>' (<span class='module_name'>{}</span>) [<span class='module_odoo_version'>{}</span>]", &dep_name, &dep_type, &module_technical_name, &module_name, &module_version_odoo);
    add(conn, "dependency", &msg)
}

pub fn register_delete_module_maintainer(conn: &Connection, maintainer_name: &str, module_technical_name: &str, module_name: &str, module_version_odoo: &str) -> Result<Model, rusqlite::Error> {
    let msg = format!("'<span class='maintainer_name'>{}</span>' has been removed as maintainer of the module '<span class='module_tech_name'>{}</span>' (<span class='module_name'>{}</span>) [<span class='module_odoo_version'>{}</span>]", &maintainer_name, &module_technical_name, &module_name, &module_version_odoo);
    add(conn, "maintainer", &msg)
}


pub fn register_update_module(
    conn: &Connection, 
    module_info: &LogUpdateModuleInfo) -> Result<Model, rusqlite::Error> {
    let mut changes: String = String::new();
    for change in module_info.module_changes {
        changes += format!("<li>{}: <span class='value_old'>{}</span> -> <span class='value_new'>{}</span></li>", &change.0, &change.1, &change.2).as_str();
    }
    let mut msg = format!(
        "Module '<span class='module_tech_name'>{}</span>' (<span class='module_name'>{}</span>) [<span class='module_version'>{}</span>] in '<span class='org_name'>{}</span>/<span class='repo_name'>{}</span>' [<span class='module_odoo_version'>{}</span>] updated:<ul class='module_changes'>{}</ul><div class='commit_info'><a class='git_commit' href='https://github.com/{}/{}/commit/{}'>{}</a> - <span class='git_author'>{}</span> - <span class='git_date'>{}</span>", 
        &module_info.module_technical_name, 
        &module_info.module_name, 
        &module_info.module_version, 
        &module_info.org_name, 
        &module_info.repo_name, 
        &module_info.module_version_odoo, 
        &changes,
        &module_info.org_name,
        &module_info.repo_name,
        &module_info.last_commit_hash,
        &module_info.last_commit_name,
        &module_info.last_commit_author,
        &module_info.last_commit_date
    );
    if !module_info.last_commit_partof.is_empty() {
        msg += format!(" - PR: <a class='git_pr' href='https://github.com/{}'>{}</a>", module_info.last_commit_partof.replace("#","/pull/"), &module_info.last_commit_partof).as_str();
    }
    msg += "</div>";
    add(conn, "module", &msg)
}

pub fn register_finished_task_collector(conn: &Connection, scan_seconds: &str, number_modules: &str, org_name: &str, odoo_version: &str) -> Result<Model, rusqlite::Error> {
    let msg = format!("Scan finished in {} seconds: <span class='number_modules'>{}</span> modules collected in '<span class='org_name'>{}</span>' [<span class='odoo_version'>{}</span>]", &scan_seconds, &number_modules, &org_name, &odoo_version);
    add(conn, "internal", &msg)
}

pub fn register_problem_module_version(conn: &Connection, module_technical_name: &str, module_name: &str, repo_name: &str, manifest_version_odoo: &str, version_odoo: &str) -> Result<Model, rusqlite::Error> {
    let msg = format!("<span class='problem'>PROBLEM DETECTED: '<span class='module_tech_name'>{}</span>' (<span class='module_name'>{}</span>) from <span class='module_repo'>{}</span> has an incorrect Odoo version: <span class='value_wrong'>{}</span> should be <span class='value_good'>{}</span></span>", &module_technical_name, &module_name, &repo_name, &manifest_version_odoo, &version_odoo);
    add(conn, "issue", &msg)
}
