// Copyright 2025 Alexandre D. Díaz
use cached::proc_macro::cached;
use serde::{Deserialize, Serialize};
use rusqlite::{Result, ToSql, params};

use crate::models::{module, author, system_event};
use oghutils::version::odoo_version_u8_to_string;

pub type Connection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub static TABLE_NAME: &str = "module_author";


#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Model {
    pub id: i64,
    pub module_id: (i64, String),
    pub author_id: (i64, String),
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TopAuthorJSON {
    pub author_id: String,
    pub count: String,
}

pub fn create_table(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        format!("CREATE TABLE IF NOT EXISTS {0} (
            id integer primary key,
            module_id integer not null references {1}(id),
            author_id integer not null references {2}(id),
            CONSTRAINT fk_module
                FOREIGN KEY (module_id)
                REFERENCES {1}(id)
                ON DELETE CASCADE,
            CONSTRAINT fk_author
                FOREIGN KEY (author_id)
                REFERENCES {2}(id)
                ON DELETE CASCADE
        )", &TABLE_NAME, &module::TABLE_NAME, &author::TABLE_NAME).as_str(),
        params![],
    )?;
    conn.execute(
        format!("CREATE UNIQUE INDEX IF NOT EXISTS uniq_module_author ON {}(module_id, author_id)", &TABLE_NAME).as_str(),
        params![],
    )
}

fn query(conn: &Connection, extra_sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Model>, rusqlite::Error> {
    let sql: String = format!("SELECT mod_au.id, mod_au.module_id, mod.technical_name, mod_au.author_id, au.name \
    FROM {} as mod_au \
    INNER JOIN {} as mod \
    ON mod.id = mod_au.module_id \
    INNER JOIN {} as au \
    ON au.id = mod_au.author_id \
    {}", &TABLE_NAME, &module::TABLE_NAME, &author::TABLE_NAME, &extra_sql);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params, 
        |row| {
            Ok(Model {
                id: row.get(0)?,
                module_id: (row.get(1)?, row.get(2)?),
                author_id: (row.get(3)?, row.get(4)?),
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
    convert = r#"{ format!("{}{}", module_id, author_id) }"#
)]
pub fn get_by_id(conn: &Connection, module_id: &i64, author_id: &i64) -> Option<Model> {
    let mod_authors = query(conn, "WHERE mod_au.module_id = ?1 AND mod_au.author_id = ?2 LIMIT 1", params![&module_id, &author_id]).unwrap();
    if mod_authors.is_empty() {
        return None;
    }
    Some(mod_authors[0].clone())
}

#[cached(
    key = "String",
    time = 3600,
    option = true,
    convert = r#"{ format!("{}{}", module_id, name) }"#
)]
pub fn get_by_name(conn: &Connection, module_id: &i64, name: &str) -> Option<Model> {
    let mod_authors = query(conn, "WHERE mod_au.module_id = ?1 AND au.name = ?2 LIMIT 1", params![&module_id, &name]).unwrap();
    if mod_authors.is_empty() {
        return None;
    }
    Some(mod_authors[0].clone())
}

#[cached(
    key = "String",
    time = 3600,
    convert = r#"{ format!("{}", module_id) }"#
)]
pub fn get_by_module_id(conn: &Connection, module_id: &i64) -> Vec<Model> {
    
    query(conn, "WHERE mod_au.module_id = ?1", params![&module_id]).unwrap()
}

#[cached(
    key = "String",
    time = 3600,
    convert = r#"{ format!("{}", module_id) }"#
)]
pub fn get_names_by_module_id(conn: &Connection, module_id: &i64) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let module_authors = get_by_module_id(conn, module_id);
    for module_author in module_authors {
        let author = author::get_by_id(conn, &module_author.author_id.0).unwrap();
        names.push(author.name);
    }
    names
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("{}", limit) }"#
)]
pub fn get_top_names(conn: &Connection, limit: &u8) -> Vec<TopAuthorJSON> {
    let mut stmt = conn.prepare(
        format!("SELECT author_id, count(*) as num_mods \
        FROM {} \
        GROUP BY author_id \
        ORDER BY num_mods DESC \
        LIMIT ?1;", &TABLE_NAME).as_str()
    ).unwrap();
    let rows = stmt.query_map(
        params![&limit], 
        |row| {
            Ok(TopAuthorJSON {
                author_id: row.get(0)?,
                count: row.get(1)?,
            })
    }).unwrap();

    let rows_iter = rows.map(|x| x.unwrap());
    rows_iter.collect::<Vec<TopAuthorJSON>>()
}

pub fn add(conn: &Connection, module_id: &i64, name: &str) -> Result<Model, rusqlite::Error> {
    let module_author_opt = get_by_name(conn, module_id, name);
    if module_author_opt.is_none() {
        let author = author::add(conn, name).unwrap();
        conn.execute(
            format!("INSERT INTO {}(module_id, author_id) VALUES (?1, ?2)", &TABLE_NAME).as_str(),
            params![&module_id, &author.id],
        )?;
        let last_id = conn.last_insert_rowid();
        let module = module::get_by_id(conn, module_id).unwrap();
        let _ = system_event::register_new_module_author(conn, name, &module.technical_name, &module.name, odoo_version_u8_to_string(&module.version_odoo).as_str());
        return Ok(Model { 
            id: last_id, 
            module_id: (module.id, module.technical_name.clone()), 
            author_id: (author.id, name.to_string()),
        });
    }
    Ok(module_author_opt.unwrap())
}

pub fn delete_by_module_id(conn: &Connection, module_id: &i64) -> Result<usize, rusqlite::Error> {
    conn.execute(format!("DELETE FROM {} WHERE module_id = ?1", &TABLE_NAME).as_str(), params![&module_id])
}

pub fn delete_by_module_id_author_id(conn: &Connection, module_id: &i64, author_id: &i64) -> Result<usize, rusqlite::Error> {
    conn.execute(format!("DELETE FROM {} WHERE module_id = ?1 AND author_id = ?2", &TABLE_NAME).as_str(), params![&module_id, &author_id])
}