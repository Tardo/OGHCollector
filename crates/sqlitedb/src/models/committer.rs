// Copyright 2025 Alexandre D. Díaz
use cached::proc_macro::cached;
use serde::{Deserialize, Serialize};
use rusqlite::{Result, ToSql, params};

pub type Connection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub static TABLE_NAME: &str = "committer";


#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Model {
    pub id: i64,
    pub name: String,
}

pub fn create_table(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        format!("CREATE TABLE IF NOT EXISTS {} (
            id integer primary key,
            name text not null
        )", &TABLE_NAME).as_str(),
        params![],
    )?;
    conn.execute(
        format!("CREATE UNIQUE INDEX IF NOT EXISTS uniq_committer_name ON {}(name)", &TABLE_NAME).as_str(),
        params![],
    )
}

fn query(conn: &Connection, extra_sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Model>, rusqlite::Error> {
    let sql: String = format!("SELECT com.id, com.name \
    FROM {} as com \
    {}", &TABLE_NAME, &extra_sql);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params, 
        |row| {
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
    option = true,
    convert = r#"{ format!("{}", committer_id) }"#
)]
pub fn get_by_id(conn: &Connection, committer_id: &i64) -> Option<Model> {
    let committers = query(conn, "WHERE com.id = ?1 LIMIT 1", params![&committer_id]).unwrap();
    if committers.is_empty() {
        return None;
    }
    Some(committers[0].clone())
}

#[cached(
    key = "String",
    time = 3600,
    option = true,
    convert = r#"{ format!("{}", name) }"#
)]
pub fn get_by_name(conn: &Connection, name: &str) -> Option<Model> {
    let committers = query(conn, "WHERE com.name = ?1 LIMIT 1", params![&name]).unwrap();
    if committers.is_empty() {
        return None;
    }
    Some(committers[0].clone())
}

pub fn add(conn: &Connection, name: &str) -> Result<Model, rusqlite::Error> {
    let committer_opt = get_by_name(conn, name);
    if committer_opt.is_none() {
        conn.execute(
            format!("INSERT INTO {}(name) VALUES (?1)", &TABLE_NAME).as_str(),
            params![&name],
        )?;
        return Ok(Model { id: conn.last_insert_rowid(), name: name.to_string() });
    }
    Ok(committer_opt.unwrap())
}