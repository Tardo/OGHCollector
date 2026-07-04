// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::dependency_module;

use super::{dependency, module, system_event};
use oghutils::version::odoo_version_u8_to_string;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = dependency_module, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub dependency_id: i64,
    pub module_id: i64,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct ModelFull {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub id: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub dependency_id: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub dependency_name: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub module_id: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub module_technical_name: String,
}

#[derive(Insertable)]
#[diesel(table_name = dependency_module)]
struct NewDependencyModule {
    dependency_id: i64,
    module_id: i64,
}

pub fn get_by_id(conn: &mut SqliteConnection, id: &i64) -> Option<ModelFull> {
    diesel::sql_query(
        "SELECT mod_dep.id, mod_dep.dependency_id, dep.name as dependency_name, \
         mod_dep.module_id, mod.technical_name as module_technical_name \
         FROM dependency_module as mod_dep \
         INNER JOIN module as mod ON mod.id = mod_dep.module_id \
         INNER JOIN dependency as dep ON dep.id = mod_dep.dependency_id \
         WHERE mod_dep.id = ? LIMIT 1",
    )
    .bind::<diesel::sql_types::BigInt, _>(id)
    .get_result::<ModelFull>(conn)
    .optional()
    .expect("DB error in dependency_module::get_by_id")
}

fn get_by_dependency_id_module_id(
    conn: &mut SqliteConnection,
    dep_id: &i64,
    mod_id: &i64,
) -> Option<Model> {
    dependency_module::table
        .filter(
            dependency_module::dependency_id
                .eq(dep_id)
                .and(dependency_module::module_id.eq(mod_id)),
        )
        .first::<Model>(conn)
        .optional()
        .expect("DB error in dependency_module::get_by_dependency_id_module_id")
}

pub fn get_names(conn: &mut SqliteConnection, module_id: &i64, dep_type_id: &i64) -> Vec<String> {
    diesel::sql_query(
        "SELECT d.name \
         FROM dependency_module as dm \
         INNER JOIN dependency as d ON dm.dependency_id = d.id \
         WHERE dm.module_id = ? AND d.dependency_type_id = ?",
    )
    .bind::<diesel::sql_types::BigInt, _>(module_id)
    .bind::<diesel::sql_types::BigInt, _>(dep_type_id)
    .load::<crate::models::NameRow>(conn)
    .expect("DB error in dependency_module::get_names")
    .into_iter()
    .map(|r| r.name)
    .collect()
}

pub fn add(
    conn: &mut SqliteConnection,
    dep_type_id: &i64,
    name: &str,
    module_id: &i64,
) -> QueryResult<ModelFull> {
    let dep = dependency::add(conn, dep_type_id, name)?;
    if let Some(existing) = get_by_dependency_id_module_id(conn, &dep.id, module_id) {
        let mod_name = module::get_by_id(conn, module_id)
            .map(|m| m.technical_name)
            .unwrap_or_default();
        return Ok(ModelFull {
            id: existing.id,
            dependency_id: dep.id,
            dependency_name: dep.name,
            module_id: existing.module_id,
            module_technical_name: mod_name,
        });
    }

    diesel::insert_into(dependency_module::table)
        .values(NewDependencyModule {
            dependency_id: dep.id,
            module_id: *module_id,
        })
        .execute(conn)?;
    let new_id = crate::models::last_insert_rowid(conn);
    let mod_info = module::get_by_id(conn, module_id).unwrap();
    let _ = system_event::register_new_dependency_module(
        conn,
        name,
        &mod_info.technical_name,
        &mod_info.name,
        odoo_version_u8_to_string(&(mod_info.version_odoo as u8)).as_str(),
    );
    Ok(ModelFull {
        id: new_id,
        dependency_id: dep.id,
        dependency_name: dep.name,
        module_id: *module_id,
        module_technical_name: mod_info.technical_name,
    })
}

pub fn delete_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> QueryResult<usize> {
    diesel::delete(dependency_module::table.filter(dependency_module::module_id.eq(module_id)))
        .execute(conn)
}

pub fn delete_by_module_id_dependecy_id(
    conn: &mut SqliteConnection,
    module_id: &i64,
    dependency_id: &i64,
) -> QueryResult<usize> {
    diesel::delete(
        dependency_module::table.filter(
            dependency_module::module_id
                .eq(module_id)
                .and(dependency_module::dependency_id.eq(dependency_id)),
        ),
    )
    .execute(conn)
}
