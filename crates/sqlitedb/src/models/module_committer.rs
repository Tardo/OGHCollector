// Copyright 2025 Alexandre D. Díaz
use cached::proc_macro::cached;
use serde::{Deserialize, Serialize};
use rusqlite::{Result, ToSql, params};

use crate::models::{module, committer, system_event};
use oghutils::version::odoo_version_u8_to_string;

pub type Connection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub static TABLE_NAME: &str = "module_committer";


#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Model {
    pub id: i64,
    pub module_id: (i64, String),
    pub committer_id: (i64, String),
    pub commits: u32,
}

pub fn create_table(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        format!("CREATE TABLE IF NOT EXISTS {0} (
            id integer primary key,
            module_id integer not null references {1}(id),
            committer_id integer not null references {2}(id),
            commits integer not null,
            CONSTRAINT fk_module
                FOREIGN KEY (module_id)
                REFERENCES {1}(id)
                ON DELETE CASCADE,
            CONSTRAINT fk_committer
                FOREIGN KEY (committer_id)
                REFERENCES {2}(id)
                ON DELETE CASCADE
        )", &TABLE_NAME, &module::TABLE_NAME, &committer::TABLE_NAME).as_str(),
        params![],
    )?;
    conn.execute(
        format!("CREATE UNIQUE INDEX IF NOT EXISTS uniq_module_committer ON {}(module_id, committer_id)", &TABLE_NAME).as_str(),
        params![],
    )
}

fn query(conn: &Connection, extra_sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Model>, rusqlite::Error> {
    let sql: String = format!("SELECT mod_com.id, mod_com.module_id, mod.technical_name, mod_com.committer_id, com.name, mod_com.commits \
    FROM {} as mod_com \
    INNER JOIN {} as mod \
    ON mod.id = mod_com.module_id \
    INNER JOIN {} as com \
    ON com.id = mod_com.committer_id \
    {}", &TABLE_NAME, &module::TABLE_NAME, &committer::TABLE_NAME, &extra_sql);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params, 
        |row| {
            Ok(Model {
                id: row.get(0)?,
                module_id: (row.get(1)?, row.get(2)?),
                committer_id: (row.get(3)?, row.get(4)?),
                commits: row.get(5)?,
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
    convert = r#"{ format!("{}{}", module_id, committer_id) }"#
)]
pub fn get_by_id(conn: &Connection, module_id: &i64, committer_id: &i64) -> Option<Model> {
    let mod_committers = query(conn, "WHERE mod_mant.module_id = ?1 AND mod_mant.committer_id = ?2 LIMIT 1", params![&module_id, &committer_id]).unwrap();
    if mod_committers.is_empty() {
        return None;
    }
    Some(mod_committers[0].clone())
}

#[cached(
    key = "String",
    time = 3600,
    option = true,
    convert = r#"{ format!("{}{}", module_id, name) }"#
)]
pub fn get_by_name(conn: &Connection, module_id: &i64, name: &str) -> Option<Model> {
    let mod_committers = query(conn, "WHERE mod_com.module_id = ?1 AND com.name = ?2 LIMIT 1", params![&module_id, &name]).unwrap();
    if mod_committers.is_empty() {
        return None;
    }
    Some(mod_committers[0].clone())
}

#[cached(
    key = "String",
    time = 3600,
    convert = r#"{ format!("{}", module_id) }"#
)]
pub fn get_by_module_id(conn: &Connection, module_id: &i64) -> Vec<Model> {
    
    query(conn, "WHERE mod_com.module_id = ?1 LIMIT 1", params![&module_id]).unwrap()
}

#[cached(
    key = "String",
    time = 3600,
    convert = r#"{ format!("{}", module_id) }"#
)]
pub fn get_names_by_module_id(conn: &Connection, module_id: &i64) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let module_committers = get_by_module_id(conn, module_id);
    for module_committer in module_committers {
        let committer = committer::get_by_id(conn, &module_committer.committer_id.0).unwrap();
        names.push(committer.name);
    }
    names
}

pub fn add(conn: &Connection, module_id: &i64, name: &str, commits: &u32) -> Result<Model, rusqlite::Error> {
    let module_committer_opt = get_by_name(conn, module_id, name);
    if module_committer_opt.is_none() {
        let committer = committer::add(conn, name).unwrap();
        conn.execute(
            format!("INSERT INTO {}(module_id, committer_id, commits) VALUES (?1, ?2, ?3)", &TABLE_NAME).as_str(),
            params![&module_id, &committer.id, &commits],
        )?;
        let last_id = conn.last_insert_rowid();
        let module = module::get_by_id(conn, module_id).unwrap();
        let _ = system_event::register_new_module_committer(conn, name, &module.technical_name, &module.name, odoo_version_u8_to_string(&module.version_odoo).as_str());
        return Ok(Model { 
            id: last_id, 
            module_id: (module.id, module.technical_name.clone()), 
            committer_id: (committer.id, committer.name.clone()),
            commits: *commits,
        });
    }
    let module_committer = module_committer_opt.unwrap();
    conn.execute(
        format!("UPDATE {} SET commits = ?3 WHERE module_id = ?1 AND committer_id = ?2", &TABLE_NAME).as_str(),
        params![&module_committer.module_id.0, &module_committer.committer_id.0, &commits],
    )?;
    Ok(Model { 
        id: module_committer.id, 
        module_id: module_committer.module_id, 
        committer_id: module_committer.committer_id,
        commits: *commits,
    })
}
