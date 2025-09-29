// Copyright 2025 Alexandre D. Díaz
use cached::proc_macro::cached;
use rusqlite::{params, Result, ToSql};
use serde::{Deserialize, Serialize};

use crate::models::{dependency_module, dependency_type, gh_organization, gh_repository, module};

pub type Connection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub static TABLE_NAME: &str = "dependency";

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Model {
    pub id: i64,
    pub dependency_type_id: (i64, String),
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DependencyModuleInfo {
    pub org: String,
    pub repo: String,
    pub technical_name: String,
}

pub fn create_table(conn: &Connection) -> Result<usize, rusqlite::Error> {
    conn.execute(
        format!(
            "CREATE TABLE IF NOT EXISTS {0} (
            id integer primary key,
            dependency_type_id integer not null references {1}(id),
            name text not null,
            CONSTRAINT fk_dependency_type
                FOREIGN KEY (dependency_type_id)
                REFERENCES {1}(id)
                ON DELETE CASCADE
        )",
            &TABLE_NAME,
            &dependency_type::TABLE_NAME
        )
        .as_str(),
        params![],
    )?;
    conn.execute(
        format!(
            "CREATE UNIQUE INDEX IF NOT EXISTS uniq_dep_type_name ON {}(dependency_type_id, name)",
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
        "SELECT dep.id, dep.dependency_type_id, dt.name, dep.name \
    FROM {} as dep \
    INNER JOIN {} as dt \
    ON dt.id = dep.dependency_type_id \
    {}",
        &TABLE_NAME,
        &dependency_type::TABLE_NAME,
        &extra_sql
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params, |row| {
        Ok(Model {
            id: row.get(0)?,
            dependency_type_id: (row.get(1)?, row.get(2)?),
            name: row.get(3)?,
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
    convert = r#"{ format!("{}", id) }"#
)]
pub fn get_by_id(conn: &Connection, id: &i64) -> Option<Model> {
    let deps = query(conn, "WHERE dep.id = ?1 LIMIT 1", params![&id]).unwrap();
    if deps.is_empty() {
        return None;
    }
    Some(deps[0].clone())
}

#[cached(
    key = "String",
    time = 3600,
    time_refresh = true,
    size = 1000,
    option = true,
    convert = r#"{ format!("{}{}", dep_type_id, name) }"#
)]
pub fn get_by_name(conn: &Connection, dep_type_id: &i64, name: &str) -> Option<Model> {
    let deps = query(
        conn,
        "WHERE dep.dependency_type_id = ?1 AND dep.name = ?2 LIMIT 1",
        params![&dep_type_id, &name],
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
    size = 1000,
    convert = r#"{ format!("{}{}", module_id, dep_type) }"#
)]
pub fn get_module_external_dependency_names(
    conn: &Connection,
    module_id: &i64,
    dep_type: &str,
) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            format!(
                "SELECT dep.name \
        FROM {0} as dep \
        INNER JOIN {1} as dep_mod \
        ON dep_mod.dependency_id = dep.id \
        INNER JOIN {2} as dep_type \
        on dep_type.id = dep.dependency_type_id \
        INNER JOIN {3} as mod \
        on mod.id = dep_mod.module_id \
        WHERE mod.id = ?1 AND dep_type.name = ?2",
                &TABLE_NAME,
                &dependency_module::TABLE_NAME,
                &dependency_type::TABLE_NAME,
                &module::TABLE_NAME
            )
            .as_str(),
        )
        .unwrap();
    let deps_rows = stmt
        .query_map(params![&module_id, &dep_type], |row| row.get(0))
        .unwrap();

    let depends_iter = deps_rows.map(|x| x.unwrap());

    depends_iter.collect::<Vec<String>>()
}

#[cached(
    key = "String",
    time = 3600,
    time_refresh = true,
    size = 1000,
    convert = r#"{ format!("{}", module_id) }"#
)]
pub fn get_module_dependency_info(conn: &Connection, module_id: &i64) -> Vec<DependencyModuleInfo> {
    let mut stmt = conn
        .prepare(
            format!(
                "SELECT ghorg.name, ghrepo.name, dep.name \
        FROM {0} as dep \
        INNER JOIN {1} as dep_mod \
        ON dep_mod.dependency_id = dep.id \
        INNER JOIN {2} as dep_type \
        on dep_type.id = dep.dependency_type_id \
        INNER JOIN {3} as mod \
        on mod.id = dep_mod.module_id \
        INNER JOIN {3} as mod_dep \
        on mod_dep.technical_name = dep.name AND mod_dep.version_odoo = mod.version_odoo \
        INNER JOIN {4} as ghrepo \
        on ghrepo.id = mod_dep.gh_repository_id \
        INNER JOIN {5} as ghorg \
        on ghorg.id = ghrepo.gh_organization_id \
        WHERE mod.id = ?1",
                &TABLE_NAME,
                &dependency_module::TABLE_NAME,
                &dependency_type::TABLE_NAME,
                &module::TABLE_NAME,
                &gh_repository::TABLE_NAME,
                &gh_organization::TABLE_NAME
            )
            .as_str(),
        )
        .unwrap();
    let deps_rows = stmt
        .query_map(params![&module_id], |row| {
            Ok(DependencyModuleInfo {
                org: row.get(0)?,
                repo: row.get(1)?,
                technical_name: row.get(2)?,
            })
        })
        .unwrap();

    let depends_iter = deps_rows.map(|x| x.unwrap());

    depends_iter.collect::<Vec<DependencyModuleInfo>>()
}

pub fn add(conn: &Connection, dep_type_id: &i64, name: &str) -> Result<Model, rusqlite::Error> {
    let dep_opt = get_by_name(conn, dep_type_id, name);
    if dep_opt.is_none() {
        let dep_type = dependency_type::get_by_id(conn, dep_type_id).unwrap();
        conn.execute(
            format!(
                "INSERT INTO {}(dependency_type_id, name) VALUES (?1, ?2)",
                &TABLE_NAME
            )
            .as_str(),
            params![&dep_type.id, &name],
        )?;
        return Ok(Model {
            id: conn.last_insert_rowid(),
            dependency_type_id: (dep_type.id, dep_type.name.clone()),
            name: name.to_string(),
        });
    }
    Ok(dep_opt.unwrap())
}
