// Copyright Alexandre D. Díaz
use cached::proc_macro::cached;
use rusqlite::{params, Result, ToSql};
use serde::{Deserialize, Serialize};

pub type Connection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub static TABLE_NAME: &str = "author";

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
            name text not null
        )",
            &TABLE_NAME
        )
        .as_str(),
        params![],
    )?;
    conn.execute(
        format!(
            "CREATE UNIQUE INDEX IF NOT EXISTS uniq_author_name ON {}(name)",
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
        "SELECT au.id, au.name \
    FROM {} as au \
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
    convert = r#"{ format!("{}", author_id) }"#
)]
pub fn get_by_id(conn: &Connection, author_id: &i64) -> Option<Model> {
    let authors = query(conn, "WHERE au.id = ?1 LIMIT 1", params![&author_id]).unwrap();
    if authors.is_empty() {
        return None;
    }
    Some(authors[0].clone())
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
    let authors = query(conn, "WHERE au.name = ?1 LIMIT 1", params![&name]).unwrap();
    if authors.is_empty() {
        return None;
    }
    Some(authors[0].clone())
}

pub fn add(conn: &Connection, name: &str) -> Result<Model, rusqlite::Error> {
    let author_opt = get_by_name(conn, name);
    if author_opt.is_none() {
        conn.execute(
            format!("INSERT INTO {}(name) VALUES (?1)", &TABLE_NAME).as_str(),
            params![&name],
        )?;
        return Ok(Model {
            id: conn.last_insert_rowid(),
            name: name.to_string(),
        });
    }
    Ok(author_opt.unwrap())
}
