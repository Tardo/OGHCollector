// Copyright 2025 Alexandre D. Díaz
use cached::proc_macro::cached;
use rusqlite::{params, Result, ToSql};
use serde::{Deserialize, Serialize};

use crate::models::{dependency, dependency_module, module};

pub type Connection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub static TABLE_NAME: &str = "dependency_osv";

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Model {
    pub id: i64,
    pub dependency_module_id: (i64, String),
    pub osv_id: String,
    pub details: String,
    pub fixed_in: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DependencyModuleOSVInfo {
    pub version_odoo: u8,
    pub module_name: String,
    pub module_technical_name: String,
    pub name: String,
    pub osv_id: String,
    pub details: String,
    pub fixed_in: String,
}

pub fn create_table(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        format!(
            "CREATE TABLE IF NOT EXISTS {0} (
            id integer primary key,
            dependency_module_id integer not null references {1}(id),
            osv_id text not null,
            details text not null,
            fixed_in text not null,
            CONSTRAINT fk_dependency_module_osv
                FOREIGN KEY (dependency_module_id)
                REFERENCES {1}(id)
                ON DELETE CASCADE
        )",
            &TABLE_NAME,
            &dependency_module::TABLE_NAME
        )
        .as_str(),
        params![],
    )?;
    conn.execute(
        format!("CREATE UNIQUE INDEX IF NOT EXISTS uniq_dep_dep_mod_id_osv_id ON {}(dependency_module_id, osv_id)", &TABLE_NAME).as_str(),
        params![],
    )
}

fn query(
    conn: &Connection,
    extra_sql: &str,
    params: &[&dyn ToSql],
) -> Result<Vec<Model>, rusqlite::Error> {
    let sql: String = format!("SELECT dep_o.id, dep_o.dependency_module_id, dep.name, dep_o.osv_id, dep_o.details, dep_o.fixed_in \
    FROM {} as dep_o \
    INNER JOIN {} as dep_mod \
    ON dep_mod.id = dep_o.dependency_module_id \
    INNER JOIN {} as dep \
    ON dep.id = dep_mod.dependency_id \
    {}", &TABLE_NAME, &dependency_module::TABLE_NAME, &dependency::TABLE_NAME, &extra_sql);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params, |row| {
        Ok(Model {
            id: row.get(0)?,
            dependency_module_id: (row.get(1)?, row.get(2)?),
            osv_id: row.get(3)?,
            details: row.get(4)?,
            fixed_in: row.get(5)?,
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
    convert = r#"{ format!("{}{}", dep_mod_id, osv_id) }"#
)]
pub fn get_by_dep_mod_id_osv_id(
    conn: &Connection,
    dep_mod_id: &i64,
    osv_id: &String,
) -> Option<Model> {
    let deps = query(
        conn,
        "WHERE dep_mod.id = ?1 AND dep_o.osv_id = ?2 LIMIT 1",
        params![&dep_mod_id, &osv_id],
    )
    .unwrap();
    if deps.is_empty() {
        return None;
    }
    Some(deps[0].clone())
}

#[cached(
    key = "String",
    time = 3600,
    time_refresh = true,
    convert = r#"{ format!("") }"#
)]
pub fn get_osv_info(conn: &Connection) -> Vec<DependencyModuleOSVInfo> {
    let sql: String = format!("SELECT mod.version_odoo, mod.name, mod.technical_name, dep.name, dep_o.osv_id, dep_o.details, dep_o.fixed_in FROM {} as dep_o
    INNER JOIN {} as dep_mod ON dep_mod.id = dep_o.dependency_module_id
    INNER JOIN {} as dep ON dep.id = dep_mod.dependency_id
    INNER JOIN {} as mod ON mod.id = dep_mod.module_id", &TABLE_NAME, &dependency_module::TABLE_NAME, &dependency::TABLE_NAME, &module::TABLE_NAME);
    let mut stmt = conn.prepare(&sql).unwrap();
    let rows = stmt
        .query_map(params![], |row| {
            Ok(DependencyModuleOSVInfo {
                version_odoo: row.get(0)?,
                module_name: row.get(1)?,
                module_technical_name: row.get(2)?,
                name: row.get(3)?,
                osv_id: row.get(4)?,
                details: row.get(5)?,
                fixed_in: row.get(6)?,
            })
        })
        .unwrap();
    let iter = rows.map(|x| x.unwrap());

    iter.collect::<Vec<DependencyModuleOSVInfo>>()
}

pub fn add(
    conn: &Connection,
    dep_mod_id: &i64,
    osv_id: &str,
    details: &str,
    fixed_in: &str,
) -> Result<Model, rusqlite::Error> {
    let dep_opt = get_by_dep_mod_id_osv_id(conn, dep_mod_id, &osv_id.to_string());
    if dep_opt.is_none() {
        let dep_mod = dependency_module::get_by_id(conn, dep_mod_id).unwrap();
        let dep = dependency::get_by_id(conn, &dep_mod.dependency_id.0).unwrap();
        conn.execute(
            format!("INSERT INTO {}(dependency_module_id, osv_id, details, fixed_in) VALUES (?1, ?2, ?3, ?4)", &TABLE_NAME).as_str(),
            params![&dep_mod_id, &osv_id, &details, &fixed_in],
        )?;
        return Ok(Model {
            id: conn.last_insert_rowid(),
            dependency_module_id: (dep_mod.id, dep.name.clone()),
            osv_id: osv_id.to_string(),
            details: details.to_string(),
            fixed_in: fixed_in.to_string(),
        });
    }
    Ok(dep_opt.unwrap())
}
