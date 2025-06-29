use cached::proc_macro::cached;
use serde::{Deserialize, Serialize};
use rusqlite::{Result, ToSql, params};

use crate::models::{dependency, module, system_event};
use oghutils::version::odoo_version_u8_to_string;

pub type Connection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub static TABLE_NAME: &str = "dependency_module";


#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Model {
    pub id: i64,
    pub dependency_id: (i64, String),
    pub module_id: (i64, String),
}

pub fn create_table(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        format!("CREATE TABLE IF NOT EXISTS {0} (
            id integer primary key,
            dependency_id integer not null references {1}(id),
            module_id integer not null references {2}(id),
            CONSTRAINT fk_dependency
                FOREIGN KEY (dependency_id)
                REFERENCES {1}(id)
                ON DELETE CASCADE,
            CONSTRAINT fk_module
                FOREIGN KEY (module_id)
                REFERENCES {2}(id)
                ON DELETE CASCADE
        )", &TABLE_NAME, &dependency::TABLE_NAME, &module::TABLE_NAME).as_str(),
        params![],
    )?;
    conn.execute(
        format!("CREATE UNIQUE INDEX IF NOT EXISTS uniq_dep_module ON {}(dependency_id, module_id)", &TABLE_NAME).as_str(),
        params![],
    )
}

fn query(conn: &Connection, extra_sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Model>, rusqlite::Error> {
    let sql = format!("SELECT mod_dep.id, mod_dep.dependency_id, dep.name, mod_dep.module_id, mod.name \
    FROM {} as mod_dep \
    INNER JOIN {} as mod \
    ON mod.id = mod_dep.module_id \
    INNER JOIN {} as dep \
    ON dep.id = mod_dep.dependency_id \
    {}", &TABLE_NAME, &module::TABLE_NAME, &dependency::TABLE_NAME, &extra_sql);
    let mut stmt = conn.prepare(&sql)?;
    let module_rows = stmt.query_map(
        params, 
        |row| {
            Ok(Model {
                id: row.get(0)?,
                dependency_id: (row.get(1)?, row.get(2)?),
                module_id: (row.get(3)?, row.get(4)?),
            })
    })?;
    let modules_iter = module_rows.map(|x| x.unwrap());
    let modules = modules_iter.collect::<Vec<Model>>();
    Ok(modules)
}

#[cached(
    key = "String",
    time = 3600, 
    option = true,
    convert = r#"{ format!("{}", id) }"#
)]
pub fn get_by_id(conn: &Connection, id: &i64) -> Option<Model> {
    let dep_mods = query(&conn, "WHERE mod_dep.id = ?1 LIMIT 1", params![&id]).unwrap();
    if dep_mods.is_empty() {
        return None;
    }
    Some(dep_mods[0].clone())
}

#[cached(
    key = "String",
    time = 3600, 
    option = true,
    convert = r#"{ format!("{}{}", dependency_id, module_id) }"#
)]
fn get_by_dependency_id_module_id(conn: &Connection, dependency_id: &i64, module_id: &i64) -> Option<Model> {
    let mod_deps = query(&conn, "WHERE mod_dep.dependency_id = ?1 AND mod_dep.module_id = ?2 LIMIT 1", params![&dependency_id, &module_id]).unwrap();
    if mod_deps.is_empty() {
        return None;
    }
    Some(mod_deps[0].clone())
}

#[cached(
    key = "String",
    time = 3600, 
    convert = r#"{ format!("{}{}", module_id, dep_type_id) }"#
)]
pub fn get_names(conn: &Connection, module_id: &i64, dep_type_id: &i64) -> Vec<String> {
    let mut stmt = conn.prepare(
        format!("SELECT d.name FROM {} as dm INNER JOIN dependency as d ON dm.dependency_id = d.id WHERE dm.module_id = ?1 AND d.dependency_type_id = ?2", &TABLE_NAME).as_str(),
    ).unwrap();
    let deps_rows = stmt.query_map(
        params![&dep_type_id, &module_id], 
        |row| {
            Ok(row.get(0)?)
    }).unwrap();

    let deps_iter = deps_rows.map(|x| x.unwrap());
    let depends = deps_iter.collect::<Vec<String>>();
    depends
}

pub fn add(conn: &Connection, dep_type_id: &i64, name: &str, module_id: &i64) -> Result<Model, rusqlite::Error> {
    let dep = dependency::add(&conn, &dep_type_id, &name)?;
    let dep_module_opt = get_by_dependency_id_module_id(&conn, &dep.id, &module_id);
    if dep_module_opt.is_none() {
        conn.execute(
            format!("INSERT INTO {}(dependency_id, module_id) VALUES (?1, ?2)", &TABLE_NAME).as_str(),
            params![&dep.id, &module_id],
        )?;
        let last_id = conn.last_insert_rowid().clone();
        let module = module::get_by_id(&conn, &module_id).unwrap();
        let _ = system_event::register_new_dependency_module(&conn, &name, &module.technical_name, &module.name, odoo_version_u8_to_string(&module.version_odoo).as_str());
        return Ok(Model { 
            id: last_id, 
            dependency_id: (dep.id.clone(), dep.name.clone()), 
            module_id: (module.id.clone(), module.technical_name.clone()),
        });
    }
    Ok(dep_module_opt.unwrap())
}

pub fn delete_by_module_id(conn: &Connection, module_id: &i64) -> Result<usize, rusqlite::Error> {
    conn.execute(
        format!("DELETE FROM {} WHERE module_id = ?1", &TABLE_NAME).as_str(),
        params![&module_id],
    )
}

pub fn delete_by_module_id_dependecy_id(conn: &Connection, module_id: &i64, dependency_id: &i64) -> Result<usize, rusqlite::Error> {
    conn.execute(format!("DELETE FROM {} WHERE module_id = ?1 AND dependency_id = ?2", &TABLE_NAME).as_str(), params![&module_id, &dependency_id])
}