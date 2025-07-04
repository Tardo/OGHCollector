// Copyright 2025 Alexandre D. Díaz
use cached::proc_macro::cached;
use serde::{Deserialize, Serialize};
use rusqlite::{Result, ToSql, params};

use crate::models::{module, maintainer, system_event};
use oghutils::version::odoo_version_u8_to_string;

pub type Connection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub static TABLE_NAME: &str = "module_maintainer";


#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Model {
    pub id: i64,
    pub module_id: (i64, String),
    pub maintainer_id: (i64, String),
}

pub fn create_table(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        format!("CREATE TABLE IF NOT EXISTS {0} (
            id integer primary key,
            module_id integer not null references {1}(id),
            maintainer_id integer not null references {2}(id),
            CONSTRAINT fk_module
                FOREIGN KEY (module_id)
                REFERENCES {1}(id)
                ON DELETE CASCADE,
            CONSTRAINT fk_maintainer
                FOREIGN KEY (maintainer_id)
                REFERENCES {2}(id)
                ON DELETE CASCADE
        )", &TABLE_NAME, &module::TABLE_NAME, &maintainer::TABLE_NAME).as_str(),
        params![],
    )?;
    conn.execute(
        format!("CREATE UNIQUE INDEX IF NOT EXISTS uniq_module_maintainer ON {}(module_id, maintainer_id)", &TABLE_NAME).as_str(),
        params![],
    )
}

fn query(conn: &Connection, extra_sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Model>, rusqlite::Error> {
    let sql: String = format!("SELECT mod_mant.id, mod_mant.module_id, mod.technical_name, mod_mant.maintainer_id, mant.name \
    FROM {} as mod_mant \
    INNER JOIN {} as mod \
    ON mod.id = mod_mant.module_id \
    INNER JOIN {} as mant \
    ON mant.id = mod_mant.maintainer_id \
    {}", &TABLE_NAME, &module::TABLE_NAME, &maintainer::TABLE_NAME, &extra_sql);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params, 
        |row| {
            Ok(Model {
                id: row.get(0)?,
                module_id: (row.get(1)?, row.get(2)?),
                maintainer_id: (row.get(3)?, row.get(4)?),
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
    convert = r#"{ format!("{}{}", module_id, maintainer_id) }"#
)]
pub fn get_by_id(conn: &Connection, module_id: &i64, maintainer_id: &i64) -> Option<Model> {
    let mod_maintainers = query(conn, "WHERE mod_mant.module_id = ?1 AND mod_mant.maintainer_id = ?2 LIMIT 1", params![&module_id, &maintainer_id]).unwrap();
    if mod_maintainers.is_empty() {
        return None;
    }
    Some(mod_maintainers[0].clone())
}

#[cached(
    key = "String",
    time = 3600,
    option = true,
    convert = r#"{ format!("{}{}", module_id, name) }"#
)]
pub fn get_by_name(conn: &Connection, module_id: &i64, name: &str) -> Option<Model> {
    let mod_maintainers = query(conn, "WHERE mod_mant.module_id = ?1 AND mant.name = ?2 LIMIT 1", params![&module_id, &name]).unwrap();
    if mod_maintainers.is_empty() {
        return None;
    }
    Some(mod_maintainers[0].clone())
}

#[cached(
    key = "String",
    time = 3600,
    convert = r#"{ format!("{}", module_id) }"#
)]
pub fn get_by_module_id(conn: &Connection, module_id: &i64) -> Vec<Model> {
    
    query(conn, "WHERE mod_mant.module_id = ?1 LIMIT 1", params![&module_id]).unwrap()
}

#[cached(
    key = "String",
    time = 3600,
    convert = r#"{ format!("{}", module_id) }"#
)]
pub fn get_names_by_module_id(conn: &Connection, module_id: &i64) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let module_maintainers = get_by_module_id(conn, module_id);
    for module_maintainer in module_maintainers {
        let maintainer = maintainer::get_by_id(conn, &module_maintainer.maintainer_id.0).unwrap();
        names.push(maintainer.name);
    }
    names
}

pub fn add(conn: &Connection, module_id: &i64, name: &str) -> Result<Model, rusqlite::Error> {
    let module_maintainer_opt = get_by_name(conn, module_id, name);
    if module_maintainer_opt.is_none() {
        let maintainer = maintainer::add(conn, name).unwrap();
        conn.execute(
            format!("INSERT INTO {}(module_id, maintainer_id) VALUES (?1, ?2)", &TABLE_NAME).as_str(),
            params![&module_id, &maintainer.id],
        )?;
        let last_id = conn.last_insert_rowid();
        let module = module::get_by_id(conn, module_id).unwrap();
        let _ = system_event::register_new_module_maintainer(conn, name, &module.technical_name, &module.name, odoo_version_u8_to_string(&module.version_odoo).as_str());
        return Ok(Model { 
            id: last_id, 
            module_id: (module.id, module.technical_name.clone()), 
            maintainer_id: (maintainer.id, maintainer.name.clone()),
        });
    }
    Ok(module_maintainer_opt.unwrap())
}

pub fn delete_by_module_id(conn: &Connection, module_id: &i64) -> Result<usize, rusqlite::Error> {
    conn.execute(format!("DELETE FROM {} WHERE module_id = ?1", &TABLE_NAME).as_str(), params![&module_id])
}

pub fn delete_by_module_id_maintainer_id(conn: &Connection, module_id: &i64, maintainer_id: &i64) -> Result<usize, rusqlite::Error> {
    conn.execute(format!("DELETE FROM {} WHERE module_id = ?1 AND maintainer_id = ?2", &TABLE_NAME).as_str(), params![&module_id, &maintainer_id])
}