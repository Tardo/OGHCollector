use cached::proc_macro::cached;
use serde::{Deserialize, Serialize};
use rusqlite::{Result, ToSql, params};

pub type Connection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub static TABLE_NAME: &str = "dependency_type";


#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Model {
    pub id: i64,
    pub name: String,
}

pub fn create_table(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        format!("CREATE TABLE IF NOT EXISTS {} (
            id integer primary key,
            name text not null unique
        )", &TABLE_NAME).as_str(),
        params![],
    )
}

pub fn populate(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        format!("INSERT OR IGNORE INTO {}(name) VALUES ('module'), ('python'), ('bin')", &TABLE_NAME).as_str(),
        params![],
    )
}

fn query(conn: &Connection, extra_sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Model>, rusqlite::Error> {
    let sql: String = format!("SELECT dt.id, dt.name \
    FROM {} as dt \
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
    convert = r#"{ format!("{}", id) }"#
)]
pub fn get_by_id(conn: &Connection, id: &i64) -> Option<Model> {
    let dep_types = query(&conn, "WHERE dt.id = ?1 LIMIT 1", params![&id]).unwrap();
    if dep_types.is_empty() {
        return None;
    }
    Some(dep_types[0].clone())
}

#[cached(
    key = "String",
    time = 3600, 
    option = true,
    convert = r#"{ format!("{}", name) }"#
)]
pub fn get_by_name(conn: &Connection, name: &str) -> Option<Model> {
    let dep_types = query(&conn, "WHERE dt.name = ?1 LIMIT 1", params![&name]).unwrap();
    if dep_types.is_empty() {
        return None;
    }
    Some(dep_types[0].clone())
}