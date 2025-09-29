// Copyright 2025 Alexandre D. Díaz
use cached::proc_macro::cached;
use rusqlite::{params, Result, ToSql};
use serde::{Deserialize, Serialize};

use crate::models::{gh_organization, module, system_event};
use crate::utils::date::get_sqlite_utc_now;

pub type Connection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub static TABLE_NAME: &str = "gh_repository";

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Model {
    pub id: i64,
    pub name: String,
    pub gh_organization_id: (i64, String),
    pub create_date: String,
    pub update_date: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RepositoryInfo {
    pub name: String,
    pub organization: String,
    pub num_modules: u16,
    pub version_odoo: u8,
}

pub fn create_table(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        format!(
            "CREATE TABLE IF NOT EXISTS {0} (
            id integer primary key,
            name text unique not null,
            gh_organization_id integer not null references {1}(id),
            create_date text not null,
            update_date text not null,
            CONSTRAINT fk_gh_organization
                FOREIGN KEY (gh_organization_id)
                REFERENCES {1}(id)
                ON DELETE CASCADE
        )",
            &TABLE_NAME,
            &gh_organization::TABLE_NAME
        )
        .as_str(),
        params![],
    )
    .unwrap();
    conn.execute(
        format!("CREATE UNIQUE INDEX IF NOT EXISTS uniq_name_gh_organization_id ON {}(name, gh_organization_id)", &TABLE_NAME).as_str(),
        params![],
    )
}

fn query(
    conn: &Connection,
    extra_sql: &str,
    params: &[&dyn ToSql],
) -> Result<Vec<Model>, rusqlite::Error> {
    let sql: String = format!("SELECT gh_repo.id, gh_repo.name, gh_repo.gh_organization_id, gh_org.name, gh_repo.create_date, gh_repo.update_date \
    FROM {} as gh_repo \
    INNER JOIN {} as gh_org \
    ON gh_org.id = gh_repo.gh_organization_id \
    {}", &TABLE_NAME, &gh_organization::TABLE_NAME, &extra_sql);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params, |row| {
        Ok(Model {
            id: row.get(0)?,
            name: row.get(1)?,
            gh_organization_id: (row.get(2)?, row.get(3)?),
            create_date: row.get(4)?,
            update_date: row.get(5)?,
        })
    })?;
    let iter = rows.map(|x| x.unwrap());
    let records = iter.collect::<Vec<Model>>();
    Ok(records)
}

#[cached(
    key = "String",
    time = 3600,
    time_refresh = true,
    size = 1000,
    option = true,
    convert = r#"{ format!("{}{}", gh_org_id, name) }"#
)]
pub fn get_by_name(conn: &Connection, gh_org_id: &i64, name: &str) -> Option<Model> {
    let gh_repos = query(
        conn,
        "WHERE gh_repo.name = ?1 AND gh_org.id = ?2 LIMIT 1",
        params![&name, &gh_org_id],
    )
    .unwrap();
    if gh_repos.is_empty() {
        return None;
    }
    Some(gh_repos[0].clone())
}

#[cached(
    key = "String",
    time = 3600,
    time_refresh = true,
    size = 1000,
    option = true,
    convert = r#"{ format!("{}", id) }"#
)]
pub fn get_by_id(conn: &Connection, id: &i64) -> Option<Model> {
    let gh_repos = query(conn, "WHERE gh_repo.id = ?1 LIMIT 1", params![&id]).unwrap();
    if gh_repos.is_empty() {
        return None;
    }
    Some(gh_repos[0].clone())
}

#[cached(
    key = "String",
    time = 3600,
    time_refresh = true,
    size = 1000,
    convert = r#"{ format!("{}", repo_name) }"#
)]
pub fn get_info_by_name(conn: &Connection, repo_name: &str) -> Vec<RepositoryInfo> {
    let mut stmt = conn.prepare(
        format!("SELECT gh_repo.name, gh_org.name, count(mod.id), mod.version_odoo FROM {} as gh_repo
        INNER JOIN {} as gh_org
        ON gh_org.id = gh_repo.gh_organization_id
        INNER JOIN {} as mod
        ON mod.gh_repository_id = gh_repo.id
        WHERE gh_repo.name = ?1
        GROUP BY gh_org.id, mod.version_odoo", &TABLE_NAME, &gh_organization::TABLE_NAME, &module::TABLE_NAME).as_str(),
    ).unwrap();
    let repos_rows = stmt
        .query_map(params![&repo_name], |row| {
            Ok(RepositoryInfo {
                name: row.get(0)?,
                organization: row.get(1)?,
                num_modules: row.get(2)?,
                version_odoo: row.get(3)?,
            })
        })
        .unwrap();
    let repos_iter = repos_rows.map(|x| x.unwrap());

    repos_iter.collect::<Vec<RepositoryInfo>>()
}

pub fn add(conn: &Connection, gh_org_id: &i64, name: &str) -> Result<Model, rusqlite::Error> {
    let repo_opt = get_by_name(conn, gh_org_id, name);
    if repo_opt.is_none() {
        let create_date: String = get_sqlite_utc_now();
        conn.execute(
            format!("INSERT INTO {}(name, gh_organization_id, create_date, update_date) VALUES (?1, ?2, ?3, ?3)", &TABLE_NAME).as_str(),
            params![&name, &gh_org_id, &create_date],
        )?;
        let last_id = conn.last_insert_rowid();
        let gh_organization = gh_organization::get_by_id(conn, gh_org_id).unwrap();
        let _ = system_event::register_new_gh_repository(conn, &gh_organization.name, name);
        return Ok(Model {
            id: last_id,
            name: name.to_string(),
            gh_organization_id: (gh_organization.id, gh_organization.name.clone()),
            create_date: create_date.clone(),
            update_date: create_date.clone(),
        });
    }
    Ok(repo_opt.unwrap())
}
