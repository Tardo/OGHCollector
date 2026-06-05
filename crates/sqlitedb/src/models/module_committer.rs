// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::module_committer;

use super::{committer, module, system_event};
use oghutils::version::odoo_version_u8_to_string;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module_committer, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_id: i64,
    pub committer_id: i64,
    pub commits: i32,
}

#[derive(Insertable)]
#[diesel(table_name = module_committer)]
struct NewModuleCommitter {
    module_id: i64,
    committer_id: i64,
    commits: i32,
}

pub fn get_by_id(
    conn: &mut SqliteConnection,
    module_id: &i64,
    committer_id: &i64,
) -> Option<Model> {
    module_committer::table
        .filter(
            module_committer::module_id
                .eq(module_id)
                .and(module_committer::committer_id.eq(committer_id)),
        )
        .first::<Model>(conn)
        .optional()
        .expect("DB error in module_committer::get_by_id")
}

pub fn get_by_name(conn: &mut SqliteConnection, module_id: &i64, name: &str) -> Option<Model> {
    if let Some(com) = committer::get_by_name(conn, name) {
        module_committer::table
            .filter(
                module_committer::module_id
                    .eq(module_id)
                    .and(module_committer::committer_id.eq(com.id)),
            )
            .first::<Model>(conn)
            .optional()
            .expect("DB error in module_committer::get_by_name")
    } else {
        None
    }
}

pub fn get_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> Vec<Model> {
    module_committer::table
        .filter(module_committer::module_id.eq(module_id))
        .load::<Model>(conn)
        .expect("DB error in module_committer::get_by_module_id")
}

pub fn get_names_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> Vec<String> {
    get_by_module_id(conn, module_id)
        .into_iter()
        .filter_map(|mc| committer::get_by_id(conn, &mc.committer_id).map(|c| c.name))
        .collect()
}

pub fn add(
    conn: &mut SqliteConnection,
    module_id: &i64,
    name: &str,
    commits: &u32,
) -> QueryResult<Model> {
    let com = committer::add(conn, name)?;
    let commits_i32 = *commits as i32;

    if let Some(existing) = get_by_id(conn, module_id, &com.id) {
        if existing.commits != commits_i32 {
            diesel::update(
                module_committer::table.filter(
                    module_committer::module_id
                        .eq(module_id)
                        .and(module_committer::committer_id.eq(com.id)),
                ),
            )
            .set(module_committer::commits.eq(commits_i32))
            .execute(conn)?;
        }
        return Ok(Model {
            id: existing.id,
            module_id: existing.module_id,
            committer_id: existing.committer_id,
            commits: commits_i32,
        });
    }

    diesel::insert_into(module_committer::table)
        .values(NewModuleCommitter {
            module_id: *module_id,
            committer_id: com.id,
            commits: commits_i32,
        })
        .execute(conn)?;
    let new_id = crate::models::last_insert_rowid(conn);
    let mod_info = module::get_by_id(conn, module_id).unwrap();
    let _ = system_event::register_new_module_committer(
        conn,
        name,
        &mod_info.technical_name,
        &mod_info.name,
        odoo_version_u8_to_string(&(mod_info.version_odoo as u8)).as_str(),
    );
    Ok(Model {
        id: new_id,
        module_id: *module_id,
        committer_id: com.id,
        commits: commits_i32,
    })
}
