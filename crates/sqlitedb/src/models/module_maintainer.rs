// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::module_maintainer;

use super::{maintainer, module, system_event};
use oghutils::version::odoo_version_u8_to_string;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module_maintainer, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_id: i64,
    pub maintainer_id: i64,
}

#[derive(Insertable)]
#[diesel(table_name = module_maintainer)]
struct NewModuleMaintainer {
    module_id: i64,
    maintainer_id: i64,
}

pub fn get_by_id(
    conn: &mut SqliteConnection,
    module_id: &i64,
    maintainer_id: &i64,
) -> Option<Model> {
    module_maintainer::table
        .filter(
            module_maintainer::module_id
                .eq(module_id)
                .and(module_maintainer::maintainer_id.eq(maintainer_id)),
        )
        .first::<Model>(conn)
        .optional()
        .expect("DB error in module_maintainer::get_by_id")
}

pub fn get_by_name(conn: &mut SqliteConnection, module_id: &i64, name: &str) -> Option<Model> {
    if let Some(maint) = maintainer::get_by_name(conn, name) {
        module_maintainer::table
            .filter(
                module_maintainer::module_id
                    .eq(module_id)
                    .and(module_maintainer::maintainer_id.eq(maint.id)),
            )
            .first::<Model>(conn)
            .optional()
            .expect("DB error in module_maintainer::get_by_name")
    } else {
        None
    }
}

pub fn get_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> Vec<Model> {
    module_maintainer::table
        .filter(module_maintainer::module_id.eq(module_id))
        .load::<Model>(conn)
        .expect("DB error in module_maintainer::get_by_module_id")
}

pub fn get_names_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> Vec<String> {
    get_by_module_id(conn, module_id)
        .into_iter()
        .filter_map(|mm| maintainer::get_by_id(conn, &mm.maintainer_id).map(|m| m.name))
        .collect()
}

pub fn add(conn: &mut SqliteConnection, module_id: &i64, name: &str) -> QueryResult<Model> {
    let maint = maintainer::add(conn, name)?;
    if let Some(existing) = get_by_id(conn, module_id, &maint.id) {
        return Ok(existing);
    }

    diesel::insert_into(module_maintainer::table)
        .values(NewModuleMaintainer {
            module_id: *module_id,
            maintainer_id: maint.id,
        })
        .execute(conn)?;
    let new_id = crate::models::last_insert_rowid(conn);
    let mod_info = module::get_by_id(conn, module_id).unwrap();
    let _ = system_event::register_new_module_maintainer(
        conn,
        name,
        &mod_info.technical_name,
        &mod_info.name,
        odoo_version_u8_to_string(&(mod_info.version_odoo as u8)).as_str(),
    );
    Ok(Model {
        id: new_id,
        module_id: *module_id,
        maintainer_id: maint.id,
    })
}

pub fn delete_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> QueryResult<usize> {
    diesel::delete(module_maintainer::table.filter(module_maintainer::module_id.eq(module_id)))
        .execute(conn)
}

pub fn delete_by_module_id_maintainer_id(
    conn: &mut SqliteConnection,
    module_id: &i64,
    maintainer_id: &i64,
) -> QueryResult<usize> {
    diesel::delete(
        module_maintainer::table.filter(
            module_maintainer::module_id
                .eq(module_id)
                .and(module_maintainer::maintainer_id.eq(maintainer_id)),
        ),
    )
    .execute(conn)
}
