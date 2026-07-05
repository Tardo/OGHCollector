// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::module_version;
use crate::utils::date::get_sqlite_utc_now;

use super::module;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module_version, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_id: i64,
    pub version_module: String,
    pub create_date: String,
    pub update_date: String,
}

#[derive(Insertable)]
#[diesel(table_name = module_version)]
struct NewModuleVersion<'a> {
    module_id: i64,
    version_module: &'a str,
    create_date: &'a str,
    update_date: &'a str,
}

pub fn get_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> Vec<Model> {
    module_version::table
        .filter(module_version::module_id.eq(module_id))
        .order(module_version::id.asc())
        .load::<Model>(conn)
        .expect("DB error in module_version::get_by_module_id")
}

pub fn get_by_module_id_version_module(
    conn: &mut SqliteConnection,
    module_id: &i64,
    version_module: &str,
) -> Option<Model> {
    module_version::table
        .filter(
            module_version::module_id
                .eq(module_id)
                .and(module_version::version_module.eq(version_module)),
        )
        .first::<Model>(conn)
        .optional()
        .expect("DB error in module_version::get_by_module_id_version_module")
}

/// The module_version row matching `module.version_module` - i.e. the latest
/// one, since `module.version_module` is always kept as the current version
/// by `module::add`. This is what "default to latest" resolves to everywhere.
pub fn resolve_current(conn: &mut SqliteConnection, module: &module::Model) -> Option<Model> {
    get_by_module_id_version_module(conn, &module.id, &module.version_module)
}

/// Returns the module_version row for `version_module`, creating it - and so
/// starting permanent history for that version - the first time it's seen.
/// Called once per collector run so re-analyzing the same version in place
/// (no manifest version bump) still refreshes `update_date`.
pub fn get_or_create(
    conn: &mut SqliteConnection,
    module_id: &i64,
    version_module: &str,
) -> QueryResult<Model> {
    if let Some(existing) = get_by_module_id_version_module(conn, module_id, version_module) {
        let update_date = get_sqlite_utc_now();
        diesel::update(module_version::table.filter(module_version::id.eq(existing.id)))
            .set(module_version::update_date.eq(&update_date))
            .execute(conn)?;
        return Ok(Model {
            update_date,
            ..existing
        });
    }

    let now = get_sqlite_utc_now();
    diesel::insert_into(module_version::table)
        .values(NewModuleVersion {
            module_id: *module_id,
            version_module,
            create_date: &now,
            update_date: &now,
        })
        .execute(conn)?;
    let new_id = crate::models::last_insert_rowid(conn);
    Ok(Model {
        id: new_id,
        module_id: *module_id,
        version_module: version_module.to_string(),
        create_date: now.clone(),
        update_date: now,
    })
}

pub fn delete_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> QueryResult<usize> {
    diesel::delete(module_version::table.filter(module_version::module_id.eq(module_id)))
        .execute(conn)
}
