// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::dependency_osv;

use super::{dependency, dependency_module};

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct Model {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub id: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub dependency_module_id: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub dependency_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub osv_id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub details: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub fixed_in: String,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct DependencyModuleOSVInfo {
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub module_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub module_technical_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub osv_id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub details: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub fixed_in: String,
}

#[derive(Insertable)]
#[diesel(table_name = dependency_osv)]
struct NewDependencyOsv<'a> {
    dependency_module_id: i64,
    osv_id: &'a str,
    details: &'a str,
    fixed_in: &'a str,
}

fn get_by_dep_mod_id_osv_id_impl(
    conn: &mut SqliteConnection,
    dep_mod_id: &i64,
    osv_id: &str,
) -> Option<Model> {
    diesel::sql_query(
        "SELECT dep_o.id, dep_o.dependency_module_id, dep.name as dependency_name, \
         dep_o.osv_id, dep_o.details, dep_o.fixed_in \
         FROM dependency_osv as dep_o \
         INNER JOIN dependency_module as dep_mod ON dep_mod.id = dep_o.dependency_module_id \
         INNER JOIN dependency as dep ON dep.id = dep_mod.dependency_id \
         WHERE dep_mod.id = ? AND dep_o.osv_id = ? LIMIT 1",
    )
    .bind::<diesel::sql_types::BigInt, _>(dep_mod_id)
    .bind::<diesel::sql_types::Text, _>(osv_id)
    .get_result::<Model>(conn)
    .optional()
    .expect("DB error in dependency_osv::get_by_dep_mod_id_osv_id")
}

pub fn get_osv_info(conn: &mut SqliteConnection) -> Vec<DependencyModuleOSVInfo> {
    diesel::sql_query(
        "SELECT mod.version_odoo, mod.name as module_name, mod.technical_name as module_technical_name, \
         dep.name, dep_o.osv_id, dep_o.details, dep_o.fixed_in \
         FROM dependency_osv as dep_o \
         INNER JOIN dependency_module as dep_mod ON dep_mod.id = dep_o.dependency_module_id \
         INNER JOIN dependency as dep ON dep.id = dep_mod.dependency_id \
         INNER JOIN module as mod ON mod.id = dep_mod.module_id",
    )
    .load::<DependencyModuleOSVInfo>(conn)
    .expect("DB error in dependency_osv::get_osv_info")
}

pub fn add(
    conn: &mut SqliteConnection,
    dep_mod_id: &i64,
    osv_id: &str,
    details: &str,
    fixed_in: &str,
) -> QueryResult<Model> {
    if let Some(existing) = get_by_dep_mod_id_osv_id_impl(conn, dep_mod_id, osv_id) {
        return Ok(existing);
    }

    let dep_mod = dependency_module::get_by_id(conn, dep_mod_id).unwrap();
    let dep = dependency::get_by_id(conn, &dep_mod.dependency_id).unwrap();

    diesel::insert_into(dependency_osv::table)
        .values(NewDependencyOsv {
            dependency_module_id: *dep_mod_id,
            osv_id,
            details,
            fixed_in,
        })
        .execute(conn)?;
    let new_id = crate::models::last_insert_rowid(conn);
    Ok(Model {
        id: new_id,
        dependency_module_id: *dep_mod_id,
        dependency_name: dep.name,
        osv_id: osv_id.to_string(),
        details: details.to_string(),
        fixed_in: fixed_in.to_string(),
    })
}
