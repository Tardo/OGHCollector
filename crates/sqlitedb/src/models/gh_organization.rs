// Copyright 2025 Alexandre D. Díaz
use cached::proc_macro::cached;
use rusqlite::{params, Result, ToSql};
use serde::{Deserialize, Serialize};

use crate::models::system_event;

pub type Connection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub static TABLE_NAME: &str = "gh_organization";

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Model {
    pub id: i64,
    pub name: String,
}

pub fn create_table(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        format!(
            "CREATE TABLE IF NOT EXISTS {} (
            id integer primary key,
            name text unique not null
        )",
            &TABLE_NAME
        )
        .as_str(),
        params![],
    )
}

fn query(
    conn: &Connection,
    extra_sql: &str,
    params: &[&dyn ToSql],
) -> Result<Vec<Model>, rusqlite::Error> {
    let sql: String = format!(
        "SELECT gh_org.id, gh_org.name \
    FROM {} as gh_org \
    {}",
        &TABLE_NAME, &extra_sql
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params, |row| {
        Ok(Model {
            id: row.get(0)?,
            name: row.get(1)?,
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
    convert = r#"{ format!("{}", name) }"#
)]
pub fn get_by_name(conn: &Connection, name: &str) -> Option<Model> {
    let gh_orgs = query(conn, "WHERE gh_org.name = ?1 LIMIT 1", params![&name]).unwrap();
    if gh_orgs.is_empty() {
        return None;
    }
    Some(gh_orgs[0].clone())
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
    let gh_orgs = query(conn, "WHERE gh_org.id = ?1 LIMIT 1", params![&id]).unwrap();
    if gh_orgs.is_empty() {
        return None;
    }
    Some(gh_orgs[0].clone())
}

pub fn add(conn: &Connection, name: &str) -> Result<Model, rusqlite::Error> {
    let org_opt = get_by_name(conn, name);
    if org_opt.is_none() {
        conn.execute(
            format!("INSERT INTO {}(name) VALUES (?1)", &TABLE_NAME).as_str(),
            params![&name],
        )?;
        let last_id = conn.last_insert_rowid();
        let _ = system_event::register_new_gh_organization(conn, name);
        return Ok(Model {
            id: last_id,
            name: name.to_string(),
        });
    }
    Ok(org_opt.unwrap())
}
